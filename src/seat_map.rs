//! Event-bound static map references and configured selection sections.
//! This module does not download images or infer geometry, stock, or adjacency.
use crate::{
    event_identity,
    page_data::UniqueJson,
    parser::{hidden_element, visible_text},
};
use anyhow::{Context, Result, ensure};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};

const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MapVenue {
    /// Regional website ID; it is not interchangeable with a Discovery ID.
    pub regional_id: String,
    pub discovery_id: Option<String>,
    pub name: String,
    pub city: String,
    pub country: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct StaticMapReference {
    pub url: String,
    /// Hash of the reference, venue scope and configured sections, not image bytes.
    /// This identifies an unverified layout candidate, never permanent geometry.
    pub layout_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct MapSection {
    pub id: String,
    pub title: String,
    pub raw_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct PriceSectionMapping {
    /// A configuration ID, not a monetary amount or an available offer.
    pub price_id: String,
    pub section_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq)]
pub struct SeatMapSnapshot {
    pub event_url: String,
    pub event_id: String,
    pub event_name: String,
    pub site_host: String,
    pub venue: MapVenue,
    pub static_map: Option<StaticMapReference>,
    /// Selection zones may be floors or general-admission areas, not seat blocks.
    /// None means a bound successful selection configuration was not exposed.
    pub configured_sections: Option<Vec<MapSection>>,
    pub price_section_mappings: Option<Vec<PriceSectionMapping>>,
    pub visible_canvas_count: usize,
    pub visible_iframe_count: usize,
    pub evidence: &'static str,
    pub geometry_verified: bool,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
    pub image_content_verified: bool,
    pub displayed_reference_matched: bool,
    /// Observed vector section outlines, not individual seats or live inventory.
    pub rendered_geometry: Option<crate::map_geometry::MapGeometry>,
    /// Explicit rendered seat labels and coordinates; not a stock count.
    pub rendered_seats: Option<crate::seat_geometry::RenderedSeats>,
}

/// Inspect an already captured page. The caller owns transport/freshness validation.
pub fn extract_seat_map(
    expected_url: &str,
    actual_url: &str,
    html: &str,
) -> Result<SeatMapSnapshot> {
    ensure!(
        html.len() <= MAX_HTML_BYTES,
        "seat map HTML exceeds size limit"
    );
    let (site_host, event_id) = event_identity::identity(expected_url)
        .context("seat map requires a public Ticketmaster event URL")?;
    ensure!(
        event_identity::same_event(expected_url, actual_url),
        "seat map event redirect mismatch"
    );
    let doc = Html::parse_document(html);
    reject_challenge(&doc)?;
    let canonicals: Vec<_> = doc.select(&select("link[rel=canonical]")).collect();
    ensure!(
        canonicals.len() == 1,
        "seat map requires one canonical event URL"
    );
    let event_url = canonicals[0]
        .value()
        .attr("href")
        .context("missing canonical event URL")?;
    ensure!(
        event_identity::same_event(expected_url, event_url),
        "seat map canonical event mismatch"
    );
    let parsed_event = crate::config::validate_public_url(event_url)?;
    let mut canonical_event = parsed_event.clone();
    canonical_event.set_query(None);
    canonical_event.set_fragment(None);
    for meta in doc.select(&select("meta[property='og:url']")) {
        ensure!(
            meta.value()
                .attr("content")
                .is_some_and(|s| event_identity::same_event(expected_url, s)),
            "seat map social event mismatch"
        );
    }
    let scripts: Vec<_> = doc.select(&select("script[id='__NEXT_DATA__']")).collect();
    ensure!(
        scripts.len() == 1 && scripts[0].value().attr("type") == Some("application/json"),
        "unsupported seat map metadata format"
    );
    let raw = scripts[0].inner_html();
    ensure!(
        raw.len() <= MAX_JSON_BYTES,
        "seat map metadata exceeds size limit"
    );
    let next: UniqueJson =
        serde_json::from_str(&raw).context("invalid or ambiguous seat map metadata")?;
    let state = next
        .0
        .pointer("/props/pageProps/initialReduxState")
        .context("unsupported seat map page state")?;
    let event = &state["eventInfo"];
    for value in [
        &next.0["query"]["eventId"],
        &state["eventId"],
        &event["eventId"],
        &event["params"],
    ] {
        ensure!(
            value.as_str() == Some(event_id.as_str()),
            "seat map metadata event mismatch"
        );
    }
    ensure!(
        event["stage"] == "success" && event.get("inFlightParams") == Some(&Value::Null),
        "seat map event metadata unfinished"
    );
    let embedded = event["canonicalUrl"]
        .as_str()
        .context("missing bound map canonical URL")?;
    let embedded_url = if embedded.starts_with('/') {
        format!("https://{site_host}{embedded}")
    } else {
        embedded.to_owned()
    };
    ensure!(
        event_identity::same_event(expected_url, &embedded_url),
        "seat map embedded canonical mismatch"
    );
    let market = market_for_host(&site_host).context("unsupported map market")?;
    ensure!(
        event["marketCode"] == market && state["config"]["countryCode"] == market,
        "seat map market configuration mismatch"
    );
    let event_name = label(&event["name"])?;
    let headings: BTreeSet<_> = doc
        .select(&select("h1"))
        .filter(|el| !hidden_element(*el))
        .map(visible_text)
        .filter(|s| !s.is_empty())
        .collect();
    ensure!(
        headings.len() == 1 && headings.contains(&event_name),
        "rendered map event name mismatch"
    );
    let queries = state
        .pointer("/api/queries")
        .and_then(Value::as_object)
        .context("missing bound map event query")?;
    ensure!(queries.len() <= 256, "too many map page queries");
    let bound_event = bound_query(queries, "eventInfo", "id", &event_id)?;
    for key in ["eventId", "canonicalUrl", "name", "marketCode", "venue"] {
        ensure!(
            event.get(key).is_some() && event.get(key) == bound_event.get(key),
            "map event query and initial state disagree"
        );
    }

    let regions: Vec<_> = doc
        .select(&select("aside[aria-label]"))
        .filter(|el| !hidden_element(*el))
        .filter(|el| {
            matches!(
                el.value().attr("aria-label"),
                Some(
                    "Mapa míst"
                        | "Mapa Obiektu"
                        | "Seat Map"
                        | "Arenalayout"
                        | "Mapa de Asientos"
                        | "Saalplan"
                        | "Stoelplattegrond"
                )
            )
        })
        .collect();
    ensure!(
        regions.len() == 1,
        "missing or ambiguous visible map region"
    );
    let visible_canvas_count = regions[0]
        .select(&select("canvas"))
        .filter(|e| !hidden_element(*e))
        .count();
    let visible_iframe_count = regions[0]
        .select(&select("iframe"))
        .filter(|e| !hidden_element(*e))
        .count();
    ensure!(
        visible_canvas_count <= 32 && visible_iframe_count <= 32,
        "too many map rendering surfaces"
    );

    let raw_venue = &event["venue"];
    let discovery_id = match raw_venue.get("discoId") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let id = identifier(value)?;
            ensure!(
                (id.starts_with('Z') || id.starts_with("Kov"))
                    && id.len() >= 8
                    && id.bytes().all(|b| b.is_ascii_alphanumeric()),
                "invalid Discovery venue namespace"
            );
            Some(id)
        }
    };
    let country = raw_venue["country"]
        .as_str()
        .context("missing map venue country")?;
    ensure!(
        country.len() == 2 && country.bytes().all(|b| b.is_ascii_uppercase()),
        "invalid map venue country"
    );
    let venue = MapVenue {
        regional_id: identifier(&raw_venue["id"])?,
        discovery_id,
        name: label(&raw_venue["name"])?,
        city: label(&raw_venue["city"])?,
        country: country.to_owned(),
    };
    let (configured_sections, price_section_mappings) =
        configured_sections(state, queries, &event_id)?;
    let static_map = match raw_venue.get("staticMap") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let url =
                validated_static_map_url(value.as_str().context("invalid static map reference")?)?;
            let scope = serde_json::to_vec(&(
                &site_host,
                &venue.regional_id,
                &venue.country,
                &url,
                &configured_sections,
            ))?;
            let layout_fingerprint = format!("map-{:x}", Sha256::digest(scope));
            Some(StaticMapReference {
                url,
                layout_fingerprint,
            })
        }
    };
    let mut displayed_reference_matched = false;
    for image in regions[0]
        .select(&select("img[src]"))
        .filter(|e| !hidden_element(*e))
    {
        if let Some(source) = image.value().attr("src")
            && let Ok(source) = validated_static_map_url(source)
        {
            let responsive = image
                .value()
                .attr("srcset")
                .is_some_and(|s| !s.trim().is_empty())
                || image
                    .ancestors()
                    .filter_map(ElementRef::wrap)
                    .filter(|e| e.value().name() == "picture")
                    .any(|picture| {
                        picture.select(&select("source[srcset]")).any(|source| {
                            source
                                .value()
                                .attr("srcset")
                                .is_some_and(|s| !s.trim().is_empty())
                        })
                    });
            ensure!(
                !responsive,
                "responsive map image needs a captured current source"
            );
            ensure!(
                static_map
                    .as_ref()
                    .is_some_and(|reference| reference.url == source),
                "visible map image conflicts with embedded map reference"
            );
            displayed_reference_matched = true;
        }
    }
    let rendered_seats = crate::seat_geometry::extract_rendered_seats(regions[0])?;
    let rendered_geometry = crate::map_geometry::extract_svg_geometry(regions[0])?;
    let evidence = if rendered_seats.is_some() {
        "rendered_seats_and_configuration"
    } else if rendered_geometry.is_some() {
        "rendered_svg_sections_and_configuration"
    } else {
        "static_reference_configuration"
    };
    Ok(SeatMapSnapshot {
        event_url: canonical_event.to_string(),
        event_id,
        event_name,
        site_host,
        venue,
        static_map,
        configured_sections,
        price_section_mappings,
        visible_canvas_count,
        visible_iframe_count,
        evidence,
        geometry_verified: false,
        stock_verified: false,
        adjacency_verified: false,
        image_content_verified: false,
        displayed_reference_matched,
        rendered_geometry,
        rendered_seats,
    })
}

type SectionConfiguration = (Option<Vec<MapSection>>, Option<Vec<PriceSectionMapping>>);

fn configured_sections(
    state: &Value,
    queries: &serde_json::Map<String, Value>,
    event_id: &str,
) -> Result<SectionConfiguration> {
    let selection = &state["ticketSelection"];
    if selection["stage"] != "success" {
        return Ok((None, None));
    }
    ensure!(
        selection.get("inFlightParams") == Some(&Value::Null)
            && selection["params"]["eventId"].as_str() == Some(event_id),
        "map selection configuration is stale or unbound"
    );
    let data = bound_query(queries, "ticketSelection", "eventId", event_id)?;
    for key in ["sections", "priceToSections"] {
        ensure!(
            selection.get(key).is_some() && selection.get(key) == data.get(key),
            "map section query and initial state disagree"
        );
    }
    ensure!(
        selection.get("priceLevels") == data.get("priceLevels"),
        "map price-level query and initial state disagree"
    );
    let mut sections = BTreeMap::new();
    for section in array(&selection["sections"], 1000)? {
        let id = identifier(&section["id"])?;
        let raw_id = section.get("rawId").map(identifier).transpose()?;
        ensure!(
            sections
                .insert(
                    id.clone(),
                    MapSection {
                        id,
                        title: label(&section["title"])?,
                        raw_id
                    }
                )
                .is_none(),
            "duplicate map section ID"
        );
    }
    // UK/AU captures expose event-bound mapping IDs without a priceLevels array.
    // Preserve those declared IDs, without inventing a label or amount for them.
    let prices = match selection.get("priceLevels") {
        None | Some(Value::Null) => None,
        Some(value) => {
            let mut prices = BTreeSet::new();
            for price in array(value, 1024)? {
                ensure!(
                    prices.insert(identifier(&price["id"])?),
                    "duplicate map price level ID"
                );
            }
            Some(prices)
        }
    };
    let mut mappings = BTreeSet::new();
    for mapping in array(&selection["priceToSections"], 4096)? {
        let price_id = identifier(&mapping["priceIdentifier"])?;
        let section_id = identifier(&mapping["sectionIdentifier"])?;
        ensure!(
            prices
                .as_ref()
                .is_none_or(|known| known.contains(&price_id))
                && sections.contains_key(&section_id),
            "map mapping references unknown section or price"
        );
        ensure!(
            mappings.insert((price_id, section_id)),
            "duplicate map section mapping"
        );
    }
    Ok((
        Some(sections.into_values().collect()),
        Some(
            mappings
                .into_iter()
                .map(|(price_id, section_id)| PriceSectionMapping {
                    price_id,
                    section_id,
                })
                .collect(),
        ),
    ))
}

fn bound_query<'a>(
    queries: &'a serde_json::Map<String, Value>,
    endpoint: &str,
    arg: &str,
    event_id: &str,
) -> Result<&'a Value> {
    let prefix = format!("{endpoint}(");
    let found: Vec<_> = queries
        .iter()
        .filter(|(key, value)| key.starts_with(&prefix) || value["endpointName"] == endpoint)
        .collect();
    ensure!(found.len() == 1, "missing or ambiguous bound map query");
    let (key, value) = found[0];
    let raw = key
        .strip_prefix(&prefix)
        .and_then(|v| v.strip_suffix(')'))
        .context("invalid bound map query identity")?;
    let args: UniqueJson =
        serde_json::from_str(raw).context("invalid bound map query arguments")?;
    ensure!(
        args.0[arg].as_str() == Some(event_id)
            && value["originalArgs"][arg].as_str() == Some(event_id)
            && value["endpointName"] == endpoint
            && value["status"] == "fulfilled",
        "map query event mismatch or unfinished"
    );
    ensure!(value["data"].is_object(), "missing bound map query data");
    Ok(&value["data"])
}

/// Public reference policy, shared with saved venue layouts. Never performs I/O.
pub(crate) fn validated_static_map_url(input: &str) -> Result<String> {
    ensure!(
        !input.is_empty()
            && input.len() <= 1024
            && input.is_ascii()
            && input.trim() == input
            && !input.bytes().any(|b| b.is_ascii_control())
            && !input.contains(['\\', '%', '?', '#']),
        "invalid static map URL"
    );
    let absolute = if input.starts_with("//") {
        format!("https:{input}")
    } else {
        input.to_owned()
    };
    let url = url::Url::parse(&absolute).context("invalid static map URL")?;
    ensure!(
        url.scheme() == "https"
            && url.username().is_empty()
            && url.password().is_none()
            && url.port().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "static map requires a public HTTPS image reference"
    );
    let raw_authority_path = absolute
        .strip_prefix("https://")
        .context("invalid static map scheme")?;
    let slash = raw_authority_path
        .find('/')
        .context("missing static map path")?;
    ensure!(
        !raw_authority_path[..slash].contains(':'),
        "static map ports are not supported"
    );
    let raw_path = &raw_authority_path[slash..];
    ensure!(
        raw_path == url.path()
            && !raw_path.contains("//")
            && raw_path.split('/').skip(1).all(|part| !part.is_empty()
                && part != "."
                && part != ".."
                && part
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.'))),
        "invalid static map path"
    );
    let allowed_prefix = match url.host_str() {
        Some("media.ticketmaster.eu") => raw_path.split('/').count() == 3,
        Some("media.ticketmaster.co.uk") => raw_path.starts_with("/tm/en-gb/tmimages/venue/maps/"),
        Some("s1.ticketm.net") => raw_path.starts_with("/tm/en-au/tmimages/venue/maps/"),
        _ => false,
    };
    ensure!(
        allowed_prefix
            && [".png", ".jpg", ".jpeg", ".gif", ".webp"]
                .iter()
                .any(|suffix| raw_path.ends_with(suffix)),
        "unsupported static map image host or path"
    );
    Ok(url.to_string())
}

fn market_for_host(host: &str) -> Option<&'static str> {
    Some(match host {
        "ticketmaster.cz" => "CZ",
        "ticketmaster.pl" => "PL",
        "ticketmaster.be" => "BE",
        "ticketmaster.de" => "DE",
        "ticketmaster.es" => "ES",
        "ticketmaster.se" => "SE",
        "ticketmaster.nl" => "NL",
        "ticketmaster.co.uk" => "UK",
        "ticketmaster.com.au" => "AU",
        _ => return None,
    })
}
fn reject_challenge(doc: &Html) -> Result<()> {
    let text = visible_text(doc.root_element()).to_lowercase();
    ensure!(
        ![
            "verify you are human",
            "verify that you are human",
            "verify you're human",
            "access denied",
            "request blocked",
            "complete the captcha",
            "complete the recaptcha",
            "waiting room",
            "you are in the queue",
            "your session has expired",
            "your browsing activity has been paused",
            "we've paused activity",
            "we’ve paused activity",
            "pardon the interruption",
            "ověřte, že jste člověk",
            "přístup odepřen",
            "platnost relace vypršela"
        ]
        .iter()
        .any(|s| text.contains(s)),
        "map page requires verification or session recovery"
    );
    for frame in doc
        .select(&select("iframe"))
        .filter(|e| !hidden_element(*e))
    {
        let title = frame.value().attr("title").unwrap_or("").to_lowercase();
        ensure!(
            !title.contains("captcha") && !title.contains("challenge"),
            "map page requires human verification"
        );
    }
    Ok(())
}
fn select(value: &str) -> Selector {
    Selector::parse(value).expect("constant selector")
}
fn identifier(value: &Value) -> Result<String> {
    let value = value.as_str().context("missing map identifier")?;
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b'.')),
        "invalid map identifier"
    );
    Ok(value.to_owned())
}
fn label(value: &Value) -> Result<String> {
    let value = value.as_str().context("missing map label")?;
    ensure!(
        value.len() <= 600 && !value.chars().any(char::is_control),
        "invalid map label"
    );
    let label = value.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(!label.is_empty(), "empty map label");
    Ok(label)
}
fn array(value: &Value, limit: usize) -> Result<&Vec<Value>> {
    let values = value
        .as_array()
        .context("missing map configuration array")?;
    ensure!(
        values.len() <= limit,
        "map configuration array exceeds limit"
    );
    Ok(values)
}
