use rusqlite::Connection;
use std::{
    path::Path,
    sync::{Arc, Barrier},
};
use tempfile::tempdir;
use ticket_watch::{cart::*, cart_journal::*};

const NOW: i64 = 1_700_000_000_000;
const START: &str = "2026-11-16T18:00:00+01:00";
const CANONICAL_START: &str = "2026-11-16T17:00:00Z";
const EVENT: &str = "https://www.ticketmaster.cz/event/korn-vstupenky/1472650657";

fn request() -> CartPlanRequest {
    serde_json::from_value(serde_json::json!({
        "intent":{"event_url":EVENT,"expected_event_start":START,"quantity":2,"max_total_minor":1000000},
        "context":{"session_id":"app-session-1","document_id":"document-1"},
        "snapshot":{
            "event_url":EVENT,"event_start":CANONICAL_START,"session_id":"app-session-1","document_id":"document-1",
            "requested_quantity":2,"observed_at_ms":NOW,"expires_at_ms":NOW+10000,
            "coverage":"complete_for_request","groups":[{
                "id":"quote-1","quantity":2,"kind":"seated","availability":"confirmed",
                "evidence":"exact_group","adjacency":"site_confirmed",
                "seats":[
                    {"id":"s-1","section":"A","row":"1","label":"1"},
                    {"id":"s-3","section":"A","row":"1","label":"3"}
                ],
                "price":{"amount_minor":400000,"currency":"CZK","basis":"group_total","fees":"complete"}
            }]
        }
    })).unwrap()
}

fn hold() -> HoldReceipt {
    HoldReceipt {
        context: context(),
        category_id: None,
        ticket_type: None,
        hold_correlation_id: "application-hold-1".into(),
        quantity: 2,
        seats: request().snapshot.groups[0].seats.clone(),
        total_minor: 400000,
        currency: "CZK".into(),
        fees: QuoteFees::Complete,
        hold_expires_at_ms: NOW + 600000,
    }
}
fn context() -> ReceiptContext {
    ReceiptContext {
        attempt_id: "attempt-1".into(),
        event_url: EVENT.into(),
        event_start: Some(START.into()),
        session_id: "app-session-1".into(),
        document_id: "document-1".into(),
        quote_id: "quote-1".into(),
        observed_at_ms: NOW + 200,
        alternate_event_or_date_reported: false,
    }
}
fn no_hold() -> NoHoldReceipt {
    NoHoldReceipt {
        context: context(),
        receipt_correlation_id: "absence-1".into(),
        absence_explicitly_confirmed: true,
        request_finished_without_hold: true,
    }
}
fn dispatched(path: &Path) -> CartJournal {
    let mut journal = CartJournal::open(path).unwrap();
    journal.begin("attempt-1", &request(), NOW).unwrap();
    assert!(
        journal
            .mark_dispatch_started("attempt-1", NOW + 100)
            .unwrap()
            .newly_started
    );
    journal
}
fn transition_count(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row("SELECT count(*) FROM cart_transitions", [], |r| r.get(0))
        .unwrap()
}

#[test]
fn begin_is_bound_idempotent_and_never_authorizes() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    let a = journal.begin("attempt-1", &request(), NOW).unwrap();
    assert_eq!(a.state, AttemptState::Prepared);
    assert!(!a.reservation_authorized);
    assert!(!a.date_verified);
    assert!(a.event_start_claims_match);
    assert_eq!(
        a.quoted_binding().unwrap().expected_event_start.as_deref(),
        Some(CANONICAL_START)
    );
    assert_eq!(
        a.quoted_binding().unwrap().source_event_start.as_deref(),
        Some(CANONICAL_START)
    );
    assert_eq!(a.evidence_basis, "caller_supplied_evidence");
    assert!(
        a.quoted_binding().unwrap().adjacent_required
            && a.quoted_binding().unwrap().cheapest_for_request
            && a.quoted_binding().unwrap().fees_complete
    );
    assert_eq!(
        a.quoted_binding().unwrap().quote_expires_at_ms,
        NOW + MAX_QUOTE_AGE_MS
    );
    assert_eq!(
        a.quoted_binding().unwrap().source_expires_at_ms,
        NOW + 10000
    );
    assert_eq!(
        a,
        journal.begin("attempt-1", &request(), NOW + 20000).unwrap()
    );
    let mut changed = request();
    changed.intent.max_total_minor = None;
    assert!(journal.begin("attempt-1", &changed, NOW).is_err());
    assert!(journal.begin("attempt-2", &request(), NOW).is_err());
    assert_eq!(transition_count(&path), 1);
}

#[test]
fn rejects_stale_partial_unpriced_and_unproven_adjacent_plans() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    assert!(
        journal
            .begin("stale", &request(), NOW + MAX_QUOTE_AGE_MS)
            .is_err()
    );
    for change in 0..6 {
        let mut r = request();
        match change {
            0 => r.intent.quantity = None,
            1 => r.snapshot.coverage = Coverage::Partial,
            2 => r.snapshot.groups[0].adjacency = AdjacencyEvidence::SeatNumbersOnly,
            3 => r.snapshot.groups[0].price.fees = QuoteFees::Unknown,
            4 => r.snapshot.groups[0].price.basis = PriceBasis::PerTicket,
            _ => r.snapshot.groups[0].availability = GroupAvailability::Unknown,
        }
        assert!(journal.begin(&format!("reject-{change}"), &r, NOW).is_err());
    }
    assert!(inspect_file(&path).unwrap().is_empty());
    assert_eq!(transition_count(&path), 0);
}

#[test]
fn restart_never_retries_a_dispatched_or_uncertain_attempt() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    drop(dispatched(&path));
    assert_eq!(
        inspect_file(&path).unwrap()[0].state,
        AttemptState::NeedsReconciliation
    );
    let mut journal = CartJournal::open(&path).unwrap();
    assert!(
        !journal
            .mark_dispatch_started("attempt-1", NOW + 200)
            .unwrap()
            .newly_started
    );
    assert!(journal.begin("attempt-2", &request(), NOW + 200).is_err());
    assert!(
        journal
            .expire_confirmed_hold("attempt-1", NOW + 10000000)
            .is_err()
    );
    assert!(journal.abandon_prepared("attempt-1", NOW + 200).is_err());
    let uncertain = journal.mark_uncertain("attempt-1", NOW + 200).unwrap();
    assert_eq!(
        uncertain,
        journal.mark_uncertain("attempt-1", NOW + 300).unwrap()
    );
    drop(journal);
    assert_eq!(
        inspect_file(&path).unwrap()[0].state,
        AttemptState::NeedsReconciliation
    );
    assert_eq!(transition_count(&path), 3);
}

#[test]
fn expired_unsent_plan_can_be_abandoned_but_not_dispatched() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    journal.begin("attempt-1", &request(), NOW).unwrap();
    assert!(
        journal
            .mark_dispatch_started("attempt-1", NOW + MAX_QUOTE_AGE_MS)
            .is_err()
    );
    let abandoned = journal
        .abandon_prepared("attempt-1", NOW + MAX_QUOTE_AGE_MS)
        .unwrap();
    assert_eq!(abandoned.state, AttemptState::AbandonedBeforeDispatch);
    assert!(
        journal
            .mark_dispatch_started("attempt-1", NOW + MAX_QUOTE_AGE_MS)
            .is_err()
    );
    let mut fresh = request();
    fresh.snapshot.observed_at_ms = NOW + 6000;
    fresh.snapshot.expires_at_ms = NOW + 16000;
    journal.begin("attempt-2", &fresh, NOW + 6000).unwrap();
}

#[test]
fn concurrent_begin_and_dispatch_commit_only_once() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let a = CartJournal::open(&path).unwrap();
    let b = CartJournal::open(&path).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let handles = vec![a, b]
        .into_iter()
        .map(|mut journal| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                journal.begin("same-attempt", &request(), NOW).unwrap();
                barrier.wait();
                journal
                    .mark_dispatch_started("same-attempt", NOW + 100)
                    .unwrap()
                    .newly_started
            })
        })
        .collect::<Vec<_>>();
    let count = handles
        .into_iter()
        .map(|h| usize::from(h.join().unwrap()))
        .sum::<usize>();
    assert_eq!(count, 1);
    assert_eq!(inspect_file(&path).unwrap().len(), 1);
    assert_eq!(transition_count(&path), 2);
}

#[test]
fn concurrent_distinct_attempts_for_same_event_cannot_both_begin() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let a = CartJournal::open(&path).unwrap();
    let b = CartJournal::open(&path).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let handles = vec![a, b]
        .into_iter()
        .enumerate()
        .map(|(i, mut journal)| {
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                journal
                    .begin(&format!("attempt-{i}"), &request(), NOW)
                    .is_ok()
            })
        })
        .collect::<Vec<_>>();
    assert_eq!(
        handles
            .into_iter()
            .map(|h| usize::from(h.join().unwrap()))
            .sum::<usize>(),
        1
    );
    assert_eq!(transition_count(&path), 1);
}

#[test]
fn hold_requires_exact_event_context_seats_quantity_price_and_expiry() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = dispatched(&path);
    for change in 0..16 {
        let mut receipt = hold();
        match change {
            0 => receipt.context.event_url = "https://ticketmaster.cz/event/1472650658".into(),
            1 => receipt.context.session_id = "other-session".into(),
            2 => receipt.context.document_id = "other-document".into(),
            3 => receipt.context.quote_id = "other-quote".into(),
            4 => receipt.quantity = 1,
            5 => receipt.seats[1] = receipt.seats[0].clone(),
            6 => receipt.seats[1].id = "other-seat".into(),
            7 => receipt.total_minor += 1,
            8 => receipt.currency = "EUR".into(),
            9 => receipt.fees = QuoteFees::Partial,
            10 => receipt.hold_correlation_id = "private/url?token=x".into(),
            11 => receipt.hold_expires_at_ms = receipt.context.observed_at_ms,
            12 => receipt.context.alternate_event_or_date_reported = true,
            13 => receipt.context.observed_at_ms = NOW + 99,
            14 => receipt.context.observed_at_ms = NOW + 301,
            _ => receipt.context.attempt_id = "another-attempt".into(),
        }
        assert!(
            journal
                .record_hold("attempt-1", &receipt, NOW + 300)
                .is_err(),
            "case {change}"
        );
    }
    assert_eq!(
        inspect_file(&path).unwrap()[0].state,
        AttemptState::NeedsReconciliation
    );
    assert_eq!(transition_count(&path), 2);
    let held = journal
        .record_hold("attempt-1", &hold(), NOW + 300)
        .unwrap();
    assert_eq!(held.state, AttemptState::Held);
    assert!(!held.reservation_authorized);
    let mut reordered = hold();
    reordered.seats.reverse();
    assert_eq!(
        held,
        journal
            .record_hold("attempt-1", &reordered, NOW + 400)
            .unwrap()
    );
    let mut conflicting = hold();
    conflicting.hold_correlation_id = "other-hold".into();
    assert!(
        journal
            .record_hold("attempt-1", &conflicting, NOW + 400)
            .is_err()
    );
    assert!(
        journal
            .record_no_hold("attempt-1", &no_hold(), NOW + 400)
            .is_err()
    );
    assert_eq!(transition_count(&path), 3);
}

#[test]
fn hold_expiry_is_explicit_and_allows_only_new_fresh_attempts() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = dispatched(&path);
    journal
        .record_hold("attempt-1", &hold(), NOW + 300)
        .unwrap();
    assert!(
        journal
            .expire_confirmed_hold("attempt-1", NOW + 599999)
            .is_err()
    );
    let expired = journal
        .expire_confirmed_hold("attempt-1", NOW + 600000)
        .unwrap();
    assert_eq!(expired.state, AttemptState::Expired);
    assert_eq!(
        expired,
        journal
            .expire_confirmed_hold("attempt-1", NOW + 700000)
            .unwrap()
    );
    assert_eq!(
        expired,
        journal
            .record_hold("attempt-1", &hold(), NOW + 700000)
            .unwrap()
    );
    assert!(
        !journal
            .mark_dispatch_started("attempt-1", NOW + 700000)
            .unwrap()
            .newly_started
    );
    assert!(
        journal
            .begin("attempt-2", &request(), NOW + 600000)
            .is_err()
    );
}

#[test]
fn no_hold_requires_explicit_evidence_and_obeys_order() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    journal.begin("attempt-1", &request(), NOW).unwrap();
    assert!(
        journal
            .record_no_hold("attempt-1", &no_hold(), NOW + 300)
            .is_err()
    );
    journal
        .mark_dispatch_started("attempt-1", NOW + 100)
        .unwrap();
    let mut unknown = no_hold();
    unknown.absence_explicitly_confirmed = false;
    assert!(
        journal
            .record_no_hold("attempt-1", &unknown, NOW + 300)
            .is_err()
    );
    let mut pending = no_hold();
    pending.request_finished_without_hold = false;
    assert!(
        journal
            .record_no_hold("attempt-1", &pending, NOW + 300)
            .is_err()
    );
    let done = journal
        .record_no_hold("attempt-1", &no_hold(), NOW + 300)
        .unwrap();
    assert_eq!(done.state, AttemptState::NoHoldConfirmed);
    assert_eq!(
        done,
        journal
            .record_no_hold("attempt-1", &no_hold(), NOW + 400)
            .unwrap()
    );
    assert!(
        journal
            .record_hold("attempt-1", &hold(), NOW + 400)
            .is_err()
    );
    journal.begin("attempt-2", &request(), NOW + 400).unwrap();
}

#[test]
fn failed_transition_audit_insert_rolls_back_dispatch_marker() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    journal.begin("attempt-1", &request(), NOW).unwrap();
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_dispatch BEFORE INSERT ON cart_transitions WHEN NEW.state='dispatch_started' BEGIN SELECT RAISE(ABORT,'injected audit failure'); END;").unwrap();
    assert!(
        journal
            .mark_dispatch_started("attempt-1", NOW + 100)
            .is_err()
    );
    let saved = inspect_file(&path).unwrap().remove(0);
    assert_eq!(saved.state, AttemptState::Prepared);
    assert!(saved.dispatch_at_ms.is_none());
    assert_eq!(transition_count(&path), 1);
    db.execute_batch("DROP TRIGGER reject_dispatch").unwrap();
    assert!(
        journal
            .mark_dispatch_started("attempt-1", NOW + 100)
            .unwrap()
            .newly_started
    );
}

#[test]
fn inspection_does_not_create_missing_db_and_unrelated_db_is_untouched() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing.sqlite");
    assert!(inspect_file(&missing).is_err());
    assert!(!missing.exists());
    assert!(CartJournal::open(Path::new(":memory:")).is_err());
    assert!(CartJournal::open(Path::new("")).is_err());
    let unrelated = dir.path().join("inventory.sqlite");
    let db = Connection::open(&unrelated).unwrap();
    db.execute_batch(
        "CREATE TABLE inventory (value TEXT);INSERT INTO inventory VALUES ('preserved');",
    )
    .unwrap();
    drop(db);
    let bytes = std::fs::read(&unrelated).unwrap();
    assert!(inspect_file(&unrelated).is_err());
    assert!(CartJournal::open(&unrelated).is_err());
    assert_eq!(bytes, std::fs::read(&unrelated).unwrap());
}

#[test]
fn receipt_cannot_be_reused_for_later_attempt_with_same_quote_and_timestamp() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = dispatched(&path);
    let mut receipt = no_hold();
    receipt.context.observed_at_ms = NOW + 100;
    journal
        .record_no_hold("attempt-1", &receipt, NOW + 100)
        .unwrap();
    journal.begin("attempt-2", &request(), NOW + 100).unwrap();
    journal
        .mark_dispatch_started("attempt-2", NOW + 100)
        .unwrap();
    assert!(
        journal
            .record_no_hold("attempt-2", &receipt, NOW + 100)
            .is_err()
    );
}

#[test]
fn weakened_active_attempt_constraint_is_rejected() {
    for index in [
        "CREATE INDEX cart_one_active_event ON cart_attempts(event_key)",
        "CREATE UNIQUE INDEX cart_one_active_event ON cart_attempts(attempt_id)",
        "CREATE UNIQUE INDEX cart_one_active_event ON cart_attempts(event_key) WHERE state='prepared'",
        "CREATE UNIQUE INDEX cart_one_active_event ON cart_attempts(event_key) WHERE state IN ('PREPARED','DISPATCH_STARTED','NEEDS_RECONCILIATION','HELD')",
        "CREATE UNIQUE INDEX cart_one_active_event ON cart_attempts(event_key) WHERE state IN ('pre pared','dispatch_started','needs_reconciliation','held')",
    ] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cart.sqlite");
        drop(CartJournal::open(&path).unwrap());
        let db = Connection::open(&path).unwrap();
        db.execute_batch("DROP INDEX cart_one_active_event")
            .unwrap();
        db.execute_batch(index).unwrap();
        drop(db);
        assert!(CartJournal::open(&path).is_err());
        assert!(inspect_file(&path).is_err());
    }
}

#[test]
fn journal_rejects_missing_invalid_and_mismatched_start_claims() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    for case in 0..6 {
        let mut input = request();
        match case {
            0 => input.intent.expected_event_start = None,
            1 => input.snapshot.event_start = None,
            2 => input.intent.expected_event_start = Some("2026-11-16".into()),
            3 => input.snapshot.event_start = Some("2026-11-16T18:00:00-00:00".into()),
            4 => input.snapshot.event_start = Some("2026-11-17T17:00:00Z".into()),
            _ => input.snapshot.event_start = Some("2026-11-16T17:00:00.000000001Z".into()),
        }
        assert!(journal.begin(&format!("bad-{case}"), &input, NOW).is_err());
    }
    assert!(inspect_file(&path).unwrap().is_empty());
    assert_eq!(transition_count(&path), 0);
}

#[test]
fn restart_binds_hold_and_no_hold_receipts_to_exact_performance_start() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    drop(dispatched(&path));
    let mut journal = CartJournal::open(&path).unwrap();
    for start in [
        None,
        Some("2026-11-16"),
        Some("2026-11-16T18:00:00-00:00"),
        Some("2026-11-17T17:00:00Z"),
        Some("2026-11-16T18:00:00Z"),
        Some("2026-11-16T17:00:00.000000001Z"),
    ] {
        let mut wrong_hold = hold();
        wrong_hold.context.event_start = start.map(str::to_owned);
        let mut wrong_absence = no_hold();
        wrong_absence.context.event_start = start.map(str::to_owned);
        assert!(
            journal
                .record_hold("attempt-1", &wrong_hold, NOW + 300)
                .is_err()
        );
        assert!(
            journal
                .record_no_hold("attempt-1", &wrong_absence, NOW + 300)
                .is_err()
        );
    }
    assert_eq!(
        inspect_file(&path).unwrap()[0].state,
        AttemptState::NeedsReconciliation
    );
    assert_eq!(transition_count(&path), 2);
    let held = journal
        .record_hold("attempt-1", &hold(), NOW + 300)
        .unwrap();
    let mut equivalent = hold();
    equivalent.context.event_start = Some(CANONICAL_START.into());
    assert_eq!(
        held,
        journal
            .record_hold("attempt-1", &equivalent, NOW + 400)
            .unwrap()
    );
    drop(journal);
    let reopened = inspect_file(&path).unwrap().remove(0);
    assert_eq!(reopened, held);
    assert!(reopened.event_start_claims_match);
    assert!(!reopened.date_verified && !reopened.reservation_authorized);
    let Some(OutcomeReceipt::Hold(receipt)) = reopened.receipt else {
        panic!("hold missing")
    };
    assert_eq!(
        receipt.context.event_start.as_deref(),
        Some(CANONICAL_START)
    );
}

fn make_legacy_v1(path: &Path) {
    use sha2::{Digest, Sha256};
    let mut old_request = request();
    old_request.intent.expected_event_start = None;
    old_request.snapshot.event_start = None;
    let old_hash = format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&old_request).unwrap())
    );
    let db = Connection::open(path).unwrap();
    db.execute("UPDATE cart_attempts SET binding_json=json_remove(binding_json,'$.expected_event_start','$.source_event_start'), receipt_json=json_remove(receipt_json,'$.receipt.context.event_start'),request_hash=?1",[old_hash]).unwrap();
    db.execute_batch("ALTER TABLE cart_attempts DROP COLUMN workflow")
        .unwrap();
    db.pragma_update(None, "user_version", 1).unwrap();
}
fn schema_version(path: &Path) -> i64 {
    Connection::open(path)
        .unwrap()
        .pragma_query_value(None, "user_version", |r| r.get(0))
        .unwrap()
}

#[test]
fn legacy_prepared_plan_is_inspectable_and_abandonable_but_never_dispatchable() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    journal.begin("attempt-1", &request(), NOW).unwrap();
    drop(journal);
    make_legacy_v1(&path);
    let legacy = inspect_file(&path).unwrap().remove(0);
    assert_eq!(schema_version(&path), 1); // Inspection never migrates.
    assert_eq!(legacy.state, AttemptState::Prepared);
    assert!(!legacy.event_start_claims_match);
    assert!(
        legacy
            .quoted_binding()
            .unwrap()
            .expected_event_start
            .is_none()
    );
    let mut journal = CartJournal::open(&path).unwrap();
    assert_eq!(schema_version(&path), 3);
    let mut old_request = request();
    old_request.intent.expected_event_start = None;
    old_request.snapshot.event_start = None;
    assert_eq!(
        legacy,
        journal.begin("attempt-1", &old_request, NOW).unwrap()
    );
    assert!(
        journal
            .mark_dispatch_started("attempt-1", NOW + 100)
            .is_err()
    );
    assert!(journal.begin("attempt-2", &request(), NOW + 100).is_err());
    journal.abandon_prepared("attempt-1", NOW + 100).unwrap();
    assert!(
        journal
            .begin("attempt-2", &request(), NOW + 100)
            .unwrap()
            .event_start_claims_match
    );
}

#[test]
fn legacy_uncertain_attempt_keeps_event_locked_after_migration_and_restart() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    drop(dispatched(&path));
    make_legacy_v1(&path);
    let mut journal = CartJournal::open(&path).unwrap();
    assert_eq!(schema_version(&path), 3);
    assert!(
        !journal
            .mark_dispatch_started("attempt-1", NOW + 200)
            .unwrap()
            .newly_started
    );
    journal.mark_uncertain("attempt-1", NOW + 200).unwrap();
    assert!(
        journal
            .record_hold("attempt-1", &hold(), NOW + 300)
            .is_err()
    );
    assert!(
        journal
            .record_no_hold("attempt-1", &no_hold(), NOW + 300)
            .is_err()
    );
    assert!(journal.abandon_prepared("attempt-1", NOW + 300).is_err());
    assert!(
        journal
            .expire_confirmed_hold("attempt-1", NOW + 1000000)
            .is_err()
    );
    assert!(journal.begin("attempt-2", &request(), NOW + 300).is_err());
    drop(journal);
    let reopened = inspect_file(&path).unwrap().remove(0);
    assert_eq!(reopened.state, AttemptState::NeedsReconciliation);
    assert!(!reopened.event_start_claims_match);
}

#[test]
fn legacy_confirmed_hold_receipt_remains_readable_and_can_only_expire_at_its_deadline() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = dispatched(&path);
    journal
        .record_hold("attempt-1", &hold(), NOW + 300)
        .unwrap();
    drop(journal);
    make_legacy_v1(&path);
    let legacy = inspect_file(&path).unwrap().remove(0);
    let Some(OutcomeReceipt::Hold(receipt)) = legacy.receipt else {
        panic!("hold missing")
    };
    assert!(receipt.context.event_start.is_none());
    assert!(!legacy.event_start_claims_match);
    let mut journal = CartJournal::open(&path).unwrap();
    assert!(
        journal
            .expire_confirmed_hold("attempt-1", NOW + 599999)
            .is_err()
    );
    assert_eq!(
        journal
            .expire_confirmed_hold("attempt-1", NOW + 600000)
            .unwrap()
            .state,
        AttemptState::Expired
    );
    assert_eq!(schema_version(&path), 3);
}

fn search_request() -> NativeSearchRequest {
    NativeSearchRequest {
        event_url: EVENT.into(),
        expected_event_start: START.into(),
        source_event_start: CANONICAL_START.into(),
        quantity: 2,
        session_id: "app-session-1".into(),
        document_id: "document-1".into(),
        config_id: "capture-1".into(),
        config_observed_at_ms: NOW,
        config_expires_at_ms: NOW + 10000,
        category_id: "category-1".into(),
        category_title: None,
        filters: CartFilters::default(),
        price_id: "price-1".into(),
        configured_unit_price_minor: 200000,
        currency: "CZK".into(),
        max_total_minor: Some(1000000),
        adjacent_required: true,
        cheapest_required: true,
    }
}
fn search_context() -> NativeSearchReceiptContext {
    NativeSearchReceiptContext {
        attempt_id: "attempt-1".into(),
        event_url: Some(EVENT.into()),
        event_start: Some(START.into()),
        session_id: "app-session-1".into(),
        source_document_id: "document-1".into(),
        outcome_document_id: "checkout-document-1".into(),
        config_id: "capture-1".into(),
        observed_at_ms: NOW + 200,
        alternate_event_or_date_reported: false,
    }
}
fn search_hold() -> NativeSearchHoldReceipt {
    let mut ranking = request().snapshot;
    ranking.document_id = "checkout-document-1".into();
    ranking.observed_at_ms = NOW + 200;
    ranking.expires_at_ms = NOW + 5200;
    NativeSearchHoldReceipt {
        context: search_context(),
        ticket_type: None,
        hold_correlation_id: "native-hold-1".into(),
        category_id: "category-1".into(),
        price_id: "price-1".into(),
        unit_price_minor: 200000,
        group_id: "quote-1".into(),
        quantity: 2,
        seats: request().snapshot.groups.remove(0).seats,
        seating: SeatingKind::Seated,
        adjacency: AdjacencyEvidence::SiteConfirmed,
        total_minor: 400000,
        currency: "CZK".into(),
        fees: QuoteFees::Complete,
        hold_expires_at_ms: NOW + 600000,
        post_action_ranking_evidence: Some(ranking),
    }
}
fn search_no_hold() -> NativeSearchOutcome {
    NativeSearchOutcome::NoHold(NativeSearchNoHoldReceipt {
        context: search_context(),
        receipt_correlation_id: "native-absence-1".into(),
        absence_explicitly_confirmed: true,
        request_finished_without_hold: true,
    })
}
fn search_dispatched(path: &Path) -> CartJournal {
    let mut journal = CartJournal::open(path).unwrap();
    journal
        .begin_search("attempt-1", &search_request(), NOW)
        .unwrap();
    assert!(
        journal
            .mark_search_dispatch_started("attempt-1", NOW + 100)
            .unwrap()
            .newly_started
    );
    journal
}
fn recorded_search(record: &AttemptRecord) -> &RecordedSearchOutcome {
    let Some(OutcomeReceipt::NativeSearch(receipt)) = &record.receipt else {
        panic!("native outcome missing")
    };
    receipt
}

#[test]
fn native_search_intent_is_configuration_only_and_shares_quoted_event_lock() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    let prepared = journal
        .begin_search("search-1", &search_request(), NOW)
        .unwrap();
    assert_eq!(prepared.workflow, AttemptWorkflow::NativeSearch);
    assert!(prepared.quoted_binding().is_none());
    assert!(prepared.event_start_claims_match);
    assert!(!prepared.reservation_authorized && !prepared.source_authenticated);
    let data = serde_json::to_value(&prepared.binding).unwrap();
    for absent in [
        "seats",
        "total_minor",
        "fees_complete",
        "cheapest_for_request",
        "quote_id",
    ] {
        assert!(data.get(absent).is_none());
    }
    assert_eq!(
        prepared
            .search_binding()
            .unwrap()
            .config_effective_expires_at_ms,
        NOW + MAX_QUOTE_AGE_MS
    );
    assert_eq!(
        prepared,
        journal
            .begin_search("search-1", &search_request(), NOW + 20000)
            .unwrap()
    );
    assert!(journal.begin("quoted-1", &request(), NOW).is_err());
    assert!(journal.begin("search-1", &request(), NOW).is_err());
    assert!(
        journal
            .mark_dispatch_started("search-1", NOW + 100)
            .is_err()
    );
    journal.abandon_prepared("search-1", NOW + 100).unwrap();
    journal.begin("quoted-1", &request(), NOW + 100).unwrap();
    assert!(
        journal
            .begin_search("search-2", &search_request(), NOW + 100)
            .is_err()
    );
    assert!(
        journal
            .mark_search_dispatch_started("quoted-1", NOW + 100)
            .is_err()
    );
}

#[test]
fn native_search_admission_requires_fresh_explicit_intent_without_weakening_user_requirements() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    for case in 0..8 {
        let mut input = search_request();
        match case {
            0 => input.quantity = 0,
            1 => input.quantity = MAX_QUANTITY + 1,
            2 => input.cheapest_required = false,
            3 => input.expected_event_start = "2026-11-16".into(),
            4 => input.source_event_start = "2026-11-17T17:00:00Z".into(),
            5 => input.currency = "EUR".into(),
            6 => input.config_expires_at_ms = NOW,
            _ => input.max_total_minor = Some(399999),
        }
        assert!(journal.begin_search("bad", &input, NOW).is_err());
    }
    assert!(
        journal
            .begin_search("stale", &search_request(), NOW + MAX_QUOTE_AGE_MS)
            .is_err()
    );
    assert!(
        journal
            .begin_search("future", &search_request(), NOW - 1)
            .is_err()
    );
    journal
        .begin_search("expired", &search_request(), NOW)
        .unwrap();
    assert!(
        journal
            .mark_search_dispatch_started("expired", NOW + MAX_QUOTE_AGE_MS)
            .is_err()
    );
    journal
        .abandon_prepared("expired", NOW + MAX_QUOTE_AGE_MS)
        .unwrap();
}

#[test]
fn native_and_quoted_concurrent_attempts_cannot_both_acquire_event() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut a = CartJournal::open(&path).unwrap();
    let mut b = CartJournal::open(&path).unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let a_barrier = Arc::clone(&barrier);
    let a_thread = std::thread::spawn(move || {
        a_barrier.wait();
        a.begin("quoted-1", &request(), NOW).is_ok()
    });
    let b_thread = std::thread::spawn(move || {
        barrier.wait();
        b.begin_search("search-1", &search_request(), NOW).is_ok()
    });
    assert_eq!(
        usize::from(a_thread.join().unwrap()) + usize::from(b_thread.join().unwrap()),
        1
    );
    assert_eq!(inspect_file(&path).unwrap().len(), 1);
}

#[test]
fn native_dispatch_survives_restart_without_repeat_and_accepts_new_outcome_document() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    drop(search_dispatched(&path));
    let inspected = inspect_file(&path).unwrap().remove(0);
    assert_eq!(inspected.state, AttemptState::NeedsReconciliation);
    let mut journal = CartJournal::open(&path).unwrap();
    assert!(
        !journal
            .mark_search_dispatch_started("attempt-1", NOW + 200)
            .unwrap()
            .newly_started
    );
    assert!(
        journal
            .record_hold("attempt-1", &hold(), NOW + 300)
            .is_err()
    );
    let held = journal
        .record_search_outcome(
            "attempt-1",
            &NativeSearchOutcome::Hold(Box::new(search_hold())),
            NOW + 300,
        )
        .unwrap();
    assert_eq!(held.state, AttemptState::Held);
    assert!(recorded_search(&held).hold_identity_confirmed);
    assert!(recorded_search(&held).review_reasons.is_empty());
    assert!(held.search_binding().is_some() && held.quoted_binding().is_none());
    let mut equivalent = search_hold();
    equivalent.context.event_start = Some(CANONICAL_START.into());
    assert_eq!(
        held,
        journal
            .record_search_outcome(
                "attempt-1",
                &NativeSearchOutcome::Hold(Box::new(equivalent)),
                NOW + 400
            )
            .unwrap()
    );
    assert_eq!(inspect_file(&path).unwrap().remove(0), held);
}

#[test]
fn native_matching_hold_without_complete_post_action_ranking_needs_review_but_has_confirmed_expiry()
{
    for case in 0..8 {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cart.sqlite");
        let mut journal = search_dispatched(&path);
        let mut hold = search_hold();
        match case {
            0 => hold.post_action_ranking_evidence = None,
            1 => hold.post_action_ranking_evidence.as_mut().unwrap().coverage = Coverage::Partial,
            2 => {
                hold.post_action_ranking_evidence
                    .as_mut()
                    .unwrap()
                    .document_id = "document-1".into()
            }
            3 => {
                hold.post_action_ranking_evidence
                    .as_mut()
                    .unwrap()
                    .observed_at_ms = NOW + 99
            }
            4 => {
                hold.post_action_ranking_evidence.as_mut().unwrap().groups[0]
                    .price
                    .amount_minor = Some(399999)
            }
            5 => {
                hold.post_action_ranking_evidence.as_mut().unwrap().groups[0].id =
                    "other-group".into()
            }
            6 => {
                hold.post_action_ranking_evidence
                    .as_mut()
                    .unwrap()
                    .event_start = Some("2026-11-17T17:00:00Z".into())
            }
            _ => {
                let ranking = hold.post_action_ranking_evidence.as_mut().unwrap();
                let mut cheaper = ranking.groups[0].clone();
                cheaper.id = "cheaper".into();
                cheaper.price.amount_minor = Some(200000);
                ranking.groups.push(cheaper);
            }
        }
        let reviewed = journal
            .record_search_outcome(
                "attempt-1",
                &NativeSearchOutcome::Hold(Box::new(hold)),
                NOW + 300,
            )
            .unwrap();
        assert_eq!(
            reviewed.state,
            AttemptState::NeedsReconciliation,
            "case {case}"
        );
        assert_eq!(
            recorded_search(&reviewed).review_reasons,
            vec![SearchReviewReason::CheapestRankingUnconfirmed]
        );
        assert!(recorded_search(&reviewed).hold_identity_confirmed);
        assert!(journal.begin("other", &request(), NOW + 300).is_err());
        assert!(
            journal
                .record_search_outcome("attempt-1", &search_no_hold(), NOW + 300)
                .is_err()
        );
        assert!(
            journal
                .expire_confirmed_hold("attempt-1", NOW + 599999)
                .is_err()
        );
        assert_eq!(
            journal
                .expire_confirmed_hold("attempt-1", NOW + 600000)
                .unwrap()
                .state,
            AttemptState::Expired
        );
    }
}

#[test]
fn native_mismatched_holds_remain_locked_and_cannot_use_unverified_expiry() {
    for case in 0..14 {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cart.sqlite");
        let mut journal = search_dispatched(&path);
        let mut hold = search_hold();
        match case {
            0 => hold.context.event_url = Some("https://ticketmaster.cz/event/1472650658".into()),
            1 => hold.context.event_start = Some("2026-11-17T17:00:00Z".into()),
            2 => hold.context.source_document_id = "other-document".into(),
            3 => hold.context.session_id = "other-session".into(),
            4 => hold.quantity = 1,
            5 => hold.seats[1] = hold.seats[0].clone(),
            6 => hold.adjacency = AdjacencyEvidence::SeatNumbersOnly,
            7 => hold.category_id = "other-category".into(),
            8 => hold.price_id = "other-price".into(),
            9 => hold.fees = QuoteFees::Unknown,
            10 => hold.hold_expires_at_ms = NOW + 200,
            11 => hold.context.config_id = "other-capture".into(),
            12 => {
                hold.context.event_url = None;
                hold.context.event_start = None;
            }
            _ => {
                hold.context.event_url = Some(
                    "https://secure.ticketmaster.cz/checkout?token=private-native-hold-token"
                        .into(),
                )
            }
        }
        let reviewed = journal
            .record_search_outcome(
                "attempt-1",
                &NativeSearchOutcome::Hold(Box::new(hold)),
                NOW + 300,
            )
            .unwrap();
        assert_eq!(
            reviewed.state,
            AttemptState::NeedsReconciliation,
            "case {case}"
        );
        assert!(!recorded_search(&reviewed).hold_identity_confirmed);
        assert!(
            !serde_json::to_string(&reviewed)
                .unwrap()
                .contains("private-native-hold-token")
        );
        assert!(
            journal
                .expire_confirmed_hold("attempt-1", NOW + 1000000)
                .is_err()
        );
        assert!(
            journal
                .record_search_outcome("attempt-1", &search_no_hold(), NOW + 300)
                .is_err()
        );
        assert!(
            journal
                .begin_search("another", &search_request(), NOW + 300)
                .is_err()
        );
        drop(journal);
        assert_eq!(
            inspect_file(&path).unwrap().remove(0).state,
            AttemptState::NeedsReconciliation
        );
    }
}

#[test]
fn native_unknown_outcome_stays_locked_and_only_initial_final_absence_releases() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = search_dispatched(&path);
    let mut context = search_context();
    context.event_url = Some("https://secure.ticketmaster.cz/checkout?token=private-value".into());
    context.event_start = None;
    let unknown = NativeSearchOutcome::Uncertain(NativeSearchUncertainReceipt {
        context,
        reason: NativeSearchUncertainty::Timeout,
    });
    let saved = journal
        .record_search_outcome("attempt-1", &unknown, NOW + 300)
        .unwrap();
    assert_eq!(saved.state, AttemptState::NeedsReconciliation);
    assert!(
        !serde_json::to_string(&saved)
            .unwrap()
            .contains("private-value")
    );
    assert!(
        journal
            .record_search_outcome("attempt-1", &search_no_hold(), NOW + 300)
            .is_err()
    );
    assert!(
        journal
            .expire_confirmed_hold("attempt-1", NOW + 1000000)
            .is_err()
    );
    let other_dir = tempdir().unwrap();
    let other_path = other_dir.path().join("cart.sqlite");
    let mut other = search_dispatched(&other_path);
    assert_eq!(
        other
            .record_search_outcome("attempt-1", &search_no_hold(), NOW + 300)
            .unwrap()
            .state,
        AttemptState::NoHoldConfirmed
    );
    other.begin("quoted", &request(), NOW + 300).unwrap();
}

#[test]
fn v2_migration_preserves_quoted_rows_and_blocks_parallel_native_search() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    let before = journal.begin("quoted", &request(), NOW).unwrap();
    drop(journal);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("ALTER TABLE cart_attempts DROP COLUMN workflow")
        .unwrap();
    db.pragma_update(None, "user_version", 2).unwrap();
    drop(db);
    assert_eq!(inspect_file(&path).unwrap().remove(0), before);
    assert_eq!(schema_version(&path), 2);
    let mut migrated = CartJournal::open(&path).unwrap();
    assert_eq!(schema_version(&path), 3);
    assert_eq!(inspect_file(&path).unwrap().remove(0), before);
    assert!(
        migrated
            .begin_search("native", &search_request(), NOW)
            .is_err()
    );
}

#[test]
fn native_outcome_audit_failure_rolls_back_receipt_and_event_unlock() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = search_dispatched(&path);
    let db = Connection::open(&path).unwrap();
    db.execute_batch("CREATE TRIGGER reject_native_absence BEFORE INSERT ON cart_transitions WHEN NEW.state='no_hold_confirmed' BEGIN SELECT RAISE(ABORT,'injected audit failure'); END;").unwrap();
    assert!(
        journal
            .record_search_outcome("attempt-1", &search_no_hold(), NOW + 300)
            .is_err()
    );
    let saved = inspect_file(&path).unwrap().remove(0);
    assert!(saved.receipt.is_none());
    assert_eq!(saved.state, AttemptState::NeedsReconciliation);
    assert!(journal.begin("quoted", &request(), NOW + 300).is_err());
    assert_eq!(transition_count(&path), 2);
    db.execute_batch("DROP TRIGGER reject_native_absence")
        .unwrap();
    assert_eq!(
        journal
            .record_search_outcome("attempt-1", &search_no_hold(), NOW + 300)
            .unwrap()
            .state,
        AttemptState::NoHoldConfirmed
    );
}

#[test]
fn quoted_filter_policy_is_durable_and_requires_matching_receipt_category_and_type() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    let mut input = request();
    input.intent.filters = CartFilters {
        allowed_category_ids: vec!["category-1".into()],
        allowed_ticket_types: vec!["standard".into()],
        allowed_sections: vec!["a".into()],
        allowed_rows: vec!["1".into()],
    };
    input.snapshot.groups[0].category_id = Some("category-1".into());
    input.snapshot.groups[0].ticket_type = Some("Standard".into());
    let prepared = journal.begin("attempt-1", &input, NOW).unwrap();
    assert_eq!(
        prepared.quoted_binding().unwrap().filters,
        input.intent.filters
    );
    journal
        .mark_dispatch_started("attempt-1", NOW + 100)
        .unwrap();
    drop(journal);
    let mut journal = CartJournal::open(&path).unwrap();
    assert!(
        journal
            .record_hold("attempt-1", &hold(), NOW + 300)
            .is_err()
    );
    let mut receipt = hold();
    receipt.category_id = Some("category-1".into());
    receipt.ticket_type = Some("VIP".into());
    assert!(
        journal
            .record_hold("attempt-1", &receipt, NOW + 300)
            .is_err()
    );
    receipt.ticket_type = Some("  STANDARD  ".into());
    assert_eq!(
        journal
            .record_hold("attempt-1", &receipt, NOW + 300)
            .unwrap()
            .state,
        AttemptState::Held
    );
}

fn filtered_native_input() -> NativeSearchRequest {
    let mut input = search_request();
    input.adjacent_required = false;
    input.category_title = Some("Standard".into());
    input.filters = CartFilters {
        allowed_category_ids: vec!["category-1".into()],
        allowed_ticket_types: vec!["standard".into()],
        allowed_sections: vec!["A".into(), "B".into()],
        allowed_rows: vec!["1".into(), "2".into()],
    };
    input
}
fn filtered_native_hold() -> NativeSearchHoldReceipt {
    let mut receipt = search_hold();
    receipt.ticket_type = Some("Standard".into());
    receipt.adjacency = AdjacencyEvidence::NotAdjacent;
    receipt.seats[1].section = "B".into();
    receipt.seats[1].row = "2".into();
    receipt.seats[1].label = Some("1".into());
    let ranking = receipt.post_action_ranking_evidence.as_mut().unwrap();
    ranking.groups[0].category_id = Some("category-1".into());
    ranking.groups[0].ticket_type = Some("Standard".into());
    ranking.groups[0].seats = receipt.seats.clone();
    ranking.groups[0].adjacency = AdjacencyEvidence::NotAdjacent;
    receipt
}

#[test]
fn native_nonadjacent_policy_honors_type_section_and_row_filters_in_hold_and_ranking() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    let input = filtered_native_input();
    let mut missing = input.clone();
    missing.category_title = None;
    assert!(journal.begin_search("missing", &missing, NOW).is_err());
    journal.begin_search("attempt-1", &input, NOW).unwrap();
    journal
        .mark_search_dispatch_started("attempt-1", NOW + 100)
        .unwrap();
    let held = journal
        .record_search_outcome(
            "attempt-1",
            &NativeSearchOutcome::Hold(Box::new(filtered_native_hold())),
            NOW + 300,
        )
        .unwrap();
    assert_eq!(held.state, AttemptState::Held);
    assert!(!held.search_binding().unwrap().request.adjacent_required);
    assert_eq!(
        held.search_binding().unwrap().request.filters,
        input.filters
    );
}

#[test]
fn native_filter_mismatch_unknown_type_or_duplicate_location_remains_locked() {
    for case in 0..5 {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cart.sqlite");
        let mut journal = CartJournal::open(&path).unwrap();
        journal
            .begin_search("attempt-1", &filtered_native_input(), NOW)
            .unwrap();
        journal
            .mark_search_dispatch_started("attempt-1", NOW + 100)
            .unwrap();
        let mut receipt = filtered_native_hold();
        match case {
            0 => receipt.ticket_type = None,
            1 => receipt.ticket_type = Some("VIP".into()),
            2 => receipt.seats[1].section = "C".into(),
            3 => receipt.seats[1].row = "3".into(),
            _ => {
                receipt.seats[1].section = "A".into();
                receipt.seats[1].row = "1".into();
            }
        }
        let reviewed = journal
            .record_search_outcome(
                "attempt-1",
                &NativeSearchOutcome::Hold(Box::new(receipt)),
                NOW + 300,
            )
            .unwrap();
        assert_eq!(
            reviewed.state,
            AttemptState::NeedsReconciliation,
            "case {case}"
        );
        assert!(!recorded_search(&reviewed).hold_identity_confirmed);
        assert!(
            journal
                .expire_confirmed_hold("attempt-1", NOW + 1000000)
                .is_err()
        );
    }
}
