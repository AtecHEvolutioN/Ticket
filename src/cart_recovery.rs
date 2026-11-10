//! Read-only recovery guidance for persisted attempts. No browser liveness,
//! current inventory or provider-side release is inferred from a journal.
use std::{collections::HashMap, path::Path};

use anyhow::{Context, Result, ensure};
use serde::Serialize;

use crate::{
    cart,
    cart_journal::{
        self, AttemptBinding, AttemptRecord, AttemptState, AttemptWorkflow, CartJournal,
        NativeSearchOutcome, NativeSearchUncertainty, OutcomeReceipt, SearchReviewReason,
    },
    event_identity,
};

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BrowserSessionState {
    Unknown,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryReason {
    NeverDispatched,
    UnresolvedDispatch,
    RecordedHoldActive,
    RecordedHoldNeedsReview,
    RecordedHoldExpiryElapsed,
    TerminalAbsenceRecorded,
    RecordedHoldExpired,
    AbandonedBeforeDispatch,
    JournalClockNeedsReview,
    JournalStateNeedsReview,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum RecoveryAction {
    InspectJournal,
    InspectNativeHistory,
    AbandonPrepared,
    ReconcileOriginalSession,
    ReviewRecordedHold,
    ReviewRecordedHoldExpiry,
    ReviewFreshIntent,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObservationKind {
    Hold,
    NoHold,
    Uncertain,
    Released,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ObservationClock {
    Valid,
    Future,
    Invalid,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RetainedObservation {
    pub kind: ObservationKind,
    pub observed_at_ms: i64,
    /// Age of a retained claim, never a current inventory freshness guarantee.
    pub age_ms: Option<u64>,
    pub clock: ObservationClock,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub uncertainty: Option<NativeSearchUncertainty>,
    pub review_reasons: Vec<SearchReviewReason>,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RecordedHoldExpiry {
    pub expires_at_ms: i64,
    pub elapsed: bool,
    /// Eligibility under the existing journal API only. This report performs
    /// no expiry transition and cannot independently establish provider release.
    pub journal_expiry_eligible: bool,
    pub evidence_basis: &'static str,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AttemptRecovery {
    pub attempt_id: String,
    pub workflow: AttemptWorkflow,
    pub state: AttemptState,
    pub event_url: String,
    pub expected_event_start: Option<String>,
    pub quantity: u32,
    pub dispatch_recorded: bool,
    pub attempt_retains_event_lock: bool,
    /// Includes other active attempts even when `inspect` selects one old ID.
    pub event_lock_owner_attempt_id: Option<String>,
    pub reason: RecoveryReason,
    pub actions: Vec<RecoveryAction>,
    pub browser_session: BrowserSessionState,
    pub original_session_required_for_reconciliation: bool,
    pub updated_at_ms: i64,
    pub retained_observation: Option<RetainedObservation>,
    pub recorded_hold_expiry: Option<RecordedHoldExpiry>,
    pub retry_authorized: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct RecoveryReport {
    pub inspected_at_ms: i64,
    pub evidence_basis: &'static str,
    pub current_inventory_verified: bool,
    pub browser_session: BrowserSessionState,
    pub retry_authorized: bool,
    pub attempts: Vec<AttemptRecovery>,
}

/// Inspect without creating/migrating a journal, opening a browser, expiring a
/// hold or clearing a lock. A missing requested ID is an error, not an empty plan.
pub fn inspect(path: &Path, attempt_id: Option<&str>, now_ms: i64) -> Result<RecoveryReport> {
    validate_now(now_ms)?;
    if let Some(id) = attempt_id {
        validate_id(id)?;
    }
    let records = cart_journal::inspect_file(path)?;
    summarize(&records, attempt_id, now_ms)
}

/// Explicitly abandon an existing never-dispatched plan. The journal transaction
/// rechecks its state, so an intervening dispatch cannot be cleared by this call.
/// This never cancels a provider request, releases tickets or authorizes a retry.
pub fn abandon_prepared(path: &Path, attempt_id: &str, now_ms: i64) -> Result<AttemptRecovery> {
    validate_now(now_ms)?;
    validate_id(attempt_id)?;
    // Validate the requested record before opening for writes or migrating an
    // older journal. The transactional method below remains the state authority.
    let before = inspect(path, Some(attempt_id), now_ms)?;
    let record = &before.attempts[0];
    ensure!(
        matches!(
            record.state,
            AttemptState::Prepared | AttemptState::AbandonedBeforeDispatch
        ) && !record.dispatch_recorded,
        "only a never-dispatched prepared attempt can be abandoned"
    );
    ensure!(
        !matches!(
            record.reason,
            RecoveryReason::JournalClockNeedsReview | RecoveryReason::JournalStateNeedsReview
        ),
        "cart journal needs review before abandonment"
    );
    let mut journal = CartJournal::open_existing(path)?;
    journal.abandon_prepared(attempt_id, now_ms)?;
    // Return the current read-only view, including any different active attempt
    // that acquired the event after the abandonment committed.
    inspect(path, Some(attempt_id), now_ms)?
        .attempts
        .into_iter()
        .next()
        .context("abandoned attempt is missing")
}

fn validate_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':')),
        "invalid application attempt ID"
    );
    Ok(())
}

fn validate_now(now_ms: i64) -> Result<()> {
    ensure!(valid_time(now_ms), "invalid recovery inspection time");
    Ok(())
}

fn valid_time(value: i64) -> bool {
    value >= 0 && value as u64 <= cart::MAX_EXACT_INTEGER
}

fn active(state: &AttemptState) -> bool {
    matches!(
        state,
        AttemptState::Prepared | AttemptState::NeedsReconciliation | AttemptState::Held
    )
}

fn binding(record: &AttemptRecord) -> (&str, Option<&str>, u32) {
    match &record.binding {
        AttemptBinding::QuotedPlan(value) => (
            &value.event_url,
            value.expected_event_start.as_deref(),
            value.quantity,
        ),
        AttemptBinding::NativeSearch(value) => (
            &value.request.event_url,
            Some(&value.request.expected_event_start),
            value.request.quantity,
        ),
    }
}

fn summarize(
    records: &[AttemptRecord],
    selected: Option<&str>,
    now_ms: i64,
) -> Result<RecoveryReport> {
    let mut owners = HashMap::new();
    for record in records.iter().filter(|record| active(&record.state)) {
        let key = event_identity::identity(binding(record).0)
            .context("invalid retained event identity")?;
        ensure!(
            owners.insert(key, record.attempt_id.clone()).is_none(),
            "multiple active attempts share an event"
        );
    }
    let mut attempts = Vec::new();
    for record in records
        .iter()
        .filter(|record| selected.is_none_or(|id| record.attempt_id == id))
    {
        let (event_url, expected_start, quantity) = binding(record);
        let key = event_identity::identity(event_url).context("invalid retained event identity")?;
        let owner = owners.get(&key).cloned();
        let retained_observation = observation(record, now_ms);
        let clock_ok = valid_time(record.created_at_ms)
            && valid_time(record.updated_at_ms)
            && record.created_at_ms <= record.updated_at_ms
            && record.updated_at_ms <= now_ms
            && record.dispatch_at_ms.is_none_or(|dispatch| {
                valid_time(dispatch)
                    && record.created_at_ms <= dispatch
                    && dispatch <= record.updated_at_ms
            })
            && retained_observation.as_ref().is_none_or(|observed| {
                observed.clock == ObservationClock::Valid
                    && record
                        .dispatch_at_ms
                        .is_some_and(|dispatch| dispatch <= observed.observed_at_ms)
                    && observed.observed_at_ms <= record.updated_at_ms
            });
        let expected_dispatch = !matches!(
            record.state,
            AttemptState::Prepared | AttemptState::AbandonedBeforeDispatch
        );
        let state_consistent = expected_dispatch == record.dispatch_at_ms.is_some();
        let expiry = recorded_expiry(record, now_ms, clock_ok && state_consistent);
        let mut actions = vec![RecoveryAction::InspectJournal];
        if record.workflow == AttemptWorkflow::NativeSearch {
            actions.push(RecoveryAction::InspectNativeHistory);
        }
        let reason = if !state_consistent {
            RecoveryReason::JournalStateNeedsReview
        } else if !clock_ok {
            RecoveryReason::JournalClockNeedsReview
        } else {
            match record.state {
                AttemptState::Prepared => {
                    actions.push(RecoveryAction::AbandonPrepared);
                    RecoveryReason::NeverDispatched
                }
                AttemptState::NeedsReconciliation | AttemptState::Held => {
                    actions.push(RecoveryAction::ReconcileOriginalSession);
                    if let Some(expiry) = &expiry {
                        actions.push(RecoveryAction::ReviewRecordedHold);
                        if expiry.journal_expiry_eligible {
                            actions.push(RecoveryAction::ReviewRecordedHoldExpiry);
                            RecoveryReason::RecordedHoldExpiryElapsed
                        } else if record.state == AttemptState::Held {
                            RecoveryReason::RecordedHoldActive
                        } else {
                            RecoveryReason::RecordedHoldNeedsReview
                        }
                    } else {
                        RecoveryReason::UnresolvedDispatch
                    }
                }
                AttemptState::NoHoldConfirmed => {
                    if owner.is_none() {
                        actions.push(RecoveryAction::ReviewFreshIntent);
                    }
                    RecoveryReason::TerminalAbsenceRecorded
                }
                AttemptState::Expired => {
                    if owner.is_none() {
                        actions.push(RecoveryAction::ReviewFreshIntent);
                    }
                    RecoveryReason::RecordedHoldExpired
                }
                AttemptState::AbandonedBeforeDispatch => {
                    if owner.is_none() {
                        actions.push(RecoveryAction::ReviewFreshIntent);
                    }
                    RecoveryReason::AbandonedBeforeDispatch
                }
            }
        };
        attempts.push(AttemptRecovery {
            attempt_id: record.attempt_id.clone(),
            workflow: record.workflow,
            state: record.state.clone(),
            event_url: event_url.into(),
            expected_event_start: expected_start.map(str::to_owned),
            quantity,
            dispatch_recorded: record.dispatch_at_ms.is_some(),
            attempt_retains_event_lock: active(&record.state),
            event_lock_owner_attempt_id: owner,
            reason,
            actions,
            browser_session: BrowserSessionState::Unknown,
            original_session_required_for_reconciliation: record.dispatch_at_ms.is_some()
                && active(&record.state),
            updated_at_ms: record.updated_at_ms,
            retained_observation,
            recorded_hold_expiry: expiry,
            retry_authorized: false,
        });
    }
    ensure!(
        selected.is_none() || !attempts.is_empty(),
        "cart attempt does not exist"
    );
    Ok(RecoveryReport {
        inspected_at_ms: now_ms,
        evidence_basis: "persisted_caller_supplied_evidence",
        current_inventory_verified: false,
        browser_session: BrowserSessionState::Unknown,
        retry_authorized: false,
        attempts,
    })
}

fn observation(record: &AttemptRecord, now_ms: i64) -> Option<RetainedObservation> {
    let (kind, observed_at_ms, uncertainty, review_reasons) = match record.receipt.as_ref()? {
        OutcomeReceipt::Hold(value) => (
            ObservationKind::Hold,
            value.context.observed_at_ms,
            None,
            vec![],
        ),
        OutcomeReceipt::NoHold(value) => (
            ObservationKind::NoHold,
            value.context.observed_at_ms,
            None,
            vec![],
        ),
        OutcomeReceipt::NativeSearch(value) => {
            let (kind, context, uncertainty) = match &value.outcome {
                NativeSearchOutcome::Hold(v) => (ObservationKind::Hold, &v.context, None),
                NativeSearchOutcome::NoHold(v) => (ObservationKind::NoHold, &v.context, None),
                NativeSearchOutcome::Released(v) => (ObservationKind::Released, &v.context, None),
                NativeSearchOutcome::Uncertain(v) => {
                    (ObservationKind::Uncertain, &v.context, Some(v.reason))
                }
            };
            (
                kind,
                context.observed_at_ms,
                uncertainty,
                value.review_reasons.clone(),
            )
        }
    };
    let (clock, age_ms) = if !valid_time(observed_at_ms) {
        (ObservationClock::Invalid, None)
    } else if observed_at_ms > now_ms {
        (ObservationClock::Future, None)
    } else {
        (
            ObservationClock::Valid,
            Some((now_ms - observed_at_ms) as u64),
        )
    };
    Some(RetainedObservation {
        kind,
        observed_at_ms,
        age_ms,
        clock,
        uncertainty,
        review_reasons,
    })
}

fn recorded_expiry(
    record: &AttemptRecord,
    now_ms: i64,
    clock_ok: bool,
) -> Option<RecordedHoldExpiry> {
    let expiry = match record.receipt.as_ref()? {
        OutcomeReceipt::Hold(value)
            if matches!(record.state, AttemptState::Held | AttemptState::Expired) =>
        {
            value.hold_expires_at_ms
        }
        OutcomeReceipt::NativeSearch(value)
            if value.hold_identity_confirmed
                && matches!(
                    record.state,
                    AttemptState::Held | AttemptState::NeedsReconciliation | AttemptState::Expired
                ) =>
        {
            let NativeSearchOutcome::Hold(value) = &value.outcome else {
                return None;
            };
            value.hold_expires_at_ms
        }
        _ => return None,
    };
    if !valid_time(expiry) {
        return None;
    }
    Some(RecordedHoldExpiry {
        expires_at_ms: expiry,
        elapsed: now_ms >= expiry,
        journal_expiry_eligible: clock_ok && active(&record.state) && now_ms >= expiry,
        evidence_basis: "persisted_matching_hold_claims",
    })
}
