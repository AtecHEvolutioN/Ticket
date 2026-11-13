//! Date windows expand catalog coverage without changing inventory evidence.
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, path::Path};
use ticket_watch::{
    catalog::{self, CountryScan, DateWindow, Selection, Snapshot, Source},
    config::{self, Config, Subscription},
    database::Db,
    hybrid,
};

fn config() -> Config {
    let mut config: Config =
        toml::from_str(include_str!("../examples/netherlands-all.example.toml")).unwrap();
    config.catalog.country_partitioning = true;
    config.catalog.country_horizon_days = 1;
    config.catalog.country_window_days = 1;
    config::validate(&config).unwrap();
    config
}

fn event(id: u32) -> Value {
    json!({"id":id.to_string(),"name":format!("Synthetic artist {id}"),
        "url":format!("https://www.ticketmaster.nl/event/synthetic/{id}"),
        "attractions":[{"id":format!("artist-{id}"),"name":format!("Artist {id}")}],
        "event_status":"onsale","dates":{"start":{"localDate":"2099-11-17"}},
        "venues":[{"name":"Synthetic Venue","city":"Amsterdam","country":{"countryCode":"NL"}}]})
}

fn page(events: Vec<Value>, number: u32, total: u32, more: bool) -> Value {
    json!({"events":events,"page":{"number":number,"total_elements":total,"total_pages":total.div_ceil(100)},
        "has_more":more,"next_page":if more && number<9 {Some(number+1)} else {None}})
}

fn elapse_fixture_cadence(path: &Path) {
    // Only this isolated fixture's cadence is changed; production cooldowns
    // intentionally cannot be shortened by callers.
    rusqlite::Connection::open(path)
        .unwrap()
        .execute(
            "UPDATE cooldowns SET until_timestamp=0 WHERE scope='catalog:refresh'",
            [],
        )
        .unwrap();
}

async fn seed(config: &Config, db: &Db, events: Vec<Value>, scan: Option<CountryScan>) {
    let snapshot = Snapshot {
        fingerprint: catalog::fingerprint(config).unwrap(),
        selections: vec![Selection {
            id: config.subscriptions[0].id.clone(),
            watches: hybrid::plan(&config.subscriptions[0], &config.profiles[0], &events).watches,
            metadata: events,
            country_scan: scan,
            ..Selection::default()
        }],
        ..Snapshot::default()
    };
    db.call(move |store| store.catalog_save(&serde_json::to_value(snapshot)?))
        .await
        .unwrap();
}

struct WindowSource {
    replies: VecDeque<Result<Value>>,
    requested: Vec<DateWindow>,
}
impl WindowSource {
    fn new(replies: Vec<Result<Value>>) -> Self {
        Self {
            replies: replies.into(),
            requested: vec![],
        }
    }
}
impl Source for WindowSource {
    async fn page(&mut self, _: &Subscription, _: u32) -> Result<Value> {
        panic!("Partitioned scans must not silently fall back to unbounded queries")
    }
    async fn window_page(
        &mut self,
        subscription: &Subscription,
        window: &DateWindow,
    ) -> Result<Value> {
        assert!(subscription.all_artists);
        assert_eq!(subscription.country.as_deref(), Some("NL"));
        assert!(window.page < 10 && window.start <= window.end);
        self.requested.push(window.clone());
        self.replies
            .pop_front()
            .unwrap_or_else(|| Err(anyhow!("unexpected request")))
    }
}

#[tokio::test]
async fn splits_saturated_country_query_and_retains_more_than_one_thousand_unique_events() {
    let config = config();
    let directory = tempfile::tempdir().unwrap();
    let db = Db::open(&directory.path().join("catalog.sqlite")).unwrap();
    seed(&config, &db, vec![event(999999)], None).await;
    // Parent-page identities reappear in children and must not become duplicates.
    let mut replies = vec![Ok(page(
        (100000..100100).map(event).collect(),
        0,
        1500,
        true,
    ))];
    for base in [100000, 100750] {
        for number in 0..8 {
            let first = number * 100;
            let end = ((number + 1) * 100).min(750);
            replies.push(Ok(page(
                (first..end).map(|n| event(base + n)).collect(),
                number,
                750,
                number < 7,
            )));
        }
    }
    let mut source = WindowSource::new(replies);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    let selection = &snapshot.selections[0];
    assert!(selection.complete, "{:?}", selection.notes);
    assert_eq!(
        selection.watches.len(),
        1501,
        "unseen cached events outside the bounded scope survive"
    );
    assert_eq!(selection.metadata.len(), 1501);
    assert_eq!(
        source.requested.len(),
        17,
        "saturation is split immediately rather than spending ten parent pages"
    );
    let parent = &source.requested[0];
    let left = &source.requested[1];
    let right = &source.requested[9];
    assert_eq!(left.start, parent.start);
    assert_eq!(left.end + 1, right.start);
    assert_eq!(right.end, parent.end);
    let scan = selection.country_scan.as_ref().unwrap();
    assert_eq!(scan.completed_windows, 2);
    assert_eq!(scan.incomplete_windows, 0);
    assert!(scan.pending.is_empty());
    assert!(
        selection
            .notes
            .iter()
            .any(|note| note.contains("Undated/TBA"))
    );
    assert!(
        db.call(|store| store.pending(i64::MAX, 100))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn budget_failure_and_reopen_preserve_the_exact_window_and_page() {
    let mut config = config();
    config.catalog.max_requests_per_refresh = 1;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.sqlite");
    let db = Db::open(&path).unwrap();
    let mut source = WindowSource::new(vec![Ok(page(vec![event(100001)], 0, 1200, true))]);
    let first = catalog::refresh(&config, &db, &mut source).await.unwrap();
    let pending = first.selections[0]
        .country_scan
        .as_ref()
        .unwrap()
        .pending
        .clone();
    assert_eq!(pending.len(), 2);
    assert!(!first.selections[0].complete);
    elapse_fixture_cadence(&path);
    drop(db);
    let db = Db::open(&path).unwrap();
    let mut failed = WindowSource::new(vec![Err(anyhow!("synthetic timeout"))]);
    let result = catalog::refresh(&config, &db, &mut failed).await.unwrap();
    assert_eq!(failed.requested, vec![pending[0].clone()]);
    assert_eq!(
        result.selections[0].country_scan.as_ref().unwrap().pending,
        pending
    );
    assert_eq!(result.selections[0].watches.len(), 1);
    elapse_fixture_cadence(&path);
    let mut source = WindowSource::new(vec![Ok(page(vec![event(100002)], 0, 200, true))]);
    let result = catalog::refresh(&config, &db, &mut source).await.unwrap();
    let cursor = &result.selections[0].country_scan.as_ref().unwrap().pending[0];
    assert_eq!(cursor.start, pending[0].start);
    assert_eq!(cursor.end, pending[0].end);
    assert_eq!(cursor.page, 1);
    elapse_fixture_cadence(&path);
    let mut source = WindowSource::new(vec![Ok(page(vec![event(100003)], 1, 200, false))]);
    let result = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert_eq!(source.requested[0], *cursor);
    assert_eq!(
        result.selections[0].country_scan.as_ref().unwrap().pending[0],
        pending[1]
    );
}

#[tokio::test]
async fn rolling_daily_quota_prevents_requests_without_losing_partition_cursor() {
    let mut config = config();
    config.catalog.max_requests_per_refresh = 1;
    config.catalog.max_requests_per_day = 1;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.sqlite");
    let db = Db::open(&path).unwrap();
    let mut source = WindowSource::new(vec![Ok(page(vec![], 0, 1500, true))]);
    let first = catalog::refresh(&config, &db, &mut source).await.unwrap();
    elapse_fixture_cadence(&path);
    let mut blocked = WindowSource::new(vec![]);
    let second = catalog::refresh(&config, &db, &mut blocked).await.unwrap();
    assert!(blocked.requested.is_empty());
    assert_eq!(
        second.selections[0].country_scan.as_ref().unwrap().pending,
        first.selections[0].country_scan.as_ref().unwrap().pending
    );
    assert!(
        second.selections[0]
            .notes
            .iter()
            .any(|note| note.contains("24-hour"))
    );
}

#[tokio::test]
async fn one_second_saturation_and_partition_ceiling_are_explicit_incomplete_gaps() {
    for created in [1, 8192] {
        let config = config();
        let directory = tempfile::tempdir().unwrap();
        let db = Db::open(&directory.path().join("catalog.sqlite")).unwrap();
        let start = chrono::Utc::now().timestamp();
        let end = start + if created == 1 { 0 } else { 10 };
        let scan = CountryScan {
            start,
            end,
            pending: VecDeque::from([DateWindow {
                start,
                end,
                page: 0,
            }]),
            created_windows: created,
            completed_windows: 0,
            incomplete_windows: 0,
        };
        seed(&config, &db, vec![event(999999)], Some(scan)).await;
        let mut source = WindowSource::new(vec![Ok(page(vec![event(100001)], 0, 1500, true))]);
        let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
        let selection = &snapshot.selections[0];
        assert!(!selection.complete);
        assert_eq!(selection.watches.len(), 2);
        assert_eq!(source.requested.len(), 1);
        assert_eq!(
            selection.country_scan.as_ref().unwrap().incomplete_windows,
            1
        );
        assert!(
            selection
                .notes
                .iter()
                .any(|note| note.contains("same timestamp"))
        );
    }
}

#[tokio::test]
async fn capacity_gap_survives_resumed_window_completion_and_retains_cached_matches() {
    let mut config = config();
    config.catalog.max_events = 1;
    config.catalog.max_requests_per_refresh = 1;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("catalog.sqlite");
    let db = Db::open(&path).unwrap();
    seed(&config, &db, vec![event(999999)], None).await;
    let mut source = WindowSource::new(vec![Ok(page(vec![event(100001)], 0, 200, true))]);
    let first = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert!(first.selections[0].scan_capacity_limited);
    elapse_fixture_cadence(&path);
    let mut source = WindowSource::new(vec![Ok(page(vec![], 1, 200, false))]);
    let result = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert!(!result.selections[0].complete);
    assert_eq!(result.selections[0].watches.len(), 1);
    assert!(result.selections[0].watches[0].url.ends_with("999999"));
}

#[tokio::test]
async fn invalid_window_page_does_not_advance_or_claim_completion() {
    let config = config();
    let directory = tempfile::tempdir().unwrap();
    let db = Db::open(&directory.path().join("catalog.sqlite")).unwrap();
    let mut source = WindowSource::new(vec![Ok(page(vec![event(100001)], 5, 1, false))]);
    let result = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert!(!result.selections[0].complete);
    assert!(result.selections[0].watches.is_empty());
    assert_eq!(
        result.selections[0].country_scan.as_ref().unwrap().pending[0].page,
        0
    );
}

#[tokio::test]
async fn an_exact_split_boundary_returned_by_both_wire_queries_is_only_one_watch() {
    let config = config();
    let directory = tempfile::tempdir().unwrap();
    let db = Db::open(&directory.path().join("catalog.sqlite")).unwrap();
    let start = chrono::Utc::now().timestamp().div_euclid(86_400) * 86_400;
    let split = start + 43_200;
    let end = start + 86_399;
    let scan = CountryScan {
        start,
        end,
        pending: VecDeque::from([
            DateWindow {
                start,
                end: split - 1,
                page: 0,
            },
            DateWindow {
                start: split,
                end,
                page: 0,
            },
        ]),
        created_windows: 2,
        completed_windows: 0,
        incomplete_windows: 0,
    };
    seed(&config, &db, vec![], Some(scan)).await;
    let mut boundary = event(100001);
    boundary["dates"]["start"]["dateTime"] = json!(
        chrono::DateTime::from_timestamp(split, 0)
            .unwrap()
            .to_rfc3339()
    );
    let mut source = WindowSource::new(vec![
        Ok(page(vec![boundary.clone()], 0, 1, false)),
        Ok(page(vec![boundary, event(100002)], 0, 2, false)),
    ]);
    let result = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert!(result.selections[0].complete);
    assert_eq!(result.selections[0].watches.len(), 2);
    assert_eq!(result.selections[0].metadata.len(), 2);
}

#[test]
fn legacy_fingerprint_and_partition_bounds_are_explicit() {
    let mut config = config();
    config.catalog.country_partitioning = false;
    let legacy = format!(
        "{:x}",
        Sha256::digest(
            serde_json::to_vec(&(
                &config.subscriptions,
                &config.profiles,
                config.catalog.max_events,
            ))
            .unwrap()
        )
    );
    assert_eq!(catalog::fingerprint(&config).unwrap(), legacy);
    config.catalog.country_partitioning = true;
    assert_ne!(catalog::fingerprint(&config).unwrap(), legacy);
    for (horizon, window, capacity) in [
        (0, 1, 1000),
        (3651, 1, 1000),
        (1, 2, 1000),
        (730, 366, 1000),
        (730, 30, 10001),
    ] {
        let mut invalid = config.clone();
        invalid.catalog.country_horizon_days = horizon;
        invalid.catalog.country_window_days = window;
        invalid.catalog.max_events = capacity;
        assert!(config::validate(&invalid).is_err());
    }
}
