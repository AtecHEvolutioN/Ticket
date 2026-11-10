use rusqlite::Connection;
use std::path::Path;
use ticket_watch::{
    config::{Config, MapWatch},
    map_health::inspect,
    map_inspection::DisplayedSeatQuote,
    map_monitor_store::MapMonitorStore,
    map_scan::{ScanOutcome, SeatScanObservation, SectionScan, StopReason},
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

fn config(path: &Path) -> Config {
    Config {
        database: path.with_extension("inventory.sqlite"),
        map_database: path.into(),
        map_watches: vec![MapWatch {
            id: "synthetic".into(),
            name: "Synthetic event".into(),
            url: "https://www.ticketmaster.cz/event/synthetic/12345678".into(),
            section_id: "s_12".into(),
            notify_initial: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}
fn seeded(config: &Config) -> MapMonitorStore {
    let mut store = MapMonitorStore::open(&config.map_database).unwrap();
    store
        .reconcile(&config.map_watches, &config.discord, START)
        .unwrap();
    for watch in config.map_watches.iter().filter(|watch| watch.enabled) {
        let mut scan = scan(START, &[("seat-1", Some(100_000))]);
        scan.section_id = watch.section_id.clone();
        store
            .record_success(&watch.id, &scan, START + 500, START + 60_500, 300_000)
            .unwrap();
    }
    store
}

#[test]
fn missing_database_is_structured_unhealthy_without_creating_directories_or_state() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("missing/nested/monitor.sqlite");
    let report = inspect(&config(&path), 120, START + 1500).unwrap();
    assert!(!report.healthy);
    assert_eq!(report.status, "not_initialized");
    assert_eq!(report.checks[0].issues, vec!["not_initialized"]);
    assert!(!path.parent().unwrap().exists());
    assert!(dir.path().read_dir().unwrap().next().is_none());
}

#[test]
fn fresh_state_is_read_only_and_never_claims_process_delivery_or_stock_verification() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("monitor.sqlite");
    let mut config = config(&path);
    config.browser.node = "this-browser-must-not-be-started".into();
    config.browser.script = dir.path().join("does-not-exist.cjs");
    config.discord.token_env = "MISSING_MAP_HEALTH_TEST_TOKEN".into();
    drop(seeded(&config));
    let before = std::fs::read(&path).unwrap();
    let report = inspect(&config, 120, START + 1500).unwrap();
    assert!(report.healthy);
    assert_eq!(report.status, "healthy");
    assert_eq!(report.scope, "displayed_price_observations");
    assert_eq!(report.checks[0].age_ms, Some(1000));
    assert_eq!(report.checks[0].next_due_ms, Some(START + 60_500));
    assert_eq!(report.checks[0].pending_alerts, 1);
    assert!(
        !report.stock_verified
            && !report.adjacency_verified
            && !report.delivery_verified
            && !report.process_running_verified
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let states: i64 = Connection::open(&path)
        .unwrap()
        .query_row(
            "SELECT COUNT(*) FROM map_monitor_outbox WHERE state='pending'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(states, 1);
}

#[test]
fn complete_legacy_and_upgraded_history_schemas_remain_healthy_without_read_side_migration() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir.path().join("monitor.sqlite"));
    drop(seeded(&config));
    let connection = Connection::open(&config.map_database).unwrap();
    connection.execute_batch("DROP TABLE market_samples; DROP TABLE market_sources; DROP TABLE market_events; DROP TABLE market_history_meta;").unwrap();
    drop(connection);
    let before = std::fs::read(&config.map_database).unwrap();
    assert!(inspect(&config, 120, START + 1500).unwrap().healthy);
    assert_eq!(std::fs::read(&config.map_database).unwrap(), before);
    let count: i64 = Connection::open(&config.map_database)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name LIKE 'market_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 0, "health inspection must not initialize history");

    drop(MapMonitorStore::open(&config.map_database).unwrap());
    let before = std::fs::read(&config.map_database).unwrap();
    assert!(inspect(&config, 120, START + 1500).unwrap().healthy);
    assert_eq!(std::fs::read(&config.map_database).unwrap(), before);
    let count: i64 = Connection::open(&config.map_database)
        .unwrap()
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type='table' AND name LIKE 'market_%'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(count, 4);
}

#[test]
fn partial_unknown_version_or_unrelated_history_extension_is_rejected_read_only() {
    for mutation in [
        "DROP TABLE market_samples",
        "UPDATE market_history_meta SET schema_version=2 WHERE id=1",
        "CREATE TABLE unrelated(payload TEXT)",
    ] {
        let dir = tempfile::tempdir().unwrap();
        let config = config(&dir.path().join("monitor.sqlite"));
        drop(seeded(&config));
        let connection = Connection::open(&config.map_database).unwrap();
        connection.execute_batch(mutation).unwrap();
        drop(connection);
        let before = std::fs::read(&config.map_database).unwrap();
        assert!(inspect(&config, 120, START + 1500).is_err(), "{mutation}");
        assert_eq!(std::fs::read(&config.map_database).unwrap(), before);
    }
}

#[test]
fn freshness_boundary_is_inclusive_and_stale_success_cannot_look_healthy() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir.path().join("monitor.sqlite"));
    let _store = seeded(&config);
    assert!(inspect(&config, 120, START + 120_500).unwrap().healthy);
    let stale = inspect(&config, 120, START + 120_501).unwrap();
    assert!(!stale.healthy);
    assert_eq!(stale.checks[0].issues, vec!["stale"]);
}

#[test]
fn blocked_unknown_unobserved_and_future_states_are_unhealthy() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir.path().join("monitor.sqlite"));
    let mut store = MapMonitorStore::open(&config.map_database).unwrap();
    store
        .reconcile(&config.map_watches, &config.discord, START)
        .unwrap();
    let initial = inspect(&config, 120, START + 1500).unwrap();
    assert!(!initial.healthy && initial.checks[0].issues.contains(&"unobserved"));
    drop(store);
    let mut store = seeded(&config);
    for health in ["blocked", "unknown"] {
        store
            .record_failure("synthetic", health, START + 1000, START + 60_000)
            .unwrap();
        let report = inspect(&config, 120, START + 1500).unwrap();
        assert!(!report.healthy);
        assert!(report.checks[0].issues.contains(&health));
        assert_eq!(report.checks[0].last_success_at_ms, Some(START + 500));
    }
    let connection = Connection::open(&config.map_database).unwrap();
    connection.execute("UPDATE map_monitor_watches SET health='ok',last_success_at_ms=?1,last_observed_at_ms=?1",[START+1501]).unwrap();
    let future = inspect(&config, 120, START + 1500).unwrap();
    assert!(!future.healthy && future.checks[0].issues.contains(&"future_timestamp"));
    assert_eq!(future.checks[0].age_ms, None);
    connection
        .execute(
            "UPDATE map_monitor_watches SET last_success_at_ms=0,last_observed_at_ms=0",
            [],
        )
        .unwrap();
    assert!(
        inspect(&config, 120, START + 1500).unwrap().checks[0]
            .issues
            .contains(&"invalid_state")
    );
}

#[test]
fn changed_or_missing_enabled_watches_cannot_reuse_old_green_state() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&dir.path().join("monitor.sqlite"));
    let _store = seeded(&config);
    config.map_watches[0].name = "New configuration".into();
    let changed = inspect(&config, 120, START + 1500).unwrap();
    assert!(!changed.healthy);
    assert!(changed.checks[0].issues.contains(&"config_changed"));
    config.map_watches[0].name = "Synthetic event".into();
    let mut second = config.map_watches[0].clone();
    second.id = "missing".into();
    second.section_id = "s_13".into();
    config.map_watches.push(second);
    let partial = inspect(&config, 120, START + 1500).unwrap();
    assert!(!partial.healthy);
    assert_eq!(partial.enabled_watches, 2);
    assert!(partial.checks[0].healthy);
    assert_eq!(partial.checks[1].issues, vec!["not_initialized"]);
    config.map_watches[1].enabled = false;
    let enabled_only = inspect(&config, 120, START + 1500).unwrap();
    assert!(enabled_only.healthy && enabled_only.checks.len() == 1);
}

#[test]
fn no_enabled_watches_is_inactive_and_a_persisted_disabled_watch_is_unhealthy() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&dir.path().join("monitor.sqlite"));
    let mut store = seeded(&config);
    store.reconcile(&[], &config.discord, START + 1000).unwrap();
    let disabled = inspect(&config, 120, START + 1500).unwrap();
    assert!(!disabled.healthy && disabled.checks[0].issues.contains(&"disabled"));
    config.map_watches[0].enabled = false;
    let inactive = inspect(&config, 120, START + 1500).unwrap();
    assert!(!inactive.healthy && inactive.status == "inactive" && inactive.checks.is_empty());
}

#[test]
fn pending_age_and_persisted_cooldowns_are_reported_without_mutation() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = config(&dir.path().join("monitor.sqlite"));
    config.alert_max_age_secs = 1;
    let mut store = seeded(&config);
    store
        .set_cooldown("host:ticketmaster.cz", START + 60_000)
        .unwrap();
    store
        .set_cooldown("discord:delivery", START + 1000)
        .unwrap();
    let report = inspect(&config, 120, START + 1500).unwrap();
    assert!(report.healthy); // Cooldowns do not invalidate a fresh source observation.
    assert_eq!(report.checks[0].pending_alerts, 0);
    assert_eq!(report.checks[0].expired_pending_alerts, 1);
    assert_eq!(report.checks[0].expired_alerts, 0);
    assert_eq!(report.cooldowns[0].scope, "discord:delivery");
    assert_eq!(report.cooldowns[0].remaining_ms, 0);
    assert_eq!(report.cooldowns[1].remaining_ms, 58_500);
    assert_eq!(store.expire(START + 1500, 1000).unwrap(), 1);
    let expired = inspect(&config, 120, START + 1500).unwrap();
    assert_eq!(expired.checks[0].expired_pending_alerts, 0);
    assert_eq!(expired.checks[0].expired_alerts, 1);
}

#[test]
fn legacy_or_unrelated_schema_is_rejected_without_migration() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir.path().join("monitor.sqlite"));
    drop(seeded(&config));
    let connection = Connection::open(&config.map_database).unwrap();
    connection.execute_batch("DROP TABLE map_monitor_outbox; DROP TABLE map_monitor_watches; DROP TABLE map_monitor_cooldowns; DROP TABLE map_monitor_meta; UPDATE map_scan_meta SET version=1;").unwrap();
    drop(connection);
    let before = std::fs::read(&config.map_database).unwrap();
    assert!(inspect(&config, 120, START + 1500).is_err());
    assert_eq!(std::fs::read(&config.map_database).unwrap(), before);
    let connection = Connection::open(&config.map_database).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT version FROM map_scan_meta", [], |row| row
                .get::<_, i64>(0))
            .unwrap(),
        1
    );
    connection
        .execute_batch("CREATE TABLE unrelated(payload TEXT);")
        .unwrap();
    assert!(inspect(&config, 120, START + 1500).is_err());
}

#[test]
fn corrupt_oversized_stored_watches_and_fingerprints_fail_closed() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir.path().join("monitor.sqlite"));
    let _store = seeded(&config);
    let connection = Connection::open(&config.map_database).unwrap();
    let raw: String = connection
        .query_row("SELECT watch_json FROM map_monitor_watches", [], |row| {
            row.get(0)
        })
        .unwrap();
    for invalid in ["{".into(), "x".repeat(65_537)] {
        connection
            .execute("UPDATE map_monitor_watches SET watch_json=?1", [invalid])
            .unwrap();
        let report = inspect(&config, 120, START + 1500).unwrap();
        assert!(!report.healthy && report.checks[0].issues.contains(&"invalid_state"));
    }
    connection
        .execute(
            "UPDATE map_monitor_watches SET watch_json=?1,fingerprint=?2",
            [raw, "x".repeat(65)],
        )
        .unwrap();
    let report = inspect(&config, 120, START + 1500).unwrap();
    assert!(!report.healthy && report.checks[0].issues.contains(&"invalid_state"));
}

#[test]
fn locked_database_is_an_error_not_a_green_or_uninitialized_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir.path().join("monitor.sqlite"));
    drop(seeded(&config));
    let connection = Connection::open(&config.map_database).unwrap();
    connection
        .execute_batch("PRAGMA journal_mode=DELETE; BEGIN EXCLUSIVE;")
        .unwrap();
    assert!(inspect(&config, 120, START + 1500).is_err());
    connection.execute_batch("ROLLBACK;").unwrap();
    assert!(inspect(&config, 120, START + 1500).unwrap().healthy);
}

#[test]
fn max_age_and_current_time_are_validated_before_filesystem_access() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(&dir.path().join("absent/monitor.sqlite"));
    for age in [0, 31_536_001, u64::MAX] {
        assert!(inspect(&config, age, START).is_err());
    }
    assert!(inspect(&config, 120, 0).is_err());
    assert!(!config.map_database.parent().unwrap().exists());
}
