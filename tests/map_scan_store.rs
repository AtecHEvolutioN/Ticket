use std::path::Path;

use rusqlite::{Connection, params};
use ticket_watch::{
    map_inspection::DisplayedSeatQuote,
    map_scan::{ScanOutcome, SeatScanObservation, SectionScan, StopReason},
    map_scan_store::{COMPARISON_MAX_AGE_MS, ChangeKind, MapScanStore},
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

#[test]
fn restart_replay_and_equivalent_price_format_do_not_duplicate_changes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nested/history.sqlite");
    let first = scan(
        START,
        &[("seat-1", Some(100_000)), ("seat-2", Some(120_000))],
    );
    let changes = MapScanStore::open(&path).unwrap().record(&first).unwrap();
    assert_eq!(changes.changes.len(), 2);
    assert!(
        changes
            .changes
            .iter()
            .all(|c| c.kind == ChangeKind::FirstObserved)
    );
    assert!(!changes.stock_verified && !changes.adjacency_verified);
    let mut reopened = MapScanStore::open(&path).unwrap();
    let duplicate = reopened.record(&first).unwrap();
    assert!(duplicate.duplicate_scan && duplicate.changes.is_empty());
    let mut next = scan(
        START + 1000,
        &[("seat-1", Some(100_000)), ("seat-2", Some(120_000))],
    );
    next.observations[0].quote.as_mut().unwrap().price_text = "1\u{a0}000,00 Kč za kus".into();
    let unchanged = reopened.record(&next).unwrap();
    assert!(!unchanged.duplicate_scan && unchanged.changes.is_empty());
}

#[test]
fn missing_and_unreadable_seats_keep_their_last_successful_baselines() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let mut store = MapScanStore::open(&path).unwrap();
    store
        .record(&scan(
            START,
            &[("seat-1", Some(100_000)), ("seat-2", Some(120_000))],
        ))
        .unwrap();
    assert!(
        store
            .record(&scan(START + 1000, &[("seat-1", None)]))
            .unwrap()
            .changes
            .is_empty()
    );
    assert!(
        store
            .record(&scan(START + 2000, &[]))
            .unwrap()
            .changes
            .is_empty()
    );
    let changes = store
        .record(&scan(
            START + 3000,
            &[("seat-2", Some(110_000)), ("seat-3", Some(90000))],
        ))
        .unwrap();
    assert_eq!(
        changes.changes.iter().map(|c| c.kind).collect::<Vec<_>>(),
        vec![ChangeKind::PriceChanged, ChangeKind::FirstObserved]
    );
    assert_eq!(
        changes.changes[0].previous_observed_at_ms,
        Some(START + 101)
    );
    assert_eq!(saved_price(&path, "seat-1"), 100_000);
}

#[test]
fn fresh_price_and_restriction_changes_are_distinct_from_identity_or_age_refreshes() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let mut store = MapScanStore::open(&path).unwrap();
    store
        .record(&scan(START, &[("seat-1", Some(100_000))]))
        .unwrap();
    let mut next = scan(START + 1000, &[("seat-1", Some(90_000))]);
    next.observations[0].quote.as_mut().unwrap().restriction = Some("Omezený výhled".into());
    let changes = store.record(&next).unwrap();
    assert_eq!(
        changes.changes.iter().map(|c| c.kind).collect::<Vec<_>>(),
        vec![ChangeKind::PriceChanged, ChangeKind::DetailsChanged]
    );
    let mut relabeled = scan(START + 2000, &[("seat-1", Some(80_000))]);
    relabeled.observations[0].row = "13".into();
    relabeled.observations[0].quote.as_mut().unwrap().row = "13".into();
    assert_eq!(
        store.record(&relabeled).unwrap().changes[0].kind,
        ChangeKind::BaselineRefreshed
    );
    let mut expired = scan(
        START + 2000 + COMPARISON_MAX_AGE_MS + 1,
        &[("seat-1", Some(70_000))],
    );
    expired.observations[0].row = "13".into();
    expired.observations[0].quote.as_mut().unwrap().row = "13".into();
    let change = store.record(&expired).unwrap();
    assert_eq!(change.changes.len(), 1);
    assert_eq!(change.changes[0].kind, ChangeKind::BaselineRefreshed);
}

#[test]
fn older_overlapping_and_conflicting_replays_are_rejected_before_mutation() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let mut store = MapScanStore::open(&path).unwrap();
    let first = scan(START, &[("seat-1", Some(100_000))]);
    store.record(&first).unwrap();
    for at in [START - 1000, START, START + 250] {
        assert!(
            store
                .record(&scan(at, &[("seat-1", Some(50_000))]))
                .is_err()
        );
    }
    assert_eq!(saved_price(&path, "seat-1"), 100_000);
    assert!(store.record(&first).unwrap().duplicate_scan);
    assert_eq!(
        store
            .record(&scan(START + 500, &[("seat-1", Some(90_000))]))
            .unwrap()
            .changes[0]
            .kind,
        ChangeKind::PriceChanged
    );
}

#[test]
fn invalid_report_and_late_corrupt_saved_quote_roll_back_the_entire_batch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let mut store = MapScanStore::open(&path).unwrap();
    let first = scan(
        START,
        &[("seat-1", Some(100_000)), ("seat-2", Some(120_000))],
    );
    store.record(&first).unwrap();
    let mut invalid = scan(START + 1000, &[("seat-1", Some(90_000))]);
    invalid.stock_verified = true;
    assert!(store.record(&invalid).is_err());
    let connection = Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE map_scan_quotes SET quote_json = ?1 WHERE seat_id = 'seat-2'",
            ["x".repeat(16 * 1024 + 1)],
        )
        .unwrap();
    assert!(
        store
            .record(&scan(
                START + 1000,
                &[("seat-1", Some(90_000)), ("seat-2", Some(100_000))]
            ))
            .is_err()
    );
    assert_eq!(saved_price(&path, "seat-1"), 100_000);
    let finished: i64 = connection
        .query_row("SELECT finished_at_ms FROM map_scan_scopes", [], |r| {
            r.get(0)
        })
        .unwrap();
    assert_eq!(finished, first.finished_at_ms);
}

#[test]
fn history_is_event_scoped_and_refuses_an_unrelated_database() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let mut store = MapScanStore::open(&path).unwrap();
    let first = scan(START, &[("seat-1", Some(100_000))]);
    store.record(&first).unwrap();
    let mut another = first.clone();
    another.event_url = another.event_url.replace("12345678", "87654321");
    another.event_id = "87654321".into();
    assert_eq!(
        store.record(&another).unwrap().changes[0].kind,
        ChangeKind::FirstObserved
    );
    let unrelated = directory.path().join("inventory.sqlite");
    let connection = Connection::open(&unrelated).unwrap();
    connection
        .execute("CREATE TABLE watches (id TEXT)", [])
        .unwrap();
    assert!(MapScanStore::open(&unrelated).is_err());
    let count: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM sqlite_master WHERE name LIKE 'map_scan_%'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn section_capacity_failure_does_not_leave_a_partial_new_baseline() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("history.sqlite");
    let mut store = MapScanStore::open(&path).unwrap();
    store
        .record(&scan(START, &[("seat-1", Some(100_000))]))
        .unwrap();
    let mut connection = Connection::open(&path).unwrap();
    let transaction = connection.transaction().unwrap();
    for n in 2..=10_000 {
        transaction.execute("INSERT INTO map_scan_quotes (site_host,event_id,section_id,seat_id,observed_at_ms,quote_json) VALUES ('ticketmaster.cz','12345678','s_12',?1,?2,'{}')",
            params![format!("old-{n}"), START]).unwrap();
    }
    transaction.commit().unwrap();
    assert!(
        store
            .record(&scan(
                START + 1000,
                &[("seat-1", Some(90_000)), ("seat-2", Some(80_000))]
            ))
            .is_err()
    );
    assert_eq!(saved_price(&path, "seat-1"), 100_000);
}
