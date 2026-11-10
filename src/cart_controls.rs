//! Validated observations of Czech form controls after a local offline demo.
//! These are configuration observations, not a ticket search or a hold.
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;

use crate::{
    cart_catalog::{CzCartCatalog, extract_cz_cart_catalog},
    event_identity,
};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlOption {
    pub checkbox_id: String,
    pub label: String,
    pub checked: bool,
    pub disabled: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq, Hash)]
#[serde(rename_all = "snake_case")]
pub enum PanelKind {
    Price,
    Section,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ControlPanel {
    pub kind: PanelKind,
    pub heading: String,
    pub opened: bool,
    pub disabled: bool,
    pub options: Vec<ControlOption>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserCartControls {
    pub event_url: String,
    pub event_id: String,
    pub category_id: String,
    pub category_title: String,
    pub demo_quantity: u32,
    pub initial_quantity: u32,
    pub final_quantity: u32,
    pub started_at_ms: i64,
    pub finished_at_ms: i64,
    pub network_disabled_before_changes: bool,
    pub form_restored: bool,
    pub panels: Vec<ControlPanel>,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
    pub fees_complete: bool,
    pub hold_verified: bool,
    pub reservation_attempted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preparation: Option<BrowserPreparation>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedPriceOption {
    pub checkbox_id: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PreparedSectionOption {
    pub section_id: String,
    pub checkbox_id: String,
    pub label: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserPreparation {
    pub price_id: String,
    pub section_ids: Vec<String>,
    pub effective_section_ids: Vec<String>,
    pub price_option: PreparedPriceOption,
    pub section_options: Vec<PreparedSectionOption>,
    pub price_selection_verified: bool,
    pub section_selection_verified: bool,
    pub filters_restored: bool,
}

#[derive(Debug, Serialize)]
pub struct CartControlsReport {
    pub evidence: &'static str,
    pub catalog: CzCartCatalog,
    pub controls: BrowserCartControls,
    pub purchase_quantity_configured: bool,
    pub reservation_authorized: bool,
}

pub fn validate(
    expected_url: &str,
    actual_url: &str,
    category_id: &str,
    demo_quantity: u32,
    html: &str,
    raw: &serde_json::Value,
    now_ms: i64,
) -> Result<CartControlsReport> {
    validate_inner(
        expected_url,
        actual_url,
        category_id,
        demo_quantity,
        html,
        raw,
        now_ms,
        false,
    )
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn validate_inner(
    expected_url: &str,
    actual_url: &str,
    category_id: &str,
    demo_quantity: u32,
    html: &str,
    raw: &serde_json::Value,
    now_ms: i64,
    allow_preparation: bool,
) -> Result<CartControlsReport> {
    ensure!(
        (1..=20).contains(&demo_quantity),
        "Demo quantity must be from 1 to 20"
    );
    ensure!(
        serde_json::to_vec(raw)?.len() <= 256 * 1024,
        "Cart control evidence exceeds limit"
    );
    let controls: BrowserCartControls =
        serde_json::from_value(raw.clone()).context("Invalid cart control evidence")?;
    ensure!(
        allow_preparation || controls.preparation.is_none(),
        "Unexpected native preparation evidence"
    );
    let catalog = extract_cz_cart_catalog(expected_url, actual_url, html)?;
    ensure!(
        controls.event_id == catalog.event_id
            && event_identity::same_event(expected_url, &controls.event_url)
            && controls.category_id == category_id
            && controls.demo_quantity == demo_quantity,
        "Cart controls do not match requested event/type/quantity"
    );
    ensure!(
        now_ms > 0
            && controls.started_at_ms > 0
            && controls.started_at_ms <= controls.finished_at_ms
            && controls.started_at_ms >= now_ms.saturating_sub(60_000)
            && controls.finished_at_ms <= now_ms.saturating_add(1000),
        "Cart control evidence is stale or in the future"
    );
    ensure!(
        controls.network_disabled_before_changes
            && controls.form_restored
            && controls.initial_quantity == 0
            && controls.final_quantity == 0
            && catalog.current_selected_count == 0
            && !controls.stock_verified
            && !controls.adjacency_verified
            && !controls.fees_complete
            && !controls.hold_verified
            && !controls.reservation_attempted,
        "Cart control demo was not isolated/restored or makes unsupported inventory claims"
    );
    let category = catalog
        .categories
        .iter()
        .find(|c| c.id == category_id)
        .context("Cart control category is absent from configuration")?;
    ensure!(
        controls.category_title == category.title
            && !category.locked
            && !category.membership_locked
            && category.selectable_quantities.contains(&demo_quantity),
        "Cart demo category or quantity is not eligible in configuration"
    );
    let rendered = catalog
        .rendered_categories
        .iter()
        .filter(|row| row.title == category.title)
        .collect::<Vec<_>>();
    ensure!(
        rendered.len() == 1
            && rendered[0].category_ids == [category_id]
            && rendered[0].enabled
            && demo_quantity >= rendered[0].selectable_min
            && demo_quantity <= rendered[0].selectable_max,
        "Cart demo requires one unambiguous enabled category control"
    );
    ensure!(
        controls.panels.len() == 2,
        "Cart demo must inspect both price and section controls"
    );
    let mut kinds = HashSet::new();
    for panel in &controls.panels {
        ensure!(
            kinds.insert(panel.kind)
                && bounded_text(&panel.heading, 100)
                && panel.options.len() <= 128,
            "Invalid cart control panel"
        );
        ensure!(
            panel.opened != panel.disabled
                && (if panel.disabled {
                    panel.options.is_empty()
                } else {
                    !panel.options.is_empty()
                }),
            "Cart control panel has incomplete or contradictory state"
        );
        let mut ids = HashSet::new();
        for option in &panel.options {
            ensure!(
                bounded_text(&option.checkbox_id, 128)
                    && option
                        .checkbox_id
                        .bytes()
                        .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-' | b':'))
                    && bounded_text(&option.label, 500)
                    && ids.insert(&option.checkbox_id),
                "Invalid or duplicate native filter option"
            );
        }
    }
    Ok(CartControlsReport {
        evidence: "offline_form_configuration",
        catalog,
        controls,
        purchase_quantity_configured: false,
        reservation_authorized: false,
    })
}

fn bounded_text(value: &str, max: usize) -> bool {
    !value.trim().is_empty() && value.len() <= max && !value.chars().any(char::is_control)
}
