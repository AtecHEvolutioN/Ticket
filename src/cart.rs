//! Pure, side-effect-free planning for one Czech adjacent-seat request.
//!
//! This module evaluates supplied quote claims, not independently verified live
//! inventory. A browser runner must obtain and revalidate those claims in the
//! same session/document before any separate reservation step. JSON files are
//! useful for review and fixtures; they cannot authorize a live reservation.
//! Neither category ranges, purchase limits, nor consecutive seat numbers prove
//! that a requested adjacent group exists. The conservative first implementation
//! accepts only explicitly confirmed seated groups in one section and row.

use std::collections::HashSet;

use chrono::{DateTime, Datelike, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};

/// Input/engineering limits, not provider purchase limits or stock counts.
pub const MAX_INPUT_BYTES: usize = 128 * 1024;
pub const MAX_GROUPS: usize = 128;
pub const MAX_QUANTITY: u32 = 20;
pub const MAX_QUOTE_AGE_MS: i64 = 5_000;
/// Preserve exact integer values when exchanging decisions with a JS runner.
pub const MAX_EXACT_INTEGER: u64 = 9_007_199_254_740_991;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CartPlanRequest {
    pub intent: CartIntent,
    pub context: BrowserContext,
    pub snapshot: QuoteSnapshot,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CartIntent {
    pub event_url: String,
    /// Intended performance start, including its known UTC offset. Missing starts
    /// permit offline diagnostics only; they never produce an admissible plan.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub expected_event_start: Option<String>,
    /// Deliberately optional at the input boundary: absence produces a fixed
    /// missing_quantity decision, never a default or Watch.min_quantity fallback.
    #[serde(default)]
    pub quantity: Option<u32>,
    #[serde(default)]
    pub max_total_minor: Option<u64>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserContext {
    /// Application-generated correlation IDs, never cookies or authentication.
    pub session_id: String,
    pub document_id: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct QuoteSnapshot {
    pub event_url: String,
    /// Caller-observed performance start, not the quote observation timestamp.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_start: Option<String>,
    pub session_id: String,
    pub document_id: String,
    /// Coverage describes this exact search quantity, not all page inventory.
    pub requested_quantity: u32,
    pub observed_at_ms: i64,
    /// A caller-imposed freshness deadline; this does not represent a hold.
    pub expires_at_ms: i64,
    #[serde(default)]
    pub coverage: Coverage,
    #[serde(default)]
    pub groups: Vec<GroupQuote>,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Coverage {
    CompleteForRequest,
    #[default]
    Partial,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupQuote {
    pub id: String,
    pub quantity: u32,
    #[serde(default)]
    pub kind: SeatingKind,
    #[serde(default)]
    pub availability: GroupAvailability,
    #[serde(default)]
    pub evidence: QuoteEvidence,
    #[serde(default)]
    pub seats: Vec<SeatIdentity>,
    #[serde(default)]
    pub adjacency: AdjacencyEvidence,
    #[serde(default)]
    pub price: GroupPrice,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct SeatIdentity {
    pub id: String,
    pub section: String,
    pub row: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Clone, Debug, Default, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct GroupPrice {
    #[serde(default)]
    pub amount_minor: Option<u64>,
    #[serde(default)]
    pub currency: Option<String>,
    #[serde(default)]
    pub basis: PriceBasis,
    #[serde(default)]
    pub fees: QuoteFees,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SeatingKind {
    Seated,
    Standing,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupAvailability {
    Confirmed,
    Unavailable,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuoteEvidence {
    ExactGroup,
    CategoryRange,
    PurchaseLimit,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AdjacencyEvidence {
    SiteConfirmed,
    NotAdjacent,
    SeatNumbersOnly,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PriceBasis {
    GroupTotal,
    PerTicket,
    Range,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QuoteFees {
    Complete,
    Partial,
    #[default]
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum PlanOutcome {
    Planned,
    Provisional,
    Refused,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RankingScope {
    CheapestForRequest,
    CheapestObserved,
}

/// Fixed machine-readable reasons; parser errors never echo submitted contents.
#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionReason {
    InputTooLarge,
    InvalidJson,
    MissingQuantity,
    InvalidQuantity,
    InvalidEventUrl,
    EventMismatch,
    MissingExpectedEventStart,
    MissingEventStart,
    InvalidExpectedEventStart,
    InvalidEventStart,
    EventStartMismatch,
    InvalidContext,
    SessionMismatch,
    DocumentMismatch,
    RequestedQuantityMismatch,
    InvalidTimestamp,
    FutureQuote,
    StaleQuote,
    ExpiredQuote,
    InvalidInputBounds,
    DuplicateGroupId,
    NoEligibleGroup,
    PartialCoverage,
    UnresolvedCandidates,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum GroupRejectionReason {
    QuantityMismatch,
    StandingNotAdjacentSeats,
    Unavailable,
    NotAdjacent,
    ExactGroupEvidenceRequired,
    SeatingUnknown,
    AvailabilityUnknown,
    SeatCountMismatch,
    DuplicateSeat,
    DifferentSectionOrRow,
    AdjacencyUnconfirmed,
    GroupTotalRequired,
    CzkRequired,
    CompleteFeesRequired,
    AmountMissing,
    AboveMaximumTotal,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct GroupRejection {
    pub group_id: String,
    pub reason: GroupRejectionReason,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SelectedGroup {
    pub group_id: String,
    /// Canonical UTC performance start from supplied evidence, when known.
    /// Matching this claim never authenticates the provider or its inventory.
    pub event_start: Option<String>,
    pub quantity: u32,
    pub total_minor: u64,
    pub currency: String,
    pub seats: Vec<SeatIdentity>,
    pub quote_observed_at_ms: i64,
    /// Exclusive effective deadline, bounded by the planner's maximum age.
    pub quote_expires_at_ms: i64,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum DecisionBasis {
    SuppliedQuoteClaims,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct CartDecision {
    pub outcome: PlanOutcome,
    pub reason: Option<DecisionReason>,
    pub ranking_scope: Option<RankingScope>,
    pub selected: Option<SelectedGroup>,
    pub rejected_groups: Vec<GroupRejection>,
    pub basis: DecisionBasis,
    pub reservation_authorized: bool,
    pub reservation_performed: bool,
}

impl CartDecision {
    fn refused(reason: DecisionReason) -> Self {
        Self {
            outcome: PlanOutcome::Refused,
            reason: Some(reason),
            ranking_scope: None,
            selected: None,
            rejected_groups: Vec::new(),
            basis: DecisionBasis::SuppliedQuoteClaims,
            reservation_authorized: false,
            reservation_performed: false,
        }
    }
}

/// Parse bounded, strict JSON and evaluate it without I/O or browser actions.
pub fn plan_json(input: &[u8], now_ms: i64) -> CartDecision {
    if input.len() > MAX_INPUT_BYTES {
        return CartDecision::refused(DecisionReason::InputTooLarge);
    }
    match serde_json::from_slice(input) {
        Ok(request) => plan(&request, now_ms),
        Err(_) => CartDecision::refused(DecisionReason::InvalidJson),
    }
}

/// Select an exact group under the declared evidence, never reserve tickets.
///
/// Complete coverage is a caller claim scoped to this event, quantity, document,
/// and time. An unresolved potentially matching group downgrades that claim to
/// cheapest observed. Definitively nonmatching groups need no price comparison.
pub fn plan(request: &CartPlanRequest, now_ms: i64) -> CartDecision {
    if let Err(reason) = validate_request(request, now_ms) {
        return CartDecision::refused(reason);
    }
    // validate_request established explicit quantity and bounded inputs.
    let quantity = request.intent.quantity.unwrap();
    let mut rejected_groups = Vec::new();
    let mut candidates = Vec::new();
    let mut unresolved = false;
    for group in &request.snapshot.groups {
        match candidate(group, quantity, request.intent.max_total_minor) {
            Ok(total) => candidates.push((total, group)),
            Err((reason, uncertain)) => {
                unresolved |= uncertain;
                rejected_groups.push(GroupRejection {
                    group_id: group.id.clone(),
                    reason,
                });
            }
        }
    }
    rejected_groups.sort_by(|a, b| a.group_id.cmp(&b.group_id));
    candidates
        .sort_by(|(a_total, a), (b_total, b)| a_total.cmp(b_total).then_with(|| a.id.cmp(&b.id)));
    let Some((total, group)) = candidates.first() else {
        let mut decision = CartDecision::refused(DecisionReason::NoEligibleGroup);
        decision.rejected_groups = rejected_groups;
        return decision;
    };
    let reason = if request.intent.expected_event_start.is_none() {
        Some(DecisionReason::MissingExpectedEventStart)
    } else if request.snapshot.event_start.is_none() {
        Some(DecisionReason::MissingEventStart)
    } else if request.snapshot.coverage == Coverage::Partial {
        Some(DecisionReason::PartialCoverage)
    } else if unresolved {
        Some(DecisionReason::UnresolvedCandidates)
    } else {
        None
    };
    CartDecision {
        outcome: if reason.is_some() {
            PlanOutcome::Provisional
        } else {
            PlanOutcome::Planned
        },
        reason,
        ranking_scope: Some(if reason.is_some() {
            RankingScope::CheapestObserved
        } else {
            RankingScope::CheapestForRequest
        }),
        selected: Some(SelectedGroup {
            group_id: group.id.clone(),
            event_start: request
                .snapshot
                .event_start
                .as_deref()
                .and_then(normalize_event_start),
            quantity,
            total_minor: *total,
            currency: "CZK".to_owned(),
            seats: group.seats.clone(),
            quote_observed_at_ms: request.snapshot.observed_at_ms,
            quote_expires_at_ms: request
                .snapshot
                .expires_at_ms
                .min(request.snapshot.observed_at_ms + MAX_QUOTE_AGE_MS),
        }),
        rejected_groups,
        basis: DecisionBasis::SuppliedQuoteClaims,
        reservation_authorized: false,
        reservation_performed: false,
    }
}

fn validate_request(request: &CartPlanRequest, now_ms: i64) -> Result<(), DecisionReason> {
    let quantity = request
        .intent
        .quantity
        .ok_or(DecisionReason::MissingQuantity)?;
    if !(1..=MAX_QUANTITY).contains(&quantity) {
        return Err(DecisionReason::InvalidQuantity);
    }
    let intent_id = crate::event_identity::identity(&request.intent.event_url)
        .filter(|(host, _)| host == "ticketmaster.cz")
        .ok_or(DecisionReason::InvalidEventUrl)?;
    let snapshot = &request.snapshot;
    let snapshot_id = crate::event_identity::identity(&snapshot.event_url)
        .filter(|(host, _)| host == "ticketmaster.cz")
        .ok_or(DecisionReason::InvalidEventUrl)?;
    if intent_id != snapshot_id {
        return Err(DecisionReason::EventMismatch);
    }
    let expected_start = request
        .intent
        .expected_event_start
        .as_deref()
        .map(|value| normalize_event_start(value).ok_or(DecisionReason::InvalidExpectedEventStart))
        .transpose()?;
    let observed_start = snapshot
        .event_start
        .as_deref()
        .map(|value| normalize_event_start(value).ok_or(DecisionReason::InvalidEventStart))
        .transpose()?;
    if expected_start.is_some() && observed_start.is_some() && expected_start != observed_start {
        return Err(DecisionReason::EventStartMismatch);
    }
    if [
        &request.context.session_id,
        &request.context.document_id,
        &snapshot.session_id,
        &snapshot.document_id,
    ]
    .iter()
    .any(|id| !valid_id(id))
    {
        return Err(DecisionReason::InvalidContext);
    }
    if request.context.session_id != snapshot.session_id {
        return Err(DecisionReason::SessionMismatch);
    }
    if request.context.document_id != snapshot.document_id {
        return Err(DecisionReason::DocumentMismatch);
    }
    if snapshot.requested_quantity != quantity {
        return Err(DecisionReason::RequestedQuantityMismatch);
    }
    if [now_ms, snapshot.observed_at_ms, snapshot.expires_at_ms]
        .iter()
        .any(|time| *time < 0 || *time as u64 > MAX_EXACT_INTEGER)
        || snapshot.expires_at_ms <= snapshot.observed_at_ms
    {
        return Err(DecisionReason::InvalidTimestamp);
    }
    if snapshot.observed_at_ms > now_ms {
        return Err(DecisionReason::FutureQuote);
    }
    if now_ms - snapshot.observed_at_ms >= MAX_QUOTE_AGE_MS {
        return Err(DecisionReason::StaleQuote);
    }
    if now_ms >= snapshot.expires_at_ms {
        return Err(DecisionReason::ExpiredQuote);
    }
    if request
        .intent
        .max_total_minor
        .is_some_and(|v| v > MAX_EXACT_INTEGER)
        || snapshot.groups.len() > MAX_GROUPS
    {
        return Err(DecisionReason::InvalidInputBounds);
    }
    let mut ids = HashSet::new();
    for group in &snapshot.groups {
        if !valid_id(&group.id)
            || group.quantity == 0
            || group.quantity > MAX_QUANTITY
            || group.seats.len() > MAX_QUANTITY as usize
            || group
                .price
                .amount_minor
                .is_some_and(|v| v > MAX_EXACT_INTEGER)
            || group
                .price
                .currency
                .as_ref()
                .is_some_and(|v| v.len() != 3 || !v.bytes().all(|c| c.is_ascii_uppercase()))
            || group.seats.iter().any(|seat| {
                !valid_id(&seat.id)
                    || !valid_label(&seat.section)
                    || !valid_label(&seat.row)
                    || seat.label.as_ref().is_some_and(|v| !valid_label(v))
            })
        {
            return Err(DecisionReason::InvalidInputBounds);
        }
        if !ids.insert(&group.id) {
            return Err(DecisionReason::DuplicateGroupId);
        }
    }
    Ok(())
}

/// Strict, bounded RFC3339 performance start with a known explicit offset.
/// Normalize to UTC without rounding away fractional seconds. Local/date-only
/// values, unknown `-00:00` offsets, and leap-second ambiguity fail closed.
/// This validates the caller's claim only, never its native source authenticity.
pub fn normalize_event_start(value: &str) -> Option<String> {
    if !(20..=35).contains(&value.len()) || !value.is_ascii() {
        return None;
    }
    let bytes = value.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
        || [0..4, 5..7, 8..10, 11..13, 14..16, 17..19]
            .iter()
            .any(|range| !bytes[range.clone()].iter().all(u8::is_ascii_digit))
        || &value[17..19] > "59"
    {
        return None;
    }
    let mut suffix = &value[19..];
    if let Some(fraction) = suffix.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if !(1..=9).contains(&digits) {
            return None;
        }
        suffix = &fraction[digits..];
    }
    if suffix != "Z" {
        let offset = suffix.as_bytes();
        if offset.len() != 6
            || !matches!(offset[0], b'+' | b'-')
            || offset[3] != b':'
            || suffix == "-00:00"
            || ![offset[1], offset[2], offset[4], offset[5]]
                .iter()
                .all(u8::is_ascii_digit)
        {
            return None;
        }
    }
    let instant = DateTime::parse_from_rfc3339(value)
        .ok()?
        .with_timezone(&Utc);
    if !(0..=9999).contains(&instant.year()) {
        return None;
    }
    Some(instant.to_rfc3339_opts(SecondsFormat::AutoSi, true))
}

fn valid_id(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':'))
}

fn valid_label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn normalized_label(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

/// The boolean flags unresolved evidence that could conceal a cheaper match.
fn candidate(
    group: &GroupQuote,
    quantity: u32,
    max_total: Option<u64>,
) -> Result<u64, (GroupRejectionReason, bool)> {
    use GroupRejectionReason as R;
    // A category/purchase-limit quantity is not an exact group size. It cannot
    // exclude a potentially cheaper match, even when it differs from the intent.
    if group.evidence != QuoteEvidence::ExactGroup {
        return Err((R::ExactGroupEvidenceRequired, true));
    }
    if group.kind == SeatingKind::Standing {
        return Err((R::StandingNotAdjacentSeats, false));
    }
    if group.availability == GroupAvailability::Unavailable {
        return Err((R::Unavailable, false));
    }
    if group.adjacency == AdjacencyEvidence::NotAdjacent {
        return Err((R::NotAdjacent, false));
    }
    if group.kind != SeatingKind::Seated {
        return Err((R::SeatingUnknown, true));
    }
    if group.availability != GroupAvailability::Confirmed {
        return Err((R::AvailabilityUnknown, true));
    }
    if group.seats.len() != group.quantity as usize {
        return Err((R::SeatCountMismatch, true));
    }
    let mut ids = HashSet::new();
    let mut locations = HashSet::new();
    for seat in &group.seats {
        if !ids.insert(&seat.id) {
            return Err((R::DuplicateSeat, true));
        }
        if let Some(label) = &seat.label
            && !locations.insert((
                normalized_label(&seat.section),
                normalized_label(&seat.row),
                normalized_label(label),
            ))
        {
            return Err((R::DuplicateSeat, true));
        }
    }
    // Establish the internally consistent unique group size before treating a
    // different quantity as definitively irrelevant to this exact-size search.
    if group.quantity != quantity {
        return Err((R::QuantityMismatch, false));
    }
    let first = &group.seats[0];
    if group.seats.iter().any(|seat| {
        normalized_label(&seat.section) != normalized_label(&first.section)
            || normalized_label(&seat.row) != normalized_label(&first.row)
    }) {
        return Err((R::DifferentSectionOrRow, true));
    }
    if group.adjacency != AdjacencyEvidence::SiteConfirmed {
        return Err((R::AdjacencyUnconfirmed, true));
    }
    if group.price.basis != PriceBasis::GroupTotal {
        return Err((R::GroupTotalRequired, true));
    }
    if group.price.currency.as_deref() != Some("CZK") {
        return Err((R::CzkRequired, true));
    }
    if group.price.fees != QuoteFees::Complete {
        return Err((R::CompleteFeesRequired, true));
    }
    let total = group.price.amount_minor.ok_or((R::AmountMissing, true))?;
    if max_total.is_some_and(|maximum| total > maximum) {
        return Err((R::AboveMaximumTotal, false));
    }
    Ok(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NOW: i64 = 1_800_000_000_000;

    fn group(id: &str, total: u64) -> GroupQuote {
        GroupQuote {
            id: id.to_owned(),
            quantity: 2,
            kind: SeatingKind::Seated,
            availability: GroupAvailability::Confirmed,
            evidence: QuoteEvidence::ExactGroup,
            seats: vec![
                SeatIdentity {
                    id: format!("{id}:1"),
                    section: "A".into(),
                    row: "6".into(),
                    label: Some("1".into()),
                },
                SeatIdentity {
                    id: format!("{id}:2"),
                    section: "A".into(),
                    row: "6".into(),
                    label: Some("3".into()),
                },
            ],
            adjacency: AdjacencyEvidence::SiteConfirmed,
            price: GroupPrice {
                amount_minor: Some(total),
                currency: Some("CZK".into()),
                basis: PriceBasis::GroupTotal,
                fees: QuoteFees::Complete,
            },
        }
    }

    fn request() -> CartPlanRequest {
        let event_url = "https://www.ticketmaster.cz/event/korn-vstupenky/1472650657".to_owned();
        CartPlanRequest {
            intent: CartIntent {
                event_url: event_url.clone(),
                expected_event_start: Some("2026-11-16T18:00:00+01:00".into()),
                quantity: Some(2),
                max_total_minor: None,
            },
            context: BrowserContext {
                session_id: "browser-1".into(),
                document_id: "document-1".into(),
            },
            snapshot: QuoteSnapshot {
                event_url,
                event_start: Some("2026-11-16T17:00:00Z".into()),
                session_id: "browser-1".into(),
                document_id: "document-1".into(),
                requested_quantity: 2,
                observed_at_ms: NOW - 100,
                expires_at_ms: NOW + 1000,
                coverage: Coverage::CompleteForRequest,
                groups: vec![group("a", 400_000)],
            },
        }
    }

    fn rejected(request: &CartPlanRequest, reason: DecisionReason) {
        let decision = plan(request, NOW);
        assert_eq!(decision.outcome, PlanOutcome::Refused);
        assert_eq!(decision.reason, Some(reason));
        assert!(decision.selected.is_none());
        assert!(!decision.reservation_authorized);
        assert!(!decision.reservation_performed);
    }

    fn group_rejected(request: &CartPlanRequest, reason: GroupRejectionReason) {
        let decision = plan(request, NOW);
        assert_eq!(decision.outcome, PlanOutcome::Refused);
        assert_eq!(decision.reason, Some(DecisionReason::NoEligibleGroup));
        assert_eq!(decision.rejected_groups.len(), 1);
        assert_eq!(decision.rejected_groups[0].reason, reason);
    }

    #[test]
    fn cheapest_total_and_ties_are_deterministic_without_a_required_price_cap() {
        let mut input = request();
        input.snapshot.groups = vec![
            group("z", 300_000),
            group("b", 500_000),
            group("a", 300_000),
        ];
        let decision = plan(&input, NOW);
        assert_eq!(decision.outcome, PlanOutcome::Planned);
        assert_eq!(
            decision.ranking_scope,
            Some(RankingScope::CheapestForRequest)
        );
        assert_eq!(decision.selected.as_ref().unwrap().group_id, "a");
        assert_eq!(decision.selected.as_ref().unwrap().total_minor, 300_000);
        input.snapshot.groups.reverse();
        assert_eq!(decision, plan(&input, NOW));
        assert_eq!(decision.basis, DecisionBasis::SuppliedQuoteClaims);
        assert!(!decision.reservation_authorized);
        assert!(!decision.reservation_performed);
    }

    #[test]
    fn quantity_is_explicit_and_snapshot_is_bound_to_that_search() {
        let mut input = request();
        input.intent.quantity = None;
        rejected(&input, DecisionReason::MissingQuantity);
        let mut json = serde_json::to_value(request()).unwrap();
        json["intent"].as_object_mut().unwrap().remove("quantity");
        assert_eq!(
            plan_json(&serde_json::to_vec(&json).unwrap(), NOW).reason,
            Some(DecisionReason::MissingQuantity)
        );
        for quantity in [0, MAX_QUANTITY + 1] {
            input.intent.quantity = Some(quantity);
            rejected(&input, DecisionReason::InvalidQuantity);
        }
        input.intent.quantity = Some(3);
        rejected(&input, DecisionReason::RequestedQuantityMismatch);
    }

    #[test]
    fn optional_ceiling_compares_exact_complete_total() {
        let mut input = request();
        input.intent.max_total_minor = Some(399_999);
        group_rejected(&input, GroupRejectionReason::AboveMaximumTotal);
        input.intent.max_total_minor = Some(400_000);
        assert_eq!(plan(&input, NOW).outcome, PlanOutcome::Planned);
    }

    #[test]
    fn public_czech_identity_allows_slug_changes_and_rejects_wrong_event() {
        let mut input = request();
        input.snapshot.event_url =
            "https://ticketmaster.cz/event/renamed/1472650657?language=en".into();
        assert_eq!(plan(&input, NOW).outcome, PlanOutcome::Planned);
        input.snapshot.event_url = "https://ticketmaster.cz/event/renamed/1472650658".into();
        rejected(&input, DecisionReason::EventMismatch);
        for url in [
            "https://www.ticketmaster.pl/event/korn/1472650657",
            "https://secure.ticketmaster.cz/event/korn/1472650657",
            "https://www.ticketmaster.cz/event/korn/1472650657?token=private",
            "https://www.ticketmaster.cz/checkout/../event/1472650657",
            "https://www.ticketmaster.cz/event/korn/%311472650657",
            "http://www.ticketmaster.cz/event/korn/1472650657",
        ] {
            input.snapshot.event_url = url.into();
            rejected(&input, DecisionReason::InvalidEventUrl);
        }
    }

    #[test]
    fn browser_session_and_document_must_match() {
        let mut input = request();
        input.snapshot.session_id = "another-session".into();
        rejected(&input, DecisionReason::SessionMismatch);
        input = request();
        input.snapshot.document_id = "another-document".into();
        rejected(&input, DecisionReason::DocumentMismatch);
        input = request();
        input.context.session_id = "https://private/session".into();
        rejected(&input, DecisionReason::InvalidContext);
    }

    #[test]
    fn freshness_expiry_and_clock_bounds_are_enforced() {
        let mut input = request();
        input.snapshot.observed_at_ms = NOW - MAX_QUOTE_AGE_MS + 1;
        assert_eq!(plan(&input, NOW).outcome, PlanOutcome::Planned);
        input.snapshot.observed_at_ms -= 1;
        rejected(&input, DecisionReason::StaleQuote);
        input = request();
        input.snapshot.observed_at_ms = NOW + 1;
        rejected(&input, DecisionReason::FutureQuote);
        input = request();
        input.snapshot.expires_at_ms = NOW;
        rejected(&input, DecisionReason::ExpiredQuote);
        input.snapshot.expires_at_ms = input.snapshot.observed_at_ms;
        rejected(&input, DecisionReason::InvalidTimestamp);
        for now in [-1, i64::MAX, i64::MIN] {
            assert_eq!(
                plan(&request(), now).reason,
                Some(DecisionReason::InvalidTimestamp)
            );
        }
    }

    #[test]
    fn category_ranges_and_purchase_limits_never_become_candidate_groups() {
        for evidence in [
            QuoteEvidence::CategoryRange,
            QuoteEvidence::PurchaseLimit,
            QuoteEvidence::Unknown,
        ] {
            let mut input = request();
            input.snapshot.groups[0].evidence = evidence;
            group_rejected(&input, GroupRejectionReason::ExactGroupEvidenceRequired);
        }
    }

    #[test]
    fn groups_are_not_split_or_combined_to_make_the_quantity() {
        let mut input = request();
        input.snapshot.groups[0].quantity = 3;
        input.snapshot.groups[0].seats.push(SeatIdentity {
            id: "a:3".into(),
            section: "A".into(),
            row: "6".into(),
            label: Some("5".into()),
        });
        group_rejected(&input, GroupRejectionReason::QuantityMismatch);
        input.snapshot.groups = vec![group("a", 100_000), group("b", 100_000)];
        for group in &mut input.snapshot.groups {
            group.quantity = 1;
            group.seats.pop();
        }
        rejected(&input, DecisionReason::NoEligibleGroup);
        input = request();
        input.snapshot.groups[0].seats.pop();
        group_rejected(&input, GroupRejectionReason::SeatCountMismatch);
    }

    #[test]
    fn explicit_adjacency_is_required_and_number_patterns_prove_nothing() {
        let mut input = request();
        // A site-confirmed odd-numbered neighboring pair is valid.
        assert_eq!(plan(&input, NOW).outcome, PlanOutcome::Planned);
        input.snapshot.groups[0].seats[1].label = Some("2".into());
        for adjacency in [
            AdjacencyEvidence::Unknown,
            AdjacencyEvidence::SeatNumbersOnly,
        ] {
            input.snapshot.groups[0].adjacency = adjacency;
            group_rejected(&input, GroupRejectionReason::AdjacencyUnconfirmed);
        }
        input.snapshot.groups[0].adjacency = AdjacencyEvidence::NotAdjacent;
        group_rejected(&input, GroupRejectionReason::NotAdjacent);
        input = request();
        input.snapshot.groups[0].kind = SeatingKind::Standing;
        group_rejected(&input, GroupRejectionReason::StandingNotAdjacentSeats);
    }

    #[test]
    fn seats_are_unique_and_in_one_section_and_row() {
        let mut input = request();
        input.snapshot.groups[0].seats[1].id = input.snapshot.groups[0].seats[0].id.clone();
        group_rejected(&input, GroupRejectionReason::DuplicateSeat);
        input = request();
        input.snapshot.groups[0].seats[1].label = Some(" 1 ".into());
        group_rejected(&input, GroupRejectionReason::DuplicateSeat);
        input = request();
        input.snapshot.groups[0].seats[1].section = "B".into();
        group_rejected(&input, GroupRejectionReason::DifferentSectionOrRow);
        input = request();
        input.snapshot.groups[0].seats[1].row = "7".into();
        group_rejected(&input, GroupRejectionReason::DifferentSectionOrRow);
    }

    #[test]
    fn price_ranges_per_ticket_prices_missing_fees_and_currency_cannot_be_ranked() {
        for basis in [
            PriceBasis::PerTicket,
            PriceBasis::Range,
            PriceBasis::Unknown,
        ] {
            let mut input = request();
            input.snapshot.groups[0].price.basis = basis;
            group_rejected(&input, GroupRejectionReason::GroupTotalRequired);
        }
        for fees in [QuoteFees::Partial, QuoteFees::Unknown] {
            let mut input = request();
            input.snapshot.groups[0].price.fees = fees;
            group_rejected(&input, GroupRejectionReason::CompleteFeesRequired);
        }
        let mut input = request();
        input.snapshot.groups[0].price.currency = Some("EUR".into());
        group_rejected(&input, GroupRejectionReason::CzkRequired);
        input = request();
        input.snapshot.groups[0].price.amount_minor = None;
        group_rejected(&input, GroupRejectionReason::AmountMissing);
    }

    #[test]
    fn partial_coverage_can_only_claim_cheapest_observed() {
        let mut input = request();
        input.snapshot.coverage = Coverage::Partial;
        let decision = plan(&input, NOW);
        assert_eq!(decision.outcome, PlanOutcome::Provisional);
        assert_eq!(decision.reason, Some(DecisionReason::PartialCoverage));
        assert_eq!(decision.ranking_scope, Some(RankingScope::CheapestObserved));
        assert!(decision.selected.is_some());
        assert!(!decision.reservation_authorized);
    }

    #[test]
    fn unresolved_matching_group_prevents_a_complete_cheapest_claim() {
        let mut input = request();
        let mut unresolved = group("b", 100_000);
        unresolved.price.fees = QuoteFees::Unknown;
        input.snapshot.groups.push(unresolved);
        let decision = plan(&input, NOW);
        assert_eq!(decision.outcome, PlanOutcome::Provisional);
        assert_eq!(decision.reason, Some(DecisionReason::UnresolvedCandidates));
        assert_eq!(decision.selected.unwrap().group_id, "a");
        input.snapshot.groups[1].availability = GroupAvailability::Unavailable;
        assert_eq!(plan(&input, NOW).outcome, PlanOutcome::Planned);
        input.snapshot.groups[1].availability = GroupAvailability::Confirmed;
        input.snapshot.groups[1].quantity = 1;
        input.snapshot.groups[1].seats.pop();
        assert_eq!(plan(&input, NOW).outcome, PlanOutcome::Planned);
    }

    #[test]
    fn malformed_or_non_group_quantities_cannot_hide_an_unresolved_cheaper_match() {
        for evidence in [
            QuoteEvidence::CategoryRange,
            QuoteEvidence::PurchaseLimit,
            QuoteEvidence::Unknown,
            QuoteEvidence::ExactGroup,
        ] {
            let mut input = request();
            let mut unclear = group("b", 1);
            unclear.evidence = evidence;
            unclear.quantity = 6;
            // Two seats plus a reported six is either an inconsistent exact
            // group or a capacity/category figure, never an exclusion proof.
            input.snapshot.groups.push(unclear);
            let decision = plan(&input, NOW);
            assert_eq!(decision.outcome, PlanOutcome::Provisional);
            assert_eq!(decision.reason, Some(DecisionReason::UnresolvedCandidates));
            assert_eq!(decision.ranking_scope, Some(RankingScope::CheapestObserved));
        }
    }

    #[test]
    fn selected_deadline_never_outlives_the_maximum_quote_age() {
        let mut input = request();
        input.snapshot.observed_at_ms = NOW - 4_000;
        input.snapshot.expires_at_ms = NOW + 3_600_000;
        let decision = plan(&input, NOW);
        assert_eq!(decision.outcome, PlanOutcome::Planned);
        assert_eq!(decision.selected.unwrap().quote_expires_at_ms, NOW + 1_000);
    }

    #[test]
    fn duplicate_candidate_identity_and_oversized_inputs_fail_closed() {
        let mut input = request();
        input.snapshot.groups.push(group("a", 300_000));
        rejected(&input, DecisionReason::DuplicateGroupId);
        input = request();
        input.snapshot.groups[0].price.amount_minor = Some(MAX_EXACT_INTEGER + 1);
        rejected(&input, DecisionReason::InvalidInputBounds);
        input = request();
        input.intent.max_total_minor = Some(u64::MAX);
        rejected(&input, DecisionReason::InvalidInputBounds);
        input = request();
        input.snapshot.groups[0].seats[0].id = "x".repeat(129);
        rejected(&input, DecisionReason::InvalidInputBounds);
        input = request();
        input.snapshot.groups[0].seats[0].row = "\n".into();
        rejected(&input, DecisionReason::InvalidInputBounds);
        input = request();
        input.snapshot.groups = (0..=MAX_GROUPS)
            .map(|id| group(&id.to_string(), 1))
            .collect();
        rejected(&input, DecisionReason::InvalidInputBounds);
        assert_eq!(
            plan_json(&vec![b' '; MAX_INPUT_BYTES + 1], NOW).reason,
            Some(DecisionReason::InputTooLarge)
        );
    }

    #[test]
    fn json_boundary_is_strict_and_does_not_echo_untrusted_parser_contents() {
        let good = serde_json::to_value(request()).unwrap();
        assert_eq!(
            plan_json(&serde_json::to_vec(&good).unwrap(), NOW).outcome,
            PlanOutcome::Planned
        );
        for path in [
            vec!["unexpected"],
            vec!["intent", "min_quantity"],
            vec!["context", "cookie"],
            vec!["snapshot", "stock"],
        ] {
            let mut value = good.clone();
            let mut target = &mut value;
            for key in &path[..path.len() - 1] {
                target = &mut target[*key];
            }
            target[path[path.len() - 1]] = serde_json::json!("untrusted-secret");
            let decision = plan_json(&serde_json::to_vec(&value).unwrap(), NOW);
            assert_eq!(decision.reason, Some(DecisionReason::InvalidJson));
            assert!(
                !serde_json::to_string(&decision)
                    .unwrap()
                    .contains("untrusted-secret")
            );
        }
        for invalid in [
            serde_json::json!(-1),
            serde_json::json!(2.5),
            serde_json::json!("2"),
        ] {
            let mut value = good.clone();
            value["intent"]["quantity"] = invalid;
            assert_eq!(
                plan_json(&serde_json::to_vec(&value).unwrap(), NOW).reason,
                Some(DecisionReason::InvalidJson)
            );
        }
        let mut value = good;
        value["snapshot"]["groups"][0]["adjacency"] = serde_json::json!("probably_adjacent");
        assert_eq!(
            plan_json(&serde_json::to_vec(&value).unwrap(), NOW).reason,
            Some(DecisionReason::InvalidJson)
        );
        assert_eq!(
            plan_json(b"{broken", NOW).reason,
            Some(DecisionReason::InvalidJson)
        );
    }

    #[test]
    fn event_start_normalization_preserves_exact_instants_and_rejects_unknown_times() {
        for (input, expected) in [
            ("2026-11-16T18:00:00+01:00", "2026-11-16T17:00:00Z"),
            ("2026-11-16T00:30:00+02:00", "2026-11-15T22:30:00Z"),
            ("2026-11-16T23:30:00-02:00", "2026-11-17T01:30:00Z"),
            ("2026-11-16T17:00:00.1Z", "2026-11-16T17:00:00.100Z"),
            (
                "2026-11-16T18:00:00.100000000+01:00",
                "2026-11-16T17:00:00.100Z",
            ),
            (
                "2026-11-16T17:00:00.000000001+00:00",
                "2026-11-16T17:00:00.000000001Z",
            ),
        ] {
            assert_eq!(normalize_event_start(input).as_deref(), Some(expected));
        }
        for input in [
            "2026-11-16",
            "2026-11-16T18:00:00",
            "2026-11-16T18:00:00-00:00",
            "2026-02-29T18:00:00Z",
            "2026-11-16T24:00:00Z",
            "2026-11-16T18:00:60Z",
            "2026-11-16T18:00:00+24:00",
            "2026-11-16T18:00:00+01:60",
            "2026-11-16 18:00:00Z",
            "2026-11-16T18:00:00.Z",
            "2026-11-16T18:00:00.0000000001Z",
            "2026-11-16T18:00:00Z ",
            "2026-11-16T18:00:00Z\n",
            "unknown",
            "2026-11-16T18:00:00+0100",
        ] {
            assert_eq!(normalize_event_start(input), None, "{input:?}");
        }
    }

    #[test]
    fn missing_event_start_keeps_only_provisional_offline_diagnostics() {
        let mut input = request();
        input.intent.expected_event_start = None;
        let decision = plan(&input, NOW);
        assert_eq!(decision.outcome, PlanOutcome::Provisional);
        assert_eq!(
            decision.reason,
            Some(DecisionReason::MissingExpectedEventStart)
        );
        assert!(decision.selected.is_some());
        assert!(!decision.reservation_authorized);
        input = request();
        input.snapshot.event_start = None;
        let decision = plan(&input, NOW);
        assert_eq!(decision.outcome, PlanOutcome::Provisional);
        assert_eq!(decision.reason, Some(DecisionReason::MissingEventStart));
        assert!(decision.selected.unwrap().event_start.is_none());
        input.intent.expected_event_start = None;
        let json = serde_json::to_value(&input).unwrap();
        assert!(json["intent"].get("expected_event_start").is_none());
        assert!(json["snapshot"].get("event_start").is_none());
        let legacy = plan_json(&serde_json::to_vec(&json).unwrap(), NOW);
        assert_eq!(legacy.outcome, PlanOutcome::Provisional);
        assert!(legacy.selected.is_some());
    }

    #[test]
    fn supplied_performance_dates_must_be_valid_and_match_the_exact_instant() {
        let mut input = request();
        assert_eq!(
            plan(&input, NOW).selected.unwrap().event_start.as_deref(),
            Some("2026-11-16T17:00:00Z")
        );
        for source in [
            "2026-11-17T17:00:00Z",
            "2026-11-16T18:00:00Z",
            "2026-11-16T17:00:00.000000001Z",
        ] {
            input.snapshot.event_start = Some(source.into());
            rejected(&input, DecisionReason::EventStartMismatch);
        }
        input = request();
        input.intent.expected_event_start = Some("2026-11-16".into());
        rejected(&input, DecisionReason::InvalidExpectedEventStart);
        input.intent.expected_event_start = None;
        input.snapshot.event_start = Some("2026-11-16".into());
        rejected(&input, DecisionReason::InvalidEventStart);
        input = request();
        input.intent.expected_event_start = Some("2026-11-16T18:00:00.1+01:00".into());
        input.snapshot.event_start = Some("2026-11-16T17:00:00.100Z".into());
        assert_eq!(plan(&input, NOW).outcome, PlanOutcome::Planned);
        input.snapshot.event_start = Some("2026-11-16T17:00:00.101Z".into());
        rejected(&input, DecisionReason::EventStartMismatch);
    }
}
