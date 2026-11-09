use std::process::Command;

use ticket_watch::{
    config::{PageAdapter, Watch},
    model::Availability,
    parser,
    store::Store,
};

const PAGE: &str = include_str!("fixtures/ticketmaster-regional.html");

fn watch(id: &str, event: &str) -> Watch {
    let mut watch = Watch {
        id: id.into(),
        name: format!("Selected event {id}"),
        artist: format!("Artist {id}"),
        country: "PL".into(),
        url: format!("https://www.ticketmaster.pl/event/{event}"),
        currency: Some("PLN".into()),
        min_quantity: Some(2),
        max_price_minor: Some(60_000),
        notify_initial: true,
        ..Watch::default()
    };
    watch.parser.adapter = PageAdapter::Ticketmaster;
    watch
}

#[test]
fn arbitrary_event_inspection_needs_no_configuration_or_credentials() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("event.html");
    std::fs::write(&file, PAGE).unwrap();
    for url in [
        "https://www.ticketmaster.pl/event/a-jazz-night/123456789",
        "https://www.ticketmaster.cz/event/another-show/987654321",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .env_remove("TICKETMASTER_API_KEY")
            .env_remove("DISCORD_BOT_TOKEN")
            .args(["inspect", "--url", url, "--html"])
            .arg(&file)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        let result: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(result["source"], "saved_html");
        assert_eq!(result["event_url"], url);
        assert_eq!(result["observation"]["observed_at"], 0);
        assert_eq!(result["observation"]["offers"].as_array().unwrap().len(), 2);
        assert_eq!(
            result["observation"]["ticket_categories"]
                .as_array()
                .unwrap()
                .len(),
            4
        );
    }
    assert_eq!(std::fs::read_dir(directory.path()).unwrap().count(), 1);
}

#[test]
fn generic_listing_counts_reach_filters_without_cross_event_deduplication() {
    let directory = tempfile::tempdir().unwrap();
    let database = directory.path().join("state.sqlite");
    let first = watch("one", "111111111");
    let second = watch("two", "222222222");
    let mut store = Store::open(&database).unwrap();
    store.reconcile(&[first.clone(), second.clone()]).unwrap();
    for watch in [&first, &second] {
        let observation = parser::parse(watch, PAGE, 10);
        assert_eq!(store.apply(watch, &observation).unwrap(), 1);
        assert_eq!(store.apply(watch, &observation).unwrap(), 0);
    }
    let alerts = store.pending(20, 10).unwrap();
    assert_eq!(alerts.len(), 2);
    for alert in &alerts {
        assert_eq!(alert.payload.offers.len(), 1); // Only the listing below 600 PLN.
        assert_eq!(alert.payload.offers[0].quantity, Some(2));
        assert_eq!(alert.payload.offers[0].price_minor, Some(57500));
    }
    assert_ne!(alerts[0].nonce, alerts[1].nonce);
    drop(store);
    let store = Store::open(&database).unwrap();
    let status = store.status().unwrap();
    assert!(status.iter().all(|row| {
        row["latest_observation"]["ticket_categories"]
            .as_array()
            .unwrap()
            .len()
            == 4
    }));
}

#[test]
fn categories_are_visible_in_status_without_becoming_stock_and_edits_clear_them() {
    let directory = tempfile::tempdir().unwrap();
    let mut store = Store::open(&directory.path().join("state.sqlite")).unwrap();
    let mut watch = watch("selected", "111111111");
    let primary_only = PAGE.split("<ul>\n  <li role=\"button\">").next().unwrap();
    let observation = parser::parse(&watch, primary_only, 10);
    assert_eq!(observation.availability, Availability::Unknown);
    assert_eq!(store.apply(&watch, &observation).unwrap(), 0);
    let status = store.status().unwrap();
    assert_eq!(status[0]["health"], "unknown");
    assert_eq!(
        status[0]["latest_observation"]["ticket_categories"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    assert!(store.pending(20, 10).unwrap().is_empty());
    watch.url = "https://www.ticketmaster.pl/event/222222222".into();
    store.reconcile(&[watch]).unwrap();
    assert!(store.status().unwrap()[0]["latest_observation"].is_null());
}
