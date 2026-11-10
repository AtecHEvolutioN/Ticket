use std::path::Path;

use rusqlite::Connection;
use ticket_watch::{
    config::{DiscordConfig, MapWatch},
    map_inspection::DisplayedSeatQuote,
    map_monitor_store::MapMonitorStore,
    map_scan::{ScanOutcome, SeatScanObservation, SectionScan, StopReason},
    map_scan_store::MapScanStore,
    market_history::{self, HistoryExport, HistorySample, HistorySeries, SeriesKind},
    model::{Availability, Evidence, FeeScope, InventoryKind, PriceScope, QuantityKind},
};

const START: i64 = 1_700_000_000_000;
const URL: &str = "https://www.ticketmaster.cz/event/synthetic-history/12345678";

fn scan(at: i64, section: &str, limit: u32, prices: &[Option<i64>]) -> SectionScan {
    let observations: Vec<_> = prices
        .iter()
        .enumerate()
        .map(|(index, price)| {
            let seat = (index + 1).to_string();
            let seat_id = format!("seat-{seat}");
            SeatScanObservation {
                seat_id: seat_id.clone(),
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
                    seat_id,
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
        event_url: URL.into(),
        event_id: "12345678".into(),
        site_host: "ticketmaster.cz".into(),
        section_id: section.into(),
        section_name: "110".into(),
        started_at_ms: at,
        finished_at_ms: at + 200,
        coverage: "partial_rendered_section".into(),
        limit,
        candidate_count: observations.len() as u32,
        visible_candidate_count: observations.len() as u32,
        attempted_count: observations.len() as u32,
        unattempted_count: 0,
        quoted_count: observations
            .iter()
            .filter(|row| row.quote.is_some())
            .count() as u32,
        stop_reason: StopReason::VisibleCandidatesExhausted,
        observations,
        stock_verified: false,
        adjacency_verified: false,
    }
}

fn export(path: &Path) -> HistoryExport {
    market_history::read_export(path, START / 1000 + 60, None, None).unwrap()
}

fn aggregate(sample: &HistorySample) -> &HistorySeries {
    let entries: Vec<_> = sample
        .series
        .iter()
        .filter(|series| series.seat.is_none())
        .collect();
    assert_eq!(entries.len(), 1);
    entries[0]
}

fn seat<'a>(sample: &'a HistorySample, number: &str) -> &'a HistorySeries {
    sample
        .series
        .iter()
        .find(|series| {
            series
                .seat
                .as_ref()
                .is_some_and(|seat| seat.number == number)
        })
        .unwrap()
}

fn watch() -> MapWatch {
    MapWatch {
        id: "history-section".into(),
        name: "Synthetic history event".into(),
        url: URL.into(),
        section_id: "s_12".into(),
        limit: 20,
        notify_initial: true,
        ..MapWatch::default()
    }
}

fn monitor(path: &Path, watch: &MapWatch) -> MapMonitorStore {
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

fn record(store: &mut MapMonitorStore, scan: &SectionScan) {
    store
        .record_success(
            "history-section",
            scan,
            scan.finished_at_ms,
            scan.finished_at_ms + 60_000,
            300_000,
        )
        .unwrap();
}

fn count(path: &Path, table: &str) -> i64 {
    assert!(
        [
            "map_scan_quotes",
            "map_scan_scopes",
            "market_events",
            "market_sources",
            "market_samples",
            "map_monitor_outbox"
        ]
        .contains(&table)
    );
    Connection::open(path)
        .unwrap()
        .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[test]
fn manual_scan_history_preserves_prices_and_gaps_without_claiming_ticket_stock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let first = scan(START, "s_12", 20, &[Some(100_000), Some(200_000), None]);
    let mut store = MapScanStore::open(&path).unwrap();
    store.record(&first).unwrap();
    drop(store);
    let mut store = MapScanStore::open(&path).unwrap();
    assert!(store.record(&first).unwrap().duplicate_scan);
    // Both valid non-overlapping scans finish in the same second, with different prices.
    store
        .record(&scan(
            START + 250,
            "s_12",
            20,
            &[Some(80_000), Some(160_000)],
        ))
        .unwrap();
    store
        .record(&scan(START + 2000, "s_12", 20, &[None]))
        .unwrap();
    let history = export(&path);
    assert_eq!(history.events.len(), 1);
    let event = &history.events[0];
    assert!(event.name.is_empty() && event.artist.is_empty());
    assert_eq!(event.country, "CZ");
    assert_eq!(event.sources.len(), 1);
    let source = &event.sources[0];
    assert_eq!(source.interval_secs, 0);
    assert_eq!(source.scope, "map_scan:section:s_12:limit:20");
    assert_eq!(source.samples.len(), 3);
    assert_eq!(source.samples[0].observed_at, source.samples[1].observed_at);
    for sample in &source.samples {
        assert_eq!(sample.availability, Availability::Unknown);
        assert!(!sample.complete);
    }
    let series = aggregate(&source.samples[0]);
    assert_eq!(source.samples[0].evidence, Evidence::Metadata);
    assert_eq!(series.kind, SeriesKind::DisplayedSeat);
    assert_eq!(series.inventory_kind, InventoryKind::Resale);
    assert_eq!(series.observed_count, 2);
    assert_eq!(series.reported_quantity, None);
    assert_eq!(series.quantity_kind, QuantityKind::Unknown);
    assert_eq!(series.price_scope, PriceScope::PerTicket);
    assert_eq!(series.fees, FeeScope::Unknown);
    assert_eq!(series.min_price_minor, Some(100_000));
    assert_eq!(series.max_price_minor, Some(200_000));
    assert_eq!(series.median_price_minor, Some(150_000.0));
    assert_eq!(aggregate(&source.samples[1]).min_price_minor, Some(80_000));
    assert!(source.samples[2].series.is_empty());
    assert_eq!(source.samples[2].evidence, Evidence::None);
    assert!(!serde_json::to_string(&history).unwrap().contains("seat-"));
    assert_eq!(count(&path, "market_samples"), 3);
    assert_eq!(count(&path, "map_scan_quotes"), 2);
}

#[test]
fn equal_prices_keep_separate_seat_series_and_the_original_aggregate_count() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut store = MapScanStore::open(&path).unwrap();
    let changes = store
        .record(&scan(START, "s_12", 20, &[Some(100_000), Some(100_000)]))
        .unwrap();
    assert_eq!(changes.changes.len(), 2);
    assert!(!changes.stock_verified && !changes.adjacency_verified);

    let history = export(&path);
    let sample = &history.events[0].sources[0].samples[0];
    assert_eq!(sample.series.len(), 3);
    let aggregate = aggregate(sample);
    assert_eq!(aggregate.observed_count, 2);
    assert_eq!(aggregate.min_price_minor, Some(100_000));
    assert_eq!(aggregate.max_price_minor, Some(100_000));
    let first = seat(sample, "1");
    let second = seat(sample, "2");
    assert_ne!(first.series_id, second.series_id);
    assert_ne!(
        first.seat.as_ref().unwrap().seat_key,
        second.seat.as_ref().unwrap().seat_key
    );
    for individual in [first, second] {
        let labels = individual.seat.as_ref().unwrap();
        assert_eq!(labels.section, "110");
        assert_eq!(labels.row, "12");
        assert_eq!(labels.seat_key.len(), 64);
        assert!(
            labels
                .seat_key
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        );
        assert_eq!(individual.observed_count, 1);
        assert_eq!(individual.min_price_minor, Some(100_000));
        assert_eq!(individual.max_price_minor, Some(100_000));
        assert_eq!(individual.median_price_minor, Some(100_000.0));
    }
    for series in &sample.series {
        assert_eq!(series.kind, SeriesKind::DisplayedSeat);
        assert_eq!(series.reported_quantity, None);
        assert_eq!(series.quantity_kind, QuantityKind::Unknown);
        assert_eq!(series.price_quantity, None);
    }
    assert_eq!(sample.availability, Availability::Unknown);
    assert!(!sample.complete);
    let exported = serde_json::to_string(&history).unwrap();
    assert!(!exported.contains("seat-1") && !exported.contains("seat-2"));
}

#[test]
fn missing_and_unquoted_seats_leave_gaps_then_resume_the_same_price_series_after_restart() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut store = MapScanStore::open(&path).unwrap();
    store
        .record(&scan(START, "s_12", 20, &[Some(100_000), Some(150_000)]))
        .unwrap();
    // Seat 2 has no tooltip in this scan, then is absent from the next scan.
    store
        .record(&scan(START + 1000, "s_12", 20, &[Some(100_000), None]))
        .unwrap();
    store
        .record(&scan(START + 2000, "s_12", 20, &[Some(100_000)]))
        .unwrap();
    assert_eq!(count(&path, "map_scan_quotes"), 2);
    drop(store);
    let mut store = MapScanStore::open(&path).unwrap();
    let changes = store
        .record(&scan(
            START + 3000,
            "s_12",
            20,
            &[Some(100_000), Some(130_000)],
        ))
        .unwrap();
    assert_eq!(changes.changes.len(), 1);
    assert_eq!(changes.changes[0].previous_price_minor, Some(150_000));
    assert!(!changes.stock_verified && !changes.adjacency_verified);
    drop(store);

    let history = export(&path);
    let samples = &history.events[0].sources[0].samples;
    assert_eq!(samples.len(), 4);
    let original = seat(&samples[0], "2");
    let returned = seat(&samples[3], "2");
    assert_eq!(original.series_id, returned.series_id);
    assert_eq!(original.seat, returned.seat);
    assert_eq!(original.min_price_minor, Some(150_000));
    assert_eq!(returned.min_price_minor, Some(130_000));
    for sample in &samples[1..3] {
        assert!(
            sample
                .series
                .iter()
                .all(|series| series.series_id != original.series_id)
        );
        assert_eq!(sample.series.len(), 2);
        assert_eq!(aggregate(sample).observed_count, 1);
        assert_eq!(seat(sample, "1").min_price_minor, Some(100_000));
    }
    for sample in samples {
        assert_eq!(sample.availability, Availability::Unknown);
        assert_eq!(sample.evidence, Evidence::Metadata);
        assert!(!sample.complete);
        assert!(sample.series.iter().all(|series| {
            series.reported_quantity.is_none() && series.quantity_kind == QuantityKind::Unknown
        }));
    }
    assert_eq!(count(&path, "market_samples"), 4);
    assert_eq!(count(&path, "map_scan_quotes"), 2);
}

#[test]
fn seat_keys_are_stable_across_scan_limits_but_scoped_to_event_and_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut store = MapScanStore::open(&path).unwrap();
    store
        .record(&scan(START, "s_12", 20, &[Some(100_000)]))
        .unwrap();
    store
        .record(&scan(START + 1000, "s_12", 30, &[Some(100_000)]))
        .unwrap();
    store
        .record(&scan(START + 2000, "s_13", 20, &[Some(100_000)]))
        .unwrap();
    let mut other_event = scan(START + 3000, "s_12", 20, &[Some(100_000)]);
    other_event.event_id = "87654321".into();
    other_event.event_url = "https://www.ticketmaster.cz/event/synthetic-other/87654321".into();
    store.record(&other_event).unwrap();

    let history = export(&path);
    assert_eq!(history.events.len(), 2);
    let event = history
        .events
        .iter()
        .find(|event| event.event_id == "tm:ticketmaster.cz:12345678")
        .unwrap();
    let key = |scope: &str| {
        let source = event
            .sources
            .iter()
            .find(|source| source.scope == scope)
            .unwrap();
        seat(&source.samples[0], "1")
            .seat
            .as_ref()
            .unwrap()
            .seat_key
            .clone()
    };
    let original = key("map_scan:section:s_12:limit:20");
    assert_eq!(original, key("map_scan:section:s_12:limit:30"));
    assert_ne!(original, key("map_scan:section:s_13:limit:20"));
    let other = history
        .events
        .iter()
        .find(|event| event.event_id == "tm:ticketmaster.cz:87654321")
        .unwrap();
    assert_ne!(
        original,
        seat(&other.sources[0].samples[0], "1")
            .seat
            .as_ref()
            .unwrap()
            .seat_key
    );
}

#[test]
fn map_sections_limits_and_monitor_configuration_have_separate_chart_sources() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut direct = MapScanStore::open(&path).unwrap();
    direct
        .record(&scan(START, "s_12", 20, &[Some(100_000)]))
        .unwrap();
    direct
        .record(&scan(START + 1000, "s_13", 20, &[Some(100_000)]))
        .unwrap();
    direct
        .record(&scan(START + 2000, "s_12", 30, &[Some(100_000)]))
        .unwrap();
    drop(direct);
    let mut watch = watch();
    let mut scheduled = monitor(&path, &watch);
    record(
        &mut scheduled,
        &scan(START + 3000, "s_12", 20, &[Some(100_000)]),
    );
    watch.interval_secs += 1;
    scheduled
        .reconcile(
            std::slice::from_ref(&watch),
            &DiscordConfig::default(),
            START + 4000,
        )
        .unwrap();
    record(
        &mut scheduled,
        &scan(START + 4000, "s_12", 20, &[Some(100_000)]),
    );
    let history = export(&path);
    let sources = &history.events[0].sources;
    assert_eq!(sources.len(), 5);
    assert_eq!(
        sources
            .iter()
            .filter(|source| source.watch_id == "map-scan")
            .count(),
        3
    );
    let monitored = sources
        .iter()
        .filter(|source| source.watch_id == watch.id)
        .collect::<Vec<_>>();
    assert_eq!(monitored.len(), 2);
    assert_ne!(monitored[0].source_id, monitored[1].source_id);
    assert_ne!(monitored[0].interval_secs, monitored[1].interval_secs);
    assert!(sources.iter().all(|source| source.samples.len() == 1));
}

#[test]
fn scheduled_failures_create_gaps_without_erasing_quote_baselines_or_faking_zero_stock() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut store = monitor(&path, &watch());
    record(&mut store, &scan(START, "s_12", 20, &[Some(100_000)]));
    store
        .record_failure("history-section", "blocked", START + 1000, START + 61_000)
        .unwrap();
    store
        .record_failure("history-section", "blocked", START + 1000, START + 61_000)
        .unwrap();
    store
        .record_failure("history-section", "unknown", START + 2000, START + 62_000)
        .unwrap();
    let history = export(&path);
    let samples = &history.events[0].sources[0].samples;
    assert_eq!(samples.len(), 3);
    assert_eq!(samples[1].availability, Availability::Blocked);
    assert_eq!(samples[2].availability, Availability::Unknown);
    for sample in &samples[1..] {
        assert!(sample.series.is_empty());
        assert_eq!(sample.evidence, Evidence::None);
        assert!(!sample.complete);
    }
    assert_eq!(count(&path, "map_scan_quotes"), 1);
    assert!(store.pending(START + 2500, 300_000, 10).unwrap().is_empty());
    let status = store.status().unwrap().remove(0);
    assert_eq!(status.last_success_at_ms, Some(START + 200));
    assert_eq!(status.health, "unknown");
    assert_eq!(count(&path, "market_samples"), 3);
}

#[test]
fn history_write_failure_rolls_back_scan_and_monitor_state_in_the_same_transaction() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("history.sqlite");
    let mut store = monitor(&path, &watch());
    let connection = Connection::open(&path).unwrap();
    connection.execute_batch("CREATE TRIGGER fail_market_sample BEFORE INSERT ON market_samples BEGIN SELECT RAISE(FAIL,'synthetic history failure'); END;").unwrap();
    let scan = scan(START, "s_12", 20, &[Some(100_000)]);
    assert!(
        store
            .record_success(
                "history-section",
                &scan,
                START + 200,
                START + 60_200,
                300_000
            )
            .is_err()
    );
    for table in [
        "map_scan_quotes",
        "map_scan_scopes",
        "market_events",
        "market_sources",
        "market_samples",
        "map_monitor_outbox",
    ] {
        assert_eq!(count(&path, table), 0, "{table}");
    }
    assert_eq!(store.status().unwrap()[0].health, "unobserved");
    connection
        .execute_batch("DROP TRIGGER fail_market_sample")
        .unwrap();
    record(&mut store, &scan);
    connection.execute_batch("CREATE TRIGGER fail_market_sample BEFORE INSERT ON market_samples BEGIN SELECT RAISE(FAIL,'synthetic history failure'); END;").unwrap();
    assert!(
        store
            .record_failure("history-section", "blocked", START + 1000, START + 61_000)
            .is_err()
    );
    assert_eq!(store.status().unwrap()[0].health, "ok");
    assert_eq!(store.status().unwrap()[0].pending_alerts, 1);
    assert_eq!(count(&path, "market_samples"), 1);
}

#[test]
fn map_schema_migration_is_additive_and_partial_market_schema_is_refused() {
    for legacy_version in [1, 2] {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("history.sqlite");
        let mut store = MapScanStore::open(&path).unwrap();
        store
            .record(&scan(START, "s_12", 20, &[Some(100_000)]))
            .unwrap();
        drop(store);
        let connection = Connection::open(&path).unwrap();
        connection.execute_batch("DROP TABLE market_samples; DROP TABLE market_sources; DROP TABLE market_events; DROP TABLE market_history_meta;").unwrap();
        if legacy_version == 1 {
            connection.execute_batch("DROP TABLE map_monitor_outbox; DROP TABLE map_monitor_watches; DROP TABLE map_monitor_cooldowns; DROP TABLE map_monitor_meta; UPDATE map_scan_meta SET version=1;").unwrap();
        }
        assert!(export(&path).events.is_empty());
        let mut reopened = MapScanStore::open(&path).unwrap();
        assert_eq!(count(&path, "map_scan_quotes"), 1);
        assert!(
            export(&path).events.is_empty(),
            "migration does not fabricate historical samples"
        );
        reopened
            .record(&scan(START + 1000, "s_12", 20, &[Some(90_000)]))
            .unwrap();
        assert_eq!(export(&path).events[0].sources[0].samples.len(), 1);
        drop(reopened);
        connection
            .execute_batch("DROP TABLE market_samples")
            .unwrap();
        assert!(MapScanStore::open(&path).is_err());
        assert_eq!(count(&path, "map_scan_quotes"), 1);
        assert_eq!(connection.query_row("SELECT count(*) FROM sqlite_master WHERE type='table' AND name='market_samples'", [], |row| row.get::<_, i64>(0)).unwrap(), 0);
    }
}
