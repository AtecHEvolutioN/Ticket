//! Editable defaults and complete per-event overrides, shared with Android JSON.
//! Applying preferences materializes an explicit intent. It never changes the
//! quantity a captured quote claims to have searched, creates a
//! reservation, or turns unavailable filter evidence into a matching ticket.
use std::collections::HashSet;

use anyhow::{Context, Result, anyhow, ensure};
use serde::{Deserialize, Serialize};

use crate::{
    cart::{self, CartFilters, CartPlanRequest},
    cart_journal::NativeSearchRequest,
};

pub const MAX_INPUT_BYTES: usize = 256 * 1024;
pub const MAX_EVENT_OVERRIDES: usize = 100;

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PriceStrategy {
    #[default]
    Cheapest,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CartPreferences {
    pub quantity: u32,
    #[serde(deserialize_with = "required_optional_budget")]
    pub max_total_minor: Option<u64>,
    pub currency: String,
    pub require_adjacent: bool,
    pub price_strategy: PriceStrategy,
    pub allowed_category_ids: Vec<String>,
    pub allowed_ticket_types: Vec<String>,
    pub allowed_sections: Vec<String>,
    pub allowed_rows: Vec<String>,
}
impl Default for CartPreferences {
    fn default() -> Self {
        Self {
            quantity: 2,
            max_total_minor: None,
            currency: "CZK".into(),
            require_adjacent: true,
            price_strategy: PriceStrategy::Cheapest,
            allowed_category_ids: vec![],
            allowed_ticket_types: vec![],
            allowed_sections: vec![],
            allowed_rows: vec![],
        }
    }
}
impl CartPreferences {
    pub fn filters(&self) -> CartFilters {
        CartFilters {
            allowed_category_ids: self.allowed_category_ids.clone(),
            allowed_ticket_types: self.allowed_ticket_types.clone(),
            allowed_sections: self.allowed_sections.clone(),
            allowed_rows: self.allowed_rows.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct EventOverride {
    pub event_url: String,
    /// A complete replacement for global defaults, not a partial field patch.
    pub preferences: CartPreferences,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PreferencesDocument {
    pub schema_version: u32,
    pub defaults: CartPreferences,
    pub event_overrides: Vec<EventOverride>,
}
impl Default for PreferencesDocument {
    fn default() -> Self {
        Self {
            schema_version: 1,
            defaults: CartPreferences::default(),
            event_overrides: vec![],
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PreferenceSource {
    Defaults,
    EventOverride,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct EffectivePreferences {
    pub event_url: String,
    pub source: PreferenceSource,
    pub preferences: CartPreferences,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PreparedPlan {
    pub effective_preferences: EffectivePreferences,
    pub request: CartPlanRequest,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct PreparedNativeSearch {
    pub effective_preferences: EffectivePreferences,
    pub request: NativeSearchRequest,
}

/// Strict bounded interchange: duplicate keys, unsupported fields/settings and
/// conflicting event aliases are rejected instead of silently discarded.
pub fn parse_document(input: &[u8]) -> Result<PreferencesDocument> {
    ensure!(
        input.len() <= MAX_INPUT_BYTES,
        "preferences document exceeds size limit"
    );
    let unique: crate::page_data::UniqueJson = serde_json::from_slice(input)
        .map_err(|_| anyhow!("invalid or ambiguous preferences JSON"))?;
    let document: PreferencesDocument = serde_json::from_value(unique.0)
        .map_err(|_| anyhow!("invalid or unsupported preferences schema"))?;
    validate_document(&document)?;
    Ok(document)
}

pub fn resolve(document: &PreferencesDocument, event_url: &str) -> Result<EffectivePreferences> {
    validate_document(document)?;
    let identity = crate::event_identity::identity(event_url)
        .context("preferences require a public event URL")?;
    let found = document.event_overrides.iter().find(|entry| {
        crate::event_identity::identity(&entry.event_url).as_ref() == Some(&identity)
    });
    Ok(EffectivePreferences {
        event_url: format!("https://{}/event/{}", identity.0, identity.1),
        source: if found.is_some() {
            PreferenceSource::EventOverride
        } else {
            PreferenceSource::Defaults
        },
        preferences: found.map_or_else(
            || document.defaults.clone(),
            |entry| entry.preferences.clone(),
        ),
    })
}

pub fn apply_to_plan(
    document: &PreferencesDocument,
    input: &CartPlanRequest,
) -> Result<PreparedPlan> {
    let effective_preferences = resolve(document, &input.intent.event_url)?;
    let prefs = &effective_preferences.preferences;
    ensure!(
        prefs.currency == "CZK",
        "current cart planning supports CZK only; no currency conversion is performed"
    );
    ensure!(
        input.snapshot.requested_quantity == prefs.quantity,
        "captured search quantity differs from preferences; capture a fresh search for the requested quantity"
    );
    let mut request = input.clone();
    request.intent.quantity = Some(prefs.quantity);
    request.intent.max_total_minor = prefs.max_total_minor;
    request.intent.require_adjacent = prefs.require_adjacent;
    request.intent.filters = prefs.filters();
    Ok(PreparedPlan {
        effective_preferences,
        request,
    })
}

pub fn apply_to_native_search(
    document: &PreferencesDocument,
    input: &NativeSearchRequest,
) -> Result<PreparedNativeSearch> {
    let effective_preferences = resolve(document, &input.event_url)?;
    let prefs = &effective_preferences.preferences;
    ensure!(
        prefs.currency == "CZK" && input.currency == "CZK",
        "current native cart planning supports CZK only; no currency conversion is performed"
    );
    // This is an unsent intent, not captured inventory evidence. The native
    // controller must still apply/revalidate this quantity on the actual form
    // before dispatch; changing it here never asserts the form already matches.
    ensure!(
        prefs.allowed_category_ids.is_empty()
            || prefs.allowed_category_ids.contains(&input.category_id),
        "native category does not match preferences"
    );
    ensure!(
        prefs.allowed_ticket_types.is_empty()
            || input
                .category_title
                .as_ref()
                .is_some_and(|v| cart::label_allowed(&prefs.allowed_ticket_types, v)),
        "native ticket type is unknown or does not match preferences"
    );
    let subtotal = input
        .configured_unit_price_minor
        .checked_mul(u64::from(prefs.quantity))
        .context("configured subtotal overflow")?;
    ensure!(
        prefs.max_total_minor.is_none_or(|limit| subtotal <= limit),
        "configured ticket subtotal already exceeds the complete-total budget"
    );
    let mut request = input.clone();
    request.quantity = prefs.quantity;
    request.max_total_minor = prefs.max_total_minor;
    request.adjacent_required = prefs.require_adjacent;
    request.cheapest_required = true;
    request.filters = prefs.filters();
    Ok(PreparedNativeSearch {
        effective_preferences,
        request,
    })
}

pub fn validate_document(document: &PreferencesDocument) -> Result<()> {
    ensure!(
        document.schema_version == 1,
        "unsupported preferences schema version"
    );
    ensure!(
        document.event_overrides.len() <= MAX_EVENT_OVERRIDES,
        "too many event preference overrides"
    );
    validate_preferences(&document.defaults)?;
    let mut events = HashSet::new();
    for entry in &document.event_overrides {
        let identity = crate::event_identity::identity(&entry.event_url)
            .context("event override requires a public event URL")?;
        ensure!(
            events.insert(identity),
            "duplicate event preference override"
        );
        validate_preferences(&entry.preferences)?;
    }
    Ok(())
}

fn validate_preferences(prefs: &CartPreferences) -> Result<()> {
    ensure!(
        (1..=cart::MAX_QUANTITY).contains(&prefs.quantity),
        "ticket quantity must be between 1 and 20"
    );
    ensure!(
        prefs
            .max_total_minor
            .is_none_or(|value| value > 0 && value <= cart::MAX_EXACT_INTEGER),
        "budget must be positive and within the exact integer limit"
    );
    ensure!(
        prefs.currency.len() == 3 && prefs.currency.bytes().all(|b| b.is_ascii_uppercase()),
        "currency must be an uppercase three-letter code"
    );
    ensure!(
        cart::valid_filters(&prefs.filters()),
        "invalid, duplicate or oversized preference filters"
    );
    Ok(())
}

fn required_optional_budget<'de, D>(deserializer: D) -> std::result::Result<Option<u64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Option::<u64>::deserialize(deserializer)
}
