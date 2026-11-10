//! Configuration-only price ordering and native filter mapping. None of these
//! checks establishes available inventory, a complete quote, seats, or a hold.
use std::collections::{HashMap, HashSet};

use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    cart,
    cart_catalog::{ConfiguredCategory, ConfiguredPrice, CzCartCatalog},
    cart_controls::{self, BrowserCartControls, CartControlsReport, ControlOption, PanelKind},
    cart_preferences::{self, EffectivePreferences, PreferencesDocument},
    event_identity,
};

const MAX_MONEY: u64 = 10_000_000_000;

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSelection {
    pub price_id: String,
    /// Native configured floor/area IDs, never inferred from actual seat labels.
    /// Empty means all configured sections compatible with this price.
    pub section_ids: Vec<String>,
}

#[derive(Clone, Debug, Serialize)]
pub struct ConfiguredCandidate {
    pub category_id: String,
    pub category_title: String,
    pub price_id: String,
    pub price_level_title: String,
    pub configured_unit_total_minor: u64,
    pub configured_group_subtotal_minor: u64,
    pub configured_section_ids: Vec<String>,
    pub native_category_control_supported: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MappedPrice {
    pub checkbox_id: String,
    pub label: String,
    pub price_id: String,
    pub amount_minor: u64,
    pub checked: bool,
    pub disabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct MappedSection {
    pub checkbox_id: String,
    pub label: String,
    pub section_id: String,
    pub checked: bool,
    pub disabled: bool,
}

#[derive(Clone, Debug, Serialize)]
pub struct NativeMapping {
    pub category_id: String,
    pub prices: Vec<MappedPrice>,
    pub sections: Vec<MappedSection>,
}

#[derive(Debug, Serialize)]
pub struct PreparationReport {
    pub evidence: &'static str,
    pub ranking_scope: &'static str,
    pub effective_preferences: EffectivePreferences,
    pub event_start: Option<String>,
    pub configured_handling_fee_minor: u64,
    pub handling_fee_scope: &'static str,
    pub candidates: Vec<ConfiguredCandidate>,
    pub native_mapping: Option<NativeMapping>,
    pub group_filters_pending: Vec<&'static str>,
    pub stock_verified: bool,
    pub seating_verified: bool,
    pub adjacency_verified: bool,
    pub fees_complete: bool,
    pub cheapest_available_verified: bool,
    pub hold_verified: bool,
    pub reservation_authorized: bool,
}

/// Orders configured prices within category/type preferences. Physical section,
/// row and adjacency requirements remain explicit pending exact-group evidence.
pub fn evaluate(
    document: &PreferencesDocument,
    catalog: &CzCartCatalog,
    controls: Option<&BrowserCartControls>,
) -> Result<PreparationReport> {
    validate_catalog(catalog)?;
    let effective_preferences = cart_preferences::resolve(document, &catalog.event_url)?;
    let prefs = &effective_preferences.preferences;
    ensure!(
        prefs.currency == "CZK",
        "Native preparation supports CZK only; no currency conversion is performed"
    );
    let mut candidates = Vec::new();
    for category in &catalog.categories {
        if category.locked
            || category.membership_locked
            || !category.selectable_quantities.contains(&prefs.quantity)
            || prefs.quantity > catalog.event_ticket_limit
            || (!prefs.allowed_category_ids.is_empty()
                && !prefs.allowed_category_ids.contains(&category.id))
            || !cart::label_allowed(&prefs.allowed_ticket_types, &category.title)
        {
            continue;
        }
        let rendered = catalog
            .rendered_categories
            .iter()
            .filter(|row| row.category_ids.contains(&category.id))
            .collect::<Vec<_>>();
        // Platinum uses a different area-filter contract; Collector and other
        // discounts may add surcharge semantics. Keep their configured prices
        // visible, but do not apply the STANDARD price/section control adapter.
        let native_category_control_supported = category.discount_type == "STANDARD"
            && rendered.len() == 1
            && rendered[0].category_ids == [category.id.clone()]
            && rendered[0].title == category.title
            && rendered[0].enabled
            && prefs.quantity >= rendered[0].selectable_min
            && prefs.quantity <= rendered[0].selectable_max;
        for price in &category.prices {
            let subtotal = price
                .total_minor
                .checked_mul(u64::from(prefs.quantity))
                .context("Configured subtotal overflow")?;
            if prefs.max_total_minor.is_some_and(|limit| subtotal > limit) {
                continue;
            }
            candidates.push(ConfiguredCandidate {
                category_id: category.id.clone(),
                category_title: category.title.clone(),
                price_id: price.price_id.clone(),
                price_level_title: price.price_level_title.clone(),
                configured_unit_total_minor: price.total_minor,
                configured_group_subtotal_minor: subtotal,
                configured_section_ids: price.section_ids.clone(),
                native_category_control_supported,
            });
        }
    }
    candidates.sort_by(|a, b| {
        a.configured_group_subtotal_minor
            .cmp(&b.configured_group_subtotal_minor)
            .then_with(|| a.category_id.cmp(&b.category_id))
            .then_with(|| a.price_id.cmp(&b.price_id))
    });
    let native_mapping = controls
        .map(|value| {
            ensure!(
                value.demo_quantity == prefs.quantity,
                "Native control quantity differs from saved preferences"
            );
            map_controls(catalog, value)
        })
        .transpose()?;
    let mut group_filters_pending = vec![
        "exact_seated_group",
        "availability",
        "complete_group_total_and_fees",
        "cheapest_available_group",
    ];
    if prefs.require_adjacent {
        group_filters_pending.push("site_confirmed_adjacency");
    }
    if !prefs.allowed_sections.is_empty() {
        group_filters_pending.push("actual_seat_sections");
    }
    if !prefs.allowed_rows.is_empty() {
        group_filters_pending.push("actual_seat_rows");
    }
    if catalog.event_start.is_none() {
        group_filters_pending.push("performance_start");
    }
    Ok(PreparationReport {
        evidence: "configuration_only",
        ranking_scope: "configured_prices_within_category_and_type_filters",
        effective_preferences,
        event_start: catalog.event_start.clone(),
        configured_handling_fee_minor: catalog.handling_fee_minor,
        handling_fee_scope: "unverified",
        candidates,
        native_mapping,
        group_filters_pending,
        stock_verified: false,
        seating_verified: false,
        adjacency_verified: false,
        fees_complete: false,
        cheapest_available_verified: false,
        hold_verified: false,
        reservation_authorized: false,
    })
}

/// Validate an explicit configured choice; this never chooses or authenticates
/// available tickets. Native floor filters are separate from seat preferences.
pub fn validate_selected(
    report: &PreparationReport,
    category_id: &str,
    selection: &NativeSelection,
) -> Result<()> {
    validate_selection(selection)?;
    let candidate = report
        .candidates
        .iter()
        .find(|candidate| {
            candidate.category_id == category_id && candidate.price_id == selection.price_id
        })
        .context(
            "Requested native price/category is excluded by preferences or configured limits",
        )?;
    ensure!(
        candidate.native_category_control_supported,
        "Native category control is ambiguous or unavailable"
    );
    ensure!(
        !candidate.configured_section_ids.is_empty()
            && selection
                .section_ids
                .iter()
                .all(|id| candidate.configured_section_ids.contains(id)),
        "Requested native sections are not configured for the selected price"
    );
    if let Some(mapping) = &report.native_mapping {
        ensure!(
            mapping.category_id == category_id
                && mapping
                    .prices
                    .iter()
                    .any(|price| price.price_id == selection.price_id && !price.disabled),
            "Requested native price is absent or disabled in the observed controls"
        );
        let effective = if selection.section_ids.is_empty() {
            &candidate.configured_section_ids
        } else {
            &selection.section_ids
        };
        ensure!(
            effective.iter().all(|id| mapping
                .sections
                .iter()
                .any(|section| &section.section_id == id && !section.disabled)),
            "Requested native section is absent or disabled in the observed controls"
        );
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub fn validate_preparation(
    expected_url: &str,
    actual_url: &str,
    category_id: &str,
    quantity: u32,
    selection: &NativeSelection,
    html: &str,
    raw: &serde_json::Value,
    now_ms: i64,
) -> Result<CartControlsReport> {
    validate_selection(selection)?;
    let report = cart_controls::validate_inner(
        expected_url,
        actual_url,
        category_id,
        quantity,
        html,
        raw,
        now_ms,
        true,
    )?;
    let prep = report
        .controls
        .preparation
        .as_ref()
        .context("Missing native preparation evidence")?;
    ensure!(
        prep.price_id == selection.price_id
            && prep.section_ids == selection.section_ids
            && prep.price_selection_verified
            && prep.section_selection_verified
            && prep.filters_restored,
        "Native preparation does not match requested filters or was not restored"
    );
    let category = report
        .catalog
        .categories
        .iter()
        .find(|c| c.id == category_id)
        .context("Native category is absent")?;
    let price = category
        .prices
        .iter()
        .find(|p| p.price_id == selection.price_id)
        .context("Native price is absent from the category")?;
    let effective = effective_sections(price, selection)?;
    ensure!(
        same_set(&effective, &prep.effective_section_ids),
        "Native preparation effective sections differ from configuration"
    );
    let mapping = map_controls(&report.catalog, &report.controls)?;
    let selected_prices = mapping
        .prices
        .iter()
        .filter(|p| p.checked)
        .collect::<Vec<_>>();
    ensure!(
        selected_prices.len() == 1
            && selected_prices[0].price_id == selection.price_id
            && !selected_prices[0].disabled
            && prep.price_option.checkbox_id == selected_prices[0].checkbox_id
            && prep.price_option.label == selected_prices[0].label,
        "Native preparation did not retain exactly the requested price filter"
    );
    let selected_sections = mapping
        .sections
        .iter()
        .filter(|s| s.checked)
        .collect::<Vec<_>>();
    ensure!(
        same_set(
            &effective,
            &selected_sections
                .iter()
                .map(|s| s.section_id.clone())
                .collect::<Vec<_>>()
        ) && prep.section_options.len() == effective.len(),
        "Native preparation section filters differ from request"
    );
    let mut seen = HashSet::new();
    for option in &prep.section_options {
        let observed = selected_sections
            .iter()
            .find(|s| s.section_id == option.section_id)
            .context("Unconfirmed native section filter")?;
        ensure!(
            seen.insert(&option.section_id)
                && !observed.disabled
                && observed.checkbox_id == option.checkbox_id
                && observed.label == option.label,
            "Native section evidence is ambiguous or disabled"
        );
    }
    Ok(report)
}

fn map_controls(catalog: &CzCartCatalog, controls: &BrowserCartControls) -> Result<NativeMapping> {
    ensure!(
        controls.network_disabled_before_changes
            && controls.form_restored
            && controls.initial_quantity == 0
            && controls.final_quantity == 0
            && !controls.stock_verified
            && !controls.adjacency_verified
            && !controls.fees_complete
            && !controls.hold_verified
            && !controls.reservation_attempted,
        "Native controls must describe a restored offline configuration inspection"
    );
    ensure!(
        controls.event_id == catalog.event_id
            && event_identity::same_event(&catalog.event_url, &controls.event_url),
        "Native controls belong to another event"
    );
    let category = catalog
        .categories
        .iter()
        .find(|c| c.id == controls.category_id)
        .context("Native control category is absent")?;
    ensure!(
        category.title == controls.category_title,
        "Native category title mismatch"
    );
    ensure!(
        category.discount_type == "STANDARD",
        "Native filter preparation supports STANDARD categories only"
    );
    let price_panels = controls
        .panels
        .iter()
        .filter(|p| p.kind == PanelKind::Price)
        .collect::<Vec<_>>();
    let section_panels = controls
        .panels
        .iter()
        .filter(|p| p.kind == PanelKind::Section)
        .collect::<Vec<_>>();
    ensure!(
        controls.panels.len() == 2
            && price_panels.len() == 1
            && section_panels.len() == 1
            && price_panels[0].opened
            && !price_panels[0].disabled
            && section_panels[0].opened
            && !section_panels[0].disabled,
        "Native preparation requires readable price and section panels"
    );
    let options = &price_panels[0].options;
    ensure!(
        options.len() == category.prices.len() && options.len() <= 128,
        "Native price option coverage is incomplete"
    );
    let mut price_ids = HashSet::new();
    let mut checkbox_ids = HashSet::new();
    let mut prices = Vec::new();
    for option in options {
        validate_option(option, &mut checkbox_ids)?;
        let (amount_minor, title) =
            price_label(&option.label).context("Unrecognized native price label")?;
        let matches = category
            .prices
            .iter()
            .filter(|p| {
                p.total_minor == amount_minor
                    && display_text(&format!("{} {}", category.title, p.price_level_title)) == title
            })
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1 && price_ids.insert(matches[0].price_id.clone()),
            "Native price label is ambiguous or unmapped"
        );
        prices.push(MappedPrice {
            checkbox_id: option.checkbox_id.clone(),
            label: option.label.clone(),
            price_id: matches[0].price_id.clone(),
            amount_minor,
            checked: option.checked,
            disabled: option.disabled,
        });
    }
    let ids = category
        .prices
        .iter()
        .flat_map(|p| p.section_ids.iter())
        .cloned()
        .collect::<HashSet<_>>();
    let options = &section_panels[0].options;
    ensure!(
        options.len() == ids.len() && !ids.is_empty() && ids.len() <= 128,
        "Native section option coverage is incomplete"
    );
    let mut section_ids = HashSet::new();
    checkbox_ids.clear();
    let mut sections = Vec::new();
    for option in options {
        validate_option(option, &mut checkbox_ids)?;
        let matches = catalog
            .sections
            .iter()
            .filter(|section| {
                ids.contains(&section.id)
                    && display_text(&section.title) == display_text(&option.label)
            })
            .collect::<Vec<_>>();
        ensure!(
            matches.len() == 1 && section_ids.insert(matches[0].id.clone()),
            "Native section label is ambiguous or unmapped"
        );
        sections.push(MappedSection {
            checkbox_id: option.checkbox_id.clone(),
            label: option.label.clone(),
            section_id: matches[0].id.clone(),
            checked: option.checked,
            disabled: option.disabled,
        });
    }
    Ok(NativeMapping {
        category_id: category.id.clone(),
        prices,
        sections,
    })
}

fn validate_catalog(catalog: &CzCartCatalog) -> Result<()> {
    ensure!(
        event_identity::identity(&catalog.event_url)
            .is_some_and(|(host, id)| host == "ticketmaster.cz" && id == catalog.event_id)
            && catalog.currency == "CZK"
            && !catalog.stock_verified
            && !catalog.hold_verified
            && catalog.categories.len() <= 128
            && !catalog.categories.is_empty()
            && catalog.sections.len() <= 256
            && catalog.handling_fee_minor <= MAX_MONEY,
        "Invalid Czech configuration catalog"
    );
    let sections = catalog
        .sections
        .iter()
        .map(|s| (&s.id, &s.title))
        .collect::<HashMap<_, _>>();
    ensure!(
        sections.len() == catalog.sections.len(),
        "Duplicate native configured section"
    );
    let mut categories = HashSet::new();
    let mut count = 0;
    for category in &catalog.categories {
        ensure!(
            categories.insert(&category.id)
                && identifier(&category.id)
                && !category.title.trim().is_empty(),
            "Invalid or duplicate configured category"
        );
        validate_prices(category, &sections)?;
        count += category.prices.len();
        ensure!(count <= 1024, "Configured price count exceeds limit");
    }
    Ok(())
}

fn validate_prices(
    category: &ConfiguredCategory,
    sections: &HashMap<&String, &String>,
) -> Result<()> {
    let mut ids = HashSet::new();
    ensure!(
        !category.prices.is_empty(),
        "Configured category has no prices"
    );
    for price in &category.prices {
        let sum = price
            .face_value_minor
            .checked_add(price.service_fee_minor)
            .and_then(|v| v.checked_add(price.upsell_fee_minor));
        ensure!(
            identifier(&price.price_id)
                && ids.insert(&price.price_id)
                && !price.price_level_title.trim().is_empty()
                && price.total_minor <= MAX_MONEY
                && sum == Some(price.total_minor)
                && price.section_ids.iter().collect::<HashSet<_>>().len()
                    == price.section_ids.len()
                && price.section_ids.iter().all(|id| sections.contains_key(id)),
            "Invalid configured price or section mapping"
        );
    }
    Ok(())
}

fn validate_selection(selection: &NativeSelection) -> Result<()> {
    ensure!(
        identifier(&selection.price_id)
            && selection.section_ids.len() <= 128
            && selection.section_ids.iter().all(|id| identifier(id))
            && selection.section_ids.iter().collect::<HashSet<_>>().len()
                == selection.section_ids.len(),
        "Invalid native filter selection"
    );
    Ok(())
}
fn effective_sections(price: &ConfiguredPrice, selection: &NativeSelection) -> Result<Vec<String>> {
    ensure!(
        !price.section_ids.is_empty()
            && selection
                .section_ids
                .iter()
                .all(|id| price.section_ids.contains(id)),
        "Native sections are incompatible with configured price"
    );
    Ok(if selection.section_ids.is_empty() {
        price.section_ids.clone()
    } else {
        selection.section_ids.clone()
    })
}
fn identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
}
fn display_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}
fn same_set(a: &[String], b: &[String]) -> bool {
    a.len() == b.len()
        && a.iter().collect::<HashSet<_>>().len() == a.len()
        && b.iter().collect::<HashSet<_>>().len() == b.len()
        && a.iter().all(|id| b.contains(id))
}
fn validate_option<'a>(option: &'a ControlOption, ids: &mut HashSet<&'a String>) -> Result<()> {
    ensure!(
        !option.checkbox_id.is_empty()
            && option.checkbox_id.len() <= 128
            && option
                .checkbox_id
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-' | b':'))
            && ids.insert(&option.checkbox_id)
            && !option.label.trim().is_empty()
            && option.label.len() <= 500
            && !option.label.chars().any(char::is_control),
        "Invalid native checkbox evidence"
    );
    Ok(())
}
fn price_label(value: &str) -> Option<(u64, String)> {
    let normalized = display_text(value);
    let (amount, title) = normalized.split_once(" Kč • ")?;
    let (whole, fraction) = amount.split_once(',')?;
    if fraction.len() != 2 || !fraction.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let groups = whole.split(' ').collect::<Vec<_>>();
    if groups.is_empty()
        || groups
            .iter()
            .any(|s| s.is_empty() || !s.bytes().all(|c| c.is_ascii_digit()))
        || (groups.len() > 1
            && (groups[0].len() > 3 || groups.iter().skip(1).any(|s| s.len() != 3)))
    {
        return None;
    }
    let digits = groups.join("");
    if digits.len() > 1 && digits.starts_with('0') {
        return None;
    }
    let minor = digits
        .parse::<u64>()
        .ok()?
        .checked_mul(100)?
        .checked_add(fraction.parse::<u64>().ok()?)?;
    if minor > MAX_MONEY || title.is_empty() {
        return None;
    }
    Some((minor, title.to_owned()))
}
