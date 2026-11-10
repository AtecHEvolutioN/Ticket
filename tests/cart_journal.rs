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
        a.binding.expected_event_start.as_deref(),
        Some(CANONICAL_START)
    );
    assert_eq!(
        a.binding.source_event_start.as_deref(),
        Some(CANONICAL_START)
    );
    assert_eq!(a.evidence_basis, "caller_supplied_evidence");
    assert!(
        a.binding.adjacent_required && a.binding.cheapest_for_request && a.binding.fees_complete
    );
    assert_eq!(a.binding.quote_expires_at_ms, NOW + MAX_QUOTE_AGE_MS);
    assert_eq!(a.binding.source_expires_at_ms, NOW + 10000);
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
    assert!(legacy.binding.expected_event_start.is_none());
    let mut journal = CartJournal::open(&path).unwrap();
    assert_eq!(schema_version(&path), 2);
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
    assert_eq!(schema_version(&path), 2);
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
    assert_eq!(schema_version(&path), 2);
}
