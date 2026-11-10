//! Read-only Czech event configuration. Nothing in this module establishes stock,
//! an adjacent seat group, a quote, or a reservation.
use crate::parser::{disabled_element, hidden_element, visible_text};
use crate::{event_identity, page_data::UniqueJson};
use anyhow::{Context, Result, ensure};
use scraper::{ElementRef, Html, Selector};
use serde::Serialize;
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

const MAX_HTML_BYTES: usize = 4 * 1024 * 1024;
const MAX_JSON_BYTES: usize = 2 * 1024 * 1024;
const MAX_TYPES: usize = 128;
const MAX_PRICES: usize = 1024;
const MAX_QUANTITY: u32 = 1000;
const MAX_MONEY_MINOR: u64 = 10_000_000_000;
const NON_ADJACENT_WARNING: &str = "upozornění: vstupenky nemusí být vedle sebe";

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CatalogEvidence {
    ConfigurationOnly,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CatalogScope {
    PrimaryConfiguration,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CatalogSeating {
    Unknown,
}

/// Date configuration does not authenticate a live quote or prove a performance
/// has not changed since this document was captured.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ConfiguredDateStatus {
    KnownInstant,
    Missing,
    Unresolved,
    EventChanged,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct CzCartCatalog {
    pub event_url: String,
    pub event_id: String,
    pub event_name: String,
    pub event_start: Option<String>,
    pub event_time_zone: Option<String>,
    pub event_date_status: ConfiguredDateStatus,
    pub currency: String,
    pub evidence: CatalogEvidence,
    pub scope: CatalogScope,
    pub stock_verified: bool,
    pub hold_verified: bool,
    pub event_ticket_limit: u32,
    pub handling_fee_minor: u64,
    /// Sum of visible steppers, counting a merged display category once.
    pub current_selected_count: u32,
    pub rendered_non_adjacent_warning: bool,
    /// Resale listing rows have a separate source and are not configuration.
    pub ignored_resale_listing_rows: u32,
    pub categories: Vec<ConfiguredCategory>,
    pub rendered_categories: Vec<RenderedCategory>,
    pub price_levels: Vec<ConfiguredPriceLevel>,
    pub sections: Vec<ConfiguredSection>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfiguredCategory {
    pub id: String,
    pub title: String,
    pub discount_type: String,
    pub locked: bool,
    pub membership_locked: bool,
    pub selectable_quantities: Vec<u32>,
    /// The server's initial snapshot; this is not the current DOM count.
    pub initial_selected_quantity: u32,
    pub seating: CatalogSeating,
    pub adjacent_candidate: bool,
    pub prices: Vec<ConfiguredPrice>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfiguredPrice {
    pub price_id: String,
    pub price_level_title: String,
    pub face_value_minor: u64,
    pub service_fee_minor: u64,
    pub upsell_fee_minor: u64,
    pub total_minor: u64,
    pub section_ids: Vec<String>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfiguredPriceLevel {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ConfiguredSection {
    pub id: String,
    pub title: String,
    pub raw_id: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct RenderedCategory {
    pub title: String,
    /// Several metadata types can share one displayed category and stepper.
    pub category_ids: Vec<String>,
    pub selectable_min: u32,
    pub selectable_max: u32,
    pub current_selected_count: u32,
    pub enabled: bool,
}

/// Extract a bounded, event-bound configuration snapshot from already saved HTML.
/// The caller must supply the public URL actually captured, including redirects.
/// This performs no network requests, browser actions, or cart mutations.
pub fn extract_cz_cart_catalog(
    expected_url: &str,
    actual_url: &str,
    html: &str,
) -> Result<CzCartCatalog> {
    ensure!(
        html.len() <= MAX_HTML_BYTES,
        "cart preflight HTML exceeds size limit"
    );
    let (host, event_id) = event_identity::identity(expected_url)
        .context("cart preflight requires a public Czech event URL")?;
    ensure!(
        host == "ticketmaster.cz"
            && event_id.bytes().all(|b| b.is_ascii_digit())
            && event_identity::same_event(expected_url, actual_url),
        "cart preflight public Czech event identity mismatch"
    );
    let doc = Html::parse_document(html);
    reject_challenge(&doc)?;
    let canonical = select("link[rel=canonical]");
    let canonical_urls: Vec<_> = doc.select(&canonical).collect();
    ensure!(
        canonical_urls.len() == 1,
        "cart preflight requires one canonical event URL"
    );
    ensure!(
        canonical_urls[0]
            .value()
            .attr("href")
            .is_some_and(|url| event_identity::same_event(expected_url, url)),
        "cart preflight canonical event mismatch"
    );
    for el in doc.select(&select("meta[property='og:url']")) {
        ensure!(
            el.value()
                .attr("content")
                .is_some_and(|url| event_identity::same_event(expected_url, url)),
            "cart preflight social event identity mismatch"
        );
    }
    let scripts: Vec<_> = doc.select(&select("script[id='__NEXT_DATA__']")).collect();
    ensure!(
        scripts.len() == 1,
        "cart preflight requires one embedded page snapshot"
    );
    ensure!(
        scripts[0].value().attr("type") == Some("application/json"),
        "unexpected embedded page format"
    );
    let raw = scripts[0].inner_html();
    ensure!(
        raw.len() <= MAX_JSON_BYTES,
        "cart preflight metadata exceeds size limit"
    );
    validate_money_lexemes(&raw)?;
    let next: UniqueJson =
        serde_json::from_str(&raw).context("invalid or ambiguous cart metadata")?;
    let next = next.0;
    let state = next
        .pointer("/props/pageProps/initialReduxState")
        .context("missing bound event state")?;
    let event = &state["eventInfo"];
    let selection = &state["ticketSelection"];
    for bound_id in [
        &next["query"]["eventId"],
        &state["eventId"],
        &event["eventId"],
        &event["params"],
        &selection["params"]["eventId"],
    ] {
        ensure!(
            bound_id.as_str() == Some(event_id.as_str()),
            "cart metadata belongs to another or unbound event"
        );
    }
    for node in [event, selection] {
        ensure!(
            node["stage"] == "success" && node.get("inFlightParams") == Some(&Value::Null),
            "cart metadata is unfinished or stale"
        );
    }
    let embedded_canonical = event["canonicalUrl"]
        .as_str()
        .context("missing embedded canonical event")?;
    let embedded_url = if embedded_canonical.starts_with('/') {
        format!("https://www.ticketmaster.cz{embedded_canonical}")
    } else {
        embedded_canonical.to_owned()
    };
    ensure!(
        event_identity::same_event(expected_url, &embedded_url),
        "embedded canonical event mismatch"
    );
    ensure!(
        event["currencyCode"] == "CZK" && state["config"]["currencyCode"] == "CZK",
        "cart metadata requires explicit consistent CZK currency"
    );
    ensure!(
        event["marketCode"] == "CZ" && state["config"]["countryCode"] == "CZ",
        "cart metadata is not Czech market configuration"
    );
    check_currency(event)?;
    check_currency(selection)?;
    ensure!(
        selection["maintenance"] == false,
        "cart selection is under maintenance"
    );
    ensure!(
        selection["feeRollup"] == "ROLLUP" && selection["priceDisplays"]["primary"] == "ROLLUP",
        "unsupported cart price presentation"
    );
    verify_query_binding(state, selection, &event_id)?;

    let event_name = label(&event["name"])?;
    let (event_start, event_time_zone, event_date_status) = configured_date(event)?;
    let headings: BTreeSet<_> = doc
        .select(&select("h1"))
        .filter(|e| !hidden_element(*e))
        .map(visible_text)
        .filter(|s| !s.is_empty())
        .collect();
    ensure!(
        headings.len() == 1 && headings.contains(&event_name),
        "rendered event name does not match embedded event"
    );
    let limit = quantity(&selection["eventTicketLimit"])?;
    ensure!(limit > 0, "event ticket limit must be positive");
    let handling_fee_minor = money(&selection["handlingFeeValue"])?;

    let mut level_map = BTreeMap::new();
    let mut price_levels = Vec::new();
    for level in array(&selection["priceLevels"], 256)? {
        let id = identifier(&level["id"])?;
        let title = label(&level["title"])?;
        ensure!(
            level_map.insert(id.clone(), title.clone()).is_none(),
            "duplicate configured price level"
        );
        price_levels.push(ConfiguredPriceLevel { id, title });
    }
    let mut section_ids = BTreeSet::new();
    let mut sections = Vec::new();
    for section in array(&selection["sections"], 256)? {
        let id = identifier(&section["id"])?;
        ensure!(
            section_ids.insert(id.clone()),
            "duplicate configured section"
        );
        sections.push(ConfiguredSection {
            id,
            title: label(&section["title"])?,
            raw_id: identifier(&section["rawId"])?,
        });
    }
    let mut mappings: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for mapping in array(&selection["priceToSections"], 2048)? {
        let price_id = identifier(&mapping["priceIdentifier"])?;
        let section_id = identifier(&mapping["sectionIdentifier"])?;
        ensure!(
            level_map.contains_key(&price_id) && section_ids.contains(&section_id),
            "unknown price or section in mapping"
        );
        ensure!(
            mappings.entry(price_id).or_default().insert(section_id),
            "duplicate price-to-section mapping"
        );
    }
    let mut categories = Vec::new();
    let mut category_ids = BTreeSet::new();
    let mut price_count = 0;
    for category in array(&selection["ticketTypes"], MAX_TYPES)? {
        let id = identifier(&category["id"])?;
        ensure!(
            category_ids.insert(id.clone()),
            "duplicate configured ticket type"
        );
        let selectable_quantities: Vec<_> =
            array(&category["quantities"], MAX_QUANTITY as usize + 1)?
                .iter()
                .map(quantity)
                .collect::<Result<_>>()?;
        ensure!(
            !selectable_quantities.is_empty()
                && selectable_quantities
                    .windows(2)
                    .all(|pair| pair[0] < pair[1])
                && selectable_quantities.iter().all(|q| *q <= limit),
            "invalid selectable quantity configuration"
        );
        let initial_selected_quantity = quantity(&category["selectedQuantity"])?;
        ensure!(
            selectable_quantities.contains(&initial_selected_quantity),
            "initial quantity is outside configured options"
        );
        let mut prices = Vec::new();
        let mut ids = BTreeSet::new();
        for price in array(&category["prices"], MAX_PRICES)? {
            price_count += 1;
            ensure!(price_count <= MAX_PRICES, "too many configured prices");
            let price_id = identifier(&price["id"])?;
            ensure!(
                ids.insert(price_id.clone()),
                "duplicate price within ticket type"
            );
            let price_level_title = level_map
                .get(&price_id)
                .context("price has no configured price level")?
                .clone();
            let face_value_minor = money(&price["faceValue"])?;
            let service_fee_minor = money(&price["serviceFeeChargesValue"])?;
            let upsell_fee_minor = money(&price["upsellFeeChargesValue"])?;
            let total_minor = face_value_minor
                .checked_add(service_fee_minor)
                .and_then(|n| n.checked_add(upsell_fee_minor))
                .context("price total overflow")?;
            ensure!(total_minor <= MAX_MONEY_MINOR, "price total exceeds limit");
            let breakdown = array(&price["priceBreakdown"], 32)?;
            ensure!(!breakdown.is_empty(), "missing face-value breakdown");
            let mut breakdown_total = 0u64;
            for item in breakdown {
                label(&item["name"])?;
                let value = item["value"]
                    .as_u64()
                    .context("price breakdown must use integer minor units")?;
                ensure!(value <= MAX_MONEY_MINOR, "price breakdown exceeds limit");
                breakdown_total = breakdown_total
                    .checked_add(value)
                    .context("price breakdown overflow")?;
            }
            ensure!(
                breakdown_total == face_value_minor,
                "face value and price breakdown disagree"
            );
            prices.push(ConfiguredPrice {
                price_id: price_id.clone(),
                price_level_title,
                face_value_minor,
                service_fee_minor,
                upsell_fee_minor,
                total_minor,
                section_ids: mappings
                    .get(&price_id)
                    .map(|s| s.iter().cloned().collect())
                    .unwrap_or_default(),
            });
        }
        ensure!(
            !prices.is_empty(),
            "ticket type has no exact configured prices"
        );
        categories.push(ConfiguredCategory {
            id,
            title: label(&category["title"])?,
            discount_type: identifier(&category["discountType"])?,
            locked: category["locked"]
                .as_bool()
                .context("missing ticket lock configuration")?,
            membership_locked: category["membershipLocked"]
                .as_bool()
                .context("missing membership lock configuration")?,
            selectable_quantities,
            initial_selected_quantity,
            // No observed per-type field documents seating. In particular, an
            // event-wide isGa=false also appears for mixed seated/standing events.
            seating: CatalogSeating::Unknown,
            adjacent_candidate: false,
            prices,
        });
    }
    ensure!(!categories.is_empty(), "no configured ticket types");
    let (rendered_categories, ignored_resale_listing_rows) =
        rendered_categories(&doc, &categories, limit)?;
    let current_selected_count = rendered_categories
        .iter()
        .map(|row| row.current_selected_count)
        .sum();
    ensure!(
        current_selected_count <= limit,
        "rendered selected count exceeds event limit"
    );
    let rendered_non_adjacent_warning = doc
        .select(&select(
            "h1, h2, h3, [role=alert], [role=dialog], [data-testid=modals]",
        ))
        .filter(|el| !hidden_element(*el))
        .any(|el| {
            visible_text(el)
                .to_lowercase()
                .contains(NON_ADJACENT_WARNING)
        });
    Ok(CzCartCatalog {
        event_url: format!("https://www.ticketmaster.cz/event/{event_id}"),
        event_id,
        event_name,
        event_start,
        event_time_zone,
        event_date_status,
        currency: "CZK".into(),
        evidence: CatalogEvidence::ConfigurationOnly,
        scope: CatalogScope::PrimaryConfiguration,
        stock_verified: false,
        hold_verified: false,
        event_ticket_limit: limit,
        handling_fee_minor,
        current_selected_count,
        rendered_non_adjacent_warning,
        ignored_resale_listing_rows,
        categories,
        rendered_categories,
        price_levels,
        sections,
    })
}

fn configured_date(
    event: &Value,
) -> Result<(Option<String>, Option<String>, ConfiguredDateStatus)> {
    let time_zone = match event.get("timeZone").filter(|v| !v.is_null()) {
        Some(value) => {
            let zone = label(value)?;
            ensure!(
                zone.len() <= 100
                    && zone.bytes().all(
                        |b| b.is_ascii_alphanumeric() || matches!(b, b'/' | b'_' | b'-' | b'+')
                    ),
                "invalid configured event time zone"
            );
            // The timezone name is retained as source metadata. The absolute
            // instant below must carry its own offset; no offset is guessed.
            Some(zone)
        }
        None => None,
    };
    let Some(dates) = event.get("dates").filter(|v| !v.is_null()) else {
        return Ok((None, time_zone, ConfiguredDateStatus::Missing));
    };
    ensure!(dates.is_object(), "invalid configured event dates");
    if event
        .get("eventChangeStatus")
        .is_some_and(|v| !v.is_null() && v != "none")
    {
        return Ok((None, time_zone, ConfiguredDateStatus::EventChanged));
    }
    if dates["dateStatus"] != "singleDate"
        || dates["dateDisplay"] != "showDateTime"
        || event["eventChangeStatus"] != "none"
    {
        return Ok((None, time_zone, ConfiguredDateStatus::Unresolved));
    }
    let normalize = |value: &Value| {
        value
            .as_str()
            .and_then(crate::cart::normalize_event_start)
            .context("configured event start needs an exact timestamp with known offset")
    };
    let start = normalize(&dates["startDate"])?;
    let event_date = normalize(&dates["eventDate"])?;
    ensure!(start == event_date, "configured event start dates disagree");
    Ok((Some(start), time_zone, ConfiguredDateStatus::KnownInstant))
}

fn verify_query_binding(state: &Value, selection: &Value, event_id: &str) -> Result<()> {
    let queries = state
        .pointer("/api/queries")
        .and_then(Value::as_object)
        .context("missing event-bound catalog query")?;
    ensure!(queries.len() <= 256, "too many cached page queries");
    let candidates: Vec<_> = queries
        .iter()
        .filter(|(key, value)| {
            key.starts_with("ticketSelection(") || value["endpointName"] == "ticketSelection"
        })
        .collect();
    ensure!(
        candidates.len() == 1,
        "ambiguous or missing event-bound catalog query"
    );
    let (key, query) = candidates[0];
    let args = key
        .strip_prefix("ticketSelection(")
        .and_then(|s| s.strip_suffix(')'))
        .context("invalid catalog query identity")?;
    let key_args: UniqueJson =
        serde_json::from_str(args).context("invalid catalog query arguments")?;
    ensure!(
        key_args.0["eventId"].as_str() == Some(event_id)
            && query["originalArgs"]["eventId"].as_str() == Some(event_id)
            && query["endpointName"] == "ticketSelection"
            && query["status"] == "fulfilled",
        "catalog query is stale, unfinished, or for another event"
    );
    let data = query["data"]
        .as_object()
        .context("missing catalog query data")?;
    for key in [
        "ticketTypes",
        "sections",
        "priceLevels",
        "priceToSections",
        "eventTicketLimit",
        "handlingFeeValue",
        "feeRollup",
        "priceDisplays",
        "maintenance",
    ] {
        ensure!(
            data.get(key).is_some() && data.get(key) == selection.get(key),
            "catalog query and initial state disagree"
        );
    }
    Ok(())
}

fn rendered_categories(
    doc: &Html,
    categories: &[ConfiguredCategory],
    limit: u32,
) -> Result<(Vec<RenderedCategory>, u32)> {
    let infos = select("[data-testid=ticketTypeInfo]");
    let spin = select("[data-testid=quantityStepper] [role=spinbutton]");
    let submit: Vec<_> = doc
        .select(&select("button[data-testid=findTicketsBtn]"))
        .filter(|e| !hidden_element(*e))
        .collect();
    ensure!(
        submit.len() == 1,
        "missing or ambiguous rendered ticket selection form"
    );
    let mut seen = BTreeSet::new();
    let mut result = Vec::new();
    let mut resale_ignored = 0;
    for info in doc.select(&infos).filter(|e| !hidden_element(*e)) {
        ensure!(result.len() < MAX_TYPES, "too many rendered categories");
        let row = info
            .ancestors()
            .filter_map(ElementRef::wrap)
            .find(|e| e.value().name() == "li")
            .context("unrecognized ticket category row")?;
        ensure!(
            row.select(&infos).filter(|e| !hidden_element(*e)).count() == 1,
            "ambiguous category row"
        );
        let text = visible_text(info);
        let controls: Vec<_> = row.select(&spin).filter(|e| !hidden_element(*e)).collect();
        // The same display component is reused for separate resale offer rows.
        // This exact observed shape is not a primary quantity-selection row.
        if row.value().attr("role") == Some("button")
            && controls.is_empty()
            && text.starts_with("Ověřená přeprodávaná vstupenka ")
        {
            resale_ignored += 1;
            ensure!(resale_ignored <= 1000, "too many resale rows");
            continue;
        }
        let matches: BTreeSet<_> = categories
            .iter()
            .filter(|c| {
                text.strip_prefix(&c.title)
                    .is_some_and(|s| s.starts_with(char::is_whitespace))
            })
            .map(|c| c.title.clone())
            .collect();
        ensure!(
            matches.len() == 1,
            "rendered category does not bind an exact configured title"
        );
        let title = matches.into_iter().next().unwrap();
        ensure!(
            seen.insert(title.clone()),
            "duplicate rendered category title"
        );
        let price_text = text.strip_prefix(&title).unwrap();
        ensure!(
            price_text.contains("Kč") || price_text.contains("CZK"),
            "rendered category lacks explicit Czech currency"
        );
        ensure!(
            !["€", "$", "£", "EUR", "USD", "GBP", "PLN", "zł"]
                .iter()
                .any(|s| price_text.contains(s)),
            "rendered category has conflicting currency"
        );
        ensure!(
            controls.len() == 1,
            "missing or ambiguous category quantity control"
        );
        let control = controls[0];
        let selectable_min = quantity_attr(control, "aria-valuemin")?;
        let selectable_max = quantity_attr(control, "aria-valuemax")?;
        let current_selected_count = quantity_attr(control, "aria-valuenow")?;
        ensure!(
            selectable_min <= current_selected_count
                && current_selected_count <= selectable_max
                && selectable_max <= limit,
            "invalid rendered quantity range"
        );
        let members: Vec<_> = categories.iter().filter(|c| c.title == title).collect();
        ensure!(
            members
                .iter()
                .any(|c| c.selectable_quantities.contains(&selectable_min))
                && members
                    .iter()
                    .any(|c| c.selectable_quantities.contains(&selectable_max))
                && members
                    .iter()
                    .any(|c| c.selectable_quantities.contains(&current_selected_count)),
            "rendered quantity is not a configured option"
        );
        let displayed_range = rendered_price_range(info)?;
        let configured_totals: Vec<_> = members
            .iter()
            .flat_map(|c| c.prices.iter().map(|p| p.total_minor))
            .collect();
        ensure!(
            displayed_range
                == (
                    *configured_totals.iter().min().unwrap(),
                    *configured_totals.iter().max().unwrap()
                ),
            "rendered price range disagrees with configured exact prices"
        );
        result.push(RenderedCategory {
            title,
            category_ids: members.iter().map(|c| c.id.clone()).collect(),
            selectable_min,
            selectable_max,
            current_selected_count,
            enabled: !disabled_element(control),
        });
    }
    ensure!(!result.is_empty(), "no rendered ticket categories");
    Ok((result, resale_ignored))
}

fn rendered_price_range(info: ElementRef<'_>) -> Result<(u64, u64)> {
    let prices: BTreeSet<_> = info
        .select(&select("span"))
        .filter(|el| !hidden_element(*el))
        .map(visible_text)
        .filter_map(|text| text.strip_suffix(" za kus").map(str::to_owned))
        .collect();
    ensure!(
        prices.len() == 1,
        "missing or ambiguous displayed price range"
    );
    let price = prices.into_iter().next().unwrap();
    let parts: Vec<_> = price.split(['–', '—', '-']).map(str::trim).collect();
    let parse = |part: &str| -> Result<u64> {
        let parsed = crate::parser::parse_price_minor(part, "CZK", ",")
            .context("invalid displayed Czech price")?;
        u64::try_from(parsed).context("negative displayed price")
    };
    match parts.as_slice() {
        [one] => {
            let value = parse(one)?;
            Ok((value, value))
        }
        [low, high] => {
            let (low, high) = (parse(low)?, parse(high)?);
            ensure!(low <= high, "invalid displayed price range");
            Ok((low, high))
        }
        _ => anyhow::bail!("ambiguous displayed price range"),
    }
}

fn reject_challenge(doc: &Html) -> Result<()> {
    for element in std::iter::once(doc.root_element())
        .chain(doc.select(&select("title, h1, h2, [role=alert], [role=dialog]")))
    {
        if hidden_element(element) {
            continue;
        }
        let text = visible_text(element).to_lowercase();
        ensure!(
            ![
                "your browsing activity has been paused",
                "we've paused activity",
                "we’ve paused activity",
                "access denied",
                "request blocked",
                "pardon the interruption",
                "verify you are human",
                "verify that you are human",
                "verify you're human",
                "complete the captcha",
                "complete the recaptcha",
                "solve the captcha",
                "checking your browser",
                "waiting room",
                "you are now in line",
                "you are in the queue",
                "your session has expired",
                "just a moment...",
                "ověřte, že jste člověk",
                "přístup odepřen",
                "platnost relace vypršela",
            ]
            .iter()
            .any(|needle| text.contains(needle)),
            "cart preflight rejected a challenge, queue, or expired session"
        );
    }
    for frame in doc.select(&select("iframe")) {
        if hidden_element(frame) {
            continue;
        }
        let title = frame.value().attr("title").unwrap_or("").to_lowercase();
        ensure!(
            !title.contains("challenge") && !title.contains("captcha"),
            "cart preflight requires human verification"
        );
    }
    Ok(())
}

fn select(s: &str) -> Selector {
    Selector::parse(s).expect("constant selector")
}
fn array(value: &Value, max: usize) -> Result<&Vec<Value>> {
    let values = value.as_array().context("missing configuration array")?;
    ensure!(values.len() <= max, "configuration array exceeds limit");
    Ok(values)
}
fn identifier(value: &Value) -> Result<String> {
    let id = value.as_str().context("missing configuration identifier")?;
    ensure!(
        !id.is_empty()
            && id.len() <= 64
            && id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-')),
        "invalid configuration identifier"
    );
    Ok(id.to_owned())
}
fn label(value: &Value) -> Result<String> {
    let text = value.as_str().context("missing configuration label")?;
    ensure!(
        !text.is_empty()
            && text.len() <= 1024
            && text.chars().count() <= 240
            && !text.chars().any(char::is_control),
        "invalid configuration label"
    );
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    ensure!(!normalized.is_empty(), "empty configuration label");
    Ok(normalized)
}
fn quantity(value: &Value) -> Result<u32> {
    let qty = value.as_u64().context("quantity must be an integer")?;
    ensure!(qty <= u64::from(MAX_QUANTITY), "quantity exceeds limit");
    Ok(qty as u32)
}
fn quantity_attr(element: ElementRef<'_>, attr: &str) -> Result<u32> {
    let value = element
        .value()
        .attr(attr)
        .context("missing rendered quantity bound")?;
    ensure!(
        !value.is_empty() && value.len() <= 4 && value.bytes().all(|c| c.is_ascii_digit()),
        "invalid rendered quantity bound"
    );
    let qty: u32 = value.parse().context("invalid rendered quantity")?;
    ensure!(qty <= MAX_QUANTITY, "rendered quantity exceeds limit");
    Ok(qty)
}
fn money(value: &Value) -> Result<u64> {
    let number = value
        .as_number()
        .context("configured money must be numeric")?
        .to_string();
    money_lexeme(&number)
}
fn money_lexeme(number: &str) -> Result<u64> {
    let (whole, fraction) = number.split_once('.').unwrap_or((number, ""));
    ensure!(
        !whole.is_empty()
            && whole.bytes().all(|b| b.is_ascii_digit())
            && fraction.len() <= 2
            && fraction.bytes().all(|b| b.is_ascii_digit()),
        "configured money has invalid precision or sign"
    );
    let whole: u64 = whole.parse().context("configured money overflow")?;
    let cents: u64 = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u64>()? * 10,
        _ => fraction.parse()?,
    };
    let amount = whole
        .checked_mul(100)
        .and_then(|n| n.checked_add(cents))
        .context("configured money overflow")?;
    ensure!(amount <= MAX_MONEY_MINOR, "configured money exceeds limit");
    Ok(amount)
}

// Check original numeric spelling before serde_json can round a floating-point
// number. Only observed major-unit money fields are constrained; coordinates
// and other unrelated page numbers can legitimately have greater precision.
fn validate_money_lexemes(raw: &str) -> Result<()> {
    let bytes = raw.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] != b'"' {
            index += 1;
            continue;
        }
        let start = index;
        index += 1;
        while index < bytes.len() {
            match bytes[index] {
                b'\\' => index += 2,
                b'"' => {
                    index += 1;
                    break;
                }
                _ => index += 1,
            }
        }
        ensure!(index <= bytes.len(), "unterminated JSON string");
        let end = index;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        if bytes.get(index) != Some(&b':') {
            continue;
        }
        let key: String =
            serde_json::from_str(&raw[start..end]).context("invalid metadata property")?;
        if !matches!(
            key.as_str(),
            "faceValue" | "serviceFeeChargesValue" | "upsellFeeChargesValue" | "handlingFeeValue"
        ) {
            continue;
        }
        index += 1;
        while index < bytes.len() && bytes[index].is_ascii_whitespace() {
            index += 1;
        }
        let value_start = index;
        while index < bytes.len()
            && !bytes[index].is_ascii_whitespace()
            && !matches!(bytes[index], b',' | b'}' | b']')
        {
            index += 1;
        }
        money_lexeme(&raw[value_start..index])?;
    }
    Ok(())
}
fn check_currency(value: &Value) -> Result<()> {
    match value {
        Value::Object(map) => {
            for (key, value) in map {
                if matches!(key.as_str(), "currency" | "currencyCode" | "priceCurrency") {
                    ensure!(
                        value == "CZK",
                        "conflicting currency in event configuration"
                    );
                }
                check_currency(value)?;
            }
        }
        Value::Array(values) => {
            for value in values {
                check_currency(value)?;
            }
        }
        _ => {}
    }
    Ok(())
}
