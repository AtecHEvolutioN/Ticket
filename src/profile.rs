//! Offline profile review. Candidate fields are never promoted to inventory evidence.
use crate::{
    config::{Watch, validate_public_url},
    event_identity::same_event,
    model::{Availability, Offer},
    parser,
};
use anyhow::{Context, Result, bail};
use scraper::{ElementRef, Html, Selector};
use serde::{Deserialize, Serialize};

const MAX_HTML_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_CAPTURE_JSON_BYTES: usize = 16 * 1024;
const MATCH_LIMIT: usize = 1000;
const PREVIEW_LIMIT: usize = 20;

#[derive(Debug, Serialize)]
pub struct ProfileReport {
    pub review_only: bool,
    pub configured_verified: bool,
    pub watch_id: String,
    pub source: CaptureSource,
    pub selector_counts_are_raw: bool,
    pub selectors: Vec<SelectorDiagnostic>,
    pub unverified_candidates: Vec<Offer>,
    pub candidates_truncated: bool,
    pub notes: Vec<String>,
}

#[derive(Debug, Serialize)]
pub struct CaptureSource {
    pub sidecar_present: bool,
    pub event_identity_matches: bool,
    pub rendered: bool,
    pub original_url: Option<String>,
    pub final_url: Option<String>,
    pub captured_at: Option<String>,
    pub status: Option<u16>,
    /// A local, editable sidecar is provenance information, not independent attestation.
    pub independently_verified: bool,
}

#[derive(Debug, Serialize)]
pub struct SelectorDiagnostic {
    pub field: &'static str,
    pub scope: &'static str,
    pub configured: bool,
    pub valid: bool,
    pub raw_match_count: usize,
    pub count_limited: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CaptureMetadata {
    #[serde(default = "capture_schema")]
    schema_version: u32,
    original_url: String,
    final_url: String,
    captured_at: String,
    status: u16,
    rendered: bool,
    profile_verified: bool,
}

fn capture_schema() -> u32 {
    1
}

fn public_event(input: &str) -> Result<String> {
    if input.len() > 4096 {
        bail!("Capture event URL exceeds the review limit");
    }
    let mut url = validate_public_url(input).context("Capture has an unsupported event URL")?;
    url.set_query(None);
    url.set_fragment(None);
    Ok(url.to_string())
}

fn source(watch: &Watch, raw: Option<&str>) -> Result<(CaptureSource, i64)> {
    let Some(raw) = raw else {
        return Ok((
            CaptureSource {
                sidecar_present: false,
                event_identity_matches: false,
                rendered: false,
                original_url: None,
                final_url: None,
                captured_at: None,
                status: None,
                independently_verified: false,
            },
            0,
        ));
    };
    if raw.len() > MAX_CAPTURE_JSON_BYTES {
        bail!("Capture metadata exceeds the review limit");
    }
    let meta: CaptureMetadata =
        serde_json::from_str(raw).context("Invalid capture metadata schema")?;
    if meta.schema_version != 1 || !meta.rendered || meta.profile_verified {
        bail!("Capture metadata must describe a rendered, unverified schema-1 capture");
    }
    if !(200..300).contains(&meta.status) {
        bail!("Capture did not receive a successful event page; no candidates reviewed");
    }
    // Validate the original strings before URL parsing can normalize ambiguous path segments.
    if !same_event(&watch.url, &meta.original_url) || !same_event(&watch.url, &meta.final_url) {
        bail!("Capture does not match the watched event; no candidates reviewed");
    }
    let original_url = public_event(&meta.original_url)?;
    let final_url = public_event(&meta.final_url)?;
    if meta.captured_at.len() > 64 {
        bail!("Invalid capture timestamp");
    }
    let captured_at = chrono::DateTime::parse_from_rfc3339(&meta.captured_at)
        .context("Capture timestamp must use RFC3339")?;
    Ok((
        CaptureSource {
            sidecar_present: true,
            event_identity_matches: true,
            rendered: true,
            original_url: Some(original_url),
            final_url: Some(final_url),
            captured_at: Some(captured_at.to_rfc3339()),
            status: Some(meta.status),
            independently_verified: false,
        },
        captured_at.timestamp(),
    ))
}

fn diagnostic(
    doc: &Html,
    rows: &[ElementRef<'_>],
    field: &'static str,
    value: &Option<String>,
    relative: bool,
) -> SelectorDiagnostic {
    let mut result = SelectorDiagnostic {
        field,
        scope: if relative {
            "within_offer_rows"
        } else {
            "document"
        },
        configured: value.is_some(),
        valid: false,
        raw_match_count: 0,
        count_limited: false,
    };
    let Some(value) = value else { return result };
    let Ok(selector) = Selector::parse(value) else {
        return result;
    };
    result.valid = true;
    let count = if relative {
        rows.iter()
            .flat_map(|row| {
                std::iter::once(*row)
                    .filter(|element| selector.matches(element))
                    .chain(row.select(&selector))
            })
            .take(MATCH_LIMIT + 1)
            .count()
    } else {
        doc.select(&selector).take(MATCH_LIMIT + 1).count()
    };
    result.raw_match_count = count.min(MATCH_LIMIT);
    result.count_limited = count > MATCH_LIMIT;
    result
}

/// Review HTML locally. This function does not open a database or perform network requests.
pub fn review(watch: &Watch, html: &str, capture_json: Option<&str>) -> Result<ProfileReport> {
    if html.len() > MAX_HTML_BYTES {
        bail!("HTML capture exceeds the review limit");
    }
    let (source, captured_at) = source(watch, capture_json)?;
    let doc = Html::parse_document(html);
    let row_selector = watch
        .parser
        .offers_selector
        .as_deref()
        .and_then(|s| Selector::parse(s).ok());
    let rows: Vec<_> = row_selector
        .as_ref()
        .map(|selector| doc.select(selector).take(MATCH_LIMIT).collect())
        .unwrap_or_default();
    let p = &watch.parser;
    let selectors = [
        ("available_selector", &p.available_selector, false),
        ("unavailable_selector", &p.unavailable_selector, false),
        ("offers_selector", &p.offers_selector, false),
        ("complete_selector", &p.complete_selector, false),
        (
            "offer_available_selector",
            &p.offer_available_selector,
            true,
        ),
        ("ticket_type_selector", &p.ticket_type_selector, true),
        ("price_selector", &p.price_selector, true),
        ("quantity_selector", &p.quantity_selector, true),
        ("section_selector", &p.section_selector, true),
        ("row_selector", &p.row_selector, true),
        ("seats_selector", &p.seats_selector, true),
        ("inventory_selector", &p.inventory_selector, true),
    ]
    .into_iter()
    .map(|(name, value, relative)| diagnostic(&doc, &rows, name, value, relative))
    .collect();
    // Deliberately isolated preview: no configured flag or source file is changed. The output
    // contains candidate fields only, without an available/unavailable inventory assertion.
    let mut candidate_watch = watch.clone();
    candidate_watch.parser.selectors_verified = true;
    let observation = parser::parse(&candidate_watch, html, captured_at);
    if observation.availability == Availability::Blocked {
        bail!("Capture contains a queue, challenge, or access restriction; no candidates reviewed");
    }
    let candidates_truncated = observation.offers.len() > PREVIEW_LIMIT;
    let mut notes = vec![
        "Review only: candidate fields are not verified live inventory. No configuration, database, or alert was changed.".into(),
        "Selector counts include hidden and disabled markup. Candidate extraction applies the parser's visibility and purchase-signal checks.".into(),
        "Validate ticket type, price/currency/fee scope and quantity meaning against the visible page before enabling a profile. Never sum overlapping offers into total stock.".into(),
        observation.detail,
    ];
    if !source.sidecar_present {
        notes.push("No capture sidecar: event identity, response status, capture time and computed browser visibility are unverified.".into());
    } else {
        notes.push("The local sidecar describes this capture but is editable; it is not independent proof of freshness or authenticity.".into());
    }
    Ok(ProfileReport {
        review_only: true,
        configured_verified: watch.parser.selectors_verified,
        watch_id: watch.id.clone(),
        source,
        selector_counts_are_raw: true,
        selectors,
        unverified_candidates: observation.offers.into_iter().take(PREVIEW_LIMIT).collect(),
        candidates_truncated,
        notes,
    })
}
