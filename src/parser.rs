//! Conservative page parsing: missing or ambiguous evidence is never sold-out evidence.
use crate::config::{ParserConfig, Watch};
use crate::model::{Availability, Evidence, Observation, Offer, QuantityKind};
use anyhow::{Result, bail};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

const MAX_HTML_BYTES: usize = 16 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 256 * 1024;
const MAX_OFFERS: usize = 1000;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct DiscoveredEvent {
    pub url: String,
    pub title: String,
}

struct Selectors {
    available: Option<Selector>,
    unavailable: Option<Selector>,
    offers: Option<Selector>,
    offer_available: Option<Selector>,
    complete: Option<Selector>,
    section: Option<Selector>,
    row: Option<Selector>,
    seats: Option<Selector>,
    ticket_type: Option<Selector>,
    inventory: Option<Selector>,
    price: Option<Selector>,
    quantity: Option<Selector>,
}

fn selector(value: &Option<String>) -> std::result::Result<Option<Selector>, &'static str> {
    value
        .as_ref()
        .map(|s| Selector::parse(s).map_err(|_| "invalid configured CSS selector"))
        .transpose()
}

impl Selectors {
    fn new(p: &ParserConfig) -> std::result::Result<Self, &'static str> {
        Ok(Self {
            available: selector(&p.available_selector)?,
            unavailable: selector(&p.unavailable_selector)?,
            offers: selector(&p.offers_selector)?,
            offer_available: selector(&p.offer_available_selector)?,
            complete: selector(&p.complete_selector)?,
            section: selector(&p.section_selector)?,
            row: selector(&p.row_selector)?,
            seats: selector(&p.seats_selector)?,
            ticket_type: selector(&p.ticket_type_selector)?,
            inventory: selector(&p.inventory_selector)?,
            price: selector(&p.price_selector)?,
            quantity: selector(&p.quantity_selector)?,
        })
    }
}

fn normalize(text: &str, limit: usize) -> String {
    text.split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(limit)
        .collect()
}

pub(crate) fn hidden_element(element: ElementRef<'_>) -> bool {
    let mut body = false;
    let mut modal = false;
    for ancestor in element.ancestors().chain(std::iter::once(*element)) {
        if let Some(e) = ancestor.value().as_element() {
            if matches!(e.name(), "script" | "style" | "template" | "noscript")
                || e.attr("hidden").is_some()
                || e.attr("inert").is_some()
                || e.attr("aria-hidden") == Some("true")
                || e.attr("style").is_some_and(inline_hidden)
            {
                return true;
            }
            body |= e.name() == "body";
            modal |= e.name() == "html" && e.attr("data-ticketwatch-native-modal") == Some("open");
        }
    }
    // Negative-only browser marker: no page evidence through a native modal.
    // Head metadata remains readable to identity validators.
    body && modal
}

pub(crate) fn inline_hidden(style: &str) -> bool {
    style.split(';').any(|declaration| {
        let Some((property, value)) = declaration.split_once(':') else {
            return false;
        };
        let property = property.trim().to_ascii_lowercase();
        let value = value.trim().to_ascii_lowercase();
        let value = value.strip_suffix("!important").unwrap_or(&value).trim();
        matches!(
            (property.as_str(), value),
            ("display", "none")
                | ("visibility", "hidden" | "collapse")
                | ("content-visibility", "hidden")
        ) || (property == "opacity"
            && value
                .trim_end_matches('%')
                .parse::<f64>()
                .is_ok_and(|number| number == 0.0))
    })
}

pub(crate) fn visible_text(element: ElementRef<'_>) -> String {
    let mut result = String::new();
    for node in element.descendants() {
        let Some(text) = node.value().as_text() else {
            continue;
        };
        let hidden = node.ancestors().any(|ancestor| {
            ancestor.value().as_element().is_some_and(|e| {
                matches!(e.name(), "script" | "style" | "template" | "noscript")
                    || e.attr("hidden").is_some()
                    || e.attr("inert").is_some()
                    || e.attr("aria-hidden") == Some("true")
                    || e.attr("style").is_some_and(inline_hidden)
            })
        });
        if !hidden {
            result.push_str(text);
            result.push(' ');
            if result.len() > 32_768 {
                break;
            }
        }
    }
    normalize(&result, 16_384)
}

fn challenge_reason(doc: &Html) -> Option<&'static str> {
    let text = visible_text(doc.root_element()).to_lowercase();
    [
        (
            "your browsing activity has been paused",
            "site paused browsing activity",
        ),
        ("we’ve paused activity", "site paused browsing activity"),
        ("we've paused activity", "site paused browsing activity"),
        ("access denied", "access-denied page"),
        ("request blocked", "access-denied page"),
        ("pardon the interruption", "human verification required"),
        ("verify you are human", "human verification required"),
        ("verify that you are human", "human verification required"),
        ("verify you're human", "human verification required"),
        ("complete the captcha", "human verification required"),
        ("solve the captcha", "human verification required"),
        ("complete the recaptcha", "human verification required"),
        ("you are now in line", "waiting room or queue"),
        ("you are in the queue", "waiting room or queue"),
        ("you're in the queue", "waiting room or queue"),
        ("your place in the queue", "waiting room or queue"),
        ("waiting room", "waiting room or queue"),
        ("checking your browser", "human verification required"),
        ("enable cookies to continue", "human verification required"),
        ("your session has expired", "session expired"),
    ]
    .into_iter()
    .find_map(|(needle, reason)| text.contains(needle).then_some(reason))
    .or_else(|| {
        let headings = Selector::parse("title, h1, h2").expect("constant selector");
        doc.select(&headings)
            .any(|el| {
                let title = visible_text(el).to_lowercase();
                matches!(
                    title.as_str(),
                    "waiting room" | "ticketmaster waiting room" | "just a moment..."
                )
            })
            .then_some("waiting room or verification page")
    })
}

fn elements<'a>(row: ElementRef<'a>, selector: &Selector) -> Vec<ElementRef<'a>> {
    std::iter::once(row)
        .filter(|element| selector.matches(element))
        .chain(row.select(selector))
        .filter(|element| !hidden_element(*element))
        .take(100)
        .collect()
}

fn field(row: ElementRef<'_>, selector: &Option<Selector>) -> Option<String> {
    let values = elements(row, selector.as_ref()?)
        .into_iter()
        .map(|element| normalize(&visible_text(element), 240))
        .filter(|text| !text.is_empty())
        .collect::<BTreeSet<_>>();
    (values.len() == 1)
        .then(|| values.into_iter().next())
        .flatten()
}

pub(crate) fn disabled_element(element: ElementRef<'_>) -> bool {
    element
        .ancestors()
        .chain(std::iter::once(*element))
        .any(|ancestor| {
            ancestor.value().as_element().is_some_and(|e| {
                e.attr("inert").is_some()
                    || e.attr("aria-disabled")
                        .is_some_and(|value| value.eq_ignore_ascii_case("true"))
                    || (e.attr("disabled").is_some()
                        && matches!(
                            e.name(),
                            "button"
                                | "input"
                                | "select"
                                | "textarea"
                                | "option"
                                | "fieldset"
                                | "optgroup"
                        ))
            })
        })
}

fn purchasable(row: ElementRef<'_>, selector: &Option<Selector>) -> bool {
    selector.as_ref().is_some_and(|selector| {
        elements(row, selector)
            .into_iter()
            .any(|element| !disabled_element(element))
    })
}

pub fn effective_quantity_kind(parser: &ParserConfig) -> QuantityKind {
    if parser.quantity_kind == QuantityKind::Unknown && parser.quantity_is_stock {
        QuantityKind::Remaining
    } else {
        parser.quantity_kind
    }
}

fn parse_quantity(text: &str) -> Option<u32> {
    let value = text.trim();
    if value.is_empty() || value.len() > 7 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|count| *count <= 1_000_000)
}

fn quantity(
    row: ElementRef<'_>,
    selector: &Option<Selector>,
    kind: QuantityKind,
) -> (Option<u32>, QuantityKind) {
    let Some(selector) = selector else {
        return (None, QuantityKind::Unknown);
    };
    let nodes = elements(row, selector);
    if nodes.len() != 1 || disabled_element(nodes[0]) {
        return (None, QuantityKind::Unknown);
    }
    let element = nodes[0];
    let (number, meaning) = if element.value().name() == "select" {
        let options = Selector::parse("option").expect("constant selector");
        let maximum = element
            .select(&options)
            .take(1000)
            .filter(|option| !hidden_element(*option) && !disabled_element(*option))
            .filter_map(|option| match option.value().attr("value") {
                Some(value) => parse_quantity(value),
                None => parse_quantity(&visible_text(option)),
            })
            .max();
        (maximum, QuantityKind::Selectable)
    } else {
        (parse_quantity(&visible_text(element)), kind)
    };
    (
        number,
        if number.is_some() {
            meaning
        } else {
            QuantityKind::Unknown
        },
    )
}

fn matches(doc: &Html, selector: &Option<Selector>) -> bool {
    selector.as_ref().is_some_and(|s| {
        doc.select(s)
            .any(|e| !hidden_element(e) && !disabled_element(e))
    })
}

/// Parse only independently validated selectors or URL-matched structured metadata.
pub fn parse(watch: &Watch, html: &str, observed_at: i64) -> Observation {
    if html.len() > MAX_HTML_BYTES {
        return Observation::unknown(&watch.id, observed_at, "page exceeds parser size limit");
    }
    let doc = Html::parse_document(html);
    if let Some(reason) = challenge_reason(&doc) {
        let mut result = Observation::unknown(&watch.id, observed_at, reason);
        result.availability = Availability::Blocked;
        return result;
    }
    if doc
        .root_element()
        .value()
        .attr("data-ticketwatch-native-modal")
        == Some("open")
    {
        return Observation::unknown(
            &watch.id,
            observed_at,
            "native modal open; page evidence unavailable",
        );
    }

    if watch.parser.adapter == crate::config::PageAdapter::Ticketmaster {
        return crate::ticketmaster::parse(watch, &doc, observed_at);
    }

    if watch.parser.selectors_verified {
        let selectors = match Selectors::new(&watch.parser) {
            Ok(s) => s,
            Err(reason) => return Observation::unknown(&watch.id, observed_at, reason),
        };
        if selectors.offers.is_some() && selectors.offer_available.is_none() {
            return Observation::unknown(
                &watch.id,
                observed_at,
                "offer rows require a verified row-relative purchasable signal",
            );
        }
        let available = matches(&doc, &selectors.available);
        let unavailable = matches(&doc, &selectors.unavailable);
        let complete = matches(&doc, &selectors.complete);
        let mut offers = BTreeMap::<String, Offer>::new();
        let mut zero_quantities = false;
        if let Some(offer_selector) = &selectors.offers {
            for (index, row) in doc
                .select(offer_selector)
                .filter(|e| !hidden_element(*e))
                .enumerate()
            {
                if index >= MAX_OFFERS {
                    return Observation::unknown(
                        &watch.id,
                        observed_at,
                        "offer limit exceeded; snapshot is incomplete",
                    );
                }
                if !purchasable(row, &selectors.offer_available) {
                    continue;
                }
                let section = field(row, &selectors.section);
                let row_text = field(row, &selectors.row);
                let seats = field(row, &selectors.seats);
                let ticket_type = field(row, &selectors.ticket_type);
                let inventory = field(row, &selectors.inventory);
                let price_text = field(row, &selectors.price);
                let currency = watch.currency.as_ref().map(|s| s.to_ascii_uppercase());
                let price_minor = price_text.as_ref().and_then(|p| {
                    parse_price_minor(
                        p,
                        currency.as_deref()?,
                        &watch.parser.price_decimal_separator,
                    )
                });
                let (quantity, quantity_kind) = quantity(
                    row,
                    &selectors.quantity,
                    effective_quantity_kind(&watch.parser),
                );
                if quantity == Some(0) {
                    zero_quantities = true;
                    continue;
                }
                let upstream_id = watch
                    .parser
                    .offer_id_attribute
                    .as_ref()
                    .and_then(|attr| row.value().attr(attr.as_str()))
                    .map(|s| normalize(s, 240))
                    .filter(|s| !s.is_empty());
                let key = if let Some(id) = upstream_id {
                    format!("id:{id}")
                } else {
                    if section.is_none()
                        && row_text.is_none()
                        && seats.is_none()
                        && ticket_type.is_none()
                    {
                        return Observation::unknown(
                            &watch.id,
                            observed_at,
                            "offer row lacks stable identity",
                        );
                    }
                    let identity = serde_json::to_vec(&(
                        &section,
                        &row_text,
                        &seats,
                        &ticket_type,
                        &inventory,
                    ))
                    .expect("serializable strings");
                    format!("composite:{:x}", Sha256::digest(identity))
                };
                let offer = Offer {
                    key: key.clone(),
                    section,
                    row: row_text,
                    seats,
                    ticket_type,
                    inventory,
                    price_text,
                    price_minor,
                    currency,
                    quantity,
                    quantity_kind,
                    price_scope: watch.parser.price_scope,
                    fees: watch.parser.fees,
                };
                if let Some(previous) = offers.get(&key) {
                    if previous != &offer {
                        return Observation::unknown(
                            &watch.id,
                            observed_at,
                            "ambiguous duplicate offer identity",
                        );
                    }
                } else {
                    offers.insert(key, offer);
                }
            }
        }
        if (available || !offers.is_empty()) && unavailable {
            return Observation::unknown(
                &watch.id,
                observed_at,
                "conflicting available and unavailable indicators",
            );
        }
        if !offers.is_empty() {
            return Observation { watch_id: watch.id.clone(), observed_at,
                availability: Availability::Available, evidence: Evidence::Offers, complete,
                offers: offers.into_values().collect(), ticket_categories: vec![], adapter: None,
                detail: "ticket offers observed through configured verified selectors; prices and stock counts remain source-qualified".into() };
        }
        if unavailable {
            return Observation {
                watch_id: watch.id.clone(),
                observed_at,
                availability: Availability::Unavailable,
                evidence: Evidence::ExplicitStatus,
                complete,
                offers: vec![],
                ticket_categories: vec![],
                adapter: None,
                detail: "explicit unavailable indicator observed".into(),
            };
        }
        if available && !zero_quantities {
            return Observation {
                watch_id: watch.id.clone(),
                observed_at,
                availability: Availability::Available,
                evidence: Evidence::ExplicitStatus,
                complete: false,
                offers: vec![],
                ticket_categories: vec![],
                adapter: None,
                detail:
                    "explicit page availability indicator; seat count and price are unconfirmed"
                        .into(),
            };
        }
        // Configured selectors taking priority prevents stale metadata from hiding a changed layout.
        if selectors.available.is_some()
            || selectors.unavailable.is_some()
            || selectors.offers.is_some()
        {
            return Observation::unknown(
                &watch.id,
                observed_at,
                "verified inventory indicators absent or incomplete",
            );
        }
    }
    metadata_observation(watch, &doc, observed_at)
}

fn currency_digits(currency: &str) -> Option<u32> {
    match currency {
        "JPY" | "KRW" | "CLP" | "VND" => Some(0),
        "BHD" | "KWD" | "JOD" | "OMR" | "TND" => Some(3),
        "USD" | "EUR" | "GBP" | "PLN" | "CZK" | "CHF" | "SEK" | "NOK" | "DKK" | "AUD" | "CAD"
        | "NZD" | "BRL" | "ZAR" | "MXN" | "AED" | "TRY" | "CNY" | "HKD" | "SGD" | "INR" | "HUF"
        | "RON" | "BGN" | "ILS" | "PEN" => Some(2),
        _ => None,
    }
}

fn currency_marker(marker: &str, currency: &str) -> bool {
    let marker = marker.trim();
    marker.is_empty()
        || marker.eq_ignore_ascii_case(currency)
        || match currency {
            "USD" | "CAD" | "AUD" | "NZD" | "MXN" => marker == "$",
            "EUR" => marker == "€",
            "GBP" => marker == "£",
            "PLN" => marker == "zł",
            "CZK" => marker == "Kč",
            "NOK" | "SEK" | "DKK" => marker == "kr",
            "JPY" | "CNY" => marker == "¥",
            _ => false,
        }
}

/// Exact source-currency minor units. Ambiguous ranges, fee expressions and unknown currencies are rejected.
pub fn parse_price_minor(text: &str, currency: &str, separator: &str) -> Option<i64> {
    let digits = currency_digits(currency)?;
    let decimal = match separator {
        "" | "." => '.',
        "," => ',',
        _ => return None,
    };
    let grouping = if decimal == '.' { ',' } else { '.' };
    let text = text.replace(['\u{00a0}', '\u{202f}'], " ");
    let first = text.find(|c: char| c.is_ascii_digit())?;
    let last = text.rfind(|c: char| c.is_ascii_digit())?;
    if !currency_marker(&text[..first], currency) || !currency_marker(&text[last + 1..], currency) {
        return None;
    }
    let number = &text[first..last + 1];
    let parts = number.split(decimal).collect::<Vec<_>>();
    if parts.len() > 2 {
        return None;
    }
    let integer = parts[0];
    let fraction = if parts.len() == 2 {
        if digits == 0
            || parts[1].is_empty()
            || parts[1].len() > digits as usize
            || !parts[1].bytes().all(|b| b.is_ascii_digit())
        {
            return None;
        }
        parts[1]
    } else {
        ""
    };
    if integer.is_empty() {
        return None;
    }
    let grouped_space = integer.contains(' ');
    let grouped_symbol = integer.contains(grouping);
    if grouped_space && grouped_symbol {
        return None;
    }
    let groups = if grouped_space {
        integer.split(' ').collect::<Vec<_>>()
    } else if grouped_symbol {
        integer.split(grouping).collect::<Vec<_>>()
    } else {
        vec![integer]
    };
    if groups
        .iter()
        .any(|s| s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()))
    {
        return None;
    }
    if groups.len() > 1 && (groups[0].len() > 3 || groups[1..].iter().any(|s| s.len() != 3)) {
        return None;
    }
    let integral = groups.concat().parse::<i64>().ok()?;
    let fractional = if fraction.is_empty() {
        0
    } else {
        fraction
            .parse::<i64>()
            .ok()?
            .checked_mul(10_i64.pow(digits - fraction.len() as u32))?
    };
    integral
        .checked_mul(10_i64.pow(digits))?
        .checked_add(fractional)
}

fn is_ticketmaster_host(host: &str) -> bool {
    let host = host.strip_prefix("www.").unwrap_or(host);
    matches!(
        host,
        "ticketmaster.com"
            | "ticketmaster.ca"
            | "ticketmaster.co.uk"
            | "ticketmaster.ie"
            | "ticketmaster.pl"
            | "ticketmaster.cz"
            | "ticketmaster.de"
            | "ticketmaster.at"
            | "ticketmaster.ch"
            | "ticketmaster.be"
            | "ticketmaster.nl"
            | "ticketmaster.fr"
            | "ticketmaster.es"
            | "ticketmaster.it"
            | "ticketmaster.dk"
            | "ticketmaster.no"
            | "ticketmaster.se"
            | "ticketmaster.fi"
            | "ticketmaster.com.au"
            | "ticketmaster.co.nz"
            | "ticketmaster.com.mx"
            | "ticketmaster.ae"
            | "ticketmaster.co.za"
    )
}

pub(crate) fn canonical_event_url(raw: &str, base: Option<&Url>) -> Option<Url> {
    let mut url = Url::parse(raw)
        .or_else(|_| {
            base.ok_or(url::ParseError::RelativeUrlWithoutBase)?
                .join(raw)
        })
        .ok()?;
    if url.scheme() != "https"
        || !url.username().is_empty()
        || url.password().is_some()
        || url.port().is_some_and(|p| p != 443)
        || !is_ticketmaster_host(url.host_str()?)
    {
        return None;
    }
    let path = url.path().to_ascii_lowercase();
    if !path.contains("/event/")
        || path.contains("/payment")
        || path.contains("/checkout")
        || path.contains("/cart")
    {
        return None;
    }
    if url.query_pairs().any(|(key, _)| {
        let key = key.to_ascii_lowercase();
        matches!(
            key.as_str(),
            "token"
                | "session"
                | "sessionid"
                | "session_id"
                | "cart"
                | "cartid"
                | "cart_id"
                | "t"
                | "jwt"
                | "authorization"
        )
    }) {
        return None;
    }
    url.set_query(None);
    url.set_fragment(None);
    Some(url)
}

pub(crate) fn event_identity(url: &Url) -> Option<(String, String)> {
    let host = url
        .host_str()?
        .strip_prefix("www.")
        .unwrap_or(url.host_str()?)
        .to_owned();
    let id = url.path_segments()?.rfind(|s| !s.is_empty())?;
    if id.is_empty() || !id.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    Some((host, id.to_ascii_lowercase()))
}

pub(crate) fn event_matches(value: &Value, expected: &Url) -> bool {
    let event_type = value.get("@type");
    let is_event = |v: &Value| {
        v.as_str()
            .is_some_and(|s| matches!(s.rsplit('/').next().unwrap_or(s), "Event" | "MusicEvent"))
    };
    if !event_type
        .is_some_and(|t| is_event(t) || t.as_array().is_some_and(|a| a.iter().any(is_event)))
    {
        return false;
    }
    let candidate = value
        .get("url")
        .and_then(Value::as_str)
        .or_else(|| value.get("@id").and_then(Value::as_str));
    candidate
        // Empty, query-only and fragment-only references inherit the watched
        // event's path when resolved, but supply no event identity themselves.
        .filter(|raw| {
            // URL resolution strips some C0 controls that str::trim retains.
            // Reject controls before either normalization can erase them.
            !raw.chars().any(char::is_control)
                && !raw.trim().is_empty()
                && !raw.trim().starts_with(['#', '?'])
        })
        .and_then(|raw| canonical_event_url(raw, Some(expected)))
        .and_then(|url| event_identity(&url))
        .zip(event_identity(expected))
        .is_some_and(|(a, b)| a == b)
}

fn offer_status(value: &Value, depth: usize) -> Option<Availability> {
    if depth > 16 {
        return None;
    }
    if let Some(array) = value.as_array() {
        let states = array
            .iter()
            .filter_map(|v| offer_status(v, depth + 1))
            .collect::<Vec<_>>();
        if states.contains(&Availability::Available) {
            return Some(Availability::Available);
        }
        // An incomplete/mixed array does not prove all offers are unavailable.
        return (!array.is_empty()
            && states.len() == array.len()
            && states.iter().all(|s| *s == Availability::Unavailable))
        .then_some(Availability::Unavailable);
    }
    if let Some(s) = value.get("availability").and_then(Value::as_str) {
        match s.rsplit('/').next().unwrap_or(s) {
            "InStock" | "LimitedAvailability" => return Some(Availability::Available),
            "SoldOut" | "OutOfStock" => return Some(Availability::Unavailable),
            _ => {}
        }
    }
    value.get("offers").and_then(|v| offer_status(v, depth + 1))
}

fn collect_events(
    value: &Value,
    expected: &Url,
    states: &mut Vec<Availability>,
    depth: usize,
    remaining: &mut usize,
) {
    if depth > 24 || *remaining == 0 {
        return;
    }
    *remaining -= 1;
    if event_matches(value, expected) {
        if let Some(state) = value.get("offers").and_then(|v| offer_status(v, 0)) {
            states.push(state);
        }
        return;
    }
    match value {
        Value::Array(values) => {
            for v in values {
                collect_events(v, expected, states, depth + 1, remaining);
            }
        }
        Value::Object(values) => {
            for v in values.values() {
                collect_events(v, expected, states, depth + 1, remaining);
            }
        }
        _ => {}
    }
}

fn metadata_observation(watch: &Watch, doc: &Html, observed_at: i64) -> Observation {
    let Some(expected) = canonical_event_url(&watch.url, None) else {
        return Observation::unknown(
            &watch.id,
            observed_at,
            "no validated page indicators; event identity cannot be established",
        );
    };
    let selector =
        Selector::parse("script[type='application/ld+json']").expect("constant selector");
    let mut states = Vec::new();
    let mut remaining = 4096;
    for node in doc.select(&selector).take(16) {
        let raw = node.inner_html();
        if raw.len() > MAX_JSON_BYTES {
            continue;
        }
        if let Ok(json) = serde_json::from_str::<Value>(&raw) {
            collect_events(&json, &expected, &mut states, 0, &mut remaining);
        }
    }
    if let Some(first) = states.first().copied() {
        if states.iter().all(|state| *state == first) {
            return Observation { watch_id: watch.id.clone(), observed_at, availability: first,
                evidence: Evidence::Metadata, complete: false, offers: vec![], ticket_categories: vec![], adapter: None,
                detail: "page structured metadata reports availability; live inventory freshness, quantities and prices are unconfirmed".into() };
        }
        return Observation::unknown(
            &watch.id,
            observed_at,
            "conflicting structured metadata for this event",
        );
    }
    Observation::unknown(
        &watch.id,
        observed_at,
        "no validated inventory indicators found; purchase links alone do not establish availability",
    )
}

/// Discover candidates, never automatically activate them. Only public Ticketmaster event URLs survive.
pub fn discover(html: &str, base_url: &str) -> Result<Vec<DiscoveredEvent>> {
    if html.len() > MAX_HTML_BYTES {
        bail!("page exceeds discovery size limit");
    }
    let base = Url::parse(base_url)?;
    if base.scheme() != "https" || !is_ticketmaster_host(base.host_str().unwrap_or_default()) {
        bail!("discovery requires a public Ticketmaster HTTPS page");
    }
    let doc = Html::parse_document(html);
    if let Some(reason) = challenge_reason(&doc) {
        bail!("discovery blocked: {reason}");
    }
    let selector = Selector::parse("a[href]").expect("constant selector");
    let mut result = Vec::new();
    let mut seen = BTreeSet::new();
    for anchor in doc.select(&selector).take(10_000) {
        let Some(url) =
            canonical_event_url(anchor.value().attr("href").unwrap_or_default(), Some(&base))
        else {
            continue;
        };
        let Some(identity) = event_identity(&url) else {
            continue;
        };
        if !seen.insert(identity) {
            continue;
        }
        let title = normalize(&visible_text(anchor), 240);
        let title = if title.is_empty() {
            normalize(
                anchor
                    .value()
                    .attr("aria-label")
                    .unwrap_or("Ticketmaster event"),
                240,
            )
        } else {
            title
        };
        result.push(DiscoveredEvent {
            url: url.into(),
            title,
        });
        if result.len() >= 1000 {
            break;
        }
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn watch() -> Watch {
        toml::from_str(
            r#"
id = "korn-pl"
name = "KoRn"
artist = "KoRn"
country = "PL"
url = "https://www.ticketmaster.pl/event/korn-bilety/496543017"
currency = "PLN"
[parser]
selectors_verified = true
available_selector = "[data-test=available]"
unavailable_selector = "[data-test=unavailable]"
offers_selector = "[data-test=offer]"
offer_available_selector = ".buy"
complete_selector = "[data-test=complete]"
offer_id_attribute = "data-offer-id"
section_selector = ".section"
row_selector = ".row"
seats_selector = ".seats"
ticket_type_selector = ".type"
inventory_selector = ".inventory"
price_selector = ".price"
quantity_selector = ".quantity"
quantity_is_stock = true
price_scope = "per_ticket"
price_decimal_separator = ","
"#,
        )
        .expect("fixture configuration")
    }

    const OFFERS: &str = include_str!("../tests/fixtures/synthetic-offers.html");

    #[test]
    fn explicit_offers_have_stable_identity_and_exact_price() {
        let first = parse(&watch(), OFFERS, 100);
        assert_eq!(first.availability, Availability::Available);
        assert_eq!(first.evidence, Evidence::Offers);
        assert!(first.complete);
        assert_eq!(first.offers[0].quantity, Some(2));
        assert_eq!(first.offers[0].price_minor, Some(91080));
        let second = parse(
            &watch(),
            &OFFERS.replace("910,80", "890,00").replace(">2<", ">3<"),
            101,
        );
        assert_eq!(first.offers[0].key, second.offers[0].key);
        assert_ne!(first.offers[0].price_minor, second.offers[0].price_minor);
    }

    #[test]
    fn generic_links_missing_rows_and_incomplete_pages_are_unknown() {
        for html in [
            "<a>Buy tickets</a>",
            "<div data-test=complete></div>",
            "<div class=seat-map></div>",
        ] {
            assert_eq!(
                parse(&watch(), html, 100).availability,
                Availability::Unknown
            );
        }
    }

    #[test]
    fn unverified_selectors_never_claim_offers() {
        let mut watch = watch();
        watch.parser.selectors_verified = false;
        let result = parse(&watch, OFFERS, 100);
        assert_eq!(result.availability, Availability::Unknown);
        assert!(result.offers.is_empty());
    }

    #[test]
    fn negative_and_conflicting_indicators() {
        assert_eq!(
            parse(&watch(), "<p data-test=unavailable>Sold out</p>", 100).availability,
            Availability::Unavailable
        );
        assert_eq!(
            parse(
                &watch(),
                "<p data-test=unavailable>Sold out</p><p data-test=available>Available</p>",
                100
            )
            .availability,
            Availability::Unknown
        );
        assert_eq!(
            parse(
                &watch(),
                &(OFFERS.to_owned() + "<p data-test=unavailable>Sold out</p>"),
                100
            )
            .availability,
            Availability::Unknown
        );
    }

    #[test]
    fn challenges_are_blocked_but_script_and_hidden_templates_are_not() {
        assert_eq!(
            parse(&watch(), "<h1>Verify you are human</h1>", 100).availability,
            Availability::Blocked
        );
        assert_eq!(
            parse(&watch(), "<h1>Waiting room</h1>", 100).availability,
            Availability::Blocked
        );
        let html = format!(
            "{OFFERS}<script>var challenge='access denied';</script><div hidden>Verify you are human</div><template>Access denied</template>"
        );
        assert_eq!(
            parse(&watch(), &html, 100).availability,
            Availability::Available
        );
    }

    #[test]
    fn duplicate_rows_never_sum_quantities_and_ambiguous_rows_are_unknown() {
        let result = parse(&watch(), &(OFFERS.to_owned() + OFFERS), 100);
        assert_eq!(result.offers.len(), 1);
        assert_eq!(result.offers[0].quantity, Some(2));
        let ambiguous = OFFERS.to_owned() + &OFFERS.replace("910,80", "890,00");
        assert_eq!(
            parse(&watch(), &ambiguous, 100).availability,
            Availability::Unknown
        );
    }

    #[test]
    fn missing_ids_require_stable_attributes_and_zero_stock_is_not_available() {
        assert_eq!(
            parse(
                &watch(),
                "<div data-test=offer><span class=price>30,00 zł</span></div>",
                100
            )
            .availability,
            Availability::Unknown
        );
        assert_eq!(
            parse(&watch(), &OFFERS.replace(">2<", ">0<"), 100).availability,
            Availability::Unknown
        );
        let mut w = watch();
        w.parser.quantity_is_stock = false;
        let result = parse(&w, OFFERS, 100);
        assert_eq!(result.offers[0].quantity, Some(2));
        assert_eq!(result.offers[0].quantity_kind, QuantityKind::Unknown);
    }

    #[test]
    fn malformed_selectors_are_unknown_not_a_panic() {
        let mut w = watch();
        w.parser.offers_selector = Some("[".into());
        assert_eq!(parse(&w, OFFERS, 100).availability, Availability::Unknown);
    }

    #[test]
    fn hidden_status_and_offer_templates_do_not_become_available() {
        for html in [
            "<p data-test=available hidden>Available</p>",
            "<p data-test=available style='display: none'>Available</p>",
        ] {
            assert_eq!(
                parse(&watch(), html, 100).availability,
                Availability::Unknown
            );
        }
        let hidden = format!("<div aria-hidden=true>{OFFERS}</div>");
        assert_eq!(
            parse(&watch(), &hidden, 100).availability,
            Availability::Unknown
        );
    }

    #[test]
    fn inert_content_cannot_supply_status_offers_or_challenge_text() {
        for html in [
            format!("<section inert>{OFFERS}</section>"),
            "<p data-test=available inert>Available</p>".to_owned(),
            "<section inert><p data-test=unavailable>Sold out</p></section>".to_owned(),
            OFFERS.replace("class=\"buy\"", "class=\"buy\" inert"),
        ] {
            let observation = parse(&watch(), &html, 100);
            assert_eq!(observation.availability, Availability::Unknown);
            assert!(observation.offers.is_empty());
        }
        assert_eq!(
            parse(
                &watch(),
                &format!("{OFFERS}<aside inert><h1>Verify you are human</h1></aside>"),
                100
            )
            .availability,
            Availability::Available
        );
    }

    #[test]
    fn native_modal_capture_marker_blocks_inventory_and_metadata_fallback_but_keeps_diagnostics() {
        let modal = format!(
            "<html data-ticketwatch-native-modal=open><body>{OFFERS}<dialog open>Filter choices</dialog></body></html>"
        );
        let observation = parse(&watch(), &modal, 100);
        assert_eq!(observation.availability, Availability::Unknown);
        assert!(observation.offers.is_empty());
        assert!(observation.detail.contains("native modal open"));
        let mut metadata_watch = watch();
        metadata_watch.parser.selectors_verified = false;
        let metadata = r#"<html data-ticketwatch-native-modal="open"><head><script type="application/ld+json">{"@type":"Event","url":"https://www.ticketmaster.pl/event/korn-bilety/496543017","offers":{"availability":"InStock"}}</script></head><body><dialog open>Filter</dialog></body></html>"#;
        assert_eq!(
            parse(&metadata_watch, metadata, 100).availability,
            Availability::Unknown
        );
        assert_eq!(
            parse(
                &watch(),
                &modal.replace("Filter choices", "Verify you are human"),
                100
            )
            .availability,
            Availability::Blocked
        );
        // Serialized dialog[open] does not establish native showModal() state.
        assert_eq!(
            parse(
                &watch(),
                &modal.replace(" data-ticketwatch-native-modal=open", ""),
                100
            )
            .availability,
            Availability::Available
        );
    }

    #[test]
    fn metadata_requires_matching_event_identity_and_remains_metadata() {
        let mut w = watch();
        w.parser.selectors_verified = false;
        let metadata = r#"<script type="application/ld+json">{"@graph":[{"@type":"MusicEvent","url":"https://www.ticketmaster.pl/event/korn-bilety/496543017","offers":{"@type":"Offer","availability":"https://schema.org/InStock"}}]}</script>"#;
        let result = parse(&w, metadata, 100);
        assert_eq!(result.availability, Availability::Available);
        assert_eq!(result.evidence, Evidence::Metadata);
        assert!(!result.complete);
        assert!(result.offers.is_empty());
        assert_eq!(
            parse(&w, &metadata.replace("496543017", "99999999"), 100).availability,
            Availability::Unknown
        );
        assert_eq!(parse(&w, r#"<script type="application/ld+json">{"@type":"Event","offers":{"url":"https://www.ticketmaster.pl/event/korn-bilety/496543017","availability":"InStock"}}</script>"#, 100).availability, Availability::Unknown);
    }

    #[test]
    fn metadata_cannot_inherit_event_identity_from_empty_query_or_fragment_references() {
        let mut watch = watch();
        watch.parser.selectors_verified = false;
        for field in ["url", "@id"] {
            for reference in [
                "",
                " ",
                "\t\n",
                "#event",
                "?language=en-us",
                "  #event",
                "  ?language=en-us",
                "\0",
                "\0#event",
                "\0?language=en-us",
                "\u{1}#event",
                "\u{1}?language=en-us",
                "/event/korn-bilety/496543017\0",
                "/event/korn-bilety/496543017\t",
            ] {
                let event = serde_json::json!({"@type":"Event",field:reference,"offers":{"availability":"https://schema.org/InStock"}});
                let html = format!("<script type='application/ld+json'>{event}</script>");
                let result = parse(&watch, &html, 100);
                assert_eq!(
                    result.availability,
                    Availability::Unknown,
                    "{field}: {reference:?}"
                );
                assert_eq!(result.evidence, Evidence::None);
            }
            let event = serde_json::json!({"@type":"Event",field:"/event/korn-bilety/496543017","offers":{"availability":"https://schema.org/InStock"}});
            let result = parse(
                &watch,
                &format!("<script type='application/ld+json'>{event}</script>"),
                100,
            );
            assert_eq!(result.availability, Availability::Available);
            assert_eq!(result.evidence, Evidence::Metadata);
        }
    }

    #[test]
    fn money_parsing_rejects_ambiguity_and_handles_currency_units() {
        for (raw, currency, sep, expected) in [
            ("1 234,56 zł", "PLN", ",", Some(123456)),
            ("€1,234.56", "EUR", ".", Some(123456)),
            ("1\u{202f}234,56 PLN", "PLN", ",", Some(123456)),
            ("12", "PLN", ",", Some(1200)),
            ("1,234 JPY", "JPY", ".", Some(1234)),
            ("12.345 KWD", "KWD", ".", Some(12345)),
            ("100 + 20 fee", "PLN", ",", None),
            ("100-200", "PLN", ",", None),
            ("-20", "PLN", ",", None),
            ("1.234", "USD", ".", None),
            ("12,34.56", "USD", ".", None),
            ("2.00 JPY", "JPY", ".", None),
            ("20 EUR", "PLN", ",", None),
            ("20", "XYZ", ".", None),
            ("9223372036854775807.00", "USD", ".", None),
        ] {
            assert_eq!(parse_price_minor(raw, currency, sep), expected, "{raw}");
        }
    }

    #[test]
    fn discovery_only_returns_public_event_urls_and_deduplicates_tracking() {
        let html = r#"
<a href="/event/korn-bilety/496543017?utm_source=demo">KoRn Krakow</a>
<a href="https://ticketmaster.pl/event/496543017#tickets">Same event</a>
<a href="https://www.ticketmaster.cz/event/korn-vstupenky/1472650657">KoRn Prague</a>
<a href="https://ticketmaster.pl.evil.example/event/1234">Lookalike</a>
<a href="https://secure.ticketmaster.pl/496543017/private?qty=2">Checkout</a>
<a href="https://www.ticketmaster.pl/event/496543017?token=secret">Session</a>
<a href="https://user:password@ticketmaster.pl/event/12345">Credentials</a>
<a href="javascript:alert(1)">Script</a>
<a href="https://www.ticketmaster.pl/event/not-an-id">Not canonical ID</a>"#;
        let events = discover(html, "https://www.ticketmaster.pl/artist/775700").unwrap();
        assert_eq!(events.len(), 2);
        assert!(
            events
                .iter()
                .all(|e| !e.url.contains('?') && !e.url.contains('#'))
        );
        assert_eq!(events[0].title, "KoRn Krakow");
    }

    #[test]
    fn partial_offers_are_positive_but_not_complete() {
        let html = OFFERS.replace("data-test=\"complete\"", "data-test=\"loading\"");
        let result = parse(&watch(), &html, 100);
        assert_eq!(result.availability, Availability::Available);
        assert!(!result.complete);
    }

    #[test]
    fn offer_rows_need_visible_enabled_purchase_evidence() {
        for html in [
            OFFERS.replace("class=\"buy\"", "class=\"other\""),
            OFFERS.replace("class=\"buy\"", "class=\"buy\" disabled"),
            OFFERS.replace("class=\"buy\"", "class=\"buy\" aria-disabled=\"true\""),
            OFFERS.replace("class=\"buy\"", "class=\"buy\" hidden"),
            OFFERS.replace("class=\"buy\"", "class=\"buy\" style=\"opacity:0.0\""),
            format!("<fieldset disabled>{OFFERS}</fieldset>"),
            format!("<div aria-disabled=true>{OFFERS}</div>"),
        ] {
            let observation = parse(&watch(), &html, 100);
            assert!(observation.offers.is_empty());
            assert_eq!(observation.availability, Availability::Unknown);
        }
        let mut without_positive = watch();
        without_positive.parser.offer_available_selector = None;
        assert!(parse(&without_positive, OFFERS, 100).offers.is_empty());
    }

    #[test]
    fn quantity_dropdowns_are_capacity_never_remaining_stock() {
        let html = OFFERS.replace("<span class=\"quantity\">2</span>",
            "<select class=quantity><option value=0>Choose</option><option value=1>1</option><option value=2>2</option><option value=8 disabled>8</option><optgroup disabled><option value=10>10</option></optgroup><option value=20 hidden>20</option></select>");
        let observation = parse(&watch(), &html, 100);
        assert_eq!(observation.offers[0].quantity, Some(2));
        assert_eq!(
            observation.offers[0].quantity_kind,
            QuantityKind::Selectable
        );
        let disabled = parse(
            &watch(),
            &html.replace(
                "<select class=quantity>",
                "<select class=quantity disabled>",
            ),
            100,
        );
        assert_eq!(disabled.offers[0].quantity, None);
        assert_eq!(disabled.offers[0].quantity_kind, QuantityKind::Unknown);
    }

    #[test]
    fn explicit_semantics_override_legacy_stock_flag_and_missing_fields_stay_unknown() {
        let mut watch = watch();
        watch.parser.quantity_kind = QuantityKind::Listing;
        watch.parser.price_scope = crate::model::PriceScope::Total;
        watch.parser.fees = crate::model::FeeScope::Excluded;
        let observation = parse(&watch, OFFERS, 100);
        let offer = &observation.offers[0];
        assert_eq!(offer.quantity_kind, QuantityKind::Listing);
        assert_eq!(offer.price_scope, crate::model::PriceScope::Total);
        assert_eq!(offer.fees, crate::model::FeeScope::Excluded);
        for ambiguous in ["1–4", "2.5", "2 tickets", "1000001", ""] {
            let observation = parse(
                &watch,
                &OFFERS.replace(">2<", &format!(">{ambiguous}<")),
                100,
            );
            assert_eq!(observation.offers[0].quantity, None);
            assert_eq!(observation.offers[0].quantity_kind, QuantityKind::Unknown);
        }
    }

    #[test]
    fn legacy_saved_offers_do_not_gain_invented_price_or_quantity_scope() {
        let old = serde_json::json!({"key":"legacy", "section":null,"row":null,"seats":null,
            "ticket_type":"Standard","inventory":null,"price_text":"10 PLN","price_minor":1000,
            "currency":"PLN","quantity":2});
        let offer: Offer = serde_json::from_value(old).unwrap();
        assert_eq!(offer.quantity_kind, QuantityKind::Unknown);
        assert_eq!(offer.price_scope, crate::model::PriceScope::Unknown);
        assert_eq!(offer.fees, crate::model::FeeScope::Unknown);
    }

    #[test]
    fn ambiguous_visible_fields_do_not_choose_an_arbitrary_price_or_count() {
        let html = OFFERS.replace(
            "</article>",
            "<span class=price>500,00 zł</span><span class=quantity>8</span></article>",
        );
        let observation = parse(&watch(), &html, 100);
        assert_eq!(observation.offers.len(), 1);
        assert_eq!(observation.offers[0].price_minor, None);
        assert_eq!(observation.offers[0].quantity, None);
        let hidden = OFFERS.replace("</article>",
            "<span class=price hidden>500,00 zł</span><span class=quantity hidden>8</span></article>");
        let observation = parse(&watch(), &hidden, 100);
        assert_eq!(observation.offers[0].price_minor, Some(91080));
        assert_eq!(observation.offers[0].quantity, Some(2));
    }
}
