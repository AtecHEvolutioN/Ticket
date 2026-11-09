//! Reusable adapters for recognized Ticketmaster page layouts, not individual events.
//!
//! The regional ticket-type component is backed by a rendered capture. Category
//! prices are kept separate from purchasable resale listings. A new or ambiguous
//! layout yields unknown inventory; labels and purchase limits are never total stock.
use crate::{
    config::Watch,
    model::{
        Availability, Evidence, FeeScope, Observation, Offer, PriceScope, QuantityKind,
        TicketCategory,
    },
    parser::{
        canonical_event_url, disabled_element, event_identity, event_matches, hidden_element,
        parse_price_minor, visible_text,
    },
};
use scraper::{ElementRef, Html, Selector};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use url::Url;

const LIMIT: usize = 1000;
const REGIONAL: &str = "ticketmaster-regional-v1";
const CURRENCIES: &[&str] = &[
    "USD", "EUR", "GBP", "PLN", "CZK", "CHF", "SEK", "NOK", "DKK", "AUD", "CAD", "NZD", "ZAR",
    "MXN", "JPY", "CNY", "HUF", "RON", "BGN",
];

fn selector(css: &str) -> Selector {
    Selector::parse(css).expect("constant adapter selector")
}

fn key(parts: &[&str]) -> String {
    let bytes = serde_json::to_vec(parts).expect("serializable identity");
    format!("tm:{:x}", Sha256::digest(bytes))
}

fn text_without_buttons(element: ElementRef<'_>) -> String {
    let text = element
        .descendants()
        .filter_map(|node| {
            let text = node.value().as_text()?;
            let excluded = node
                .ancestors()
                .filter_map(ElementRef::wrap)
                .any(|ancestor| ancestor.value().name() == "button" || hidden_element(ancestor));
            (!excluded).then_some(text.as_ref())
        })
        .collect::<Vec<_>>()
        .join(" ");
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn visible(element: ElementRef<'_>) -> bool {
    !hidden_element(element) && !disabled_element(element)
}

/// Find the component's direct span fields structurally, without generated CSS classes.
fn fields(info: ElementRef<'_>) -> Option<Vec<ElementRef<'_>>> {
    let div = selector("div");
    let mut groups = info.select(&div).filter_map(|container| {
        let children = container
            .children()
            .filter_map(ElementRef::wrap)
            .collect::<Vec<_>>();
        if children.is_empty() || !children.iter().all(|child| child.value().name() == "span") {
            return None;
        }
        children
            .iter()
            .all(|child| visible(*child))
            .then_some(children)
    });
    let found = groups.next()?;
    groups.next().is_none().then_some(found)
}

fn number(raw: &str) -> Option<u32> {
    let value = raw.trim();
    if value.is_empty() || value.len() > 7 || !value.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    value
        .parse::<u32>()
        .ok()
        .filter(|value| *value <= 1_000_000)
}

fn listing_quantity(text: &str) -> Option<u32> {
    let normalized = text
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase();
    // Exact prefixes preserve the difference between availability and an order limit.
    for prefix in [
        "dostępne ",
        "available ",
        "verfügbar ",
        "beschikbaar ",
        "disponibles ",
        "disponibili ",
        "dostupné ",
        "dostupných: ",
    ] {
        if let Some(value) = normalized.strip_prefix(prefix) {
            return number(value);
        }
    }
    if let Some(value) = normalized.strip_suffix(" beschikbaar") {
        return number(value);
    }
    None
}

fn resale_label(text: &str) -> bool {
    matches!(
        text.to_lowercase().as_str(),
        "bilety resale"
            | "resale tickets"
            | "resale"
            | "tickets resale"
            | "resale-tickets"
            | "verified resale ticket"
            | "ověřená přeprodávaná vstupenka"
    )
}

/// This hint only disambiguates a visible monetary amount. Structured event
/// prices, availability and quantities never become live inventory evidence.
type CurrencyHint = Result<Option<String>, ()>;

struct CurrencyScan {
    codes: BTreeSet<String>,
    remaining: usize,
    invalid: bool,
}

impl CurrencyScan {
    fn enter(&mut self, depth: usize) -> bool {
        if depth > 24 || self.remaining == 0 {
            self.invalid = true;
            return false;
        }
        self.remaining -= 1;
        true
    }

    fn offers(&mut self, value: &Value, expected: &Url, depth: usize) {
        if !self.enter(depth) {
            return;
        }
        if let Some(values) = value.as_array() {
            for value in values {
                self.offers(value, expected, depth + 1);
            }
        } else if let Some(object) = value.as_object() {
            if let Some(raw) = object.get("url") {
                let matches = raw
                    .as_str()
                    .and_then(|raw| canonical_event_url(raw, Some(expected)))
                    .and_then(|url| event_identity(&url))
                    .zip(event_identity(expected))
                    .is_some_and(|(actual, expected)| actual == expected);
                if !matches {
                    return;
                }
            }
            if let Some(code) = object.get("priceCurrency") {
                match code.as_str().map(str::to_ascii_uppercase) {
                    Some(code) if CURRENCIES.contains(&code.as_str()) => {
                        self.codes.insert(code);
                    }
                    _ => self.invalid = true,
                }
            }
            if let Some(offers) = object.get("offers") {
                self.offers(offers, expected, depth + 1);
            }
        }
    }

    fn events(&mut self, value: &Value, expected: &Url, depth: usize) {
        if !self.enter(depth) {
            return;
        }
        if event_matches(value, expected) {
            if let Some(offers) = value.get("offers") {
                self.offers(offers, expected, depth + 1);
            }
            return;
        }
        match value {
            Value::Array(values) => {
                for value in values {
                    self.events(value, expected, depth + 1);
                }
            }
            Value::Object(values) => {
                for value in values.values() {
                    self.events(value, expected, depth + 1);
                }
            }
            _ => {}
        }
    }
}

fn page_currency(watch: &Watch, doc: &Html) -> CurrencyHint {
    let Some(expected) = canonical_event_url(&watch.url, None) else {
        return Ok(None);
    };
    let mut scan = CurrencyScan {
        codes: BTreeSet::new(),
        remaining: 4096,
        invalid: false,
    };
    for (index, node) in doc
        .select(&selector("script[type='application/ld+json']"))
        .take(17)
        .enumerate()
    {
        let raw = node.inner_html();
        if index == 16 || raw.len() > 256 * 1024 {
            scan.invalid = true;
            break;
        }
        match serde_json::from_str::<Value>(&raw) {
            Ok(value) => scan.events(&value, &expected, 0),
            Err(_) => scan.invalid = true,
        }
    }
    if scan.invalid || scan.codes.len() > 1 {
        Err(())
    } else {
        Ok(scan.codes.into_iter().next())
    }
}

fn currency(text: &str, configured: Option<&str>, hint: &CurrencyHint) -> Option<String> {
    let hint = hint.as_ref().ok()?;
    let mut detected = BTreeSet::new();
    for token in text.split(|character: char| !character.is_ascii_alphabetic()) {
        let token = token.to_ascii_uppercase();
        if CURRENCIES.contains(&token.as_str()) {
            detected.insert(token);
        }
    }
    for (marker, code) in [("zł", "PLN"), ("Kč", "CZK"), ("€", "EUR"), ("£", "GBP")] {
        if text.contains(marker) {
            detected.insert(code.to_string());
        }
    }
    for code in configured.into_iter().chain(hint.as_deref()) {
        let code = code.to_ascii_uppercase();
        if !CURRENCIES.contains(&code.as_str()) {
            return None;
        }
        detected.insert(code);
    }
    if detected.len() > 1 {
        return None;
    }
    let resolved = detected.into_iter().next()?; // $, kr and ¥ alone are ambiguous.
    let has_kr = text
        .split(|character: char| !character.is_ascii_alphabetic())
        .any(|token| token.eq_ignore_ascii_case("kr"));
    // A configured currency can disambiguate a compatible symbol, but cannot
    // override contradictory page evidence. This also protects currency-only
    // filters when exact monetary parsing remains unknown.
    if (text.contains('$') && !["USD", "AUD", "CAD", "NZD", "MXN"].contains(&resolved.as_str()))
        || (has_kr && !["SEK", "NOK", "DKK"].contains(&resolved.as_str()))
        || (text.contains('¥') && !["JPY", "CNY"].contains(&resolved.as_str()))
    {
        None
    } else {
        Some(resolved)
    }
}

fn amount(text: &str, currency: &str) -> Option<i64> {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    // Only unit labels observed in rendered regional components are removed,
    // once and at a boundary. Fees, qualifiers and ranges remain strict failures.
    let text = normalized.strip_prefix("je ").unwrap_or_else(|| {
        [" za kus", " per stuk", " cada una", " per biljett", " each"]
            .iter()
            .find_map(|suffix| normalized.strip_suffix(suffix))
            .unwrap_or(&normalized)
    });
    let values = [".", ","]
        .into_iter()
        .filter_map(|separator| parse_price_minor(text, currency, separator))
        .collect::<BTreeSet<_>>();
    (values.len() == 1)
        .then(|| values.into_iter().next())
        .flatten()
}

fn price_range(text: &str, currency: Option<&str>) -> (Option<i64>, Option<i64>) {
    let Some(currency) = currency else {
        return (None, None);
    };
    let parts = text
        .split(['–', '—', '-'])
        .map(str::trim)
        .collect::<Vec<_>>();
    match parts.as_slice() {
        [one] => {
            let value = amount(one, currency);
            (value, value)
        }
        [low, high] => match (amount(low, currency), amount(high, currency)) {
            (Some(low), Some(high)) if low <= high => (Some(low), Some(high)),
            _ => (None, None),
        },
        _ => (None, None),
    }
}

fn dimensions(row: ElementRef<'_>) -> Option<(Option<String>, Option<String>)> {
    let descriptions = row
        .select(&selector("dl"))
        .filter(|e| visible(*e))
        .collect::<Vec<_>>();
    if descriptions.len() != 1 {
        return None;
    }
    let mut section = None;
    let mut seat_row = None;
    for group in descriptions[0].children().filter_map(ElementRef::wrap) {
        let terms = group
            .select(&selector("dt"))
            .filter(|e| visible(*e))
            .collect::<Vec<_>>();
        let values = group
            .select(&selector("dd"))
            .filter(|e| visible(*e))
            .collect::<Vec<_>>();
        if terms.len() != 1 || values.len() != 1 {
            return None;
        }
        let label = visible_text(terms[0]).trim().to_lowercase();
        let value = visible_text(values[0]);
        if value.is_empty() {
            return None;
        }
        let destination = match label.as_str() {
            "sektor" | "section" | "sector" | "bereich" | "vak" | "settore" | "secteur"
            | "sekce" => Some(&mut section),
            "rząd" | "row" | "reihe" | "rij" | "fila" | "rang" | "řada" => Some(&mut seat_row),
            _ => None,
        };
        if let Some(destination) = destination
            && destination.replace(value).is_some()
        {
            return None;
        }
    }
    // Regional resale can include general-admission rows without a numbered row.
    section.as_ref()?;
    Some((section, seat_row))
}

fn parse_resale(
    watch: &Watch,
    row: ElementRef<'_>,
    fields: &[ElementRef<'_>],
    hint: &CurrencyHint,
) -> Option<Offer> {
    if row.value().name() != "li" || row.value().attr("role") != Some("button") || fields.len() != 4
    {
        return None;
    }
    let inventory = visible_text(fields[0]);
    if !resale_label(&inventory) {
        return None;
    }
    let quantity = listing_quantity(&visible_text(fields[1]))?;
    if quantity == 0 {
        return None;
    }
    let ticket_type = text_without_buttons(fields[2]);
    if ticket_type.is_empty() {
        return None;
    }
    let price_text = visible_text(fields[3]);
    let currency = currency(&price_text, watch.currency.as_deref(), hint);
    let price_minor = currency
        .as_deref()
        .and_then(|code| amount(&price_text, code));
    // A missing/ambiguous price still carries the observed source string, but will
    // never satisfy a price ceiling. Do not turn an unrecognized field into an offer.
    if !price_text.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let (section, seat_row) = dimensions(row)?;
    let identity = key(&[
        section.as_deref().unwrap_or(""),
        seat_row.as_deref().unwrap_or(""),
        &ticket_type,
        &inventory,
    ]);
    Some(Offer {
        key: identity,
        section,
        row: seat_row,
        seats: None,
        ticket_type: Some(ticket_type),
        inventory: Some(inventory),
        price_text: Some(price_text),
        price_minor,
        currency,
        quantity: Some(quantity),
        quantity_kind: QuantityKind::Listing,
        price_scope: PriceScope::PerTicket,
        fees: FeeScope::Unknown,
    })
}

fn quick_pick(row: ElementRef<'_>) -> bool {
    row.value().name() == "div"
        && row.value().attr("role") == Some("button")
        && row
            .parent()
            .and_then(ElementRef::wrap)
            .is_some_and(|parent| parent.value().attr("data-testid") == Some("quickpicksList"))
        && dimensions(row).is_some()
}

fn parse_category(
    watch: &Watch,
    row: ElementRef<'_>,
    fields: &[ElementRef<'_>],
    hint: &CurrencyHint,
) -> Option<TicketCategory> {
    let quick_pick = quick_pick(row);
    if (!quick_pick && row.value().attr("role") == Some("button")) || fields.len() != 2 {
        return None;
    }
    let steppers = row
        .select(&selector(
            "[data-testid='quantityStepper'] [role='spinbutton']",
        ))
        .filter(|e| visible(*e))
        .collect::<Vec<_>>();
    if (!quick_pick && steppers.len() != 1) || (quick_pick && !steppers.is_empty()) {
        return None;
    }
    let ticket_type = text_without_buttons(fields[0]);
    let price_text = visible_text(fields[1]);
    if ticket_type.is_empty() || !price_text.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    let currency = currency(&price_text, watch.currency.as_deref(), hint);
    let (price_min_minor, price_max_minor) = price_range(&price_text, currency.as_deref());
    let (section, seat_row) = if quick_pick {
        dimensions(row)?
    } else {
        (None, None)
    };
    let identity = if quick_pick {
        key(&[
            &ticket_type,
            section.as_deref().unwrap_or(""),
            seat_row.as_deref().unwrap_or(""),
        ])
    } else {
        key(&[&ticket_type])
    };
    Some(TicketCategory {
        key: identity,
        ticket_type,
        section,
        row: seat_row,
        price_text: Some(price_text),
        price_min_minor,
        price_max_minor,
        currency,
        // Quick-pick cards have no visible quantity. A nearby ticket-count
        // filter is the requested search size, not available or selectable stock.
        selectable_quantity: steppers
            .first()
            .and_then(|stepper| stepper.value().attr("aria-valuemax"))
            .and_then(number),
    })
}

pub(crate) fn parse(watch: &Watch, doc: &Html, observed_at: i64) -> Observation {
    let mut observation = Observation::unknown(
        &watch.id,
        observed_at,
        "unsupported Ticketmaster layout; inventory unknown",
    );
    observation.adapter = Some("ticketmaster-unsupported".into());
    let infos = doc
        .select(&selector("[data-testid='ticketTypeInfo']"))
        .filter(|info| visible(*info))
        .take(LIMIT + 1)
        .collect::<Vec<_>>();
    if infos.len() > LIMIT {
        observation.detail = "Ticketmaster component limit exceeded; inventory unknown".into();
        return observation;
    }
    if infos.is_empty() {
        if doc
            .select(&selector("li[role='menuitem'][data-listing-id]"))
            .any(visible)
        {
            observation.adapter = Some("ticketmaster-global-unsupported".into());
            observation.detail = "global Ticketmaster listing layout detected; locked-offer and price semantics need validation; inventory unknown".into();
        }
        return observation;
    }
    observation.adapter = Some(REGIONAL.into());
    let currency_hint = page_currency(watch, doc);
    let mut offers = BTreeMap::new();
    let mut categories = BTreeMap::new();
    let mut skipped = 0;
    for info in infos {
        let Some(row) = info
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|ancestor| ancestor.value().name() == "li" || quick_pick(*ancestor))
        else {
            skipped += 1;
            continue;
        };
        if !visible(row)
            || row
                .select(&selector("[data-testid='ticketTypeInfo']"))
                .filter(|e| visible(*e))
                .count()
                != 1
        {
            skipped += 1;
            continue;
        }
        let Some(fields) = fields(info) else {
            skipped += 1;
            continue;
        };
        if let Some(offer) = parse_resale(watch, row, &fields, &currency_hint) {
            if offers
                .get(&offer.key)
                .is_some_and(|previous| previous != &offer)
            {
                observation.detail =
                    "ambiguous duplicate Ticketmaster listing; inventory unknown".into();
                return observation;
            }
            offers.insert(offer.key.clone(), offer);
        } else if let Some(category) = parse_category(watch, row, &fields, &currency_hint) {
            if categories
                .get(&category.key)
                .is_some_and(|previous| previous != &category)
            {
                observation.detail =
                    "ambiguous duplicate Ticketmaster category; inventory unknown".into();
                return observation;
            }
            categories.insert(category.key.clone(), category);
        } else {
            skipped += 1;
        }
    }
    observation.offers = offers.into_values().collect();
    observation.ticket_categories = categories.into_values().collect();
    if !observation.offers.is_empty() {
        observation.availability = Availability::Available;
        observation.evidence = Evidence::Offers;
    }
    observation.detail = format!(
        "regional Ticketmaster layout: {} resale listings, {} displayed ticket categories, {skipped} unrecognized or inactive rows; category ranges and selectable limits do not establish remaining stock; snapshot is incomplete",
        observation.offers.len(),
        observation.ticket_categories.len()
    );
    observation
}
