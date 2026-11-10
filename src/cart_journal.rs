//! Durable bookkeeping for a future cart adapter; this module performs no requests.
//!
//! Every quote and receipt is **caller-supplied evidence**, not a Ticketmaster
//! verification. Recording a plan never authorizes a reservation. The caller must
//! commit `mark_dispatch_started` BEFORE any stock-affecting action, and must never
//! repeat that action when `newly_started` is false. A dispatched attempt remains
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
const SCHEMA_VERSION: i64 = 2;
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
  receipt_hash TEXT
);
CREATE TABLE cart_transitions (
  sequence INTEGER PRIMARY KEY,
  attempt_id TEXT NOT NULL REFERENCES cart_attempts(attempt_id),
  state TEXT NOT NULL,
  occurred_at_ms INTEGER NOT NULL
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

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct AttemptRecord {
    pub attempt_id: String,
    pub state: AttemptState,
    pub binding: PlanBinding,
    pub created_at_ms: i64,
    pub updated_at_ms: i64,
    pub dispatch_at_ms: Option<i64>,
    pub receipt: Option<OutcomeReceipt>,
    pub evidence_basis: &'static str,
    pub reservation_authorized: bool,
    pub date_verified: bool,
    /// Matching caller-supplied start claims, not native source authenticity.
    pub event_start_claims_match: bool,
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
                tx.execute_batch(ACTIVE_INDEX_SQL)?;
                tx.pragma_update(None, "application_id", APPLICATION_ID)?;
                tx.pragma_update(None, "user_version", SCHEMA_VERSION)?;
            } else {
                let version = validate_schema(&tx)?;
                if version == 1 {
                    // Preserve old rows and active-event locks. Unknown legacy
                    // dates are never fabricated, and dispatch checks below
                    // prohibit acting on an undated prepared plan.
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
            quantity: selected.quantity,
            max_total_minor: request.intent.max_total_minor,
            total_minor: selected.total_minor,
            currency: selected.currency,
            seats: sorted_seats(selected.seats),
            quote_observed_at_ms: selected.quote_observed_at_ms,
            quote_expires_at_ms: selected.quote_expires_at_ms,
            source_expires_at_ms: request.snapshot.expires_at_ms,
            cheapest_for_request: true,
            adjacent_required: true,
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

    /// Commit BEFORE the caller touches stock. A crash after this commit is
    /// uncertain even if the caller never got as far as sending its request.
    pub fn mark_dispatch_started(
        &mut self,
        attempt_id: &str,
        now_ms: i64,
    ) -> Result<DispatchResult> {
        validate_time(now_ms)?;
        let tx = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let stored = require(&tx, attempt_id)?;
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
        ensure!(
            now_ms >= stored.record.binding.quote_observed_at_ms
                && now_ms < stored.record.binding.quote_expires_at_ms,
            "quote expired before dispatch; fresh planning is required"
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
        ensure!(
            stored.raw_state == "held",
            "only a confirmed hold can expire"
        );
        let Some(OutcomeReceipt::Hold(receipt)) = &stored.record.receipt else {
            bail!("confirmed hold receipt missing")
        };
        ensure!(
            now_ms >= stored.record.updated_at_ms && now_ms >= receipt.hold_expires_at_ms,
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

fn validate_schema(connection: &Connection) -> Result<i64> {
    let application_id: i64 =
        connection.pragma_query_value(None, "application_id", |r| r.get(0))?;
    let version: i64 = connection.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(
        application_id == APPLICATION_ID && (1..=SCHEMA_VERSION).contains(&version),
        "not a supported dedicated cart journal"
    );
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
    let row = connection.query_row(
        "SELECT request_hash,binding_json,state,created_at_ms,updated_at_ms,dispatch_at_ms,receipt_json,receipt_hash FROM cart_attempts WHERE attempt_id=?1",
        [attempt_id],
        |r| Ok((r.get::<_,String>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,i64>(3)?,r.get::<_,i64>(4)?,r.get::<_,Option<i64>>(5)?,r.get::<_,Option<String>>(6)?,r.get::<_,Option<String>>(7)?)),
    ).optional()?;
    let Some((
        request_hash,
        binding_json,
        raw_state,
        created_at_ms,
        updated_at_ms,
        dispatch_at_ms,
        receipt_json,
        receipt_hash,
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
    let binding: PlanBinding = serde_json::from_str(&binding_json)?;
    let receipt: Option<OutcomeReceipt> = receipt_json
        .as_deref()
        .map(serde_json::from_str)
        .transpose()?;
    let event_start_claims_match = matching_start_claims(&binding, receipt.as_ref());
    Ok(Some(StoredAttempt {
        record: AttemptRecord {
            attempt_id: attempt_id.to_owned(),
            state,
            binding,
            created_at_ms,
            updated_at_ms,
            dispatch_at_ms,
            receipt,
            evidence_basis: "caller_supplied_evidence",
            reservation_authorized: false,
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
    };
    let binding = &record.binding;
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
