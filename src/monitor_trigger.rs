//! Read-only bridge from the latest bound monitor observation to a recommendation
//! for fresh native preparation. No browser, reservation, notification or journal
//! mutation is performed, and the result can never authorize a dispatch.
use std::time::Duration;

use anyhow::{Result, anyhow, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};

use crate::{
    cart_preferences::{self, CartPreferences, EffectivePreferences},
    catalog,
    config::Config,
    model::{
        Availability, Evidence, FeeScope, InventoryKind, Observation, Offer, PriceScope,
        QuantityKind,
    },
    preference_store::PreferenceSnapshot,
};

const MAX_CATALOG_BYTES: i64 = 16 * 1024 * 1024;
const MAX_OBSERVATION_BYTES: i64 = 1024 * 1024;
const MAX_OBSERVATION_ROWS: usize = 1000;
pub const MAX_OFFER_CHECKS: usize = 128;
pub const MAX_AGE_MS: i64 = 5000;
const MAX_SAFE_REVISION: u64 = 9_007_199_254_740_991;

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Check {
    Matches,
    Mismatch,
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TriggerReason {
    WatchDisabled,
    WatchNotRegistered,
    WatchConfigurationChanged,
    UnsupportedMarket,
    UnsupportedCurrency,
    NoLatestObservation,
    ObservationBindingMismatch,
    InvalidObservation,
    ObservationTimeUnknown,
    ObservationFromFuture,
    ObservationStale,
    LatestObservationBlocked,
    LatestObservationUnknown,
    LatestObservationUnavailable,
    MetadataIsNotInventory,
    ExplicitStatusNeedsOfferEvidence,
    NoOfferEvidence,
    NoObservedOfferRows,
    OfferDiagnosticsTruncated,
    NoEligibleExaminedOffer,
    FreshPreparationCandidate,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MissingEvidence {
    NativeCategoryIdentity,
    AuthenticatedPerformance,
    ExactSeats,
    Adjacency,
    CompleteGroupTotal,
    CheckoutFees,
    CheapestEligibleGroup,
    JournalLockNotChecked,
    StockQuantity,
    Currency,
    TicketType,
    Section,
    Row,
    PriceBasis,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OfferReason {
    StockQuantityUnknown,
    QuantityInsufficient,
    CurrencyMismatch,
    TicketTypeMismatch,
    SectionMismatch,
    RowMismatch,
    DisplayedBudgetExceeded,
    DisplayedSubtotalOutOfRange,
    FreshPreparationRequired,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SubtotalBasis {
    PerTicketTimesRequestedQuantity,
    EqualSizeListingTotal,
}

#[derive(Debug, Serialize)]
pub struct OfferChecks {
    pub quantity: Check,
    pub currency: Check,
    pub ticket_type: Check,
    pub section: Check,
    pub row: Check,
    pub category: Check,
    pub adjacency: Check,
    pub displayed_budget: Check,
}

#[derive(Debug, Serialize)]
pub struct OfferCheck {
    /// Position in the observed rows, never a provider listing identifier.
    pub offer_index: usize,
    /// Source classification only; it does not prove stock, price tier or a cart.
    pub inventory_kind: InventoryKind,
    pub checks: OfferChecks,
    pub displayed_group_subtotal_minor: Option<u64>,
    pub subtotal_basis: Option<SubtotalBasis>,
    pub displayed_fee_scope: FeeScope,
    pub missing_evidence: Vec<MissingEvidence>,
    pub reasons: Vec<OfferReason>,
    pub needs_fresh_native_preparation: bool,
}

#[derive(Debug, Serialize)]
pub struct TriggerPreview {
    pub watch_id: String,
    pub effective_preferences: EffectivePreferences,
    pub preference_revision: u64,
    pub preference_document_hash: String,
    pub observed_at_ms: Option<i64>,
    pub observation_age_ms: Option<i64>,
    /// Stored monitor timestamps are whole seconds. Age uses their conservative
    /// lower endpoint, so a preview can expire slightly earlier than five seconds.
    pub observation_timestamp_resolution_ms: u32,
    pub source_snapshot_complete: bool,
    pub observed_offer_rows: usize,
    pub examined_offer_rows: usize,
    pub offers_truncated: bool,
    pub reasons: Vec<TriggerReason>,
    pub missing_evidence: Vec<MissingEvidence>,
    pub offers: Vec<OfferCheck>,
    pub needs_fresh_native_preparation: bool,
    pub cheapest_verified: bool,
    pub reservation_authorized: bool,
}

fn missing_base() -> Vec<MissingEvidence> {
    vec![
        MissingEvidence::NativeCategoryIdentity,
        MissingEvidence::AuthenticatedPerformance,
        MissingEvidence::ExactSeats,
        MissingEvidence::Adjacency,
        MissingEvidence::CompleteGroupTotal,
        MissingEvidence::CheckoutFees,
        MissingEvidence::CheapestEligibleGroup,
        MissingEvidence::JournalLockNotChecked,
    ]
}

impl TriggerPreview {
    fn reject(mut self, reason: TriggerReason) -> Self {
        self.reasons.push(reason);
        self
    }
}

fn unique_json(raw: &str) -> Result<Value> {
    serde_json::from_str::<crate::page_data::UniqueJson>(raw)
        .map(|value| value.0)
        .map_err(|_| anyhow!("stored monitor JSON is invalid or ambiguous"))
}

fn catalog_snapshot(tx: &Transaction<'_>) -> Result<Option<Value>> {
    let stored: Option<(i64, Option<String>)> = tx.query_row(
        "SELECT length(CAST(snapshot_json AS BLOB)), CASE WHEN length(CAST(snapshot_json AS BLOB))<=?1 THEN snapshot_json END FROM catalog_snapshot WHERE id=1",
        [MAX_CATALOG_BYTES], |row| Ok((row.get(0)?, row.get(1)?)),
    ).optional().map_err(|_| anyhow!("read monitor catalog snapshot"))?;
    let Some((size, raw)) = stored else {
        return Ok(None);
    };
    ensure!(
        (0..=MAX_CATALOG_BYTES).contains(&size),
        "stored monitor catalog exceeds size limit"
    );
    unique_json(
        raw.as_deref()
            .ok_or_else(|| anyhow!("stored monitor catalog is missing"))?,
    )
    .map(Some)
}

fn object_fields(value: &Value, allowed: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|object| object.keys().all(|key| allowed.contains(&key.as_str())))
}

fn bounded_text(value: &str, limit: usize) -> bool {
    value.len() <= limit && !value.chars().any(char::is_control)
}

fn parse_observation(raw: &str) -> Result<Observation> {
    let value = unique_json(raw)?;
    ensure!(
        object_fields(
            &value,
            &[
                "watch_id",
                "observed_at",
                "availability",
                "evidence",
                "complete",
                "offers",
                "ticket_categories",
                "adapter",
                "detail"
            ]
        ),
        "stored observation fields are unsupported"
    );
    let offers = value
        .get("offers")
        .and_then(Value::as_array)
        .ok_or_else(|| anyhow!("stored observation rows are invalid"))?;
    ensure!(
        offers.len() <= MAX_OBSERVATION_ROWS
            && offers.iter().all(|offer| object_fields(
                offer,
                &[
                    "key",
                    "section",
                    "row",
                    "seats",
                    "ticket_type",
                    "inventory",
                    "inventory_kind",
                    "price_text",
                    "price_minor",
                    "currency",
                    "quantity",
                    "quantity_kind",
                    "price_scope",
                    "fees"
                ]
            )),
        "stored offer fields are unsupported or oversized"
    );
    if let Some(categories) = value.get("ticket_categories") {
        ensure!(
            categories
                .as_array()
                .is_some_and(|rows| rows.len() <= MAX_OBSERVATION_ROWS
                    && rows.iter().all(|row| object_fields(
                        row,
                        &[
                            "key",
                            "ticket_type",
                            "inventory_kind",
                            "section",
                            "row",
                            "price_text",
                            "price_min_minor",
                            "price_max_minor",
                            "currency",
                            "selectable_quantity"
                        ]
                    ))),
            "stored category fields are unsupported or oversized"
        );
    }
    let observation: Observation = serde_json::from_value(value)
        .map_err(|_| anyhow!("stored observation schema is invalid"))?;
    ensure!(
        observation.watch_id.len() <= 128
            && observation.detail.len() <= 4096
            && observation
                .adapter
                .as_ref()
                .is_none_or(|value| bounded_text(value, 128)),
        "stored observation fields exceed limits"
    );
    for offer in &observation.offers {
        ensure!(
            bounded_text(&offer.key, 512)
                && [
                    &offer.section,
                    &offer.row,
                    &offer.seats,
                    &offer.ticket_type,
                    &offer.inventory
                ]
                .iter()
                .all(|value| value.as_ref().is_none_or(|value| bounded_text(value, 512)))
                && offer
                    .price_text
                    .as_ref()
                    .is_none_or(|value| bounded_text(value, 2048))
                && offer
                    .price_minor
                    .is_none_or(
                        |price| (0..=crate::cart::MAX_EXACT_INTEGER as i64).contains(&price)
                    )
                && offer.quantity.is_none_or(|quantity| quantity <= 1_000_000)
                && offer
                    .currency
                    .as_ref()
                    .is_none_or(|code| code.len() == 3
                        && code.bytes().all(|byte| byte.is_ascii_uppercase())),
            "stored offer values are invalid or oversized"
        );
    }
    Ok(observation)
}

fn label_check(allowed: &[String], observed: Option<&str>) -> Check {
    if allowed.is_empty() {
        return Check::Matches;
    }
    match observed.filter(|value| !value.trim().is_empty()) {
        Some(value) if crate::cart::label_allowed(allowed, value) => Check::Matches,
        Some(_) => Check::Mismatch,
        None => Check::Unknown,
    }
}

fn offer_check(index: usize, offer: &Offer, prefs: &CartPreferences) -> OfferCheck {
    let quantity = match (offer.quantity_kind, offer.quantity) {
        (QuantityKind::Listing | QuantityKind::Remaining, Some(value))
            if value >= prefs.quantity =>
        {
            Check::Matches
        }
        (QuantityKind::Listing | QuantityKind::Remaining, Some(_)) => Check::Mismatch,
        _ => Check::Unknown,
    };
    let currency = match offer.currency.as_deref() {
        Some(code) if code == prefs.currency => Check::Matches,
        Some(_) => Check::Mismatch,
        None => Check::Unknown,
    };
    let ticket_type = label_check(&prefs.allowed_ticket_types, offer.ticket_type.as_deref());
    let section = label_check(&prefs.allowed_sections, offer.section.as_deref());
    let row = label_check(&prefs.allowed_rows, offer.row.as_deref());
    let (subtotal, subtotal_basis) = if currency == Check::Matches {
        match (
            offer.price_scope,
            offer
                .price_minor
                .and_then(|price| u64::try_from(price).ok()),
        ) {
            (PriceScope::PerTicket, Some(price)) => (
                price.checked_mul(u64::from(prefs.quantity)),
                Some(SubtotalBasis::PerTicketTimesRequestedQuantity),
            ),
            (PriceScope::Total, Some(price))
                if offer.quantity_kind == QuantityKind::Listing
                    && offer.quantity == Some(prefs.quantity) =>
            {
                (Some(price), Some(SubtotalBasis::EqualSizeListingTotal))
            }
            _ => (None, None),
        }
    } else {
        (None, None)
    };
    let computed_subtotal = subtotal;
    let subtotal_out_of_range =
        computed_subtotal.is_some_and(|value| value > crate::cart::MAX_EXACT_INTEGER);
    let subtotal = computed_subtotal.filter(|value| *value <= crate::cart::MAX_EXACT_INTEGER);
    let subtotal_basis = subtotal.and(subtotal_basis);
    let displayed_budget = match (prefs.max_total_minor, computed_subtotal) {
        (None, _) => Check::Matches,
        (Some(limit), Some(amount)) if amount > limit => Check::Mismatch,
        (Some(_), Some(_)) if offer.fees == FeeScope::Included => Check::Matches,
        _ => Check::Unknown,
    };
    let mut missing = missing_base();
    for (status, evidence) in [
        (quantity, MissingEvidence::StockQuantity),
        (currency, MissingEvidence::Currency),
        (ticket_type, MissingEvidence::TicketType),
        (section, MissingEvidence::Section),
        (row, MissingEvidence::Row),
    ] {
        if status == Check::Unknown {
            missing.push(evidence);
        }
    }
    if subtotal.is_none() {
        missing.push(MissingEvidence::PriceBasis);
    }
    let mut reasons = Vec::new();
    if subtotal_out_of_range {
        reasons.push(OfferReason::DisplayedSubtotalOutOfRange);
    }
    if quantity == Check::Unknown {
        reasons.push(OfferReason::StockQuantityUnknown);
    }
    for (status, reason) in [
        (quantity, OfferReason::QuantityInsufficient),
        (currency, OfferReason::CurrencyMismatch),
        (ticket_type, OfferReason::TicketTypeMismatch),
        (section, OfferReason::SectionMismatch),
        (row, OfferReason::RowMismatch),
        (displayed_budget, OfferReason::DisplayedBudgetExceeded),
    ] {
        if status == Check::Mismatch {
            reasons.push(reason);
        }
    }
    let candidate = !subtotal_out_of_range
        && quantity == Check::Matches
        && ![currency, ticket_type, section, row, displayed_budget].contains(&Check::Mismatch);
    if candidate {
        reasons.push(OfferReason::FreshPreparationRequired);
    }
    OfferCheck {
        offer_index: index,
        inventory_kind: offer.inventory_kind,
        checks: OfferChecks {
            quantity,
            currency,
            ticket_type,
            section,
            row,
            category: if prefs.allowed_category_ids.is_empty() {
                Check::Matches
            } else {
                Check::Unknown
            },
            adjacency: if prefs.require_adjacent {
                Check::Unknown
            } else {
                Check::Matches
            },
            displayed_budget,
        },
        displayed_group_subtotal_minor: subtotal,
        subtotal_basis,
        displayed_fee_scope: offer.fees,
        missing_evidence: missing,
        reasons,
        needs_fresh_native_preparation: candidate,
    }
}

pub fn preview(
    config: &Config,
    watch_id: &str,
    prefs: &PreferenceSnapshot,
    now_ms: i64,
) -> Result<TriggerPreview> {
    ensure!(
        (1..=crate::cart::MAX_EXACT_INTEGER as i64).contains(&now_ms)
            && !watch_id.is_empty()
            && watch_id.len() <= 128
            && watch_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric()
                    || matches!(byte, b'-' | b'_' | b'.' | b':')),
        "invalid monitor preview input"
    );
    cart_preferences::validate_document(&prefs.document)?;
    ensure!(
        (1..=MAX_SAFE_REVISION).contains(&prefs.revision)
            && prefs.updated_at_ms >= 0
            && prefs.updated_at_ms <= now_ms,
        "invalid preferences revision or timestamp"
    );
    let document = serde_json::to_vec(&prefs.document)?;
    ensure!(
        document.len() <= cart_preferences::MAX_INPUT_BYTES
            && prefs.document_hash == format!("{:x}", Sha256::digest(&document)),
        "preferences snapshot hash does not match document"
    );
    ensure!(
        config.database.is_file(),
        "monitor database does not exist or is not a file"
    );
    let mut connection =
        Connection::open_with_flags(&config.database, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .map_err(|_| anyhow!("open monitor database read-only"))?;
    connection.busy_timeout(Duration::from_secs(2))?;
    let tx = connection
        .transaction()
        .map_err(|_| anyhow!("start monitor read snapshot"))?;
    let snapshot = catalog::decode(config, catalog_snapshot(&tx)?)
        .map_err(|_| anyhow!("stored catalog does not match supported schema"))?;
    let watches = catalog::effective_watches(config, &snapshot)
        .map_err(|_| anyhow!("resolve current monitor watches"))?;
    let matches = watches
        .iter()
        .filter(|watch| watch.id == watch_id)
        .collect::<Vec<_>>();
    ensure!(
        matches.len() == 1,
        "watch is not unique in the current configuration and catalog"
    );
    let watch = matches[0];
    let effective = cart_preferences::resolve(&prefs.document, &watch.url)?;
    let result = TriggerPreview {
        watch_id: watch_id.into(),
        effective_preferences: effective,
        preference_revision: prefs.revision,
        preference_document_hash: prefs.document_hash.clone(),
        observed_at_ms: None,
        observation_age_ms: None,
        observation_timestamp_resolution_ms: 1000,
        source_snapshot_complete: false,
        observed_offer_rows: 0,
        examined_offer_rows: 0,
        offers_truncated: false,
        reasons: vec![],
        missing_evidence: missing_base(),
        offers: vec![],
        needs_fresh_native_preparation: false,
        cheapest_verified: false,
        reservation_authorized: false,
    };
    if !watch.enabled {
        return Ok(result.reject(TriggerReason::WatchDisabled));
    }
    let registered: Option<(String, bool, Option<i64>)> = tx
        .query_row(
            "SELECT fingerprint,enabled,last_observed_at FROM watches WHERE id=?1",
            [watch_id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()
        .map_err(|_| anyhow!("read monitor watch binding"))?;
    let Some((fingerprint, enabled, last_observed)) = registered else {
        return Ok(result.reject(TriggerReason::WatchNotRegistered));
    };
    if !enabled {
        return Ok(result.reject(TriggerReason::WatchDisabled));
    }
    if fingerprint != crate::store::fingerprint(watch)? {
        return Ok(result.reject(TriggerReason::WatchConfigurationChanged));
    }
    if crate::event_identity::identity(&watch.url)
        .is_none_or(|identity| identity.0 != "ticketmaster.cz")
    {
        return Ok(result.reject(TriggerReason::UnsupportedMarket));
    }
    if result.effective_preferences.preferences.currency != "CZK"
        || watch.currency.as_deref().is_some_and(|code| code != "CZK")
    {
        return Ok(result.reject(TriggerReason::UnsupportedCurrency));
    }
    let stored: Option<(i64, Option<String>)> = tx.query_row("SELECT length(CAST(observation_json AS BLOB)), CASE WHEN length(CAST(observation_json AS BLOB))<=?2 THEN observation_json END FROM watch_observations WHERE watch_id=?1", rusqlite::params![watch_id, MAX_OBSERVATION_BYTES], |row| Ok((row.get(0)?, row.get(1)?))).optional().map_err(|_| anyhow!("read latest monitor observation"))?;
    let Some((size, raw)) = stored else {
        return Ok(result.reject(TriggerReason::NoLatestObservation));
    };
    if !(0..=MAX_OBSERVATION_BYTES).contains(&size) {
        return Ok(result.reject(TriggerReason::InvalidObservation));
    }
    let observation = match raw.as_deref().map(parse_observation) {
        Some(Ok(value)) => value,
        _ => return Ok(result.reject(TriggerReason::InvalidObservation)),
    };
    if observation.watch_id != watch.id || last_observed != Some(observation.observed_at) {
        return Ok(result.reject(TriggerReason::ObservationBindingMismatch));
    }
    let mut result = result;
    result.source_snapshot_complete = observation.complete;
    let observed_ms = match observation
        .observed_at
        .checked_mul(1000)
        .filter(|value| (1..=crate::cart::MAX_EXACT_INTEGER as i64).contains(value))
    {
        Some(value) => value,
        None => return Ok(result.reject(TriggerReason::ObservationTimeUnknown)),
    };
    result.observed_at_ms = Some(observed_ms);
    if observed_ms > now_ms {
        return Ok(result.reject(TriggerReason::ObservationFromFuture));
    }
    result.observation_age_ms = Some(now_ms - observed_ms);
    if now_ms - observed_ms > MAX_AGE_MS {
        return Ok(result.reject(TriggerReason::ObservationStale));
    }
    match observation.availability {
        Availability::Blocked => return Ok(result.reject(TriggerReason::LatestObservationBlocked)),
        Availability::Unknown => return Ok(result.reject(TriggerReason::LatestObservationUnknown)),
        Availability::Unavailable => {
            return Ok(result.reject(TriggerReason::LatestObservationUnavailable));
        }
        Availability::Available => {}
    }
    match observation.evidence {
        Evidence::Metadata => return Ok(result.reject(TriggerReason::MetadataIsNotInventory)),
        Evidence::ExplicitStatus => {
            return Ok(result.reject(TriggerReason::ExplicitStatusNeedsOfferEvidence));
        }
        Evidence::None => return Ok(result.reject(TriggerReason::NoOfferEvidence)),
        Evidence::Offers => {}
    }
    result.observed_offer_rows = observation.offers.len();
    if observation.offers.is_empty() {
        return Ok(result.reject(TriggerReason::NoObservedOfferRows));
    }
    result.offers_truncated = observation.offers.len() > MAX_OFFER_CHECKS;
    if result.offers_truncated {
        result
            .reasons
            .push(TriggerReason::OfferDiagnosticsTruncated);
    }
    result.offers = observation
        .offers
        .iter()
        .take(MAX_OFFER_CHECKS)
        .enumerate()
        .map(|(index, offer)| offer_check(index, offer, &result.effective_preferences.preferences))
        .collect();
    result.examined_offer_rows = result.offers.len();
    result.needs_fresh_native_preparation = result
        .offers
        .iter()
        .any(|offer| offer.needs_fresh_native_preparation);
    result
        .reasons
        .push(if result.needs_fresh_native_preparation {
            TriggerReason::FreshPreparationCandidate
        } else {
            TriggerReason::NoEligibleExaminedOffer
        });
    Ok(result)
}
