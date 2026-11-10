use rusqlite::{Connection, params};
use std::path::Path;
use ticket_watch::{
    config::{DiscordConfig, MapWatch},
    map_inspection::DisplayedSeatQuote,
    map_monitor_store::MapMonitorStore,
    map_scan::{ScanOutcome, SeatScanObservation, SectionScan, StopReason},
    map_scan_store::{COMPARISON_MAX_AGE_MS, MapScanStore},
    model::{FeeScope, InventoryKind, PriceScope, QuantityKind},
};

const START: i64 = 1_700_000_000_000;

fn scan(at: i64, prices: &[(&str, Option<i64>)]) -> SectionScan {
    let observations: Vec<_> = prices
        .iter()
        .enumerate()
        .map(|(index, (id, price))| {
            let seat = id.strip_prefix("seat-").unwrap().to_owned();
            SeatScanObservation {
                seat_id: (*id).into(),
                row: "12".into(),
                seat: seat.clone(),
                observed_at_ms: at + 100 + index as i64,
                outcome: if price.is_some() {
                    ScanOutcome::Quoted
                } else {
                    ScanOutcome::TooltipUnavailable
                },
                quote: price.map(|price_minor| DisplayedSeatQuote {
                    evidence: "rendered_seat_hover".into(),
                    seat_id: (*id).into(),
                    zone: "1. POSCHODÍ".into(),
                    section: "110".into(),
                    row: "12".into(),
                    seat,
                    ticket_type: "Ověřené přeprodávané vstupenky".into(),
                    restriction: None,
                    price_minor,
                    currency: "CZK".into(),
                    price_basis: "per_ticket".into(),
                    price_text: format!("{},{:02} Kč za kus", price_minor / 100, price_minor % 100),
                    fees_complete: false,
                    available_quantity: None,
                }),
            }
        })
        .collect();
    SectionScan {
        event_url: "https://www.ticketmaster.cz/event/synthetic/12345678".into(),
        event_id: "12345678".into(),
        site_host: "ticketmaster.cz".into(),
        section_id: "s_12".into(),
        section_name: "110".into(),
        started_at_ms: at,
        finished_at_ms: at + 500,
        coverage: "partial_rendered_section".into(),
        limit: 20,
        candidate_count: observations.len() as u32,
        visible_candidate_count: observations.len() as u32,
        attempted_count: observations.len() as u32,
        unattempted_count: 0,
        quoted_count: observations.iter().filter(|o| o.quote.is_some()).count() as u32,
        stop_reason: StopReason::VisibleCandidatesExhausted,
        observations,
        stock_verified: false,
        adjacency_verified: false,
    }
}

fn saved_price(path: &Path, id: &str) -> i64 {
    let connection = Connection::open(path).unwrap();
    let raw: String = connection
        .query_row(
            "SELECT quote_json FROM map_scan_quotes WHERE seat_id = ?1",
            [id],
            |r| r.get(0),
        )
        .unwrap();
    serde_json::from_str::<DisplayedSeatQuote>(&raw)
        .unwrap()
        .price_minor
}

fn watch(initial: bool) -> MapWatch {
    MapWatch {
        id: "synthetic-section".into(),
        name: "Synthetic event".into(),
        url: "https://www.ticketmaster.cz/event/synthetic/12345678".into(),
        section_id: "s_12".into(),
        notify_initial: initial,
        ..MapWatch::default()
    }
}
fn setup(path: &Path, watch: &MapWatch) -> MapMonitorStore {
    let mut store = MapMonitorStore::open(path).unwrap();
    store
        .reconcile(
            std::slice::from_ref(watch),
            &DiscordConfig::default(),
            START,
        )
        .unwrap();
    store
}
fn record(store: &mut MapMonitorStore, scan: &SectionScan) -> usize {
    store
        .record_success(
            "synthetic-section",
            scan,
            scan.finished_at_ms,
            scan.finished_at_ms + 60_000,
            300_000,
        )
        .unwrap()
        .enqueued
}
fn state_count(path: &Path, state: &str) -> i64 {
    Connection::open(path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM map_monitor_outbox WHERE state=?1",
            [state],
            |r| r.get(0),
        )
        .unwrap()
}

#[test]
fn initial_baseline_is_silent_but_new_seats_and_price_changes_queue_precise_observations() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut store = setup(&path, &watch(false));
    assert_eq!(
        record(&mut store, &scan(START, &[("seat-1", Some(100_000))])),
        0
    );
    assert_eq!(
        record(
            &mut store,
            &scan(
                START + 1000,
                &[("seat-1", Some(90_000)), ("seat-2", Some(80_000))]
            )
        ),
        2
    );
    let pending = store.pending(START + 1500, 300_000, 10).unwrap();
    assert_eq!(pending.len(), 2);
    assert_eq!(pending[0].payload.kind, "displayed_price_observed");
    assert_eq!(pending[1].payload.kind, "displayed_price_changed");
    assert_eq!(pending[1].payload.offers[0].price_minor, Some(90_000));
    for entry in pending {
        for offer in entry.payload.offers {
            assert_eq!(offer.inventory_kind, InventoryKind::Resale);
            assert_eq!(offer.quantity, None);
            assert_eq!(offer.quantity_kind, QuantityKind::Unknown);
            assert_eq!(offer.price_scope, PriceScope::PerTicket);
            assert_eq!(offer.fees, FeeScope::Unknown);
        }
    }
    let status = store.status().unwrap().remove(0);
    assert_eq!(status.health, "ok");
    assert_eq!(status.last_success_at_ms, Some(START + 1500));
}

#[test]
fn queue_survives_restart_and_ambiguous_delivery_retries_keep_the_same_nonce() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let config = watch(true);
    let first = scan(START, &[("seat-1", Some(100_000))]);
    let mut store = setup(&path, &config);
    assert_eq!(record(&mut store, &first), 1);
    let original = store.pending(START + 500, 300_000, 10).unwrap().remove(0);
    assert_eq!(record(&mut store, &first), 0);
    store
        .retry(original.id, START + 2000, "transport secret=do-not-store")
        .unwrap();
    drop(store);
    let mut store = setup(&path, &config);
    assert!(store.pending(START + 1000, 300_000, 10).unwrap().is_empty());
    let retry = store.pending(START + 2000, 300_000, 10).unwrap().remove(0);
    assert_eq!(retry.id, original.id);
    assert_eq!(retry.nonce, original.nonce);
    assert_eq!(retry.attempts, 1);
    let reason: String = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT last_error FROM map_monitor_outbox WHERE id=?1",
            [retry.id],
            |r| r.get(0),
        )
        .unwrap();
    assert!(!reason.contains("do-not-store"));
    store.delivered(retry.id, "123456789012345678").unwrap();
    assert!(store.pending(START + 3000, 300_000, 10).unwrap().is_empty());
    assert_eq!(state_count(&path, "delivered"), 1);
}

#[test]
fn verified_map_quote_is_resale_even_when_restriction_mentions_original_tickets() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut store = setup(&path, &watch(true));
    let mut report = scan(START, &[("seat-1", Some(100_000))]);
    report.observations[0].quote.as_mut().unwrap().restriction =
        Some("Original printed ticket required".into());
    assert_eq!(record(&mut store, &report), 1);
    let pending = store.pending(START + 500, 300_000, 10).unwrap().remove(0);
    assert_eq!(
        pending.payload.offers[0].inventory_kind,
        InventoryKind::Resale
    );
    assert_eq!(
        pending.payload.offers[0].inventory.as_deref(),
        Some("Original printed ticket required")
    );

    let mut unsupported = scan(START + 1000, &[("seat-2", Some(90_000))]);
    unsupported.observations[0]
        .quote
        .as_mut()
        .unwrap()
        .ticket_type = "Official Platinum".into();
    assert!(
        store
            .record_success(
                "synthetic-section",
                &unsupported,
                START + 1500,
                START + 61_500,
                300_000
            )
            .is_err()
    );
    assert_eq!(state_count(&path, "pending"), 1);
}

#[test]
fn legacy_map_payload_missing_sale_kind_survives_restart_as_unknown_without_rewriting_it() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let watch = watch(true);
    let mut store = setup(&path, &watch);
    assert_eq!(
        record(&mut store, &scan(START, &[("seat-1", Some(100_000))])),
        1
    );
    let original = store.pending(START + 500, 300_000, 10).unwrap().remove(0);
    let mut payload = serde_json::to_value(&original.payload).unwrap();
    payload["offers"][0]
        .as_object_mut()
        .unwrap()
        .remove("inventory_kind");
    let raw = serde_json::to_string(&payload).unwrap();
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE map_monitor_outbox SET payload_json=?1 WHERE id=?2",
            params![raw, original.id],
        )
        .unwrap();
    drop(store);
    let mut store = setup(&path, &watch);
    let pending = store.pending(START + 1000, 300_000, 10).unwrap().remove(0);
    assert_eq!(pending.id, original.id);
    assert_eq!(pending.nonce, original.nonce);
    assert_eq!(
        pending.payload.offers[0].inventory_kind,
        InventoryKind::Unknown
    );
    let saved: String = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT payload_json FROM map_monitor_outbox WHERE id=?1",
            [original.id],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(saved, raw);
    assert_eq!(state_count(&path, "pending"), 1);

    // Missing classification never bypasses the independent current-price comparison.
    payload["offers"][0]["price_minor"] = serde_json::json!(90_000);
    payload["offers"][0]["price_text"] = serde_json::json!("900,00 Kč za kus");
    Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE map_monitor_outbox SET payload_json=?1 WHERE id=?2",
            params![serde_json::to_string(&payload).unwrap(), original.id],
        )
        .unwrap();
    assert!(store.pending(START + 1500, 300_000, 10).unwrap().is_empty());
    assert_eq!(state_count(&path, "cancelled"), 1);
}

#[test]
fn explicit_conflicting_or_malformed_sale_kind_cannot_take_the_legacy_path() {
    for (encoded_kind, malformed) in [
        (serde_json::json!("primary"), false),
        (serde_json::json!("unknown"), false),
        (serde_json::json!("not-a-sale-kind"), true),
        (serde_json::Value::Null, true),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("monitor.sqlite");
        let mut store = setup(&path, &watch(true));
        assert_eq!(
            record(&mut store, &scan(START, &[("seat-1", Some(100_000))])),
            1
        );
        let original = store.pending(START + 500, 300_000, 10).unwrap().remove(0);
        let mut payload = serde_json::to_value(&original.payload).unwrap();
        payload["offers"][0]["inventory_kind"] = encoded_kind;
        Connection::open(&path)
            .unwrap()
            .execute(
                "UPDATE map_monitor_outbox SET payload_json=?1 WHERE id=?2",
                params![serde_json::to_string(&payload).unwrap(), original.id],
            )
            .unwrap();
        let result = store.pending(START + 1000, 300_000, 10);
        if malformed {
            assert!(result.is_err());
        } else {
            assert!(result.unwrap().is_empty());
            assert_eq!(state_count(&path, "cancelled"), 1);
        }
        assert_eq!(state_count(&path, "delivered"), 0);
    }
}

#[test]
fn route_changes_cancel_saved_queue_and_fresh_initial_observations_can_use_seeded_history() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let first = scan(START, &[("seat-1", Some(100_000))]);
    MapScanStore::open(&path).unwrap().record(&first).unwrap();
    let config = watch(true);
    let mut store = setup(&path, &config);
    assert_eq!(record(&mut store, &first), 1);
    let old = store.pending(START + 500, 300_000, 10).unwrap().remove(0);
    let route = DiscordConfig {
        channel_id: Some("123456789012345678".into()),
        ..Default::default()
    };
    store
        .reconcile(std::slice::from_ref(&config), &route, START + 1000)
        .unwrap();
    assert!(store.pending(START + 1000, 300_000, 10).unwrap().is_empty());
    assert_eq!(
        record(
            &mut store,
            &scan(START + 1000, &[("seat-1", Some(100_000))])
        ),
        1
    );
    let new = store.pending(START + 1500, 300_000, 10).unwrap().remove(0);
    assert_ne!(old.nonce, new.nonce);
    store.delivered(old.id, "123456789012345679").unwrap();
    assert_eq!(state_count(&path, "cancelled"), 1);
    assert_eq!(state_count(&path, "pending"), 1);
    let mut edited = config;
    edited.notify_initial = false;
    edited.name = "Renamed watch".into();
    store
        .reconcile(&[edited.clone()], &route, START + 2000)
        .unwrap();
    assert_eq!(
        record(&mut store, &scan(START + 2000, &[("seat-1", Some(80_000))])),
        0
    );
    store.reconcile(&[], &route, START + 3000).unwrap();
    assert!(!store.status().unwrap()[0].enabled);
    assert!(store.due(START + 100_000, 10).unwrap().is_empty());
    store.reconcile(&[edited], &route, START + 4000).unwrap();
    assert_eq!(
        record(&mut store, &scan(START + 4000, &[("seat-1", Some(70_000))])),
        0
    );
}

#[test]
fn source_age_expiry_and_persisted_cooldowns_survive_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let config = watch(true);
    let mut store = setup(&path, &config);
    record(&mut store, &scan(START, &[("seat-1", Some(100_000))]));
    store
        .set_cooldown("host:ticketmaster.cz", START + 30_000)
        .unwrap();
    store
        .set_cooldown("host:ticketmaster.cz", START + 2000)
        .unwrap();
    store
        .set_cooldown("discord:delivery", START + 2000)
        .unwrap();
    drop(store);
    let mut store = setup(&path, &config);
    assert_eq!(
        store.cooldown("host:ticketmaster.cz").unwrap(),
        Some(START + 30_000)
    );
    assert_eq!(store.status().unwrap()[0].next_due_ms, START + 60_500);
    assert!(store.due(START + 60_000, 10).unwrap().is_empty());
    assert_eq!(store.due(START + 60_500, 10).unwrap().len(), 1);
    assert!(store.pending(START + 1000, 300_000, 10).unwrap().is_empty());
    assert_eq!(store.pending(START + 2000, 300_000, 10).unwrap().len(), 1);
    assert!(store.pending(START + 1200, 1000, 10).unwrap().is_empty());
    assert_eq!(state_count(&path, "expired"), 1); // source time +100, not enqueue +500
}

#[test]
fn blocked_health_cancels_pending_without_changing_price_baselines_or_last_success() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut store = setup(&path, &watch(true));
    record(&mut store, &scan(START, &[("seat-1", Some(100_000))]));
    store
        .record_failure(
            "synthetic-section",
            "blocked",
            START + 1000,
            START + 900_000,
        )
        .unwrap();
    let status = store.status().unwrap().remove(0);
    assert_eq!(status.health, "blocked");
    assert_eq!(status.last_success_at_ms, Some(START + 500));
    assert_eq!(saved_price(&path, "seat-1"), 100_000);
    assert!(store.pending(START + 1500, 300_000, 10).unwrap().is_empty());
    assert_eq!(
        record(&mut store, &scan(START + 2000, &[("seat-1", Some(90_000))])),
        1
    );
    assert_eq!(
        store.pending(START + 2500, 300_000, 10).unwrap()[0]
            .payload
            .kind,
        "displayed_price_changed"
    );
}

#[test]
fn mismatched_stale_or_future_scan_never_mutates_history_or_runtime() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut store = setup(&path, &watch(true));
    let first = scan(START, &[("seat-1", Some(100_000))]);
    let mut mismatch = first.clone();
    mismatch.section_id = "s_13".into();
    for (scan, now) in [
        (&first, START + 130_000),
        (&first, START - 10_000),
        (&mismatch, START + 500),
    ] {
        assert!(
            store
                .record_success("synthetic-section", scan, now, now + 60_000, 300_000)
                .is_err()
        );
    }
    assert_eq!(store.status().unwrap()[0].health, "unobserved");
    assert_eq!(state_count(&path, "pending"), 0);
    assert_eq!(record(&mut store, &first), 1);
}

#[test]
fn outbox_failure_rolls_back_history_and_runtime_together() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut store = setup(&path, &watch(false));
    record(&mut store, &scan(START, &[("seat-1", Some(100_000))]));
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_alert BEFORE INSERT ON map_monitor_outbox BEGIN SELECT RAISE(ABORT,'synthetic failure'); END;").unwrap();
    let next = scan(START + 1000, &[("seat-1", Some(90_000))]);
    assert!(
        store
            .record_success(
                "synthetic-section",
                &next,
                START + 1500,
                START + 61_500,
                300_000
            )
            .is_err()
    );
    assert_eq!(saved_price(&path, "seat-1"), 100_000);
    assert_eq!(
        store.status().unwrap()[0].last_success_at_ms,
        Some(START + 500)
    );
    assert_eq!(state_count(&path, "pending"), 0);
    connection
        .execute_batch("DROP TRIGGER fail_alert;")
        .unwrap();
    assert_eq!(record(&mut store, &next), 1);
}

#[test]
fn pending_revalidates_superseded_prices_and_chunks_at_six_seats() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut store = setup(&path, &watch(true));
    assert_eq!(
        record(
            &mut store,
            &scan(
                START,
                &[
                    ("seat-1", Some(100_000)),
                    ("seat-2", Some(100_000)),
                    ("seat-3", Some(100_000)),
                    ("seat-4", Some(100_000)),
                    ("seat-5", Some(100_000)),
                    ("seat-6", Some(100_000)),
                    ("seat-7", Some(100_000))
                ]
            )
        ),
        2
    );
    let first = store.pending(START + 500, 300_000, 10).unwrap();
    assert_eq!(
        first
            .iter()
            .map(|a| a.payload.offers.len())
            .collect::<Vec<_>>(),
        vec![6, 1]
    );
    assert_eq!(
        record(&mut store, &scan(START + 1000, &[("seat-1", Some(90_000))])),
        1
    );
    let next = store.pending(START + 1500, 300_000, 10).unwrap();
    assert_eq!(next.len(), 2);
    assert_eq!(next[0].payload.offers[0].key, "seat-7");
    assert_eq!(next[1].payload.offers[0].price_minor, Some(90_000));
    assert_eq!(state_count(&path, "cancelled"), 1);
}

#[test]
fn disabled_change_flags_and_stale_baseline_refreshes_do_not_notify() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut config = watch(false);
    config.notify_price_changes = false;
    config.notify_details_changes = false;
    let mut store = setup(&path, &config);
    record(&mut store, &scan(START, &[("seat-1", Some(100_000))]));
    let mut changed = scan(START + 1000, &[("seat-1", Some(90_000))]);
    changed.observations[0].quote.as_mut().unwrap().restriction = Some("Limited view".into());
    assert_eq!(record(&mut store, &changed), 0);
    config.notify_price_changes = true;
    config.notify_details_changes = true;
    store
        .reconcile(&[config], &DiscordConfig::default(), START + 2000)
        .unwrap();
    assert_eq!(
        record(&mut store, &scan(START + 2000, &[("seat-1", Some(90_000))])),
        0
    );
    let mut details = scan(START + 3000, &[("seat-1", Some(90_000))]);
    details.observations[0].quote.as_mut().unwrap().restriction = Some("Limited view".into());
    assert_eq!(record(&mut store, &details), 1);
    assert_eq!(
        store.pending(START + 3500, 300_000, 10).unwrap()[0]
            .payload
            .kind,
        "displayed_details_changed"
    );
    assert_eq!(
        record(
            &mut store,
            &scan(
                START + 4000 + COMPARISON_MAX_AGE_MS,
                &[("seat-1", Some(70_000))]
            )
        ),
        0
    );
}

#[test]
fn version_one_history_migrates_without_losing_baseline() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    MapScanStore::open(&path)
        .unwrap()
        .record(&scan(START, &[("seat-1", Some(100_000))]))
        .unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("DROP TABLE map_monitor_outbox; DROP TABLE map_monitor_watches; DROP TABLE map_monitor_cooldowns; DROP TABLE map_monitor_meta; UPDATE map_scan_meta SET version=1;").unwrap();
    drop(connection);
    let mut store = setup(&path, &watch(false));
    assert_eq!(saved_price(&path, "seat-1"), 100_000);
    assert_eq!(
        record(&mut store, &scan(START + 1000, &[("seat-1", Some(90_000))])),
        0
    );
    let version: i64 = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT version FROM map_scan_meta WHERE id=?1",
            params![1],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(version, 2);
    assert_eq!(
        record(&mut store, &scan(START + 2000, &[("seat-1", Some(80_000))])),
        1
    );
}

#[test]
fn saved_display_text_must_match_its_price_but_equivalent_format_is_accepted() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut store = setup(&path, &watch(true));
    record(&mut store, &scan(START, &[("seat-1", Some(100_000))]));
    let pending = store.pending(START + 500, 300_000, 10).unwrap().remove(0);
    let mut payload = pending.payload;
    payload.offers[0].price_text = Some("1\u{a0}000,00 Kč za kus".into());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE map_monitor_outbox SET payload_json=?1 WHERE id=?2",
            params![serde_json::to_string(&payload).unwrap(), pending.id],
        )
        .unwrap();
    assert_eq!(store.pending(START + 1000, 300_000, 10).unwrap().len(), 1);
    payload.offers[0].price_text = Some("1,00 Kč za kus".into());
    connection
        .execute(
            "UPDATE map_monitor_outbox SET payload_json=?1 WHERE id=?2",
            params![serde_json::to_string(&payload).unwrap(), pending.id],
        )
        .unwrap();
    assert!(store.pending(START + 1000, 300_000, 10).is_err());
    assert_eq!(state_count(&path, "delivered"), 0);
}

#[test]
fn terminal_retention_is_bounded_and_full_pending_queue_rolls_back_new_scan() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut store = setup(&path, &watch(true));
    record(&mut store, &scan(START, &[("seat-1", Some(100_000))]));
    let first = store.pending(START + 500, 300_000, 10).unwrap().remove(0);
    store.delivered(first.id, "123456789012345678").unwrap();
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("WITH RECURSIVE copies(n) AS (SELECT 1 UNION ALL SELECT n+1 FROM copies WHERE n<9999)
        INSERT INTO map_monitor_outbox(watch_id,fingerprint,route_fingerprint,nonce,payload_json,state,not_before_ms,created_at_ms,observed_at_ms)
        SELECT o.watch_id,o.fingerprint,o.route_fingerprint,printf('%064d',copies.n),o.payload_json,'delivered',o.not_before_ms,o.created_at_ms,o.observed_at_ms
        FROM map_monitor_outbox o CROSS JOIN copies WHERE o.id=1;").unwrap();
    assert_eq!(
        record(&mut store, &scan(START + 1000, &[("seat-1", Some(90_000))])),
        1
    );
    let total: i64 = connection
        .query_row("SELECT COUNT(*) FROM map_monitor_outbox", [], |r| r.get(0))
        .unwrap();
    assert_eq!(total, 10_000);
    assert_eq!(state_count(&path, "delivered"), 9999);
    assert_eq!(state_count(&path, "pending"), 1);
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM map_monitor_outbox WHERE id=?1",
                [first.id],
                |r| r.get::<_, i64>(0)
            )
            .unwrap(),
        0
    );
    connection
        .execute("UPDATE map_monitor_outbox SET state='pending'", [])
        .unwrap();
    let blocked = scan(START + 2000, &[("seat-1", Some(80_000))]);
    assert!(
        store
            .record_success(
                "synthetic-section",
                &blocked,
                START + 2500,
                START + 62_500,
                300_000
            )
            .is_err()
    );
    assert_eq!(saved_price(&path, "seat-1"), 90_000);
    assert_eq!(
        store.status().unwrap()[0].last_success_at_ms,
        Some(START + 1500)
    );
    assert_eq!(state_count(&path, "pending"), 10_000);
}

#[test]
fn long_provider_cooldowns_remain_durable_without_forcing_an_early_retry() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let config = watch(false);
    let mut store = setup(&path, &config);
    store
        .set_cooldown("host:ticketmaster.cz", i64::MAX)
        .unwrap();
    store
        .record_failure("synthetic-section", "blocked", START, i64::MAX)
        .unwrap();
    drop(store);
    let store = setup(&path, &config);
    assert_eq!(
        store.cooldown("host:ticketmaster.cz").unwrap(),
        Some(i64::MAX)
    );
    assert_eq!(store.status().unwrap()[0].next_due_ms, i64::MAX);
    assert!(store.due(START + 31_536_000_001, 10).unwrap().is_empty());
}
