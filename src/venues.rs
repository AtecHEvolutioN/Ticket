//! Reusable venue metadata, deliberately separate from event inventory.
//!
//! A venue can have several named seating layouts. A saved section label is
//! reference data: it neither proves that section is sold for a new event nor
//! enables a parser or stock filter without checking that event.
use std::collections::HashSet;

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{config::validate_public_url, event_identity, hybrid};

pub const MAX_VENUES: usize = 10_000;
pub(crate) const MAX_PRESET_BYTES: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VenuePreset {
    /// Stable local key; leave empty when importing a new venue.
    #[serde(default)]
    pub id: String,
    pub site_host: String,
    #[serde(default)]
    pub provider_venue_id: Option<String>,
    pub name: String,
    pub city: String,
    pub country: String,
    #[serde(default)]
    pub layouts: Vec<VenueLayout>,
    #[serde(default)]
    pub source_event_urls: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct VenueLayout {
    /// User-assigned identity, such as concert-end-stage or basketball.
    pub id: String,
    pub name: String,
    /// A reference hint, never an automatically activated parser.
    #[serde(default)]
    pub adapter: Option<String>,
    #[serde(default)]
    pub sections: Vec<String>,
    /// Observed static map image reference; never a live seat or stock source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub map_url: Option<String>,
    #[serde(default)]
    pub source_event_urls: Vec<String>,
    /// Records an explicit layout review, not event availability verification.
    #[serde(default)]
    pub verified_at: Option<i64>,
}

impl VenuePreset {
    pub fn normalized(&self) -> Result<Self> {
        let mut value = self.clone();
        value.site_host = normalized_host(&value.site_host)?;
        value.name = label(&value.name, 400, "venue name")?;
        value.city = label(&value.city, 200, "venue city")?;
        value.country = value.country.trim().to_ascii_uppercase();
        ensure!(
            value.country.len() == 2 && value.country.bytes().all(|b| b.is_ascii_uppercase()),
            "Venue country must be a two-letter code"
        );
        if let Some(provider_id) = &value.provider_venue_id {
            identifier(provider_id, 160, "provider venue ID")?;
        }
        if !value.id.is_empty() {
            ensure!(
                value.id.starts_with("venue-")
                    && value.id.len() == 70
                    && value.id[6..].bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid local venue ID"
            );
        }
        ensure!(value.layouts.len() <= 32, "Too many venue layouts");
        normalize_sources(&mut value.source_event_urls, &value.site_host)?;
        let mut layout_ids = HashSet::new();
        for layout in &mut value.layouts {
            identifier(&layout.id, 100, "layout ID")?;
            ensure!(layout_ids.insert(layout.id.clone()), "Duplicate layout ID");
            layout.name = label(&layout.name, 200, "layout name")?;
            if let Some(adapter) = &layout.adapter {
                identifier(adapter, 100, "adapter hint")?;
            }
            ensure!(layout.sections.len() <= 1000, "Too many section labels");
            let mut sections = HashSet::new();
            let mut normalized_sections = Vec::new();
            for section in &layout.sections {
                let section = label(section, 200, "section label")?;
                if sections.insert(section.clone()) {
                    normalized_sections.push(section);
                }
            }
            layout.sections = normalized_sections;
            normalize_sources(&mut layout.source_event_urls, &value.site_host)?;
            if let Some(map_url) = &layout.map_url {
                layout.map_url = Some(crate::seat_map::validated_static_map_url(map_url)?);
                ensure!(
                    !layout.source_event_urls.is_empty(),
                    "A map reference needs a public source event"
                );
            }
            if let Some(at) = layout.verified_at {
                ensure!(
                    at > 0 && !layout.source_event_urls.is_empty(),
                    "A reviewed layout needs a positive review timestamp and a public source event"
                );
            }
        }
        ensure!(
            serde_json::to_vec(&value)?.len() <= MAX_PRESET_BYTES,
            "Venue preset is too large"
        );
        Ok(value)
    }

    pub(crate) fn match_key(&self) -> String {
        digest(&[
            &self.site_host,
            &normalized_label(&self.name),
            &normalized_label(&self.city),
            &self.country,
        ])
    }

    pub(crate) fn generated_id(&self) -> String {
        let key = match &self.provider_venue_id {
            Some(id) => digest(&["provider", &self.site_host, id]),
            None => digest(&["name", &self.match_key()]),
        };
        format!("venue-{key}")
    }
}

/// Extract static identity from the normalized Discovery event envelope. No
/// prices, sale status, inventory, capacity, or seat-map claims are imported.
pub fn from_discovery_event(event: &Value) -> Result<Vec<VenuePreset>> {
    let event_url = event
        .get("url")
        .and_then(Value::as_str)
        .context("Venue registration needs an event URL")?;
    ensure!(
        event_identity::same_event(event_url, event_url),
        "Venue registration needs a supported public event URL"
    );
    let url = validate_public_url(event_url)?;
    let host = normalized_host(url.host_str().context("Missing event host")?)?;
    let venues = event
        .get("venues")
        .and_then(Value::as_array)
        .context("Event has no normalized venue array")?;
    ensure!(venues.len() <= 20, "Too many event venues");
    venues
        .iter()
        .map(|venue| {
            VenuePreset {
                site_host: host.clone(),
                provider_venue_id: venue
                    .get("id")
                    .and_then(Value::as_str)
                    .filter(|s| !s.is_empty())
                    .map(str::to_owned),
                name: venue
                    .get("name")
                    .and_then(Value::as_str)
                    .context("Venue has no name")?
                    .to_owned(),
                city: venue
                    .get("city")
                    .and_then(Value::as_str)
                    .context("Venue has no city")?
                    .to_owned(),
                country: venue
                    .pointer("/country/countryCode")
                    .and_then(Value::as_str)
                    .context("Venue has no country")?
                    .to_owned(),
                source_event_urls: vec![event_url.to_owned()],
                ..VenuePreset::default()
            }
            .normalized()
        })
        .collect()
}

fn normalized_host(input: &str) -> Result<String> {
    ensure!(
        !input.is_empty()
            && input.len() <= 253
            && input.trim() == input
            && !input.contains(['/', ':', '?', '#', '@']),
        "Invalid Ticketmaster site host"
    );
    let input = input.to_ascii_lowercase();
    let url = validate_public_url(&format!("https://{input}/event/12345"))?;
    let host = url.host_str().context("Missing site host")?;
    Ok(host.strip_prefix("www.").unwrap_or(host).to_owned())
}

fn label(input: &str, max: usize, field: &str) -> Result<String> {
    ensure!(!input.chars().any(char::is_control), "Invalid {field}");
    let value = input.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(
        !value.is_empty() && value.len() <= max,
        "Invalid {field} length"
    );
    Ok(value)
}

fn identifier(input: &str, max: usize, field: &str) -> Result<()> {
    ensure!(
        !input.is_empty()
            && input.len() <= max
            && input
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
        "Invalid {field}"
    );
    Ok(())
}

fn normalized_label(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn normalize_sources(urls: &mut Vec<String>, site_host: &str) -> Result<()> {
    ensure!(urls.len() <= 100, "Too many venue source events");
    let mut normalized: Vec<String> = Vec::new();
    let mut seen = HashSet::new();
    for raw in urls.iter() {
        let identity = event_identity::identity(raw)
            .context("Venue source must be a supported public event URL")?;
        let canonical = hybrid::canonical_event_url(raw)?;
        let parsed = validate_public_url(&canonical)?;
        ensure!(
            parsed.host_str() == Some(site_host),
            "Venue source belongs to another Ticketmaster site"
        );
        if seen.insert(identity) {
            normalized.push(canonical);
        }
    }
    *urls = normalized;
    Ok(())
}

fn digest(parts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for part in parts {
        digest.update((part.len() as u64).to_le_bytes());
        digest.update(part.as_bytes());
    }
    format!("{:x}", digest.finalize())
}
