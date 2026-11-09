//! Synthetic end-to-end catalog selection -> website evidence -> durable alert tests.
//! No Ticketmaster or Discord network calls and no real credentials are involved.
use std::collections::{HashSet, VecDeque};

use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use tempfile::TempDir;
use ticket_watch::{
    catalog::{self, Selection, Snapshot, Source},
    config::{self, Config, Subscription, WebsiteProfile},
    database::Db,
    hybrid, parser,
};

const ATTRACTION: &str = "synthetic-korn-id";

#[derive(Default)]
struct FakeDiscovery {
    replies: VecDeque<Result<Value>>,
    requests: Vec<(String, u32)>,
    cooldown: Option<i64>,
    paused: bool,
}

impl FakeDiscovery {
    fn pages(pages: Vec<Value>) -> Self {
        Self {
            replies: pages.into_iter().map(Ok).collect(),
            ..Self::default()
        }
    }
}

impl Source for FakeDiscovery {
    async fn page(&mut self, subscription: &Subscription, page: u32) -> Result<Value> {
        self.requests
            .push((subscription.attraction_id.clone(), page));
        self.replies
            .pop_front()
            .unwrap_or_else(|| Err(anyhow!("unexpected synthetic Discovery request")))
    }

    fn cooldown_until(&self) -> Option<i64> {
        self.cooldown
    }

    fn is_paused(&self) -> bool {
        self.paused
    }
}

fn config() -> Config {
    let fixture: Config = toml::from_str(include_str!("../examples/offline-demo.toml")).unwrap();
    let watch = &fixture.watches[0];
    let profile = WebsiteProfile {
        id: "synthetic-pl".into(),
        hosts: vec!["www.ticketmaster.pl".into(), "ticketmaster.pl".into()],
        parser: watch.parser.clone(),
        min_quantity: watch.min_quantity,
        max_price_minor: watch.max_price_minor,
        currency: watch.currency.clone(),
        ..WebsiteProfile::default()
    };
    let subscription = Subscription {
        id: "chosen-korn-krakow".into(),
        artist: "KoRn".into(),
        attraction_id: ATTRACTION.into(),
        country: Some("PL".into()),
        city: Some("Krakow".into()),
        profile: profile.id.clone(),
        ..Subscription::default()
    };
    let config = Config {
        subscriptions: vec![subscription],
        profiles: vec![profile],
        ..Config::default()
    };
    config::validate(&config).unwrap();
    config
}

fn event(id: &str) -> Value {
    json!({
        "id":id,
        "name":"Synthetic KoRn show",
        "url":format!("https://www.ticketmaster.pl/event/korn-bilety/{id}"),
        "attraction_ids":[ATTRACTION],
        "event_status":"onsale",
        "dates":{"start":{"localDate":"2099-11-17"}, "timezone":"Europe/Warsaw"},
        "sales":{"public":{"startDateTime":"2099-01-01T12:00:00Z"}},
        "price_ranges":[{"currency":"PLN", "min":1, "max":5000}],
        "venues":[{"id":"synthetic-arena", "name":"Synthetic Arena", "city":"Krakow", "country":{"countryCode":"PL"}}]
    })
}

fn page(events: Vec<Value>, next: Option<u32>) -> Value {
    json!({"events":events, "has_more":next.is_some(), "next_page":next, "inventory_verified":false})
}

async fn seed_snapshot(config: &Config, db: &Db, events: Vec<Value>) -> Snapshot {
    let watches = hybrid::plan(&config.subscriptions[0], &config.profiles[0], &events).watches;
    let snapshot = Snapshot {
        fingerprint: catalog::fingerprint(config).unwrap(),
        selections: vec![Selection {
            id: config.subscriptions[0].id.clone(),
            watches,
            metadata: events,
            complete: true,
            updated_at: Some(1),
            ..Selection::default()
        }],
        ..Snapshot::default()
    };
    let saved = serde_json::to_value(&snapshot).unwrap();
    db.call(move |store| store.catalog_save(&saved))
        .await
        .unwrap();
    snapshot
}

#[tokio::test]
async fn different_events_share_venue_reference_without_sharing_inventory() {
    let config = config();
    let directory = TempDir::new().unwrap();
    let db = Db::open(&directory.path().join("venue-catalog.sqlite")).unwrap();
    let mut second = event("496543018");
    second["name"] = json!("Another synthetic show");
    second["price_ranges"] = json!([{"currency":"PLN", "min":900, "max":1000}]);
    let mut source = FakeDiscovery::pages(vec![page(vec![event("496543017"), second], None)]);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    let selected = &snapshot.selections[0];
    assert_eq!(selected.watches.len(), 2);
    assert_ne!(selected.watches[0].id, selected.watches[1].id);
    assert_eq!(
        selected.metadata[0]["venue_preset_ids"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        selected.metadata[0]["venue_preset_ids"],
        selected.metadata[1]["venue_preset_ids"]
    );
    let venues = db.call(|store| store.venue_list()).await.unwrap();
    assert_eq!(venues.len(), 1);
    assert_eq!(venues[0].source_event_urls.len(), 2);
    assert!(venues[0].layouts.is_empty());
    let serialized = serde_json::to_value(&venues[0]).unwrap();
    assert!(serialized.get("price_ranges").is_none());
    assert!(serialized.get("availability").is_none());
    assert!(
        db.call(|store| store.pending(i64::MAX, 10))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn incomplete_venue_metadata_keeps_monitoring_but_cannot_create_a_preset() {
    let config = config();
    let directory = TempDir::new().unwrap();
    let db = Db::open(&directory.path().join("venue-missing-name.sqlite")).unwrap();
    let mut selected = event("496543017");
    selected["venues"][0]
        .as_object_mut()
        .unwrap()
        .remove("name");
    let mut source = FakeDiscovery::pages(vec![page(vec![selected], None)]);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert_eq!(snapshot.selections[0].watches.len(), 1);
    assert!(
        snapshot.selections[0]
            .notes
            .iter()
            .any(|note| note.contains("Venue reference"))
    );
    assert!(
        db.call(|store| store.venue_list())
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn discovery_selects_only_chosen_events_and_website_evidence_creates_the_alert() {
    let config = config();
    let directory = TempDir::new().unwrap();
    let db = Db::open(&directory.path().join("hybrid.sqlite")).unwrap();
    let chosen = event("496543017");
    let mut unrelated_artist = event("wrong-artist");
    unrelated_artist["attraction_ids"] = json!(["different-performer"]);
    let mut unrelated_city = event("wrong-city");
    unrelated_city["venues"][0]["city"] = json!("Warsaw");
    let mut source = FakeDiscovery::pages(vec![page(
        vec![chosen, unrelated_artist, unrelated_city],
        None,
    )]);

    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert_eq!(source.requests, vec![(ATTRACTION.into(), 0)]);
    assert!(snapshot.selections[0].complete);
    assert_eq!(snapshot.selections[0].skipped.len(), 2);
    assert_eq!(snapshot.selections[0].metadata.len(), 1);
    assert_eq!(snapshot.selections[0].metadata[0]["event_status"], "onsale");
    assert_eq!(
        snapshot.selections[0].metadata[0]["inventory_verified"],
        false
    );

    let watches = catalog::effective_watches(&config, &snapshot).unwrap();
    assert_eq!(watches.len(), 1);
    let watch = watches[0].clone();
    assert!(!watch.notify_metadata);
    db.call(move |store| store.reconcile(&watches))
        .await
        .unwrap();
    assert!(
        db.call(|store| store.pending(i64::MAX, 10))
            .await
            .unwrap()
            .is_empty()
    );

    // Page JSON-LD cannot substitute for missing verified inventory selectors.
    let structured = json!({
        "@context":"https://schema.org", "@type":"MusicEvent", "url":watch.url,
        "offers":{"@type":"Offer", "availability":"https://schema.org/InStock"}
    });
    let hint_html = format!("<script type=\"application/ld+json\">{structured}</script>");
    let hint = parser::parse(&watch, &hint_html, 10);
    assert_eq!(
        hint.availability,
        ticket_watch::model::Availability::Unknown
    );
    let hint_watch = watch.clone();
    assert_eq!(
        db.call(move |store| store.apply(&hint_watch, &hint))
            .await
            .unwrap(),
        0
    );

    let unavailable = parser::parse(
        &watch,
        include_str!("fixtures/synthetic-unavailable.html"),
        11,
    );
    let unavailable_watch = watch.clone();
    assert_eq!(
        db.call(move |store| store.apply(&unavailable_watch, &unavailable))
            .await
            .unwrap(),
        0
    );
    let available = parser::parse(&watch, include_str!("fixtures/synthetic-offers.html"), 12);
    let available_watch = watch.clone();
    assert_eq!(
        db.call(move |store| store.apply(&available_watch, &available))
            .await
            .unwrap(),
        1
    );
    let repeated = parser::parse(&watch, include_str!("fixtures/synthetic-offers.html"), 13);
    assert_eq!(
        db.call(move |store| store.apply(&watch, &repeated))
            .await
            .unwrap(),
        0
    );

    let queued = db.call(|store| store.pending(13, 10)).await.unwrap();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].payload.offers[0].price_minor, Some(91_080));
    assert_eq!(queued[0].payload.offers[0].quantity, Some(2));
    assert_eq!(queued[0].payload.kind, "available");
}

#[tokio::test]
async fn overlapping_subscriptions_share_a_watch_and_manual_pause_takes_precedence() {
    let mut config = config();
    let mut second = config.subscriptions[0].clone();
    second.id = "another-selection".into();
    config.subscriptions.push(second);
    let directory = TempDir::new().unwrap();
    let db = Db::open(&directory.path().join("hybrid.sqlite")).unwrap();
    let mut source = FakeDiscovery::pages(vec![
        page(vec![event("496543017")], None),
        page(vec![event("496543017")], None),
    ]);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert_eq!(snapshot.selections.len(), 2);
    let watches = catalog::effective_watches(&config, &snapshot).unwrap();
    assert_eq!(watches.len(), 1);
    let mut manual = watches[0].clone();
    manual.id = "explicit-pause".into();
    manual.enabled = false;
    manual.url = manual.url.replace("www.ticketmaster.pl", "ticketmaster.pl");
    manual.url.push_str("?language=en");
    config.watches.push(manual);
    let effective = catalog::effective_watches(&config, &snapshot).unwrap();
    assert_eq!(effective.len(), 1);
    assert_eq!(effective[0].id, "explicit-pause");
    assert!(!effective[0].enabled);
}

#[tokio::test]
async fn truncated_catalog_refresh_adds_known_matches_without_removing_cached_events() {
    let mut config = config();
    config.catalog.max_requests_per_refresh = 1;
    let directory = TempDir::new().unwrap();
    let db = Db::open(&directory.path().join("hybrid.sqlite")).unwrap();
    seed_snapshot(&config, &db, vec![event("previously-selected")]).await;
    let mut source = FakeDiscovery::pages(vec![page(vec![event("newly-selected")], Some(1))]);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert_eq!(source.requests.len(), 1);
    assert!(!snapshot.selections[0].complete);
    assert!(
        snapshot.selections[0]
            .notes
            .iter()
            .any(|note| note.contains("request budget"))
    );
    let urls: HashSet<_> = catalog::effective_watches(&config, &snapshot)
        .unwrap()
        .into_iter()
        .map(|watch| watch.url)
        .collect();
    assert_eq!(urls.len(), 2);
    assert!(urls.iter().any(|url| url.ends_with("previously-selected")));
    assert!(urls.iter().any(|url| url.ends_with("newly-selected")));
    assert!(
        db.call(|store| store.pending(i64::MAX, 10))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn restart_preserves_catalog_and_budget_while_changed_selection_invalidates_cache() {
    let mut config = config();
    config.catalog.max_requests_per_day = 1;
    config.catalog.max_requests_per_refresh = 1;
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("hybrid.sqlite");
    let db = Db::open(&path).unwrap();
    seed_snapshot(&config, &db, vec![event("previously-selected")]).await;
    assert!(
        db.call(|store| store.reserve_catalog_request(chrono::Utc::now().timestamp(), 1))
            .await
            .unwrap()
    );
    drop(db);

    let db = Db::open(&path).unwrap();
    let cached = catalog::load(&config, &db).await.unwrap();
    assert_eq!(
        catalog::effective_watches(&config, &cached).unwrap().len(),
        1
    );
    let mut source = FakeDiscovery::pages(vec![page(vec![], None)]);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert!(source.requests.is_empty());
    assert_eq!(snapshot.requests_attempted, 0);
    assert_eq!(
        catalog::effective_watches(&config, &snapshot)
            .unwrap()
            .len(),
        1
    );
    assert!(
        snapshot.selections[0]
            .notes
            .iter()
            .any(|note| note.contains("24-hour"))
    );
    config.subscriptions[0].city = Some("Warsaw".into());
    let invalidated = catalog::load(&config, &db).await.unwrap();
    assert!(invalidated.selections.is_empty());
    assert!(
        catalog::effective_watches(&config, &invalidated)
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn failed_api_attempt_consumes_budget_and_preserves_cached_inventory_scope() {
    let config = config();
    let directory = TempDir::new().unwrap();
    let db = Db::open(&directory.path().join("hybrid.sqlite")).unwrap();
    seed_snapshot(&config, &db, vec![event("previously-selected")]).await;
    let mut source = FakeDiscovery {
        replies: VecDeque::from([Err(anyhow!("synthetic provider timeout"))]),
        ..FakeDiscovery::default()
    };
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert_eq!(snapshot.requests_attempted, 1);
    assert_eq!(
        catalog::effective_watches(&config, &snapshot)
            .unwrap()
            .len(),
        1
    );
    assert!(!snapshot.selections[0].complete);
    assert!(
        !db.call(|store| store.reserve_catalog_request(chrono::Utc::now().timestamp(), 1))
            .await
            .unwrap()
    );
    assert!(
        db.call(|store| store.pending(i64::MAX, 10))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn unique_event_cap_does_not_charge_overlapping_subscriptions_twice() {
    let mut config = config();
    config.catalog.max_events = 2;
    for id in ["overlap", "another-show"] {
        let mut subscription = config.subscriptions[0].clone();
        subscription.id = id.into();
        config.subscriptions.push(subscription);
    }
    let directory = TempDir::new().unwrap();
    let db = Db::open(&directory.path().join("hybrid.sqlite")).unwrap();
    let mut source = FakeDiscovery::pages(vec![
        page(vec![event("same-show")], None),
        page(vec![event("same-show")], None),
        page(vec![event("different-show")], None),
    ]);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    let watches = catalog::effective_watches(&config, &snapshot).unwrap();
    assert_eq!(watches.len(), 2);
    assert!(watches.iter().any(|watch| watch.url.ends_with("same-show")));
    assert!(
        watches
            .iter()
            .any(|watch| watch.url.ends_with("different-show"))
    );
    assert!(
        snapshot
            .selections
            .iter()
            .all(|selection| selection.complete)
    );
}

#[tokio::test]
async fn partial_refresh_retires_explicitly_cancelled_or_past_events_but_keeps_omitted_ones() {
    let mut config = config();
    config.catalog.max_requests_per_refresh = 1;
    let directory = TempDir::new().unwrap();
    let db = Db::open(&directory.path().join("hybrid.sqlite")).unwrap();
    seed_snapshot(
        &config,
        &db,
        vec![event("cancelled"), event("past"), event("omitted")],
    )
    .await;
    let mut cancelled = event("cancelled");
    cancelled["event_status"] = json!("cancelled");
    let mut past = event("past");
    past["dates"]["start"]["localDate"] = json!("2000-01-01");
    let mut source = FakeDiscovery::pages(vec![page(vec![cancelled, past, event("new")], Some(1))]);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert!(!snapshot.selections[0].complete);
    let watches = catalog::effective_watches(&config, &snapshot).unwrap();
    assert_eq!(watches.len(), 2);
    assert!(watches.iter().any(|watch| watch.url.ends_with("omitted")));
    assert!(watches.iter().any(|watch| watch.url.ends_with("new")));
    assert!(
        db.call(|store| store.pending(i64::MAX, 10))
            .await
            .unwrap()
            .is_empty()
    );
}

// Simulate elapsed wall-clock cadence in this isolated fixture database. Production code
// only extends cooldowns; resetting the test row avoids sleeping for real minutes.
fn expire_catalog_cadence(path: &std::path::Path) {
    rusqlite::Connection::open(path)
        .unwrap()
        .execute(
            "UPDATE cooldowns SET until_timestamp=0 WHERE scope='catalog:refresh'",
            [],
        )
        .unwrap();
}

#[tokio::test]
async fn bounded_refresh_resumes_pagination_and_failed_pages_keep_the_scan_cursor() {
    let mut config = config();
    config.catalog.max_requests_per_refresh = 1;
    let directory = TempDir::new().unwrap();
    let path = directory.path().join("hybrid.sqlite");
    let db = Db::open(&path).unwrap();
    seed_snapshot(&config, &db, vec![event("old-not-in-new-scan")]).await;

    let mut first_page = FakeDiscovery::pages(vec![page(vec![event("page-zero")], Some(1))]);
    let initial = catalog::refresh(&config, &db, &mut first_page)
        .await
        .unwrap();
    assert_eq!(first_page.requests, vec![(ATTRACTION.into(), 0)]);
    assert!(!initial.selections[0].complete);
    assert_eq!(initial.selections[0].next_page, 1);
    assert_eq!(
        catalog::effective_watches(&config, &initial).unwrap().len(),
        2
    );

    expire_catalog_cadence(&path);
    let mut failed_page = FakeDiscovery {
        replies: VecDeque::from([Err(anyhow!("synthetic page one timeout"))]),
        ..FakeDiscovery::default()
    };
    let failed = catalog::refresh(&config, &db, &mut failed_page)
        .await
        .unwrap();
    assert_eq!(failed_page.requests, vec![(ATTRACTION.into(), 1)]);
    assert_eq!(failed.selections[0].next_page, 1);
    assert_eq!(
        catalog::effective_watches(&config, &failed).unwrap().len(),
        2
    );

    expire_catalog_cadence(&path);
    let mut last_page = FakeDiscovery::pages(vec![page(vec![event("page-one")], None)]);
    let complete = catalog::refresh(&config, &db, &mut last_page)
        .await
        .unwrap();
    assert_eq!(last_page.requests, vec![(ATTRACTION.into(), 1)]);
    assert!(complete.selections[0].complete);
    assert_eq!(complete.selections[0].next_page, 0);
    let watches = catalog::effective_watches(&config, &complete).unwrap();
    assert_eq!(watches.len(), 2);
    assert!(watches.iter().any(|watch| watch.url.ends_with("page-zero")));
    assert!(watches.iter().any(|watch| watch.url.ends_with("page-one")));
    assert!(
        !watches
            .iter()
            .any(|watch| watch.url.ends_with("old-not-in-new-scan"))
    );
}

#[tokio::test]
async fn paused_provider_backoff_longer_than_a_day_is_preserved_in_full() {
    let config = config();
    let directory = TempDir::new().unwrap();
    let db = Db::open(&directory.path().join("hybrid.sqlite")).unwrap();
    let until = chrono::Utc::now().timestamp() + 172_800;
    let mut source = FakeDiscovery {
        cooldown: Some(until),
        paused: true,
        ..FakeDiscovery::default()
    };
    catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert!(source.requests.is_empty());
    assert_eq!(
        db.call(|store| store.cooldown("discovery:api"))
            .await
            .unwrap(),
        Some(until)
    );
}
