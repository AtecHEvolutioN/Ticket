//! Country scope broadens discovery, not stock evidence or polling promises.
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::collections::VecDeque;
use ticket_watch::{
    catalog::{self, Selection, Snapshot, Source},
    config::{self, Config, Subscription},
    database::Db,
    hybrid,
};

struct CountrySource {
    replies: VecDeque<Result<Value>>,
    requested: Vec<u32>,
}
impl CountrySource {
    fn new(replies: Vec<Result<Value>>) -> Self {
        Self {
            replies: replies.into(),
            requested: vec![],
        }
    }
}
impl Source for CountrySource {
    async fn page(&mut self, subscription: &Subscription, page: u32) -> Result<Value> {
        assert!(subscription.all_artists);
        assert!(subscription.attraction_id.is_empty());
        assert_eq!(subscription.country.as_deref(), Some("NL"));
        self.requested.push(page);
        self.replies
            .pop_front()
            .unwrap_or_else(|| Err(anyhow!("unexpected request")))
    }
}
fn config() -> Config {
    let config: Config =
        toml::from_str(include_str!("../examples/netherlands-all.example.toml")).unwrap();
    config::validate(&config).unwrap();
    config
}
fn event(id: &str, artist: &str) -> Value {
    json!({"id":id,"name":format!("Synthetic {artist} show"),"url":format!("https://www.ticketmaster.nl/event/synthetic/{id}"),
      "attraction_ids":[format!("attraction-{artist}")],"attractions":[{"id":format!("attraction-{artist}"),"name":artist}],
      "event_status":"onsale","dates":{"start":{"localDate":"2099-11-17"}},
      "sales":{"public":{"startDateTime":"2099-10-01T10:00:00Z"},"presales":[{"name":"Fan presale","startDateTime":"2099-09-30T10:00:00Z"}]},
      "venues":[{"name":"Synthetic Venue","city":"Amsterdam","country":{"countryCode":"NL"}}]})
}
fn page(events: Vec<Value>, next: Option<u32>) -> Value {
    json!({"events":events,"has_more":next.is_some(),"next_page":next})
}
async fn seed(config: &Config, db: &Db, events: Vec<Value>) {
    let watches = hybrid::plan(&config.subscriptions[0], &config.profiles[0], &events).watches;
    let snapshot = Snapshot {
        fingerprint: catalog::fingerprint(config).unwrap(),
        selections: vec![Selection {
            id: config.subscriptions[0].id.clone(),
            watches,
            metadata: events,
            complete: true,
            ..Selection::default()
        }],
        ..Snapshot::default()
    };
    db.call(move |store| store.catalog_save(&serde_json::to_value(snapshot)?))
        .await
        .unwrap();
}

#[test]
fn country_scope_requires_explicit_opt_in_and_unambiguous_location() {
    let good = config();
    assert!(good.subscriptions[0].all_artists);
    for mutation in [0, 1, 2, 3] {
        let mut invalid = good.clone();
        match mutation {
            0 => invalid.subscriptions[0].all_artists = false,
            1 => invalid.subscriptions[0].country = None,
            2 => invalid.subscriptions[0].attraction_id = "artist-123".into(),
            _ => invalid.subscriptions[0].country = Some("nl".into()),
        }
        assert!(config::validate(&invalid).is_err());
    }
    let legacy: Subscription = serde_json::from_value(
        json!({"id":"old","artist":"Chosen","attraction_id":"id-123","profile":"regional"}),
    )
    .unwrap();
    assert!(!legacy.all_artists);
    assert!(
        serde_json::to_value(legacy)
            .unwrap()
            .get("all_artists")
            .is_none(),
        "legacy serialized fingerprints stay unchanged"
    );
}

#[tokio::test]
async fn country_selection_keeps_actual_artists_presales_and_location_proof() {
    let config = config();
    let directory = tempfile::tempdir().unwrap();
    let db = Db::open(&directory.path().join("country.sqlite")).unwrap();
    let mut missing_artist = event("10000003", "not an actual name");
    missing_artist["attractions"] = json!([]);
    missing_artist["attraction_ids"] = json!([]);
    let mut wrong_country = event("10000004", "Outside");
    wrong_country["venues"][0]["country"]["countryCode"] = json!("BE");
    let mut wrong_host = event("10000005", "Different site");
    wrong_host["url"] = json!("https://www.ticketmaster.be/event/10000005");
    let mut source = CountrySource::new(vec![Ok(page(
        vec![
            event("10000001", "First artist"),
            event("10000002", "Second artist"),
            missing_artist,
            wrong_country,
            wrong_host,
        ],
        None,
    ))]);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    let selected = &snapshot.selections[0];
    assert!(selected.complete);
    assert_eq!(selected.watches.len(), 3);
    assert_eq!(
        selected
            .watches
            .iter()
            .map(|w| w.artist.as_str())
            .collect::<Vec<_>>(),
        vec!["First artist", "Second artist", "Artist not provided"]
    );
    assert_eq!(selected.metadata[0]["presales"][0]["name"], "Fan presale");
    assert_eq!(
        selected.metadata[0]["attractions"][0]["name"],
        "First artist"
    );
    assert_eq!(selected.metadata[0]["inventory_verified"], false);
    assert!(
        selected
            .notes
            .iter()
            .any(|note| note.contains("no artist filter"))
    );
    assert!(
        db.call(|store| store.pending(i64::MAX, 100))
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn deep_page_ceiling_is_incomplete_and_does_not_retire_cached_events() {
    let config = config();
    let directory = tempfile::tempdir().unwrap();
    let db = Db::open(&directory.path().join("country.sqlite")).unwrap();
    seed(&config, &db, vec![event("10000000", "Cached artist")]).await;
    let replies = (0..10)
        .map(|index| {
            Ok(page(
                vec![event(&format!("2000000{index}"), "Another artist")],
                Some(index + 1),
            ))
        })
        .collect();
    let mut source = CountrySource::new(replies);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert_eq!(source.requested, (0..10).collect::<Vec<_>>());
    let selected = &snapshot.selections[0];
    assert!(!selected.complete);
    assert_eq!(selected.watches.len(), 11);
    assert!(
        selected
            .watches
            .iter()
            .any(|watch| watch.url.ends_with("10000000"))
    );
    assert!(
        selected
            .notes
            .iter()
            .any(|note| note.contains("1,000-result") && note.contains("not every event"))
    );
}

#[tokio::test]
async fn capped_failed_refresh_preserves_cached_matches_even_when_updated_after_new_events() {
    let mut config = config();
    config.catalog.max_events = 2;
    let directory = tempfile::tempdir().unwrap();
    let db = Db::open(&directory.path().join("country.sqlite")).unwrap();
    seed(
        &config,
        &db,
        vec![
            event("10000001", "Cached one"),
            event("10000002", "Cached two"),
        ],
    )
    .await;
    let mut source = CountrySource::new(vec![
        Ok(page(
            vec![
                event("10000003", "New event"),
                event("10000001", "Updated artist"),
            ],
            Some(1),
        )),
        Err(anyhow!("synthetic timeout")),
    ]);
    let snapshot = catalog::refresh(&config, &db, &mut source).await.unwrap();
    let selected = &snapshot.selections[0];
    assert!(!selected.complete);
    assert_eq!(selected.watches.len(), 2);
    assert!(
        selected
            .watches
            .iter()
            .any(|watch| watch.url.ends_with("10000001") && watch.artist == "Updated artist")
    );
    assert!(
        selected
            .watches
            .iter()
            .any(|watch| watch.url.ends_with("10000002"))
    );
    assert!(
        !selected
            .watches
            .iter()
            .any(|watch| watch.url.ends_with("10000003"))
    );
    assert!(
        selected
            .notes
            .iter()
            .any(|note| note.contains("unique event limit"))
    );
    assert!(selected.notes.iter().any(|note| note.contains("failed")));
}

#[tokio::test]
async fn resumed_scan_cannot_hide_an_earlier_capacity_gap() {
    let mut config = config();
    config.catalog.max_events = 2;
    config.catalog.max_requests_per_refresh = 1;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("country.sqlite");
    let db = Db::open(&path).unwrap();
    seed(
        &config,
        &db,
        vec![
            event("10000001", "Cached one"),
            event("10000002", "Cached two"),
        ],
    )
    .await;
    let mut first = CountrySource::new(vec![Ok(page(
        vec![
            event("10000003", "New event"),
            event("10000001", "Updated artist"),
        ],
        Some(1),
    ))]);
    let first = catalog::refresh(&config, &db, &mut first).await.unwrap();
    assert!(first.selections[0].scan_capacity_limited);
    assert_eq!(first.selections[0].next_page, 1);
    // Simulate elapsed cadence only in this temporary fixture. Production
    // set_cooldown deliberately cannot shorten an existing provider deadline.
    rusqlite::Connection::open(&path)
        .unwrap()
        .execute(
            "UPDATE cooldowns SET until_timestamp=0 WHERE scope='catalog:refresh'",
            [],
        )
        .unwrap();
    let mut source = CountrySource::new(vec![Ok(page(vec![], None))]);
    let last = catalog::refresh(&config, &db, &mut source).await.unwrap();
    assert_eq!(source.requested, vec![1]);
    assert_eq!(last.selections[0].next_page, 0);
    assert!(!last.selections[0].complete);
    assert!(
        last.selections[0]
            .notes
            .iter()
            .any(|note| note.contains("previously reached"))
    );
    assert!(
        !last.selections[0]
            .watches
            .iter()
            .any(|watch| watch.url.ends_with("10000003"))
    );
}
