use std::path::PathBuf;

use rusqlite::{Connection, params};
use sha2::{Digest, Sha256};
use tempfile::TempDir;
use ticket_watch::{
    config::Watch,
    market_history::{
        self, EventMetadata, HistoryEntry, HistoryExport, HistorySeat, SampleInput, SeriesKind,
        SourceMetadata,
    },
    model::{
        Availability, Evidence, FeeScope, InventoryKind, Observation, Offer, PriceScope,
        QuantityKind, TicketCategory,
    },
};

const URL: &str = "https://www.ticketmaster.cz/event/example-show/123456789";
struct Fixture {
    _dir: TempDir,
    path: PathBuf,
    connection: Connection,
}
impl Fixture {
    fn new() -> Self {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("history.sqlite");
        let connection = Connection::open(&path).unwrap();
        market_history::initialize(&connection).unwrap();
        Self {
            _dir: directory,
            path,
            connection,
        }
    }
    fn record(&mut self, sample: &SampleInput) {
        let transaction = self.connection.transaction().unwrap();
        market_history::record_sample(&transaction, &event(), &source(), sample).unwrap();
        transaction.commit().unwrap();
    }
    fn export(&self) -> HistoryExport {
        market_history::read_export(&self.path, 10_000, None, None).unwrap()
    }
}
fn event() -> EventMetadata {
    EventMetadata {
        event_url: URL.into(),
        name: "Example show".into(),
        artist: "Example artist".into(),
        country: "CZ".into(),
        city: Some("Praha".into()),
    }
}
fn source() -> SourceMetadata {
    SourceMetadata {
        watch_id: "selected".into(),
        scope: "event_page".into(),
        interval_secs: 60,
        config_fingerprint: "a".repeat(64),
    }
}
fn entry(id: &str, price: Option<i64>) -> HistoryEntry {
    HistoryEntry {
        entry_id: id.into(),
        ticket_type: Some("Standard".into()),
        inventory_kind: InventoryKind::Primary,
        currency: Some("CZK".into()),
        price_scope: PriceScope::PerTicket,
        fees: FeeScope::Included,
        quantity_kind: QuantityKind::Listing,
        kind: SeriesKind::Offer,
        seat: None,
        min_price_minor: price,
        max_price_minor: price,
        quantity: Some(2),
    }
}
fn sample(at: i64, entries: Vec<HistoryEntry>) -> SampleInput {
    SampleInput {
        observed_at: at,
        availability: Availability::Available,
        evidence: Evidence::Offers,
        complete: false,
        entries,
    }
}
fn observation(at: i64) -> Observation {
    Observation {
        watch_id: "selected".into(),
        observed_at: at,
        availability: Availability::Available,
        evidence: Evidence::Offers,
        complete: false,
        offers: vec![Offer {
            key: "provider-secret-key".into(),
            ticket_type: Some("Standard".into()),
            inventory: Some("private raw inventory".into()),
            inventory_kind: InventoryKind::Primary,
            price_text: Some("https://private.invalid/checkout?token=private".into()),
            price_minor: Some(10001),
            currency: Some("CZK".into()),
            quantity: Some(2),
            quantity_kind: QuantityKind::Listing,
            price_scope: PriceScope::PerTicket,
            fees: FeeScope::Unknown,
            ..Offer::default()
        }],
        ticket_categories: vec![],
        adapter: Some("regional-v1".into()),
        detail: "https://private.invalid/session?token=private".into(),
    }
}
fn watch() -> Watch {
    Watch {
        id: "selected".into(),
        url: URL.into(),
        name: "Example show".into(),
        artist: "Example artist".into(),
        country: "CZ".into(),
        city: Some("Praha".into()),
        ..Watch::default()
    }
}

#[test]
fn per_seat_prices_are_distinct_from_other_seats_and_the_aggregate_projection() {
    let mut fixture = Fixture::new();
    let mut aggregate_a = entry("aggregate:provider-private-a", Some(5000));
    aggregate_a.kind = SeriesKind::DisplayedSeat;
    aggregate_a.quantity_kind = QuantityKind::Unknown;
    let mut aggregate_b = aggregate_a.clone();
    aggregate_b.entry_id = "aggregate:provider-private-b".into();
    let mut seat_a = aggregate_a.clone();
    seat_a.entry_id = "seat:provider-private-a".into();
    seat_a.seat = Some(HistorySeat {
        seat_key: "a".repeat(64),
        section: "  Lower   Bowl ".into(),
        row: "A".into(),
        number: "1".into(),
    });
    let mut seat_b = aggregate_b.clone();
    seat_b.entry_id = "seat:provider-private-b".into();
    seat_b.seat = Some(HistorySeat {
        seat_key: "B".repeat(64),
        section: "Lower Bowl".into(),
        row: "A".into(),
        number: "2".into(),
    });
    let mut repeated_a = seat_a.clone();
    repeated_a.entry_id = "different-transient-provider-row".into();
    let mut observed = sample(
        100,
        vec![aggregate_a, aggregate_b, seat_a, seat_b.clone(), repeated_a],
    );
    observed.availability = Availability::Unknown;
    observed.evidence = Evidence::None;
    fixture.record(&observed);
    let export = fixture.export();
    let series = &export.events[0].sources[0].samples[0].series;
    assert_eq!(series.len(), 3);
    let aggregate = series.iter().find(|series| series.seat.is_none()).unwrap();
    assert_eq!(aggregate.observed_count, 2);
    assert_eq!(aggregate.min_price_minor, Some(5000));
    let seats = series
        .iter()
        .filter(|series| series.seat.is_some())
        .collect::<Vec<_>>();
    assert_eq!(seats.len(), 2);
    assert_ne!(seats[0].series_id, seats[1].series_id);
    assert!(seats.iter().all(|series| series.observed_count == 1
        && series.reported_quantity.is_none()
        && series.seat.as_ref().unwrap().section == "Lower Bowl"));
    assert!(
        seats
            .iter()
            .any(|series| series.seat.as_ref().unwrap().seat_key == "b".repeat(64))
    );
    let raw = serde_json::to_string(&export).unwrap();
    assert!(!raw.contains("provider-private"));
    assert!(!raw.contains("transient-provider"));

    // A price observation after a missing or blocked sample never fills the gap.
    observed.observed_at = 101;
    observed.entries.clear();
    fixture.record(&observed);
    observed.observed_at = 102;
    observed.availability = Availability::Blocked;
    observed.entries.push(seat_b.clone());
    fixture.record(&observed);
    observed.observed_at = 103;
    observed.availability = Availability::Unknown;
    seat_b.min_price_minor = Some(6000);
    seat_b.max_price_minor = Some(6000);
    observed.entries = vec![seat_b];
    fixture.record(&observed);
    let export = fixture.export();
    let samples = &export.events[0].sources[0].samples;
    assert_eq!(samples.len(), 4);
    assert!(samples[1].series.is_empty());
    assert!(samples[2].series.is_empty());
    assert_eq!(samples[3].series[0].min_price_minor, Some(6000));
    assert_eq!(
        samples[3].series[0].series_id,
        seats
            .iter()
            .find(|series| series.seat.as_ref().unwrap().number == "2")
            .unwrap()
            .series_id
    );
}

#[test]
fn invalid_seat_identity_is_dropped_and_legacy_fields_default_to_aggregate() {
    let mut fixture = Fixture::new();
    let base = HistorySeat {
        seat_key: "a".repeat(64),
        section: "101".into(),
        row: "A".into(),
        number: "1".into(),
    };
    let mut invalid = Vec::new();
    for (index, mut seat) in (0..7).map(|index| (index, base.clone())) {
        match index {
            0 => seat.seat_key = "raw-provider-id".into(),
            1 => seat.section = String::new(),
            2 => seat.row = "unknown".into(),
            3 => seat.number = "?".into(),
            4 => seat.row = "A\0".into(),
            5 => seat.section = "https://private.invalid/session".into(),
            _ => seat.number = "1".repeat(257),
        }
        let mut candidate = entry(&format!("invalid-{index}"), Some(100));
        candidate.kind = SeriesKind::DisplayedSeat;
        candidate.seat = Some(seat);
        invalid.push(candidate);
    }
    let mut wrong_kind = entry("offer-with-seat", Some(100));
    wrong_kind.seat = Some(base.clone());
    invalid.push(wrong_kind);
    fixture.record(&sample(100, invalid));
    assert!(
        fixture.export().events[0].sources[0].samples[0]
            .series
            .is_empty()
    );

    let mut legacy = serde_json::to_value(entry("legacy", Some(100))).unwrap();
    legacy.as_object_mut().unwrap().remove("seat");
    let legacy: HistoryEntry = serde_json::from_value(legacy).unwrap();
    assert!(legacy.seat.is_none());
    fixture.record(&sample(101, vec![legacy]));
    let export = fixture.export();
    let mut series =
        serde_json::to_value(&export.events[0].sources[0].samples[1].series[0]).unwrap();
    series.as_object_mut().unwrap().remove("seat");
    let series: market_history::HistorySeries = serde_json::from_value(series).unwrap();
    assert!(series.seat.is_none());

    let mut first = entry("first", Some(100));
    first.kind = SeriesKind::DisplayedSeat;
    first.quantity_kind = QuantityKind::Unknown;
    first.seat = Some(base);
    let mut conflicting = first.clone();
    conflicting.entry_id = "second".into();
    conflicting.min_price_minor = Some(200);
    conflicting.max_price_minor = Some(200);
    fixture.record(&sample(102, vec![first.clone(), conflicting]));
    assert!(
        fixture.export().events[0].sources[0].samples[2]
            .series
            .is_empty()
    );
    let mut invalid_price = first.clone();
    invalid_price.entry_id = "third".into();
    invalid_price.min_price_minor = Some(-1);
    invalid_price.max_price_minor = Some(-1);
    fixture.record(&sample(103, vec![first.clone(), invalid_price]));
    assert!(
        fixture.export().events[0].sources[0].samples[3]
            .series
            .is_empty()
    );
    let mut invalid_label = first.clone();
    invalid_label.entry_id = "fourth".into();
    invalid_label.seat.as_mut().unwrap().row = "unknown".into();
    fixture.record(&sample(104, vec![invalid_label, first]));
    assert!(
        fixture.export().events[0].sources[0].samples[4]
            .series
            .is_empty()
    );
}

#[test]
fn existing_pre_seat_schema_v1_aggregate_hashes_still_export_without_migration() {
    let mut fixture = Fixture::new();
    let legacy = entry("legacy", Some(100));
    fixture.record(&sample(100, vec![legacy.clone()]));
    // The exact pre-seat grouping tuple has eight dimensions and no seat/null
    // element. This reproduces a previously persisted schema-v1 series ID.
    let old_hash = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                &legacy.ticket_type,
                legacy.inventory_kind,
                &legacy.currency,
                legacy.price_scope,
                Option::<u32>::None,
                legacy.fees,
                legacy.quantity_kind,
                &legacy.kind
            ))
            .unwrap()
        )
    );
    let raw: String = fixture
        .connection
        .query_row("SELECT sample_json FROM market_samples", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut old_sample: serde_json::Value = serde_json::from_str(&raw).unwrap();
    old_sample["series"][0]["series_id"] = old_hash.clone().into();
    old_sample["series"][0]
        .as_object_mut()
        .unwrap()
        .remove("seat");
    fixture
        .connection
        .execute(
            "UPDATE market_samples SET sample_json=?1",
            [old_sample.to_string()],
        )
        .unwrap();
    let before = std::fs::read(&fixture.path).unwrap();
    let export = fixture.export();
    let series = &export.events[0].sources[0].samples[0].series[0];
    assert_eq!(series.series_id, old_hash);
    assert!(series.seat.is_none());
    assert_eq!(series.min_price_minor, Some(100));
    assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
    fixture.record(&sample(101, vec![legacy]));
    assert_eq!(
        fixture.export().events[0].sources[0].samples[1].series[0].series_id,
        old_hash
    );
}

#[test]
fn history_persists_restart_and_exact_replay_but_retains_same_second_changes() {
    let mut fixture = Fixture::new();
    let first = sample(
        100,
        vec![
            entry("a", Some(10001)),
            entry("b", Some(10002)),
            entry("a", Some(10001)),
        ],
    );
    fixture.record(&first);
    fixture.record(&first);
    fixture.record(&sample(100, vec![entry("a", Some(9000))]));
    fixture.record(&sample(101, vec![entry("a", Some(9000))]));
    let output = fixture.export();
    let event = &output.events[0];
    assert_eq!(
        (event.first_observed_at, event.last_observed_at),
        (100, 101)
    );
    assert_eq!(event.sources.len(), 1);
    let samples = &event.sources[0].samples;
    assert_eq!(samples.len(), 3);
    assert_eq!(samples[0].observed_at, samples[1].observed_at);
    let series = &samples[0].series[0];
    assert_eq!(series.observed_count, 2);
    assert_eq!(series.reported_quantity, Some(4));
    assert_eq!(
        (
            series.min_price_minor,
            series.max_price_minor,
            series.median_price_minor
        ),
        (Some(10001), Some(10002), Some(10001.5))
    );
    assert_eq!(samples[1].series[0].median_price_minor, Some(9000.0));
    let reopened = Connection::open(&fixture.path).unwrap();
    market_history::initialize(&reopened).unwrap();
    assert_eq!(output, fixture.export());
    assert_eq!(
        serde_json::from_str::<HistoryExport>(&serde_json::to_string(&output).unwrap()).unwrap(),
        output
    );
}

#[test]
fn grouping_preserves_every_channel_currency_scope_fee_quantity_type_and_kind_dimension() {
    let mut fixture = Fixture::new();
    let baseline = entry("base", Some(1000));
    let mut entries = vec![baseline.clone()];
    let mut changed = baseline.clone();
    changed.entry_id = "resale".into();
    changed.inventory_kind = InventoryKind::Resale;
    entries.push(changed);
    let mut changed = baseline.clone();
    changed.entry_id = "unknown-channel".into();
    changed.inventory_kind = InventoryKind::Unknown;
    entries.push(changed);
    let mut changed = baseline.clone();
    changed.entry_id = "currency".into();
    changed.currency = Some("EUR".into());
    entries.push(changed);
    let mut changed = baseline.clone();
    changed.entry_id = "unknown-currency".into();
    changed.currency = None;
    entries.push(changed);
    let mut changed = baseline.clone();
    changed.entry_id = "scope".into();
    changed.price_scope = PriceScope::Total;
    entries.push(changed);
    let mut changed = baseline.clone();
    changed.entry_id = "unknown-scope".into();
    changed.price_scope = PriceScope::Unknown;
    entries.push(changed);
    let mut changed = baseline.clone();
    changed.entry_id = "fees".into();
    changed.fees = FeeScope::Excluded;
    entries.push(changed);
    let mut changed = baseline.clone();
    changed.entry_id = "quantity-kind".into();
    changed.quantity_kind = QuantityKind::Remaining;
    entries.push(changed);
    let mut changed = baseline.clone();
    changed.entry_id = "type".into();
    changed.ticket_type = Some("VIP".into());
    entries.push(changed);
    let mut changed = baseline;
    changed.entry_id = "seat".into();
    changed.kind = SeriesKind::DisplayedSeat;
    changed.quantity_kind = QuantityKind::Unknown;
    entries.push(changed);
    fixture.record(&sample(100, entries));
    let export = fixture.export();
    let series = &export.events[0].sources[0].samples[0].series;
    assert_eq!(series.len(), 11);
    assert!(series.iter().all(|s| s.observed_count == 1));
    assert_eq!(
        series
            .iter()
            .find(|s| s.kind == SeriesKind::DisplayedSeat)
            .unwrap()
            .reported_quantity,
        None
    );
}

#[test]
fn total_prices_require_known_bundle_sizes_and_do_not_compare_two_with_four_tickets() {
    let mut fixture = Fixture::new();
    let mut two = entry("two", Some(20000));
    two.price_scope = PriceScope::Total;
    let mut four = entry("four", Some(36000));
    four.price_scope = PriceScope::Total;
    four.quantity = Some(4);
    let mut unknown = entry("unknown", Some(100));
    unknown.price_scope = PriceScope::Total;
    unknown.quantity = None;
    let mut unknown_kind = entry("unknown-kind", Some(200));
    unknown_kind.price_scope = PriceScope::Total;
    unknown_kind.quantity_kind = QuantityKind::Unknown;
    fixture.record(&sample(100, vec![two, four, unknown, unknown_kind]));
    let export = fixture.export();
    let series = &export.events[0].sources[0].samples[0].series;
    assert_eq!(series.len(), 4);
    assert_eq!(
        series
            .iter()
            .find(|s| s.price_quantity == Some(2))
            .unwrap()
            .min_price_minor,
        Some(20000)
    );
    assert_eq!(
        series
            .iter()
            .find(|s| s.price_quantity == Some(4))
            .unwrap()
            .min_price_minor,
        Some(36000)
    );
    assert!(series.iter().filter(|s| s.price_quantity.is_none()).all(
        |s| s.min_price_minor.is_none()
            && s.max_price_minor.is_none()
            && s.median_price_minor.is_none()
            && s.reported_quantity.is_none()
            && s.observed_count == 1
    ));
    let mut remaining = entry("remaining", Some(5000));
    remaining.price_scope = PriceScope::Total;
    remaining.quantity_kind = QuantityKind::Remaining;
    remaining.quantity = Some(8);
    fixture.record(&sample(101, vec![remaining]));
    let export = fixture.export();
    let remaining = &export.events[0].sources[0].samples[1].series[0];
    assert_eq!(remaining.price_quantity, None);
    assert_eq!(remaining.min_price_minor, None);
    assert_eq!(remaining.max_price_minor, None);
    assert_eq!(remaining.median_price_minor, None);
    assert_eq!(remaining.reported_quantity, Some(8));
}

#[test]
fn ranges_missing_prices_invalid_rows_and_ambiguous_duplicate_ids_never_invent_prices_or_stock() {
    let mut fixture = Fixture::new();
    let mut ranged = entry("range", Some(1000));
    ranged.max_price_minor = Some(3000);
    let mut missing = entry("missing", None);
    missing.quantity = None;
    fixture.record(&sample(
        100,
        vec![
            ranged,
            missing,
            entry("negative", Some(-1)),
            entry("too-large", Some(i64::MAX)),
            entry("conflict", Some(400)),
            entry("conflict", Some(500)),
        ],
    ));
    let export = fixture.export();
    let series = &export.events[0].sources[0].samples[0].series[0];
    assert_eq!(series.observed_count, 2);
    assert_eq!(series.min_price_minor, Some(1000));
    assert_eq!(series.max_price_minor, Some(3000));
    assert_eq!(series.median_price_minor, None);
    assert_eq!(series.reported_quantity, None);
    let mut zero = entry("zero", Some(0));
    zero.quantity = Some(0);
    fixture.record(&sample(101, vec![zero]));
    let export = fixture.export();
    let series = &export.events[0].sources[0].samples[1].series[0];
    assert_eq!(series.min_price_minor, Some(0));
    assert_eq!(series.reported_quantity, None);
    let mut selectable = entry("capacity", Some(1));
    selectable.quantity_kind = QuantityKind::Selectable;
    selectable.quantity = Some(99);
    fixture.record(&sample(102, vec![selectable]));
    assert_eq!(
        fixture.export().events[0].sources[0].samples[2].series[0].reported_quantity,
        None
    );
}

#[test]
fn unknown_and_blocked_observations_preserve_gaps_in_same_source_as_valid_adapter() {
    let mut fixture = Fixture::new();
    let transaction = fixture.connection.transaction().unwrap();
    let selected = watch();
    market_history::record_observation(&transaction, &selected, &observation(100)).unwrap();
    let mut gap = observation(101);
    gap.adapter = None;
    gap.availability = Availability::Blocked;
    gap.evidence = Evidence::None;
    market_history::record_observation(&transaction, &selected, &gap).unwrap();
    gap.observed_at = 102;
    gap.availability = Availability::Unknown;
    market_history::record_observation(&transaction, &selected, &gap).unwrap();
    gap.observed_at = 103;
    gap.availability = Availability::Unavailable;
    gap.evidence = Evidence::Offers;
    market_history::record_observation(&transaction, &selected, &gap).unwrap();
    transaction.commit().unwrap();
    let export = fixture.export();
    let source = &export.events[0].sources[0];
    assert_eq!(export.events[0].sources.len(), 1);
    assert_eq!(source.samples.len(), 4);
    assert!(!source.samples[0].series.is_empty());
    assert!(
        source.samples[1..]
            .iter()
            .all(|sample| sample.series.is_empty())
    );
    assert_eq!(source.samples[1].availability, Availability::Blocked);
    assert!(!source.samples[3].complete);
}

#[test]
fn filters_do_not_hide_observed_prices_and_private_raw_fields_are_absent() {
    let mut fixture = Fixture::new();
    let mut selected = watch();
    selected.max_price_minor = Some(1);
    selected.min_quantity = Some(100);
    selected.url.push_str("?language=cs#tickets");
    let transaction = fixture.connection.transaction().unwrap();
    market_history::record_observation(&transaction, &selected, &observation(100)).unwrap();
    transaction.commit().unwrap();
    let export = fixture.export();
    assert_eq!(
        export.events[0].sources[0].samples[0].series[0].min_price_minor,
        Some(10001)
    );
    let raw = serde_json::to_string(&export).unwrap();
    for absent in [
        "provider-secret-key",
        "private.invalid",
        "private raw inventory",
        "?language",
        "#tickets",
    ] {
        assert!(!raw.contains(absent), "{absent}");
    }
}

#[test]
fn displayed_categories_keep_ranges_separate_from_offer_prices_and_selection_capacity() {
    let mut fixture = Fixture::new();
    let mut observed = observation(100);
    observed.availability = Availability::Unknown;
    observed.evidence = Evidence::None;
    observed.offers.clear();
    observed.ticket_categories.push(TicketCategory {
        key: "category".into(),
        ticket_type: "Regular".into(),
        inventory_kind: InventoryKind::Unknown,
        section: None,
        row: None,
        price_text: Some("100–200 CZK".into()),
        price_min_minor: Some(10000),
        price_max_minor: Some(20000),
        currency: Some("CZK".into()),
        selectable_quantity: Some(12),
    });
    let transaction = fixture.connection.transaction().unwrap();
    market_history::record_observation(&transaction, &watch(), &observed).unwrap();
    transaction.commit().unwrap();
    let export = fixture.export();
    let sample = &export.events[0].sources[0].samples[0];
    let series = &sample.series[0];
    assert_eq!(sample.availability, Availability::Unknown);
    assert_eq!(series.kind, SeriesKind::DisplayedCategory);
    assert_eq!(series.price_scope, PriceScope::Unknown);
    assert_eq!(series.median_price_minor, None);
    assert_eq!(series.reported_quantity, None);
    assert_eq!(series.observed_count, 1);
}

#[test]
fn source_changes_stay_separate_and_aliases_keep_one_event_identity() {
    let mut fixture = Fixture::new();
    let transaction = fixture.connection.transaction().unwrap();
    let first = watch();
    market_history::record_observation(&transaction, &first, &observation(100)).unwrap();
    let mut changed = first.clone();
    changed.interval_secs = 5;
    market_history::record_observation(&transaction, &changed, &observation(101)).unwrap();
    changed.url = "https://ticketmaster.cz/event/renamed/123456789?language=cs".into();
    changed.max_price_minor = Some(50000);
    market_history::record_observation(&transaction, &changed, &observation(102)).unwrap();
    changed.id = "second-watch".into();
    let mut observed = observation(103);
    observed.watch_id = changed.id.clone();
    market_history::record_observation(&transaction, &changed, &observed).unwrap();
    transaction.commit().unwrap();
    let export = fixture.export();
    assert_eq!(export.events.len(), 1);
    assert_eq!(export.events[0].sources.len(), 4);
    assert_eq!(export.events[0].first_observed_at, 100);
    assert_eq!(export.events[0].last_observed_at, 103);
}

#[test]
fn legacy_database_reads_without_mutation_and_additive_initialization_preserves_unrelated_tables() {
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("legacy.sqlite");
    let connection = Connection::open(&path).unwrap();
    connection
        .execute_batch(
            "CREATE TABLE preserved(value TEXT); INSERT INTO preserved VALUES ('existing');",
        )
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(
        market_history::read_export(&path, 1000, None, None)
            .unwrap()
            .events
            .is_empty()
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    market_history::initialize(&connection).unwrap();
    market_history::initialize(&connection).unwrap();
    assert_eq!(
        connection
            .query_row("SELECT value FROM preserved", [], |row| row
                .get::<_, String>(0))
            .unwrap(),
        "existing"
    );
    assert_eq!(
        connection
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table' AND name LIKE 'market_%'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        4
    );
    let missing = directory.path().join("missing.sqlite");
    assert!(market_history::read_export(&missing, 1000, None, None).is_err());
    assert!(!missing.exists());
}

#[test]
fn unsupported_or_partial_history_schema_is_rejected_without_mutation() {
    let fixture = Fixture::new();
    fixture
        .connection
        .execute("UPDATE market_history_meta SET schema_version=2", [])
        .unwrap();
    let before = std::fs::read(&fixture.path).unwrap();
    assert!(market_history::initialize(&fixture.connection).is_err());
    assert!(market_history::read_export(&fixture.path, 1000, None, None).is_err());
    assert_eq!(std::fs::read(&fixture.path).unwrap(), before);
    let partial = Connection::open_in_memory().unwrap();
    partial
        .execute_batch("CREATE TABLE market_events(event_id TEXT)")
        .unwrap();
    assert!(market_history::initialize(&partial).is_err());
    assert_eq!(
        partial
            .query_row(
                "SELECT COUNT(*) FROM sqlite_master WHERE type='table'",
                [],
                |row| row.get::<_, u32>(0)
            )
            .unwrap(),
        1
    );
}

#[test]
fn rollback_is_atomic_and_empty_manual_metadata_does_not_erase_known_event_labels() {
    let mut fixture = Fixture::new();
    fixture.record(&sample(100, vec![entry("a", Some(100))]));
    {
        let transaction = fixture.connection.transaction().unwrap();
        market_history::record_sample(&transaction, &event(), &source(), &sample(101, vec![]))
            .unwrap();
    }
    assert_eq!(fixture.export().events[0].sources[0].samples.len(), 1);
    let transaction = fixture.connection.transaction().unwrap();
    let empty = EventMetadata {
        event_url: URL.into(),
        name: String::new(),
        artist: String::new(),
        country: String::new(),
        city: None,
    };
    let mut direct = source();
    direct.watch_id = "manual".into();
    direct.interval_secs = 0;
    direct.scope = "map_section:A:limit:10".into();
    market_history::record_sample(&transaction, &empty, &direct, &sample(102, vec![])).unwrap();
    transaction.commit().unwrap();
    let export = fixture.export();
    assert_eq!(export.events[0].name, "Example show");
    assert_eq!(export.events[0].artist, "Example artist");
    assert_eq!(export.events[0].city.as_deref(), Some("Praha"));
    assert_eq!(export.events[0].sources.len(), 2);
}

#[test]
fn public_generic_fixture_routes_hash_separately_and_invalid_or_replay_timestamps_are_rejected() {
    let mut fixture = Fixture::new();
    let transaction = fixture.connection.transaction().unwrap();
    let mut generic = watch();
    generic.url = "https://www.ticketmaster.cz/event/synthetic-fixture".into();
    market_history::record_observation(&transaction, &generic, &observation(100)).unwrap();
    market_history::record_observation(&transaction, &generic, &observation(0)).unwrap();
    assert!(market_history::record_observation(&transaction, &generic, &observation(-1)).is_err());
    let mut wrong = observation(100);
    wrong.watch_id = "other".into();
    assert!(market_history::record_observation(&transaction, &generic, &wrong).is_err());
    transaction.commit().unwrap();
    let export = fixture.export();
    assert!(export.events[0].event_id.starts_with("url:"));
    assert_eq!(export.events[0].sources[0].samples.len(), 1);
}

#[test]
fn export_ranges_and_event_filter_apply_before_explicit_sample_limit() {
    let mut fixture = Fixture::new();
    fixture.record(&sample(1, vec![]));
    let transaction = fixture.connection.transaction().unwrap();
    let mut other = event();
    other.event_url = "https://www.ticketmaster.cz/event/other/987654321".into();
    market_history::record_sample(&transaction, &other, &source(), &sample(2, vec![])).unwrap();
    let original_source: String = transaction
        .query_row(
            "SELECT source_id FROM market_sources WHERE event_id='tm:ticketmaster.cz:123456789'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    let raw: String = transaction
        .query_row(
            "SELECT sample_json FROM market_samples WHERE source_id=?1",
            [&original_source],
            |row| row.get(0),
        )
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&raw).unwrap();
    for at in 3..=(market_history::MAX_EXPORT_SAMPLES as i64 + 3) {
        value["observed_at"] = at.into();
        transaction.execute("INSERT INTO market_samples(source_id,observed_at,payload_hash,sample_json) VALUES (?1,?2,?3,?4)",params![original_source,at,at.to_string(),value.to_string()]).unwrap();
    }
    transaction.commit().unwrap();
    assert!(
        market_history::read_export(&fixture.path, 20_000, None, None)
            .unwrap_err()
            .to_string()
            .contains("sample limit")
    );
    let selected =
        market_history::read_export_event(&fixture.path, 20_000, None, None, &other.event_url)
            .unwrap();
    assert_eq!(selected.events.len(), 1);
    assert_eq!(selected.events[0].sources[0].samples.len(), 1);
    let narrow = market_history::read_export(&fixture.path, 20_000, Some(2), Some(3)).unwrap();
    assert_eq!(narrow.events.len(), 2);
    assert!(market_history::read_export(&fixture.path, 100, Some(101), Some(99)).is_err());
}

#[test]
fn stored_corrupt_oversized_or_unsafe_history_is_not_exported() {
    let mut fixture = Fixture::new();
    fixture.record(&sample(100, vec![entry("a", Some(100))]));
    let original: String = fixture
        .connection
        .query_row("SELECT sample_json FROM market_samples", [], |row| {
            row.get(0)
        })
        .unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&original).unwrap();
    value["series"][0]["min_price_minor"] = (-1).into();
    fixture
        .connection
        .execute(
            "UPDATE market_samples SET sample_json=?1",
            [value.to_string()],
        )
        .unwrap();
    assert!(market_history::read_export(&fixture.path, 1000, None, None).is_err());
    fixture
        .connection
        .execute(
            "UPDATE market_samples SET sample_json=?1",
            ["x".repeat(1024 * 1024 + 1)],
        )
        .unwrap();
    assert!(market_history::read_export(&fixture.path, 1000, None, None).is_err());
    fixture
        .connection
        .execute("UPDATE market_samples SET sample_json=?1", [original])
        .unwrap();
    fixture
        .connection
        .execute(
            "UPDATE market_events SET event_url=event_url||'?language=secret'",
            [],
        )
        .unwrap();
    assert!(market_history::read_export(&fixture.path, 1000, None, None).is_err());
}
