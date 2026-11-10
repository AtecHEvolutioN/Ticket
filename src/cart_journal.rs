//! Durable bookkeeping for a future cart adapter; this module performs no requests.
//!
//! Every quote and receipt is **caller-supplied evidence**, not a Ticketmaster
//! verification. Recording a plan never authorizes a reservation. The caller must
//! commit `mark_dispatch_started` or `mark_search_dispatch_started` BEFORE any
//! stock-affecting action, and never repeat it when `newly_started` is false. A dispatched attempt remains
//! unresolved across restarts until an explicit matching outcome is supplied.
//! Application correlation IDs must never contain cookies, authentication tokens,
//! checkout URLs, or private cart identifiers. Performance start claims must
//! match the intended instant, including its UTC offset. This consistency check
//! is not independent date verification or authentication of the native source.

use std::{collections::HashSet, path::Path, time::Duration};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::cart::{self, CartPlanRequest, PlanOutcome, QuoteFees, RankingScope, SeatIdentity};

const APPLICATION_ID: i64 = 0x54434A31; // TCJ1, distinct from the inventory database.
const SCHEMA_VERSION: i64 = 4;
const ACTIVE_INDEX_SQL: &str =
    "CREATE UNIQUE INDEX cart_one_active_event ON cart_attempts(event_key)
  WHERE state IN ('prepared','dispatch_started','needs_reconciliation','held')";
const SCHEMA: &str = "
CREATE TABLE cart_attempts (
  attempt_id TEXT PRIMARY KEY,
  event_key TEXT NOT NULL,
  request_hash TEXT NOT NULL,
  binding_json TEXT NOT NULL,
  state TEXT NOT NULL CHECK(state IN
    ('prepared','dispatch_started','needs_reconciliation','held','no_hold_confirmed','expired','abandoned_before_dispatch')),
  created_at_ms INTEGER NOT NULL,
  updated_at_ms INTEGER NOT NULL,
  dispatch_at_ms INTEGER,
  receipt_json TEXT,
  receipt_hash TEXT,
  workflow TEXT NOT NULL DEFAULT 'quoted_plan' CHECK(workflow IN ('quoted_plan','native_search'))
);
CREATE TABLE cart_transitions (
  sequence INTEGER PRIMARY KEY,
  attempt_id TEXT NOT NULL REFERENCES cart_attempts(attempt_id),
  state TEXT NOT NULL,
  occurred_at_ms INTEGER NOT NULL
);";
const SEARCH_HISTORY_SCHEMA: &str = "
CREATE TABLE cart_search_outcomes (
  sequence INTEGER PRIMARY KEY,
  attempt_id TEXT NOT NULL REFERENCES cart_attempts(attempt_id),
  reconciliation_id TEXT,
  operation_hash TEXT,
  supersedes_outcome_hash TEXT,
  outcome_hash TEXT NOT NULL,
  receipt_json TEXT NOT NULL,
  recorded_at_ms INTEGER NOT NULL,
  UNIQUE(attempt_id,reconciliation_id)
);";

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
    Prepared,
    NeedsReconciliation,
    Held,
    NoHoldConfirmed,
    Expired,
    AbandonedBeforeDispatch,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct PlanBinding {
    pub event_url: String,
    /// Canonical UTC instants. Optional only to preserve historical v1 rows;
    /// new plans and outcomes require both matching, nonempty values.
    #[serde(default)]
    pub expected_event_start: Option<String>,
    #[serde(default)]
    pub source_event_start: Option<String>,
    pub session_id: String,
    pub document_id: String,
    pub quote_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_category_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected_ticket_type: Option<String>,
    #[serde(default, skip_serializing_if = "cart::CartFilters::is_empty")]
    pub filters: cart::CartFilters,
    pub quantity: u32,
    pub max_total_minor: Option<u64>,
    pub total_minor: u64,
    pub currency: String,
    pub seats: Vec<SeatIdentity>,
    pub quote_observed_at_ms: i64,
    pub quote_expires_at_ms: i64,
    pub source_expires_at_ms: i64,
    pub cheapest_for_request: bool,
    pub adjacent_required: bool,
    pub fees_complete: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum AttemptWorkflow {
    QuotedPlan,
    NativeSearch,
}

/// Configuration intent captured before Find tickets, which may itself reserve.
/// None of these fields establishes stock, exact seats, complete fees, adjacency,
/// or the cheapest currently available group.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSearchRequest {
    pub event_url: String,
    pub expected_event_start: String,
    pub source_event_start: String,
    pub quantity: u32,
    pub session_id: String,
    pub document_id: String,
    pub config_id: String,
    pub config_observed_at_ms: i64,
    pub config_expires_at_ms: i64,
    pub category_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_title: Option<String>,
    #[serde(default, skip_serializing_if = "cart::CartFilters::is_empty")]
    pub filters: cart::CartFilters,
    pub price_id: String,
    pub configured_unit_price_minor: u64,
    pub currency: String,
    pub max_total_minor: Option<u64>,
    pub adjacent_required: bool,
    pub cheapest_required: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSearchBinding {
    pub request: NativeSearchRequest,
    pub config_effective_expires_at_ms: i64,
}

/// Untagged serialization preserves the previous quoted binding JSON shape;
/// `AttemptRecord.workflow` explicitly identifies the interpretation.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum AttemptBinding {
    QuotedPlan(PlanBinding),
    NativeSearch(NativeSearchBinding),
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSearchReceiptContext {
    pub attempt_id: String,
    /// Public event identity only. Never a checkout URL or private cart token.
    pub event_url: Option<String>,
    pub event_start: Option<String>,
    pub session_id: String,
    pub source_document_id: String,
    /// Navigation after Find tickets may replace the source document.
    pub outcome_document_id: String,
    pub config_id: String,
    pub observed_at_ms: i64,
    pub alternate_event_or_date_reported: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSearchHoldReceipt {
    pub context: NativeSearchReceiptContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_type: Option<String>,
    pub hold_correlation_id: String,
    pub category_id: String,
    pub price_id: String,
    pub unit_price_minor: u64,
    pub group_id: String,
    pub quantity: u32,
    pub seats: Vec<SeatIdentity>,
    pub seating: cart::SeatingKind,
    pub adjacency: cart::AdjacencyEvidence,
    pub total_minor: u64,
    pub currency: String,
    pub fees: QuoteFees,
    pub hold_expires_at_ms: i64,
    /// Optional complete ranking captured AFTER dispatch in the outcome document.
    /// This must never be represented as a pre-action availability quote.
    pub post_action_ranking_evidence: Option<cart::QuoteSnapshot>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSearchNoHoldReceipt {
    pub context: NativeSearchReceiptContext,
    pub receipt_correlation_id: String,
    pub absence_explicitly_confirmed: bool,
    pub request_finished_without_hold: bool,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum NativeSearchUncertainty {
    Pending,
    Timeout,
    Challenge,
    UnrecognizedResponse,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSearchUncertainReceipt {
    pub context: NativeSearchReceiptContext,
    pub reason: NativeSearchUncertainty,
}

/// Explicit native confirmation that a previously observed hold was released.
/// An empty cart, elapsed timer or navigation alone cannot supply these claims.
/// This is accepted only by reconciliation of that same previously observed hold.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSearchReleasedReceipt {
    pub context: NativeSearchReceiptContext,
    pub hold_correlation_id: String,
    pub release_correlation_id: String,
    pub release_explicitly_confirmed: bool,
    pub absence_explicitly_confirmed: bool,
    pub request_terminal: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "receipt", rename_all = "snake_case")]
pub enum NativeSearchOutcome {
    Hold(Box<NativeSearchHoldReceipt>),
    NoHold(NativeSearchNoHoldReceipt),
    Uncertain(NativeSearchUncertainReceipt),
    Released(NativeSearchReleasedReceipt),
}

/// A compare-and-swap observation, never a request to submit or release tickets.
/// The operation ID binds the complete sanitized input for idempotent replay.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NativeSearchReconciliation {
    pub reconciliation_id: String,
    pub expected_outcome_hash: String,
    pub outcome: NativeSearchOutcome,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct NativeSearchHistoryEntry {
    pub sequence: i64,
    pub reconciliation_id: Option<String>,
    pub supersedes_outcome_hash: Option<String>,
    pub outcome_hash: String,
    pub recorded_at_ms: i64,
    pub receipt: RecordedSearchOutcome,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SearchReviewReason {
    ContextMismatch,
    EventOrDateMismatch,
    InvalidObservationTime,
    QuantityOrSeatsMismatch,
    AdjacencyUnconfirmed,
    PriceOrFilterMismatch,
    HoldExpiryUnconfirmed,
    CheapestRankingUnconfirmed,
    OutcomeUncertain,
    NoHoldNotFinal,
    ReleaseNotFinal,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct RecordedSearchOutcome {
    pub outcome: NativeSearchOutcome,
    pub review_reasons: Vec<SearchReviewReason>,
    /// Only a matching held group may be expired using its recorded hold expiry.
    /// A mismatched/unknown hold cannot release the event on an unverified timer.
    pub hold_identity_confirmed: bool,
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub state: AttemptState,
    pub workflow: AttemptWorkflow,
    pub binding: AttemptBinding,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub dispatch_at_ms: Option<i64>,
    pub receipt: Option<OutcomeReceipt>,
    /// Current native evidence fingerprint for reconciliation compare-and-swap.
    /// This is not a provider token or proof that the evidence is authentic.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub native_outcome_hash: Option<String>,
    pub evidence_basis: &'static str,
    pub reservation_authorized: bool,
    pub source_authenticated: bool,
    pub date_verified: bool,
    /// Matching caller-supplied start claims, not native source authenticity.
    pub event_start_claims_match: bool,
}

impl AttemptRecord {
    pub fn quoted_binding(&self) -> Option<&PlanBinding> {
        match &self.binding {
            AttemptBinding::QuotedPlan(v) => Some(v),
            _ => None,
        }
    }
    pub fn search_binding(&self) -> Option<&NativeSearchBinding> {
        match &self.binding {
            AttemptBinding::NativeSearch(v) => Some(v),
            _ => None,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct DispatchResult {
    pub record: AttemptRecord,
    /// True only for the transaction that first commits the dispatch marker.
    /// This is a concurrency result, not permission to reserve tickets.
    pub newly_started: bool,
}

/// Native-adapter observations to bind a result to the original request.
/// The journal validates consistency only; it does not authenticate the source.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct ReceiptContext {
    pub attempt_id: String,
    pub event_url: String,
    /// Optional solely for reading legacy receipts. New outcomes must provide
    /// the observed performance start, matching the intended exact instant.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub event_start: Option<String>,
    pub session_id: String,
    pub document_id: String,
    pub quote_id: String,
    pub observed_at_ms: i64,
    /// Set by an adapter if the native UI reports a substituted date or event.
    /// False is not independent date verification; see `date_verified` in reports.
    pub alternate_event_or_date_reported: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct HoldReceipt {
    pub context: ReceiptContext,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ticket_type: Option<String>,
    /// An application-generated ID or digest, never a raw private cart token.
    pub hold_correlation_id: String,
    pub quantity: u32,
    pub seats: Vec<SeatIdentity>,
    pub total_minor: u64,
    pub currency: String,
    pub fees: QuoteFees,
    /// Must come from the actual hold receipt, never the quote freshness deadline.
    pub hold_expires_at_ms: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct NoHoldReceipt {
    pub context: ReceiptContext,
    /// An application correlation for explicit native evidence of no held cart.
    pub receipt_correlation_id: String,
    /// A timeout, empty page, or expired quote must never be represented as true.
    pub absence_explicitly_confirmed: bool,
    /// The dispatched request reached a terminal native result. Looking at an
    /// empty cart while a request is still pending is not sufficient evidence.
    pub request_finished_without_hold: bool,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(tag = "kind", content = "receipt", rename_all = "snake_case")]
pub enum OutcomeReceipt {
    Hold(HoldReceipt),
    NoHold(NoHoldReceipt),
    NativeSearch(RecordedSearchOutcome),
}

pub struct CartJournal {
    connection: Connection,
}

struct StoredAttempt {
    record: AttemptRecord,
    raw_state: String,
    request_hash: String,
    receipt_hash: Option<String>,
}

impl CartJournal {
    /// Open a dedicated cart journal. Existing unrelated databases are rejected
    /// before changing journal mode, creating tables, or writing metadata.
    pub fn open(path: &Path) -> Result<Self> {
        ensure!(
            !path.as_os_str().is_empty() && path != Path::new(":memory:"),
            "cart journal must be durable on disk"
        );
        let mut connection = Connection::open_with_flags(
            path,
            OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_CREATE,
        )
        .context("open dedicated cart journal")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        {
            let tx = connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
            let application_id: i64 =
                tx.pragma_query_value(None, "application_id", |r| r.get(0))?;
            let version: i64 = tx.pragma_query_value(None, "user_version", |r| r.get(0))?;
            let table_count: i64 = tx.query_row(
                "SELECT count(*) FROM sqlite_schema WHERE name NOT LIKE 'sqlite_%'",
                [],
                |r| r.get(0),
            )?;
            if application_id == 0 && version == 0 && table_count == 0 {
                tx.execute_batch(SCHEMA)?;
                tx.execute_batch(SEARCH_HISTORY_SCHEMA)?;
                tx.execute_batch(ACTIVE_INDEX_SQL)?;
                tx.pragma_update(None, "application_id", APPLICATION_ID)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            } else {
                let version = validate_schema(&tx)?;
                if version < 3 {
                    // Preserve existing rows, dates, receipts and shared locks.
                    tx.execute_batch("ALTER TABLE cart_attempts ADD COLUMN workflow TEXT NOT NULL DEFAULT 'quoted_plan' CHECK(workflow IN ('quoted_plan','native_search'))")?;
                }
                if version < 4 {
                    tx.execute_batch(SEARCH_HISTORY_SCHEMA)?;
                    // Preserve the last v3 native observation verbatim before
                    // making reconciliation available. Never invent lost history.
                    tx.execute("INSERT INTO cart_search_outcomes(attempt_id,outcome_hash,receipt_json,recorded_at_ms) SELECT attempt_id,receipt_hash,receipt_json,updated_at_ms FROM cart_attempts WHERE workflow='native_search' AND receipt_json IS NOT NULL", [])?;
                    tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
                }
            }
            tx.commit()?;
        }
        // FULL makes the committed dispatch marker durable before caller action.
        connection.pragma_update(None, "journal_mode", "WAL")?;
        connection.pragma_update(None, "synchronous", "FULL")?;
        connection.pragma_update(None, "foreign_keys", true)?;
        Ok(Self { connection })
    }

    /// Atomically bind a stable application attempt ID to a fresh, fully ranked
    /// planner decision. Exact retries return the existing record, even later.
    pub fn begin(
        &mut self,
        attempt_id: &str,
        request: &CartPlanRequest,
        now_ms: i64,
    ) -> Result<AttemptRecord> {
        validate_id(attempt_id)?;
        validate_time(now_ms)?;
        let input = serde_json::to_vec(request)?;
        ensure!(
            input.len() <= cart::MAX_INPUT_BYTES,
            "cart plan exceeds input limit"
        );
        let request_hash = digest(&input);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load(&tx, attempt_id)? {
            ensure!(
                existing.record.workflow == AttemptWorkflow::QuotedPlan,
                "attempt belongs to native search workflow"
            );
            ensure!(
                existing.request_hash == request_hash,
                "attempt ID is bound to another plan"
            );
            tx.commit()?;
            return Ok(existing.record);
        }
        let decision = cart::plan(request, now_ms);
        ensure!(
            decision.outcome == PlanOutcome::Planned
                && decision.ranking_scope == Some(RankingScope::CheapestForRequest),
            "journal requires a fresh planned cheapest adjacent group: {:?}",
            decision.reason
        );
        let selected = decision
            .selected
            .context("planned decision has no selected group")?;
        let (host, event_id) = crate::event_identity::identity(&request.intent.event_url)
            .context("invalid planned event identity")?;
        let event_key = format!("{host}:{event_id}");
        let binding = PlanBinding {
            event_url: format!("https://{host}/event/{event_id}"),
            expected_event_start: Some(
                request
                    .intent
                    .expected_event_start
                    .as_deref()
                    .and_then(cart::normalize_event_start)
                    .context("planned intent has no valid expected start")?,
            ),
            source_event_start: Some(
                selected
                    .event_start
                    .context("planned quote has no valid event start")?,
            ),
            session_id: request.context.session_id.clone(),
            document_id: request.context.document_id.clone(),
            quote_id: selected.group_id,
            selected_category_id: selected.category_id,
            selected_ticket_type: selected.ticket_type,
            filters: request.intent.filters.clone(),
            quantity: selected.quantity,
            max_total_minor: request.intent.max_total_minor,
            total_minor: selected.total_minor,
            currency: selected.currency,
            seats: sorted_seats(selected.seats),
            quote_observed_at_ms: selected.quote_observed_at_ms,
            quote_expires_at_ms: selected.quote_expires_at_ms,
            source_expires_at_ms: request.snapshot.expires_at_ms,
            cheapest_for_request: true,
            adjacent_required: request.intent.require_adjacent,
            fees_complete: true,
        };
        tx.execute(
            "INSERT INTO cart_attempts
             (attempt_id,event_key,request_hash,binding_json,state,created_at_ms,updated_at_ms)
             VALUES (?1,?2,?3,?4,'prepared',?5,?5)",
            params![
                attempt_id,
                event_key,
                request_hash,
                serde_json::to_string(&binding)?,
                now_ms
            ],
        )
        .context(
            "an active cart attempt already exists for this event, or journal insert failed",
        )?;
        transition(&tx, attempt_id, "prepared", now_ms)?;
        let record = require(&tx, attempt_id)?.record;
        tx.commit()?;
        Ok(record)
    }

    /// Persist an intent BEFORE invoking a native Find tickets control. This
    /// shares the event lock with fully quoted plans and does not prove stock.
    pub fn begin_search(
        &mut self,
        attempt_id: &str,
        request: &NativeSearchRequest,
        now_ms: i64,
    ) -> Result<AttemptRecord> {
        validate_id(attempt_id)?;
        validate_time(now_ms)?;
        let request = normalize_search_request(request)?;
        let input = serde_json::to_vec(&request)?;
        ensure!(
            input.len() <= cart::MAX_INPUT_BYTES,
            "native search request exceeds input limit"
        );
        let request_hash = digest(&input);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if let Some(existing) = load(&tx, attempt_id)? {
            ensure!(
                existing.record.workflow == AttemptWorkflow::NativeSearch
                    && existing.request_hash == request_hash,
                "attempt ID is bound to another request or workflow"
            );
            tx.commit()?;
            return Ok(existing.record);
        }
        let expires = request
            .config_expires_at_ms
            .min(request.config_observed_at_ms + cart::MAX_QUOTE_AGE_MS);
        ensure!(
            request.config_observed_at_ms <= now_ms && now_ms < expires,
            "native search configuration is not fresh"
        );
        let (host, event_id) =
            crate::event_identity::identity(&request.event_url).context("invalid native event")?;
        let event_key = format!("{host}:{event_id}");
        let binding = NativeSearchBinding {
            request,
            config_effective_expires_at_ms: expires,
        };
        tx.execute("INSERT INTO cart_attempts(attempt_id,event_key,request_hash,binding_json,state,created_at_ms,updated_at_ms,workflow) VALUES (?1,?2,?3,?4,'prepared',?5,?5,'native_search')",
            params![attempt_id,event_key,request_hash,serde_json::to_string(&binding)?,now_ms])
            .context("an active cart attempt already exists for this event, or native intent insert failed")?;
        transition(&tx, attempt_id, "prepared", now_ms)?;
        let record = require(&tx, attempt_id)?.record;
        tx.commit()?;
        Ok(record)
    }

    /// Persist post-action observations without inventing pre-action quotes.
    /// Unknown or mismatched outcomes retain the event lock and their evidence.
    /// A different subsequent outcome cannot erase an unresolved observed hold.
    pub fn record_search_outcome(
        &mut self,
        attempt_id: &str,
        outcome: &NativeSearchOutcome,
        now_ms: i64,
    ) -> Result<AttemptRecord> {
        validate_time(now_ms)?;
        let mut outcome = outcome.clone();
        sanitize_search_outcome(&mut outcome)?;
        ensure!(
            !matches!(outcome, NativeSearchOutcome::Released(_)),
            "hold release requires reconciliation of a recorded hold"
        );
        let input = serde_json::to_vec(&outcome)?;
        ensure!(
            input.len() <= cart::MAX_INPUT_BYTES,
            "native outcome exceeds input limit"
        );
        let receipt_hash = digest(&input);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = require(&tx, attempt_id)?;
        ensure!(
            stored.record.workflow == AttemptWorkflow::NativeSearch,
            "outcome workflow mismatch"
        );
        if stored.receipt_hash.as_deref() == Some(&receipt_hash) {
            tx.commit()?;
            return Ok(stored.record);
        }
        let initial_replay: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM cart_search_outcomes WHERE attempt_id=?1 AND reconciliation_id IS NULL AND outcome_hash=?2)",
            params![attempt_id,receipt_hash], |r| r.get(0)
        )?;
        if initial_replay {
            tx.commit()?;
            return Ok(stored.record);
        }
        ensure!(
            stored.record.receipt.is_none(),
            "recorded native outcome requires separate reconciliation; it cannot be replaced"
        );
        ensure!(
            matches!(
                stored.raw_state.as_str(),
                "dispatch_started" | "needs_reconciliation"
            ),
            "native outcome is out of order"
        );
        ensure!(
            now_ms >= stored.record.updated_at_ms,
            "transition timestamp goes backwards"
        );
        let recorded = assess_search_outcome(&stored.record, outcome, now_ms)?;
        let state = search_outcome_state(&recorded);
        let receipt = OutcomeReceipt::NativeSearch(recorded);
        let receipt_json = serde_json::to_string(&receipt)?;
        ensure!(
            receipt_json.len() <= cart::MAX_INPUT_BYTES,
            "recorded native outcome exceeds input limit"
        );
        tx.execute("UPDATE cart_attempts SET state=?2,updated_at_ms=?3,receipt_json=?4,receipt_hash=?5 WHERE attempt_id=?1",
            params![attempt_id,state,now_ms,receipt_json,receipt_hash])?;
        tx.execute("INSERT INTO cart_search_outcomes(attempt_id,outcome_hash,receipt_json,recorded_at_ms) VALUES (?1,?2,?3,?4)",params![attempt_id,receipt_hash,receipt_json,now_ms])?;
        transition(&tx, attempt_id, state, now_ms)?;
        let record = require(&tx, attempt_id)?.record;
        tx.commit()?;
        Ok(record)
    }

    /// Reconcile fresh native evidence in the original request lineage without
    /// resubmission. Prior outcomes remain in the append-only history. A known
    /// hold can only be confirmed as the same hold, or explicitly released.
    /// Exact retries return the CURRENT record without rewinding later history.
    pub fn reconcile_search(
        &mut self,
        attempt_id: &str,
        reconciliation: &NativeSearchReconciliation,
        now_ms: i64,
    ) -> Result<AttemptRecord> {
        validate_id(attempt_id)?;
        validate_time(now_ms)?;
        let mut reconciliation = reconciliation.clone();
        validate_id(&reconciliation.reconciliation_id)?;
        ensure!(
            valid_digest(&reconciliation.expected_outcome_hash),
            "invalid expected outcome fingerprint"
        );
        sanitize_search_outcome(&mut reconciliation.outcome)?;
        let input = serde_json::to_vec(&reconciliation)?;
        ensure!(
            input.len() <= cart::MAX_INPUT_BYTES,
            "reconciliation exceeds input limit"
        );
        let operation_hash = digest(&input);
        let outcome_hash = digest(&serde_json::to_vec(&reconciliation.outcome)?);
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = require(&tx, attempt_id)?;
        ensure!(
            stored.record.workflow == AttemptWorkflow::NativeSearch,
            "reconciliation workflow mismatch"
        );
        let prior_operation: Option<String> = tx.query_row(
            "SELECT operation_hash FROM cart_search_outcomes WHERE attempt_id=?1 AND reconciliation_id=?2",
            params![attempt_id,reconciliation.reconciliation_id], |r| r.get(0)
        ).optional()?;
        if let Some(previous_hash) = prior_operation {
            ensure!(
                previous_hash == operation_hash,
                "reconciliation ID is bound to different evidence"
            );
            tx.commit()?;
            return Ok(stored.record);
        }
        ensure!(
            matches!(stored.raw_state.as_str(), "needs_reconciliation" | "held"),
            "reconciliation is out of order"
        );
        ensure!(
            stored.receipt_hash.as_deref() == Some(reconciliation.expected_outcome_hash.as_str()),
            "native outcome changed; inspect the current record before reconciling"
        );
        let previous = match &stored.record.receipt {
            Some(OutcomeReceipt::NativeSearch(v)) => v,
            _ => bail!("reconciliation requires a recorded native outcome"),
        };
        let latest: (String,String) = tx.query_row(
            "SELECT outcome_hash,receipt_json FROM cart_search_outcomes WHERE attempt_id=?1 ORDER BY sequence DESC LIMIT 1",
            [attempt_id], |r| Ok((r.get(0)?,r.get(1)?))
        ).context("native outcome history is missing")?;
        ensure!(
            latest.0 == reconciliation.expected_outcome_hash
                && latest.1 == serde_json::to_string(stored.record.receipt.as_ref().unwrap())?,
            "native outcome history does not match the current record"
        );
        let context = search_context(&reconciliation.outcome);
        validate_time(context.observed_at_ms)?;
        ensure!(
            now_ms >= stored.record.updated_at_ms
                && context.observed_at_ms > stored.record.updated_at_ms
                && (previous
                    .review_reasons
                    .contains(&SearchReviewReason::InvalidObservationTime)
                    || context.observed_at_ms > search_context(&previous.outcome).observed_at_ms)
                && context.observed_at_ms <= now_ms
                && now_ms - context.observed_at_ms < cart::MAX_QUOTE_AGE_MS,
            "reconciliation requires a newer, fresh native observation"
        );
        validate_search_reconciliation(&stored.record, previous, &reconciliation.outcome, now_ms)?;
        let uncertainty_only = matches!(
            (&previous.outcome, &reconciliation.outcome),
            (
                NativeSearchOutcome::Uncertain(_),
                NativeSearchOutcome::Uncertain(_)
            )
        );
        let recorded = assess_search_outcome(&stored.record, reconciliation.outcome, now_ms)?;
        // A recovery observation must come from the bound session. Known event
        // and date claims cannot change. Continuing uncertainty may still lack
        // these claims; it retains its review reasons and cannot unlock the event.
        ensure!(
            !recorded.review_reasons.iter().any(|reason| {
                matches!(
                    reason,
                    SearchReviewReason::ContextMismatch
                        | SearchReviewReason::InvalidObservationTime
                        | SearchReviewReason::ReleaseNotFinal
                ) || (*reason == SearchReviewReason::EventOrDateMismatch && !uncertainty_only)
            }),
            "reconciliation evidence is not bound to the original request or terminal release"
        );
        let state = search_outcome_state(&recorded);
        let receipt_json = serde_json::to_string(&OutcomeReceipt::NativeSearch(recorded))?;
        ensure!(
            receipt_json.len() <= cart::MAX_INPUT_BYTES,
            "recorded native outcome exceeds input limit"
        );
        tx.execute("INSERT INTO cart_search_outcomes(attempt_id,reconciliation_id,operation_hash,supersedes_outcome_hash,outcome_hash,receipt_json,recorded_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?7)",
            params![attempt_id,reconciliation.reconciliation_id,operation_hash,reconciliation.expected_outcome_hash,outcome_hash,receipt_json,now_ms])?;
        tx.execute("UPDATE cart_attempts SET state=?2,updated_at_ms=?3,receipt_json=?4,receipt_hash=?5 WHERE attempt_id=?1",
            params![attempt_id,state,now_ms,receipt_json,outcome_hash])?;
        transition(&tx, attempt_id, state, now_ms)?;
        let record = require(&tx, attempt_id)?.record;
        tx.commit()?;
        Ok(record)
    }

    /// Commit BEFORE the caller touches stock. A crash after this commit is
    /// uncertain even if the caller never got as far as sending its request.
    pub fn mark_dispatch_started(
        &mut self,
        attempt_id: &str,
        now_ms: i64,
    ) -> Result<DispatchResult> {
        self.dispatch(attempt_id, now_ms, AttemptWorkflow::QuotedPlan)
    }

    pub fn mark_search_dispatch_started(
        &mut self,
        attempt_id: &str,
        now_ms: i64,
    ) -> Result<DispatchResult> {
        self.dispatch(attempt_id, now_ms, AttemptWorkflow::NativeSearch)
    }

    fn dispatch(
        &mut self,
        attempt_id: &str,
        now_ms: i64,
        workflow: AttemptWorkflow,
    ) -> Result<DispatchResult> {
        validate_time(now_ms)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = require(&tx, attempt_id)?;
        ensure!(
            stored.record.workflow == workflow,
            "dispatch workflow mismatch"
        );
        if stored.raw_state != "prepared" {
            ensure!(
                stored.record.dispatch_at_ms.is_some(),
                "attempt cannot be dispatched in this state"
            );
            tx.commit()?;
            return Ok(DispatchResult {
                record: stored.record,
                newly_started: false,
            });
        }
        ensure!(
            stored.record.event_start_claims_match,
            "legacy or mismatched event-start binding cannot be dispatched; abandon the unsent plan"
        );
        ensure!(
            now_ms >= stored.record.updated_at_ms,
            "transition timestamp goes backwards"
        );
        let (observed, expires) = match &stored.record.binding {
            AttemptBinding::QuotedPlan(v) => (v.quote_observed_at_ms, v.quote_expires_at_ms),
            AttemptBinding::NativeSearch(v) => (
                v.request.config_observed_at_ms,
                v.config_effective_expires_at_ms,
            ),
        };
        ensure!(
            now_ms >= observed && now_ms < expires,
            "source evidence expired before dispatch; fresh preparation is required"
        );
        tx.execute("UPDATE cart_attempts SET state='dispatch_started',dispatch_at_ms=?2,updated_at_ms=?2 WHERE attempt_id=?1",params![attempt_id,now_ms])?;
        transition(&tx, attempt_id, "dispatch_started", now_ms)?;
        let record = require(&tx, attempt_id)?.record;
        tx.commit()?;
        Ok(DispatchResult {
            record,
            newly_started: true,
        })
    }

    /// Discard an unsent plan, including one whose quote has expired. This is
    /// forbidden after dispatch, even if a caller believes its request failed.
    pub fn abandon_prepared(&mut self, attempt_id: &str, now_ms: i64) -> Result<AttemptRecord> {
        validate_time(now_ms)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = require(&tx, attempt_id)?;
        if stored.raw_state == "abandoned_before_dispatch" {
            tx.commit()?;
            return Ok(stored.record);
        }
        ensure!(
            stored.raw_state == "prepared",
            "only an unsent plan can be abandoned"
        );
        ensure!(
            now_ms >= stored.record.updated_at_ms,
            "transition timestamp goes backwards"
        );
        tx.execute("UPDATE cart_attempts SET state='abandoned_before_dispatch',updated_at_ms=?2 WHERE attempt_id=?1",params![attempt_id,now_ms])?;
        transition(&tx, attempt_id, "abandoned_before_dispatch", now_ms)?;
        let record = require(&tx, attempt_id)?.record;
        tx.commit()?;
        Ok(record)
    }

    /// Record an uncertain native outcome. This never frees the event for retry.
    pub fn mark_uncertain(&mut self, attempt_id: &str, now_ms: i64) -> Result<AttemptRecord> {
        validate_time(now_ms)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = require(&tx, attempt_id)?;
        if stored.raw_state == "needs_reconciliation" {
            tx.commit()?;
            return Ok(stored.record);
        }
        ensure!(
            stored.raw_state == "dispatch_started",
            "only a dispatched attempt can become uncertain"
        );
        ensure!(
            now_ms >= stored.record.updated_at_ms,
            "transition timestamp goes backwards"
        );
        tx.execute("UPDATE cart_attempts SET state='needs_reconciliation',updated_at_ms=?2 WHERE attempt_id=?1",params![attempt_id,now_ms])?;
        transition(&tx, attempt_id, "needs_reconciliation", now_ms)?;
        let record = require(&tx, attempt_id)?.record;
        tx.commit()?;
        Ok(record)
    }

    pub fn record_hold(
        &mut self,
        attempt_id: &str,
        receipt: &HoldReceipt,
        now_ms: i64,
    ) -> Result<AttemptRecord> {
        self.record_outcome(attempt_id, OutcomeReceipt::Hold(receipt.clone()), now_ms)
    }

    pub fn record_no_hold(
        &mut self,
        attempt_id: &str,
        receipt: &NoHoldReceipt,
        now_ms: i64,
    ) -> Result<AttemptRecord> {
        self.record_outcome(attempt_id, OutcomeReceipt::NoHold(receipt.clone()), now_ms)
    }

    fn record_outcome(
        &mut self,
        attempt_id: &str,
        mut receipt: OutcomeReceipt,
        now_ms: i64,
    ) -> Result<AttemptRecord> {
        validate_time(now_ms)?;
        // Sorting is only for exact-set comparison, never adjacency inference.
        if let OutcomeReceipt::Hold(hold) = &mut receipt {
            hold.seats = sorted_seats(std::mem::take(&mut hold.seats));
        }
        let context = match &mut receipt {
            OutcomeReceipt::Hold(v) => &mut v.context,
            OutcomeReceipt::NoHold(v) => &mut v.context,
            OutcomeReceipt::NativeSearch(_) => bail!("use record_search_outcome"),
        };
        let (host, event_id) = crate::event_identity::identity(&context.event_url)
            .context("invalid receipt public event URL")?;
        context.event_url = format!("https://{host}/event/{event_id}");
        context.event_start = Some(
            context
                .event_start
                .as_deref()
                .and_then(cart::normalize_event_start)
                .context("receipt requires a valid offset-qualified event start")?,
        );
        let serialized = serde_json::to_string(&receipt)?;
        ensure!(
            serialized.len() <= cart::MAX_INPUT_BYTES,
            "receipt exceeds input limit"
        );
        let receipt_hash = digest(serialized.as_bytes());
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = require(&tx, attempt_id)?;
        ensure!(
            stored.record.workflow == AttemptWorkflow::QuotedPlan,
            "outcome workflow mismatch"
        );
        if stored.receipt_hash.as_deref() == Some(&receipt_hash) {
            tx.commit()?;
            return Ok(stored.record);
        }
        ensure!(
            matches!(
                stored.raw_state.as_str(),
                "dispatch_started" | "needs_reconciliation"
            ),
            "outcome is conflicting or out of order"
        );
        ensure!(
            now_ms >= stored.record.updated_at_ms,
            "transition timestamp goes backwards"
        );
        validate_receipt(&stored.record, &receipt, now_ms)?;
        let state = match receipt {
            OutcomeReceipt::Hold(_) => "held",
            OutcomeReceipt::NoHold(_) => "no_hold_confirmed",
            OutcomeReceipt::NativeSearch(_) => bail!("use record_search_outcome"),
        };
        tx.execute("UPDATE cart_attempts SET state=?2,updated_at_ms=?3,receipt_json=?4,receipt_hash=?5 WHERE attempt_id=?1",params![attempt_id,state,now_ms,serialized,receipt_hash])?;
        transition(&tx, attempt_id, state, now_ms)?;
        let record = require(&tx, attempt_id)?.record;
        tx.commit()?;
        Ok(record)
    }

    /// Only a previously confirmed hold timestamp permits expiry. Quote age,
    /// elapsed time after dispatch, and timeouts cannot clear an uncertain cart.
    pub fn expire_confirmed_hold(
        &mut self,
        attempt_id: &str,
        now_ms: i64,
    ) -> Result<AttemptRecord> {
        validate_time(now_ms)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = require(&tx, attempt_id)?;
        if stored.raw_state == "expired" {
            tx.commit()?;
            return Ok(stored.record);
        }
        let expiry = match &stored.record.receipt {
            Some(OutcomeReceipt::Hold(receipt)) if stored.raw_state == "held" => {
                receipt.hold_expires_at_ms
            }
            Some(OutcomeReceipt::NativeSearch(recorded))
                if matches!(stored.raw_state.as_str(), "held" | "needs_reconciliation")
                    && recorded.hold_identity_confirmed =>
            {
                let NativeSearchOutcome::Hold(receipt) = &recorded.outcome else {
                    bail!("confirmed hold missing")
                };
                receipt.hold_expires_at_ms
            }
            _ => bail!("only a matching confirmed hold can expire"),
        };
        ensure!(
            now_ms >= stored.record.updated_at_ms && now_ms >= expiry,
            "confirmed hold has not expired"
        );
        tx.execute(
            "UPDATE cart_attempts SET state='expired',updated_at_ms=?2 WHERE attempt_id=?1",
            params![attempt_id, now_ms],
        )?;
        transition(&tx, attempt_id, "expired", now_ms)?;
        let record = require(&tx, attempt_id)?.record;
        tx.commit()?;
        Ok(record)
    }
}

/// Inspect without creating or migrating a database or changing attempt states.
/// A dispatched attempt is always reported as needing reconciliation, including
/// after a restart; inspection never tells a worker to retry a stock action.
pub fn inspect_file(path: &Path) -> Result<Vec<AttemptRecord>> {
    ensure!(
        path.is_file(),
        "cart journal does not exist or is not a file"
    );
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let tx = connection.transaction()?;
    validate_schema(&tx)?;
    let records = {
        let mut statement =
            tx.prepare("SELECT attempt_id FROM cart_attempts ORDER BY created_at_ms,attempt_id")?;
        let ids = statement
            .query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids.iter()
            .map(|id| Ok(require(&tx, id)?.record))
            .collect::<Result<Vec<_>>>()?
    };
    tx.commit()?;
    Ok(records)
}

/// Read all retained native observations without creating or migrating a file.
/// A legacy native receipt is exposed as a single snapshot; earlier evidence
/// predating schema 4 was not retained and is never reconstructed here.
pub fn inspect_search_history(
    path: &Path,
    attempt_id: &str,
) -> Result<Vec<NativeSearchHistoryEntry>> {
    validate_id(attempt_id)?;
    ensure!(
        path.is_file(),
        "cart journal does not exist or is not a file"
    );
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let tx = connection.transaction()?;
    let version = validate_schema(&tx)?;
    let stored = require(&tx, attempt_id)?;
    ensure!(
        stored.record.workflow == AttemptWorkflow::NativeSearch,
        "history requires a native search attempt"
    );
    let records = if version < 4 {
        match stored.record.receipt {
            Some(OutcomeReceipt::NativeSearch(receipt)) => vec![NativeSearchHistoryEntry {
                sequence: 0,
                reconciliation_id: None,
                supersedes_outcome_hash: None,
                outcome_hash: stored
                    .receipt_hash
                    .context("legacy native fingerprint missing")?,
                recorded_at_ms: stored.record.updated_at_ms,
                receipt,
            }],
            None => vec![],
            _ => bail!("native history has an incompatible receipt"),
        }
    } else {
        let mut statement = tx.prepare("SELECT sequence,reconciliation_id,supersedes_outcome_hash,outcome_hash,recorded_at_ms,receipt_json FROM cart_search_outcomes WHERE attempt_id=?1 ORDER BY sequence")?;
        let rows = statement.query_map([attempt_id], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, Option<String>>(1)?,
                r.get::<_, Option<String>>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, String>(5)?,
            ))
        })?;
        rows.map(|row| {
            let (
                sequence,
                reconciliation_id,
                supersedes_outcome_hash,
                outcome_hash,
                recorded_at_ms,
                json,
            ) = row?;
            ensure!(
                json.len() <= cart::MAX_INPUT_BYTES,
                "native history record exceeds input limit"
            );
            let OutcomeReceipt::NativeSearch(receipt) = serde_json::from_str(&json)? else {
                bail!("native history has an incompatible receipt")
            };
            Ok(NativeSearchHistoryEntry {
                sequence,
                reconciliation_id,
                supersedes_outcome_hash,
                outcome_hash,
                recorded_at_ms,
                receipt,
            })
        })
        .collect::<Result<Vec<_>>>()?
    };
    tx.commit()?;
    Ok(records)
}

fn validate_schema(connection: &Connection) -> Result<i64> {
    let application_id: i64 =
        connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(
        application_id == APPLICATION_ID && (1..=SCHEMA_VERSION).contains(&version),
        "not a supported dedicated cart journal"
    );
    if version >= 3 {
        connection.prepare("SELECT workflow FROM cart_attempts LIMIT 0")?;
    }
    if version >= 4 {
        connection.prepare("SELECT sequence,attempt_id,reconciliation_id,operation_hash,supersedes_outcome_hash,outcome_hash,receipt_json,recorded_at_ms FROM cart_search_outcomes LIMIT 0")?;
    }
    // Preparing these queries also rejects damaged/incomplete journal schemas.
    connection.prepare("SELECT attempt_id,event_key,request_hash,binding_json,state,created_at_ms,updated_at_ms,dispatch_at_ms,receipt_json,receipt_hash FROM cart_attempts LIMIT 0")?;
    connection
        .prepare("SELECT sequence,attempt_id,state,occurred_at_ms FROM cart_transitions LIMIT 0")?;
    let index_sql: Option<String> = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type='index' AND name='cart_one_active_event'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    // This module emitted the definition verbatim. Do not lowercase or strip
    // whitespace inside SQL literals: 'PREPARED' is not the stored 'prepared'.
    ensure!(
        index_sql.as_deref() == Some(ACTIVE_INDEX_SQL),
        "cart journal active-attempt constraint missing or altered"
    );
    Ok(version)
}

fn load(connection: &Connection, attempt_id: &str) -> Result<Option<StoredAttempt>> {
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    let workflow_column = if version >= 3 {
        "workflow"
    } else {
        "'quoted_plan'"
    };
    let sql = format!(
        "SELECT request_hash,binding_json,state,created_at_ms,updated_at_ms,dispatch_at_ms,receipt_json,receipt_hash,{workflow_column} FROM cart_attempts WHERE attempt_id=?1"
    );
    let row = connection
        .query_row(&sql, [attempt_id], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, i64>(3)?,
                r.get::<_, i64>(4)?,
                r.get::<_, Option<i64>>(5)?,
                r.get::<_, Option<String>>(6)?,
                r.get::<_, Option<String>>(7)?,
                r.get::<_, String>(8)?,
            ))
        })
        .optional()?;
    let Some((
        request_hash,
        binding_json,
        raw_state,
        created_at_ms,
        updated_at_ms,
        dispatch_at_ms,
        receipt_json,
        receipt_hash,
        workflow,
    )) = row
    else {
        return Ok(None);
    };
    ensure!(
        binding_json.len() <= cart::MAX_INPUT_BYTES
            && receipt_json
                .as_ref()
                .is_none_or(|v| v.len() <= cart::MAX_INPUT_BYTES),
        "journal record exceeds size limit"
    );
    let state = match raw_state.as_str() {
        "prepared" => AttemptState::Prepared,
        "dispatch_started" | "needs_reconciliation" => AttemptState::NeedsReconciliation,
        "held" => AttemptState::Held,
        "no_hold_confirmed" => AttemptState::NoHoldConfirmed,
        "expired" => AttemptState::Expired,
        "abandoned_before_dispatch" => AttemptState::AbandonedBeforeDispatch,
        _ => bail!("unknown cart journal state"),
    };
    let (workflow, binding) = match workflow.as_str() {
        "quoted_plan" => (
            AttemptWorkflow::QuotedPlan,
            AttemptBinding::QuotedPlan(serde_json::from_str(&binding_json)?),
        ),
        "native_search" => (
            AttemptWorkflow::NativeSearch,
            AttemptBinding::NativeSearch(serde_json::from_str(&binding_json)?),
        ),
        _ => bail!("unknown cart attempt workflow"),
    };
    let receipt: Option<OutcomeReceipt> = receipt_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let event_start_claims_match = match &binding {
        AttemptBinding::QuotedPlan(v) => matching_start_claims(v, receipt.as_ref()),
        AttemptBinding::NativeSearch(v) => matching_search_start_claims(v, receipt.as_ref()),
    };
    Ok(Some(StoredAttempt {
        record: AttemptRecord {
            attempt_id: attempt_id.to_owned(),
            state,
            workflow,
            binding,
            created_at_ms,
            updated_at_ms,
            dispatch_at_ms,
            native_outcome_hash: if workflow == AttemptWorkflow::NativeSearch {
                receipt_hash.clone()
            } else {
                None
            },
            receipt,
            evidence_basis: "caller_supplied_evidence",
            reservation_authorized: false,
            source_authenticated: false,
            date_verified: false,
            event_start_claims_match,
        },
        raw_state,
        request_hash,
        receipt_hash,
    }))
}

fn require(connection: &Connection, attempt_id: &str) -> Result<StoredAttempt> {
    validate_id(attempt_id)?;
    load(connection, attempt_id)?.context("unknown cart attempt")
}

fn transition(connection: &Connection, attempt_id: &str, state: &str, now_ms: i64) -> Result<()> {
    connection.execute(
        "INSERT INTO cart_transitions(attempt_id,state,occurred_at_ms) VALUES (?1,?2,?3)",
        params![attempt_id, state, now_ms],
    )?;
    Ok(())
}

fn validate_receipt(record: &AttemptRecord, receipt: &OutcomeReceipt, now_ms: i64) -> Result<()> {
    let context = match receipt {
        OutcomeReceipt::Hold(v) => &v.context,
        OutcomeReceipt::NoHold(v) => &v.context,
        OutcomeReceipt::NativeSearch(_) => bail!("quoted receipt required"),
    };
    let binding = record.quoted_binding().context("quoted binding required")?;
    ensure!(
        matching_start_claims(binding, Some(receipt)),
        "receipt performance start does not match a complete intended event-start binding"
    );
    ensure!(
        context.attempt_id == record.attempt_id,
        "receipt attempt mismatch"
    );
    validate_time(context.observed_at_ms)?;
    ensure!(
        !context.alternate_event_or_date_reported,
        "native receipt reports an alternate event or date"
    );
    ensure!(
        crate::event_identity::same_event(&binding.event_url, &context.event_url),
        "receipt event mismatch"
    );
    ensure!(
        context.session_id == binding.session_id
            && context.document_id == binding.document_id
            && context.quote_id == binding.quote_id,
        "receipt session, document, or quote mismatch"
    );
    let dispatch_at_ms = record
        .dispatch_at_ms
        .context("attempt has not been dispatched")?;
    ensure!(
        context.observed_at_ms >= dispatch_at_ms && context.observed_at_ms <= now_ms,
        "receipt observation is before dispatch or in the future"
    );
    match receipt {
        OutcomeReceipt::Hold(hold) => {
            validate_id(&hold.hold_correlation_id)?;
            validate_time(hold.hold_expires_at_ms)?;
            ensure!(
                hold.category_id
                    .as_ref()
                    .is_none_or(|v| validate_id(v).is_ok())
                    && hold.ticket_type.as_ref().is_none_or(|v| safe_seat_label(v)),
                "invalid hold category/type evidence"
            );
            ensure!(
                binding.filters.allowed_category_ids.is_empty()
                    || (hold.category_id.is_some()
                        && hold.category_id == binding.selected_category_id
                        && hold
                            .category_id
                            .as_ref()
                            .is_some_and(|v| binding.filters.allowed_category_ids.contains(v))),
                "hold category does not satisfy bound preferences"
            );
            ensure!(
                binding.filters.allowed_ticket_types.is_empty()
                    || hold
                        .ticket_type
                        .as_ref()
                        .is_some_and(|v| cart::label_allowed(
                            &binding.filters.allowed_ticket_types,
                            v
                        ) && binding
                            .selected_ticket_type
                            .as_ref()
                            .is_some_and(|expected| cart::normalized_label(expected)
                                == cart::normalized_label(v))),
                "hold ticket type does not satisfy bound preferences"
            );
            ensure!(
                hold.quantity == binding.quantity && hold.seats.len() == binding.quantity as usize,
                "receipt quantity mismatch"
            );
            let ids: HashSet<_> = hold.seats.iter().map(|s| &s.id).collect();
            ensure!(
                ids.len() == hold.seats.len() && hold.seats == binding.seats,
                "receipt must contain the exact unique planned seat set"
            );
            ensure!(
                hold.total_minor == binding.total_minor
                    && hold.currency == "CZK"
                    && hold.fees == QuoteFees::Complete,
                "receipt complete CZK total mismatch"
            );
            ensure!(
                hold.hold_expires_at_ms > context.observed_at_ms,
                "hold expiry must follow its observation"
            );
            // Late recovery may observe an already-expired, previously confirmed
            // receipt. Keep it held until the explicit expiry transition occurs.
        }
        OutcomeReceipt::NativeSearch(_) => bail!("quoted receipt required"),
        OutcomeReceipt::NoHold(no_hold) => {
            validate_id(&no_hold.receipt_correlation_id)?;
            ensure!(
                no_hold.absence_explicitly_confirmed && no_hold.request_finished_without_hold,
                "uncertainty or a pending request is not confirmed absence of a hold"
            );
        }
    }
    Ok(())
}

fn matching_start_claims(binding: &PlanBinding, receipt: Option<&OutcomeReceipt>) -> bool {
    let Some(expected) = binding
        .expected_event_start
        .as_deref()
        .and_then(cart::normalize_event_start)
    else {
        return false;
    };
    let source = binding
        .source_event_start
        .as_deref()
        .and_then(cart::normalize_event_start);
    if source.as_ref() != Some(&expected) {
        return false;
    }
    if let Some(receipt) = receipt {
        let context = match receipt {
            OutcomeReceipt::Hold(v) => &v.context,
            OutcomeReceipt::NoHold(v) => &v.context,
            OutcomeReceipt::NativeSearch(_) => return false,
        };
        return context
            .event_start
            .as_deref()
            .and_then(cart::normalize_event_start)
            .as_ref()
            == Some(&expected);
    }
    true
}

fn normalize_search_request(request: &NativeSearchRequest) -> Result<NativeSearchRequest> {
    let mut request = request.clone();
    let (host, id) = crate::event_identity::identity(&request.event_url)
        .context("invalid native search public event URL")?;
    ensure!(
        host == "ticketmaster.cz",
        "native search requires a Czech event"
    );
    request.event_url = format!("https://{host}/event/{id}");
    request.expected_event_start = cart::normalize_event_start(&request.expected_event_start)
        .context("invalid expected native event start")?;
    request.source_event_start = cart::normalize_event_start(&request.source_event_start)
        .context("invalid configured native event start")?;
    ensure!(
        request.expected_event_start == request.source_event_start,
        "native search performance start mismatch"
    );
    for id in [
        &request.session_id,
        &request.document_id,
        &request.config_id,
        &request.category_id,
        &request.price_id,
    ] {
        validate_id(id)?;
    }
    validate_time(request.config_observed_at_ms)?;
    validate_time(request.config_expires_at_ms)?;
    ensure!(
        request.config_expires_at_ms > request.config_observed_at_ms,
        "invalid native configuration expiry"
    );
    ensure!(
        (1..=cart::MAX_QUANTITY).contains(&request.quantity),
        "native search requires an explicit bounded quantity"
    );
    ensure!(
        request.cheapest_required,
        "native search must preserve the cheapest requirement"
    );
    ensure!(
        request.currency == "CZK"
            && request.configured_unit_price_minor <= cart::MAX_EXACT_INTEGER
            && request
                .max_total_minor
                .is_none_or(|v| v <= cart::MAX_EXACT_INTEGER),
        "invalid native price filter"
    );
    ensure!(
        cart::valid_filters(&request.filters)
            && request
                .category_title
                .as_ref()
                .is_none_or(|v| safe_seat_label(v)),
        "invalid native preference filters or category title"
    );
    ensure!(
        request.filters.allowed_category_ids.is_empty()
            || request
                .filters
                .allowed_category_ids
                .contains(&request.category_id),
        "native category does not satisfy preferences"
    );
    ensure!(
        request.filters.allowed_ticket_types.is_empty()
            || request
                .category_title
                .as_ref()
                .is_some_and(|v| cart::label_allowed(&request.filters.allowed_ticket_types, v)),
        "native ticket type is unknown or does not satisfy preferences"
    );
    let subtotal = request
        .configured_unit_price_minor
        .checked_mul(u64::from(request.quantity))
        .context("native configured subtotal overflow")?;
    ensure!(
        subtotal <= cart::MAX_EXACT_INTEGER
            && request.max_total_minor.is_none_or(|cap| subtotal <= cap),
        "configured price filter exceeds request limits"
    );
    Ok(request)
}

fn search_context(outcome: &NativeSearchOutcome) -> &NativeSearchReceiptContext {
    match outcome {
        NativeSearchOutcome::Hold(v) => &v.context,
        NativeSearchOutcome::NoHold(v) => &v.context,
        NativeSearchOutcome::Uncertain(v) => &v.context,
        NativeSearchOutcome::Released(v) => &v.context,
    }
}

fn sanitize_search_outcome(outcome: &mut NativeSearchOutcome) -> Result<()> {
    let context = match outcome {
        NativeSearchOutcome::Hold(v) => &mut v.context,
        NativeSearchOutcome::NoHold(v) => &mut v.context,
        NativeSearchOutcome::Uncertain(v) => &mut v.context,
        NativeSearchOutcome::Released(v) => &mut v.context,
    };
    for id in [
        &context.attempt_id,
        &context.session_id,
        &context.source_document_id,
        &context.outcome_document_id,
        &context.config_id,
    ] {
        validate_id(id)?;
    }
    // Keep only public identities and normalized dates; never save checkout URLs
    // or arbitrary unrecognized response contents in the read-only report.
    context.event_url = context
        .event_url
        .as_deref()
        .and_then(crate::event_identity::identity)
        .map(|(host, id)| format!("https://{host}/event/{id}"));
    context.event_start = context
        .event_start
        .as_deref()
        .and_then(cart::normalize_event_start);
    if let NativeSearchOutcome::Hold(hold) = outcome {
        for id in [
            &hold.hold_correlation_id,
            &hold.category_id,
            &hold.price_id,
            &hold.group_id,
        ] {
            validate_id(id)?;
        }
        ensure!(
            hold.ticket_type.as_ref().is_none_or(|v| safe_seat_label(v)),
            "invalid native ticket type evidence"
        );
        ensure!(
            hold.seats.len() <= cart::MAX_QUANTITY as usize,
            "native hold seat list exceeds limit"
        );
        for seat in &hold.seats {
            validate_id(&seat.id)?;
            ensure!(
                safe_seat_label(&seat.section)
                    && safe_seat_label(&seat.row)
                    && seat.label.as_ref().is_none_or(|v| safe_seat_label(v)),
                "invalid native seat label"
            );
        }
        hold.seats = sorted_seats(std::mem::take(&mut hold.seats));
        if let Some(ranking) = &mut hold.post_action_ranking_evidence {
            // Invalid ranking remains unproved, not a source of private text.
            let safe = crate::event_identity::identity(&ranking.event_url).is_some()
                && validate_id(&ranking.session_id).is_ok()
                && validate_id(&ranking.document_id).is_ok()
                && ranking.groups.len() <= cart::MAX_GROUPS
                && ranking.groups.iter().all(|g| {
                    validate_id(&g.id).is_ok()
                        && g.category_id
                            .as_ref()
                            .is_none_or(|v| validate_id(v).is_ok())
                        && g.ticket_type.as_ref().is_none_or(|v| safe_seat_label(v))
                        && g.seats.len() <= cart::MAX_QUANTITY as usize
                        && g.seats.iter().all(|s| {
                            validate_id(&s.id).is_ok()
                                && safe_seat_label(&s.section)
                                && safe_seat_label(&s.row)
                                && s.label.as_ref().is_none_or(|v| safe_seat_label(v))
                        })
                });
            if !safe {
                hold.post_action_ranking_evidence = None;
            } else {
                let (host, id) = crate::event_identity::identity(&ranking.event_url).unwrap();
                ranking.event_url = format!("https://{host}/event/{id}");
                ranking.event_start = ranking
                    .event_start
                    .as_deref()
                    .and_then(cart::normalize_event_start);
            }
        }
    } else if let NativeSearchOutcome::NoHold(v) = outcome {
        validate_id(&v.receipt_correlation_id)?;
    } else if let NativeSearchOutcome::Released(v) = outcome {
        validate_id(&v.hold_correlation_id)?;
        validate_id(&v.release_correlation_id)?;
    }
    Ok(())
}

fn safe_seat_label(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 128 && !value.chars().any(char::is_control)
}

fn matching_search_start_claims(
    binding: &NativeSearchBinding,
    receipt: Option<&OutcomeReceipt>,
) -> bool {
    let expected = cart::normalize_event_start(&binding.request.expected_event_start);
    if expected.is_none()
        || expected != cart::normalize_event_start(&binding.request.source_event_start)
    {
        return false;
    }
    match receipt {
        None => true,
        Some(OutcomeReceipt::NativeSearch(v)) => {
            search_context(&v.outcome)
                .event_start
                .as_deref()
                .and_then(cart::normalize_event_start)
                == expected
        }
        _ => false,
    }
}

fn assess_search_outcome(
    record: &AttemptRecord,
    outcome: NativeSearchOutcome,
    now_ms: i64,
) -> Result<RecordedSearchOutcome> {
    use SearchReviewReason as R;
    let binding = record
        .search_binding()
        .context("native search binding required")?;
    let request = &binding.request;
    let context = search_context(&outcome);
    let dispatch = record
        .dispatch_at_ms
        .context("native request not dispatched")?;
    let mut reasons = Vec::new();
    if context.attempt_id != record.attempt_id
        || context.session_id != request.session_id
        || context.source_document_id != request.document_id
        || context.config_id != request.config_id
    {
        reasons.push(R::ContextMismatch);
    }
    if context.alternate_event_or_date_reported
        || !context
            .event_url
            .as_deref()
            .is_some_and(|url| crate::event_identity::same_event(&request.event_url, url))
        || context.event_start.as_deref() != Some(request.expected_event_start.as_str())
    {
        reasons.push(R::EventOrDateMismatch);
    }
    if validate_time(context.observed_at_ms).is_err()
        || context.observed_at_ms < dispatch
        || context.observed_at_ms > now_ms
    {
        reasons.push(R::InvalidObservationTime);
    }
    let mut hold_identity_confirmed = false;
    match &outcome {
        NativeSearchOutcome::Hold(hold) => {
            let ids: HashSet<_> = hold.seats.iter().map(|s| &s.id).collect();
            let labels: HashSet<_> = hold
                .seats
                .iter()
                .filter_map(|s| {
                    s.label.as_ref().map(|v| {
                        (
                            cart::normalized_label(&s.section),
                            cart::normalized_label(&s.row),
                            cart::normalized_label(v),
                        )
                    })
                })
                .collect();
            let labeled_count = hold.seats.iter().filter(|s| s.label.is_some()).count();
            let same_row = hold.seats.first().is_some_and(|first| {
                hold.seats.iter().all(|s| {
                    s.section.trim().eq_ignore_ascii_case(first.section.trim())
                        && s.row.trim().eq_ignore_ascii_case(first.row.trim())
                })
            });
            if hold.quantity != request.quantity
                || hold.seats.len() != request.quantity as usize
                || ids.len() != hold.seats.len()
                || labels.len() != labeled_count
                || (request.adjacent_required && !same_row)
            {
                reasons.push(R::QuantityOrSeatsMismatch);
            }
            if hold.seating != cart::SeatingKind::Seated
                || (request.adjacent_required
                    && hold.adjacency != cart::AdjacencyEvidence::SiteConfirmed)
            {
                reasons.push(R::AdjacencyUnconfirmed);
            }
            let subtotal = request
                .configured_unit_price_minor
                .saturating_mul(u64::from(request.quantity));
            if hold.category_id != request.category_id
                || (!request.filters.allowed_category_ids.is_empty()
                    && !request
                        .filters
                        .allowed_category_ids
                        .contains(&hold.category_id))
                || (!request.filters.allowed_ticket_types.is_empty()
                    && !hold.ticket_type.as_ref().is_some_and(|v| {
                        cart::label_allowed(&request.filters.allowed_ticket_types, v)
                            && request.category_title.as_ref().is_some_and(|expected| {
                                cart::normalized_label(expected) == cart::normalized_label(v)
                            })
                    }))
                || hold.seats.iter().any(|seat| {
                    !cart::label_allowed(&request.filters.allowed_sections, &seat.section)
                        || !cart::label_allowed(&request.filters.allowed_rows, &seat.row)
                })
                || hold.price_id != request.price_id
                || hold.unit_price_minor != request.configured_unit_price_minor
                || hold.currency != "CZK"
                || hold.fees != QuoteFees::Complete
                || hold.total_minor > cart::MAX_EXACT_INTEGER
                || hold.total_minor < subtotal
                || request
                    .max_total_minor
                    .is_some_and(|cap| hold.total_minor > cap)
            {
                reasons.push(R::PriceOrFilterMismatch);
            }
            if validate_time(hold.hold_expires_at_ms).is_err()
                || hold.hold_expires_at_ms <= context.observed_at_ms
            {
                reasons.push(R::HoldExpiryUnconfirmed);
            }
            hold_identity_confirmed = reasons.is_empty();
            if !search_ranking_matches(request, hold, dispatch, now_ms) {
                reasons.push(R::CheapestRankingUnconfirmed);
            }
        }
        NativeSearchOutcome::NoHold(receipt) => {
            if !receipt.absence_explicitly_confirmed || !receipt.request_finished_without_hold {
                reasons.push(R::NoHoldNotFinal);
            }
        }
        NativeSearchOutcome::Uncertain(_) => reasons.push(R::OutcomeUncertain),
        NativeSearchOutcome::Released(receipt) => {
            if !receipt.release_explicitly_confirmed
                || !receipt.absence_explicitly_confirmed
                || !receipt.request_terminal
            {
                reasons.push(R::ReleaseNotFinal);
            }
        }
    }
    Ok(RecordedSearchOutcome {
        outcome,
        review_reasons: reasons,
        hold_identity_confirmed,
    })
}

fn search_outcome_state(recorded: &RecordedSearchOutcome) -> &'static str {
    if !recorded.review_reasons.is_empty() {
        return "needs_reconciliation";
    }
    match &recorded.outcome {
        NativeSearchOutcome::Hold(_) => "held",
        NativeSearchOutcome::NoHold(_) | NativeSearchOutcome::Released(_) => "no_hold_confirmed",
        NativeSearchOutcome::Uncertain(_) => "needs_reconciliation",
    }
}

fn valid_digest(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|c| c.is_ascii_digit() || (b'a'..=b'f').contains(&c))
}

fn same_search_lineage(record: &AttemptRecord, context: &NativeSearchReceiptContext) -> bool {
    let Some(binding) = record.search_binding() else {
        return false;
    };
    context.attempt_id == record.attempt_id
        && context.session_id == binding.request.session_id
        && context.source_document_id == binding.request.document_id
        && context.config_id == binding.request.config_id
}

fn validate_search_reconciliation(
    record: &AttemptRecord,
    previous: &RecordedSearchOutcome,
    next: &NativeSearchOutcome,
    now_ms: i64,
) -> Result<()> {
    if let NativeSearchOutcome::Hold(hold) = next {
        ensure!(
            hold.hold_expires_at_ms > now_ms,
            "reconciliation cannot confirm an already expired hold"
        );
    }
    match (&previous.outcome, next) {
        (NativeSearchOutcome::Uncertain(old), NativeSearchOutcome::Uncertain(new)) => {
            // A passive page may still expose no authenticated event/date or
            // hold receipt. Record that continuing uncertainty without inventing
            // missing claims or weakening validation of a hold/terminal absence.
            let binding = record
                .search_binding()
                .context("native request binding missing")?;
            for context in [&old.context, &new.context] {
                ensure!(
                    same_search_lineage(record, context)
                        && !context.alternate_event_or_date_reported
                        && context.event_url.as_ref().is_none_or(|url| {
                            crate::event_identity::same_event(url, &binding.request.event_url)
                        })
                        && context
                            .event_start
                            .as_ref()
                            .is_none_or(|start| start == &binding.request.expected_event_start),
                    "uncertain observation has conflicting request lineage or concrete event/date claims"
                );
            }
            ensure!(
                old.context.event_url.as_ref().is_none_or(|url| new
                    .context
                    .event_url
                    .as_ref()
                    .is_some_and(|next| crate::event_identity::same_event(url, next)))
                    && old.context.event_start.as_ref().is_none_or(|start| new
                        .context
                        .event_start
                        .as_ref()
                        == Some(start)),
                "uncertain observation cannot erase concrete event or date claims"
            );
        }
        (NativeSearchOutcome::Hold(old), NativeSearchOutcome::Hold(new)) => {
            ensure!(
                same_search_lineage(record, &old.context),
                "previous hold came from a conflicting request lineage"
            );
            ensure!(
                old.hold_correlation_id == new.hold_correlation_id
                    && old.group_id == new.group_id
                    && old.category_id == new.category_id
                    && old.price_id == new.price_id
                    && old.unit_price_minor == new.unit_price_minor
                    && old.quantity == new.quantity
                    && old.total_minor == new.total_minor
                    && old.currency == new.currency
                    && old.hold_expires_at_ms == new.hold_expires_at_ms,
                "reconciliation cannot replace or renew an observed hold"
            );
            ensure!(
                !old.context.alternate_event_or_date_reported
                    && old.context.event_url.as_ref().is_none_or(|url| new
                        .context
                        .event_url
                        .as_ref()
                        .is_some_and(|next| crate::event_identity::same_event(url, next)))
                    && old.context.event_start.as_ref().is_none_or(|start| new
                        .context
                        .event_start
                        .as_ref()
                        == Some(start)),
                "reconciliation cannot overwrite a concrete conflicting event or date"
            );
            ensure!(
                old.ticket_type
                    .as_ref()
                    .is_none_or(|label| new.ticket_type.as_ref().is_some_and(|next| {
                        cart::normalized_label(label) == cart::normalized_label(next)
                    }))
                    && (old.seating == cart::SeatingKind::Unknown || old.seating == new.seating)
                    && (matches!(
                        old.adjacency,
                        cart::AdjacencyEvidence::Unknown | cart::AdjacencyEvidence::SeatNumbersOnly
                    ) || old.adjacency == new.adjacency)
                    && (old.fees != QuoteFees::Complete || new.fees == QuoteFees::Complete),
                "reconciliation cannot contradict known hold evidence"
            );
            // Fill missing seat identities/labels, but never drop or substitute
            // an already observed seat. Source IDs still establish no adjacency.
            ensure!(
                old.seats
                    .iter()
                    .all(|seat| new.seats.iter().any(|next| seat.id == next.id
                        && cart::normalized_label(&seat.section)
                            == cart::normalized_label(&next.section)
                        && cart::normalized_label(&seat.row) == cart::normalized_label(&next.row)
                        && seat.label.as_ref().is_none_or(|label| next
                            .label
                            .as_ref()
                            .is_some_and(|value| cart::normalized_label(label)
                                == cart::normalized_label(value))))),
                "reconciliation cannot replace observed seat identities"
            );
        }
        (NativeSearchOutcome::Hold(old), NativeSearchOutcome::Released(released)) => {
            ensure!(
                same_search_lineage(record, &old.context),
                "previous hold came from a conflicting request lineage"
            );
            ensure!(
                released.hold_correlation_id == old.hold_correlation_id,
                "release must identify the previously observed hold"
            );
            ensure!(
                released.release_explicitly_confirmed
                    && released.absence_explicitly_confirmed
                    && released.request_terminal,
                "release requires explicit terminal native confirmation"
            );
        }
        (NativeSearchOutcome::Hold(_), _) => {
            bail!("an observed hold requires matching hold evidence or explicit correlated release")
        }
        (_, NativeSearchOutcome::Released(_)) => {
            bail!("release requires a previously observed hold")
        }
        (NativeSearchOutcome::Released(_), _) => bail!("released outcome is terminal"),
        _ => {}
    }
    Ok(())
}

fn search_ranking_matches(
    request: &NativeSearchRequest,
    hold: &NativeSearchHoldReceipt,
    dispatch: i64,
    now_ms: i64,
) -> bool {
    let Some(snapshot) = &hold.post_action_ranking_evidence else {
        return false;
    };
    if snapshot.observed_at_ms < dispatch {
        return false;
    }
    let plan_request = CartPlanRequest {
        intent: cart::CartIntent {
            event_url: request.event_url.clone(),
            expected_event_start: Some(request.expected_event_start.clone()),
            quantity: Some(request.quantity),
            max_total_minor: request.max_total_minor,
            require_adjacent: request.adjacent_required,
            filters: request.filters.clone(),
        },
        context: cart::BrowserContext {
            session_id: request.session_id.clone(),
            document_id: hold.context.outcome_document_id.clone(),
        },
        snapshot: snapshot.clone(),
    };
    let decision = cart::plan(&plan_request, now_ms);
    if decision.outcome != PlanOutcome::Planned
        || decision.ranking_scope != Some(RankingScope::CheapestForRequest)
    {
        return false;
    }
    decision.selected.is_some_and(|v| {
        v.group_id == hold.group_id
            && v.quantity == hold.quantity
            && v.total_minor == hold.total_minor
            && v.currency == hold.currency
            && (request.filters.allowed_category_ids.is_empty()
                || v.category_id.as_deref() == Some(hold.category_id.as_str()))
            && (request.filters.allowed_ticket_types.is_empty()
                || v.ticket_type
                    .as_ref()
                    .zip(hold.ticket_type.as_ref())
                    .is_some_and(|(a, b)| cart::normalized_label(a) == cart::normalized_label(b)))
            && sorted_seats(v.seats) == hold.seats
    })
}

fn sorted_seats(mut seats: Vec<SeatIdentity>) -> Vec<SeatIdentity> {
    seats.sort_by(|a, b| a.id.cmp(&b.id));
    seats
}
fn validate_id(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 128
            && value
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.' | b':')),
        "invalid application correlation ID"
    );
    Ok(())
}
fn validate_time(value: i64) -> Result<()> {
    ensure!(
        value >= 0 && value as u64 <= cart::MAX_EXACT_INTEGER,
        "invalid timestamp"
    );
    Ok(())
}
fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
