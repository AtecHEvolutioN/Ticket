use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use tempfile::tempdir;
use ticket_watch::{
    cart::{AdjacencyEvidence, CartPlanRequest, QuoteFees, SeatIdentity, SeatingKind},
    cart_journal::*,
    cart_recovery::{
        self, BrowserSessionState, ObservationClock, ObservationKind, RecoveryAction,
        RecoveryReason,
    },
};

const NOW: i64 = 1_700_000_000_000;
const START: &str = "2026-11-16T17:00:00Z";
const EVENT: &str = "https://www.ticketmaster.cz/event/synthetic/1472650657";

fn request() -> NativeSearchRequest {
    NativeSearchRequest {
        event_url: EVENT.into(),
        expected_event_start: START.into(),
        source_event_start: START.into(),
        quantity: 2,
        session_id: "session-1".into(),
        document_id: "document-1".into(),
        config_id: "config-1".into(),
        config_observed_at_ms: NOW,
        config_expires_at_ms: NOW + 5000,
        category_id: "category-1".into(),
        category_title: None,
        filters: Default::default(),
        price_id: "price-1".into(),
        configured_unit_price_minor: 200000,
        currency: "CZK".into(),
        max_total_minor: None,
        adjacent_required: true,
        cheapest_required: true,
    }
}

fn context() -> NativeSearchReceiptContext {
    NativeSearchReceiptContext {
        attempt_id: "attempt-1".into(),
        event_url: Some(EVENT.into()),
        event_start: Some(START.into()),
        session_id: "session-1".into(),
        source_document_id: "document-1".into(),
        outcome_document_id: "outcome-1".into(),
        config_id: "config-1".into(),
        observed_at_ms: NOW + 200,
        alternate_event_or_date_reported: false,
    }
}

fn dispatched(path: &Path) -> CartJournal {
    let mut journal = CartJournal::open(path).unwrap();
    journal.begin_search("attempt-1", &request(), NOW).unwrap();
    journal
        .mark_search_dispatch_started("attempt-1", NOW + 100)
        .unwrap();
    journal
}

fn held() -> NativeSearchHoldReceipt {
    NativeSearchHoldReceipt {
        context: context(),
        ticket_type: None,
        hold_correlation_id: "app-hold-1".into(),
        category_id: "category-1".into(),
        price_id: "price-1".into(),
        unit_price_minor: 200000,
        group_id: "group-1".into(),
        quantity: 2,
        seats: vec![
            SeatIdentity {
                id: "seat-1".into(),
                section: "A".into(),
                row: "1".into(),
                label: Some("1".into()),
            },
            SeatIdentity {
                id: "seat-3".into(),
                section: "A".into(),
                row: "1".into(),
                label: Some("3".into()),
            },
        ],
        seating: SeatingKind::Seated,
        adjacency: AdjacencyEvidence::SiteConfirmed,
        total_minor: 400000,
        currency: "CZK".into(),
        fees: QuoteFees::Complete,
        hold_expires_at_ms: NOW + 600000,
        post_action_ranking_evidence: None,
    }
}

fn version(path: &Path) -> i64 {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .unwrap()
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .unwrap()
}

#[test]
fn missing_empty_unrelated_and_invalid_requests_never_initialize_a_journal() {
    let dir = tempdir().unwrap();
    let missing = dir.path().join("missing-parent/absent.sqlite");
    assert!(cart_recovery::inspect(&missing, None, NOW).is_err());
    assert!(cart_recovery::abandon_prepared(&missing, "attempt-1", NOW).is_err());
    assert!(CartJournal::open_existing(&missing).is_err());
    assert!(!missing.exists());
    assert!(!missing.parent().unwrap().exists());

    let empty = dir.path().join("empty.sqlite");
    std::fs::write(&empty, []).unwrap();
    assert!(CartJournal::open_existing(&empty).is_err());
    assert!(cart_recovery::inspect(&empty, None, NOW).is_err());
    assert!(cart_recovery::abandon_prepared(&empty, "attempt-1", NOW).is_err());
    assert_eq!(std::fs::read(&empty).unwrap(), Vec::<u8>::new());

    let unrelated = dir.path().join("other.sqlite");
    let connection = Connection::open(&unrelated).unwrap();
    connection
        .execute("CREATE TABLE unrelated(value TEXT)", [])
        .unwrap();
    drop(connection);
    let before = std::fs::read(&unrelated).unwrap();
    assert!(CartJournal::open_existing(&unrelated).is_err());
    assert!(cart_recovery::abandon_prepared(&unrelated, "attempt-1", NOW).is_err());
    assert_eq!(std::fs::read(&unrelated).unwrap(), before);

    let invalid = cart_recovery::inspect(&missing, Some("private/token-marker"), NOW).unwrap_err();
    assert!(!format!("{invalid:#}").contains("token-marker"));
    assert!(cart_recovery::inspect(&missing, None, -1).is_err());
    assert!(cart_recovery::inspect(dir.path(), None, NOW).is_err());
}

#[test]
fn inspection_is_read_only_and_abandonment_only_migrates_an_explicit_unsent_attempt() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    journal.begin_search("attempt-1", &request(), NOW).unwrap();
    drop(journal);
    let legacy = Connection::open(&path).unwrap();
    legacy
        .execute("DROP TABLE cart_search_outcomes", [])
        .unwrap();
    legacy.pragma_update(None, "user_version", 3).unwrap();
    drop(legacy);
    let report = cart_recovery::inspect(&path, Some("attempt-1"), NOW + 900000).unwrap();
    let entry = &report.attempts[0];
    assert_eq!(version(&path), 3);
    assert_eq!(entry.reason, RecoveryReason::NeverDispatched);
    assert!(entry.attempt_retains_event_lock);
    assert_eq!(
        entry.event_lock_owner_attempt_id.as_deref(),
        Some("attempt-1")
    );
    assert_eq!(entry.browser_session, BrowserSessionState::Unknown);
    assert!(entry.actions.contains(&RecoveryAction::AbandonPrepared));
    assert!(!entry.retry_authorized && !report.current_inventory_verified);
    assert!(entry.retained_observation.is_none());
    assert!(cart_recovery::inspect(&path, Some("absent"), NOW).is_err());
    assert_eq!(version(&path), 3);

    let abandoned = cart_recovery::abandon_prepared(&path, "attempt-1", NOW + 900000).unwrap();
    assert_eq!(version(&path), 4);
    assert_eq!(abandoned.state, AttemptState::AbandonedBeforeDispatch);
    assert!(!abandoned.attempt_retains_event_lock && !abandoned.retry_authorized);
    assert!(abandoned.event_lock_owner_attempt_id.is_none());
    assert_eq!(
        abandoned,
        cart_recovery::abandon_prepared(&path, "attempt-1", NOW + 900001).unwrap()
    );
}

#[test]
fn dispatch_without_receipt_is_locked_and_cannot_be_abandoned_after_a_restart() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    drop(dispatched(&path));
    let before = inspect_file(&path).unwrap();
    let report = cart_recovery::inspect(&path, None, NOW + 60000000).unwrap();
    let entry = &report.attempts[0];
    assert_eq!(entry.reason, RecoveryReason::UnresolvedDispatch);
    assert_eq!(entry.state, AttemptState::NeedsReconciliation);
    assert!(entry.original_session_required_for_reconciliation);
    assert!(
        entry
            .actions
            .contains(&RecoveryAction::ReconcileOriginalSession)
    );
    assert!(!entry.actions.contains(&RecoveryAction::AbandonPrepared));
    assert!(entry.recorded_hold_expiry.is_none());
    assert!(entry.retained_observation.is_none());
    assert!(cart_recovery::abandon_prepared(&path, "attempt-1", NOW + 60000000).is_err());
    assert_eq!(inspect_file(&path).unwrap(), before);
}

#[test]
fn contradictory_prepared_state_cannot_hide_a_persisted_dispatch_marker() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    drop(dispatched(&path));
    let connection = Connection::open(&path).unwrap();
    // Simulate a damaged/manual edit rather than a valid journal transition.
    connection
        .execute(
            "UPDATE cart_attempts SET state='prepared' WHERE attempt_id='attempt-1'",
            [],
        )
        .unwrap();
    drop(connection);
    let report = cart_recovery::inspect(&path, None, NOW + 300).unwrap();
    let attempt = &report.attempts[0];
    assert_eq!(attempt.reason, RecoveryReason::JournalStateNeedsReview);
    assert!(attempt.dispatch_recorded && attempt.attempt_retains_event_lock);
    assert!(!attempt.actions.contains(&RecoveryAction::AbandonPrepared));
    assert!(cart_recovery::abandon_prepared(&path, "attempt-1", NOW + 300).is_err());
}

#[test]
fn concurrent_dispatch_and_abandonment_cannot_both_succeed() {
    for _ in 0..12 {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cart.sqlite");
        let mut journal = CartJournal::open(&path).unwrap();
        journal.begin_search("attempt-1", &request(), NOW).unwrap();
        drop(journal);
        let barrier = std::sync::Barrier::new(2);
        let (dispatched, abandoned) = std::thread::scope(|scope| {
            let dispatch = scope.spawn(|| {
                let mut journal = CartJournal::open_existing(&path).unwrap();
                barrier.wait();
                journal.mark_search_dispatch_started("attempt-1", NOW + 100)
            });
            let abandon = scope.spawn(|| {
                barrier.wait();
                cart_recovery::abandon_prepared(&path, "attempt-1", NOW + 100)
            });
            (dispatch.join().unwrap(), abandon.join().unwrap())
        });
        assert_ne!(dispatched.is_ok(), abandoned.is_ok());
        let record = inspect_file(&path).unwrap().remove(0);
        if let Ok(dispatched) = dispatched {
            assert!(dispatched.newly_started);
            assert_eq!(record.state, AttemptState::NeedsReconciliation);
            assert!(record.dispatch_at_ms.is_some());
        } else {
            assert_eq!(record.state, AttemptState::AbandonedBeforeDispatch);
            assert!(record.dispatch_at_ms.is_none());
        }
    }
}

#[test]
fn uncertain_or_nonterminal_absence_never_becomes_release_from_age_or_missing_receipt_fields() {
    for partial_absence in [false, true] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cart.sqlite");
        let mut journal = dispatched(&path);
        let outcome = if partial_absence {
            NativeSearchOutcome::NoHold(NativeSearchNoHoldReceipt {
                context: context(),
                receipt_correlation_id: "diagnostic-only".into(),
                absence_explicitly_confirmed: false,
                request_finished_without_hold: false,
            })
        } else {
            let mut context = context();
            context.event_url = None;
            context.event_start = None;
            NativeSearchOutcome::Uncertain(NativeSearchUncertainReceipt {
                context,
                reason: NativeSearchUncertainty::UnrecognizedResponse,
            })
        };
        journal
            .record_search_outcome("attempt-1", &outcome, NOW + 300)
            .unwrap();
        drop(journal);
        let before = inspect_file(&path).unwrap();
        let report = cart_recovery::inspect(&path, None, NOW + 86400000).unwrap();
        let entry = &report.attempts[0];
        assert_eq!(entry.reason, RecoveryReason::UnresolvedDispatch);
        assert!(entry.attempt_retains_event_lock && !entry.retry_authorized);
        assert!(entry.recorded_hold_expiry.is_none());
        let observation = entry.retained_observation.as_ref().unwrap();
        assert_eq!(observation.age_ms, Some(86399800));
        assert!(!observation.review_reasons.is_empty());
        assert!(
            entry
                .actions
                .contains(&RecoveryAction::InspectNativeHistory)
        );
        assert!(cart_recovery::abandon_prepared(&path, "attempt-1", NOW + 86400000).is_err());
        assert_eq!(inspect_file(&path).unwrap(), before);
    }
}

#[test]
fn only_matching_hold_claims_offer_expiry_review_and_inspection_never_expires_them() {
    for mismatched in [false, true] {
        let dir = tempdir().unwrap();
        let path = dir.path().join("cart.sqlite");
        let mut journal = dispatched(&path);
        let mut receipt = held();
        if mismatched {
            receipt.quantity = 3;
        }
        journal
            .record_search_outcome(
                "attempt-1",
                &NativeSearchOutcome::Hold(Box::new(receipt)),
                NOW + 300,
            )
            .unwrap();
        drop(journal);
        let before = inspect_file(&path).unwrap();
        let early = cart_recovery::inspect(&path, None, NOW + 500000)
            .unwrap()
            .attempts
            .remove(0);
        let late = cart_recovery::inspect(&path, None, NOW + 600000)
            .unwrap()
            .attempts
            .remove(0);
        if mismatched {
            assert_eq!(late.reason, RecoveryReason::UnresolvedDispatch);
            assert!(early.recorded_hold_expiry.is_none() && late.recorded_hold_expiry.is_none());
            assert!(
                !late
                    .actions
                    .contains(&RecoveryAction::ReviewRecordedHoldExpiry)
            );
        } else {
            assert_eq!(early.reason, RecoveryReason::RecordedHoldNeedsReview);
            assert!(!early.recorded_hold_expiry.unwrap().journal_expiry_eligible);
            assert_eq!(late.reason, RecoveryReason::RecordedHoldExpiryElapsed);
            assert!(late.recorded_hold_expiry.unwrap().journal_expiry_eligible);
            assert!(
                late.actions
                    .contains(&RecoveryAction::ReviewRecordedHoldExpiry)
            );
        }
        assert!(late.attempt_retains_event_lock && !late.retry_authorized);
        assert_eq!(inspect_file(&path).unwrap(), before);
    }
}

#[test]
fn inconsistent_hold_without_dispatch_marker_never_offers_expiry_eligibility() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = dispatched(&path);
    journal
        .record_search_outcome(
            "attempt-1",
            &NativeSearchOutcome::Hold(Box::new(held())),
            NOW + 300,
        )
        .unwrap();
    drop(journal);
    let connection = Connection::open(&path).unwrap();
    // Preserve the receipt but simulate a damaged dispatch marker.
    connection
        .execute(
            "UPDATE cart_attempts SET dispatch_at_ms=NULL WHERE attempt_id='attempt-1'",
            [],
        )
        .unwrap();
    drop(connection);
    let before = inspect_file(&path).unwrap();
    let report = cart_recovery::inspect(&path, None, NOW + 700000).unwrap();
    let attempt = &report.attempts[0];
    assert_eq!(attempt.reason, RecoveryReason::JournalStateNeedsReview);
    let expiry = attempt.recorded_hold_expiry.as_ref().unwrap();
    assert!(expiry.elapsed);
    assert!(!expiry.journal_expiry_eligible);
    assert!(
        !attempt
            .actions
            .contains(&RecoveryAction::ReviewRecordedHoldExpiry)
    );
    assert!(attempt.attempt_retains_event_lock && !attempt.retry_authorized);
    assert_eq!(inspect_file(&path).unwrap(), before);
}

#[test]
fn invalid_dispatch_times_prevent_expiry_and_fresh_intent_recommendations() {
    for has_hold in [false, true] {
        for dispatch_at in [
            -1,
            ticket_watch::cart::MAX_EXACT_INTEGER as i64 + 1,
            NOW - 1,
            NOW + 250,
            NOW + 301,
            NOW + 700001,
        ] {
            let dir = tempdir().unwrap();
            let path = dir.path().join("cart.sqlite");
            let mut journal = dispatched(&path);
            let outcome = if has_hold {
                NativeSearchOutcome::Hold(Box::new(held()))
            } else {
                NativeSearchOutcome::NoHold(NativeSearchNoHoldReceipt {
                    context: context(),
                    receipt_correlation_id: "terminal-absence".into(),
                    absence_explicitly_confirmed: true,
                    request_finished_without_hold: true,
                })
            };
            journal
                .record_search_outcome("attempt-1", &outcome, NOW + 300)
                .unwrap();
            drop(journal);
            let connection = Connection::open(&path).unwrap();
            // Simulate a damaged timestamp while preserving the actual receipt.
            connection
                .execute(
                    "UPDATE cart_attempts SET dispatch_at_ms=?1 WHERE attempt_id='attempt-1'",
                    [dispatch_at],
                )
                .unwrap();
            drop(connection);
            let before = inspect_file(&path).unwrap();
            let entry = cart_recovery::inspect(&path, None, NOW + 700000)
                .unwrap()
                .attempts
                .remove(0);
            assert_eq!(entry.reason, RecoveryReason::JournalClockNeedsReview);
            assert_eq!(
                entry.actions,
                [
                    RecoveryAction::InspectJournal,
                    RecoveryAction::InspectNativeHistory
                ]
            );
            assert!(entry.dispatch_recorded && !entry.retry_authorized);
            assert_eq!(entry.attempt_retains_event_lock, has_hold);
            if has_hold {
                let expiry = entry.recorded_hold_expiry.unwrap();
                assert!(expiry.elapsed && !expiry.journal_expiry_eligible);
            } else {
                assert!(entry.recorded_hold_expiry.is_none());
            }
            assert_eq!(inspect_file(&path).unwrap(), before);
        }
    }
}

#[test]
fn terminal_attempt_does_not_authorize_retry_or_hide_a_newer_event_lock() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = dispatched(&path);
    journal
        .record_search_outcome(
            "attempt-1",
            &NativeSearchOutcome::NoHold(NativeSearchNoHoldReceipt {
                context: context(),
                receipt_correlation_id: "terminal-absence".into(),
                absence_explicitly_confirmed: true,
                request_finished_without_hold: true,
            }),
            NOW + 300,
        )
        .unwrap();
    let old = cart_recovery::inspect(&path, Some("attempt-1"), NOW + 400)
        .unwrap()
        .attempts
        .remove(0);
    assert_eq!(old.state, AttemptState::NoHoldConfirmed);
    assert!(!old.attempt_retains_event_lock && !old.retry_authorized);
    assert!(old.event_lock_owner_attempt_id.is_none());
    assert!(old.actions.contains(&RecoveryAction::ReviewFreshIntent));
    assert_eq!(
        old.retained_observation.unwrap().kind,
        ObservationKind::NoHold
    );
    journal
        .begin_search("attempt-2", &request(), NOW + 400)
        .unwrap();
    let old = cart_recovery::inspect(&path, Some("attempt-1"), NOW + 500)
        .unwrap()
        .attempts
        .remove(0);
    assert_eq!(
        old.event_lock_owner_attempt_id.as_deref(),
        Some("attempt-2")
    );
    assert!(!old.actions.contains(&RecoveryAction::ReviewFreshIntent));
}

#[test]
fn quoted_hold_and_recorded_expiry_remain_distinct_from_provider_authentication_or_new_stock() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let plan: CartPlanRequest = serde_json::from_value(serde_json::json!({
        "intent":{"event_url":EVENT,"expected_event_start":START,"quantity":2,"max_total_minor":1000000},
        "context":{"session_id":"session-1","document_id":"document-1"},
        "snapshot":{"event_url":EVENT,"event_start":START,"session_id":"session-1","document_id":"document-1",
            "requested_quantity":2,"observed_at_ms":NOW,"expires_at_ms":NOW+10000,
            "coverage":"complete_for_request","groups":[{"id":"quote-1","quantity":2,"kind":"seated",
                "availability":"confirmed","evidence":"exact_group","adjacency":"site_confirmed",
                "seats":held().seats,"price":{"amount_minor":400000,"currency":"CZK","basis":"group_total","fees":"complete"}}]}
    })).unwrap();
    let mut journal = CartJournal::open(&path).unwrap();
    journal.begin("attempt-1", &plan, NOW).unwrap();
    journal
        .mark_dispatch_started("attempt-1", NOW + 100)
        .unwrap();
    journal
        .record_hold(
            "attempt-1",
            &HoldReceipt {
                context: ReceiptContext {
                    attempt_id: "attempt-1".into(),
                    event_url: EVENT.into(),
                    event_start: Some(START.into()),
                    session_id: "session-1".into(),
                    document_id: "document-1".into(),
                    quote_id: "quote-1".into(),
                    observed_at_ms: NOW + 200,
                    alternate_event_or_date_reported: false,
                },
                category_id: None,
                ticket_type: None,
                hold_correlation_id: "hold-1".into(),
                quantity: 2,
                seats: held().seats,
                total_minor: 400000,
                currency: "CZK".into(),
                fees: QuoteFees::Complete,
                hold_expires_at_ms: NOW + 600000,
            },
            NOW + 300,
        )
        .unwrap();
    let report = cart_recovery::inspect(&path, None, NOW + 400).unwrap();
    assert_eq!(
        report.attempts[0].reason,
        RecoveryReason::RecordedHoldActive
    );
    assert_eq!(report.attempts[0].workflow, AttemptWorkflow::QuotedPlan);
    assert!(
        !report.attempts[0]
            .actions
            .contains(&RecoveryAction::InspectNativeHistory)
    );
    assert!(!report.current_inventory_verified && !report.retry_authorized);
    journal
        .expire_confirmed_hold("attempt-1", NOW + 600000)
        .unwrap();
    let report = cart_recovery::inspect(&path, None, NOW + 700000).unwrap();
    let expired = &report.attempts[0];
    assert_eq!(expired.reason, RecoveryReason::RecordedHoldExpired);
    assert!(!expired.attempt_retains_event_lock && !expired.retry_authorized);
    assert!(
        !expired
            .recorded_hold_expiry
            .as_ref()
            .unwrap()
            .journal_expiry_eligible
    );
}

#[test]
fn clock_anomalies_do_not_offer_mutations_or_turn_invalid_observations_into_fresh_evidence() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = CartJournal::open(&path).unwrap();
    journal.begin_search("attempt-1", &request(), NOW).unwrap();
    let before_time = cart_recovery::inspect(&path, None, NOW - 1)
        .unwrap()
        .attempts
        .remove(0);
    assert_eq!(before_time.reason, RecoveryReason::JournalClockNeedsReview);
    assert!(cart_recovery::abandon_prepared(&path, "attempt-1", NOW - 1).is_err());
    assert!(
        !before_time
            .actions
            .contains(&RecoveryAction::AbandonPrepared)
    );
    journal
        .mark_search_dispatch_started("attempt-1", NOW + 100)
        .unwrap();
    let mut future = context();
    future.observed_at_ms = NOW + 10000;
    journal
        .record_search_outcome(
            "attempt-1",
            &NativeSearchOutcome::Uncertain(NativeSearchUncertainReceipt {
                context: future,
                reason: NativeSearchUncertainty::Timeout,
            }),
            NOW + 300,
        )
        .unwrap();
    let entry = cart_recovery::inspect(&path, None, NOW + 400)
        .unwrap()
        .attempts
        .remove(0);
    assert_eq!(entry.reason, RecoveryReason::JournalClockNeedsReview);
    assert_eq!(
        entry.actions,
        [
            RecoveryAction::InspectJournal,
            RecoveryAction::InspectNativeHistory
        ]
    );
    let observation = entry.retained_observation.unwrap();
    assert_eq!(observation.clock, ObservationClock::Future);
    assert!(observation.age_ms.is_none());
    assert!(
        observation
            .review_reasons
            .contains(&SearchReviewReason::InvalidObservationTime)
    );
    assert!(entry.attempt_retains_event_lock && !entry.retry_authorized);
}

#[test]
fn pre_dispatch_observation_retains_evidence_but_requires_clock_review() {
    let dir = tempdir().unwrap();
    let path = dir.path().join("cart.sqlite");
    let mut journal = dispatched(&path);
    let mut early = context();
    early.observed_at_ms = NOW + 99;
    journal
        .record_search_outcome(
            "attempt-1",
            &NativeSearchOutcome::Uncertain(NativeSearchUncertainReceipt {
                context: early,
                reason: NativeSearchUncertainty::UnrecognizedResponse,
            }),
            NOW + 300,
        )
        .unwrap();
    drop(journal);
    let before = inspect_file(&path).unwrap();
    let entry = cart_recovery::inspect(&path, None, NOW + 700000)
        .unwrap()
        .attempts
        .remove(0);
    assert_eq!(entry.reason, RecoveryReason::JournalClockNeedsReview);
    assert_eq!(
        entry.actions,
        [
            RecoveryAction::InspectJournal,
            RecoveryAction::InspectNativeHistory
        ]
    );
    let retained = entry.retained_observation.unwrap();
    assert_eq!(retained.observed_at_ms, NOW + 99);
    assert!(
        retained
            .review_reasons
            .contains(&SearchReviewReason::InvalidObservationTime)
    );
    assert!(entry.attempt_retains_event_lock && !entry.retry_authorized);
    assert!(entry.recorded_hold_expiry.is_none());
    assert_eq!(inspect_file(&path).unwrap(), before);
}
