use std::{
    path::Path,
    process::{Command, Output},
};

use serde_json::{Value, json};
use ticket_watch::{
    cart_preferences::PreferencesDocument,
    config::{Config, Watch},
    model::{Availability, Evidence, FeeScope, Observation, Offer, PriceScope, QuantityKind},
    store::Store,
};

fn run(directory: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .current_dir(directory)
        .env_remove("TICKETMASTER_API_KEY")
        .env_remove("DISCORD_BOT_TOKEN")
        .args(args)
        .output()
        .unwrap()
}

fn success(output: Output) -> Value {
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    serde_json::from_slice(&output.stdout).unwrap()
}

#[test]
fn registry_commands_roundtrip_android_exports_and_refuse_stale_writes_and_reads() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("android.json");
    let store = dir.path().join("preferences.sqlite");
    let document: Value =
        serde_json::from_slice(include_bytes!("fixtures/android-ticket-preferences.json")).unwrap();
    std::fs::write(&source, serde_json::to_vec(&document).unwrap()).unwrap();
    let save = |revision: &str| {
        run(
            dir.path(),
            &[
                "cart-preferences-save",
                source.to_str().unwrap(),
                "--store",
                store.to_str().unwrap(),
                "--expected-revision",
                revision,
            ],
        )
    };
    let first = success(save("0"));
    assert_eq!(first["revision"], 1);
    assert_eq!(first["document"], document);
    assert_eq!(first["document_hash"].as_str().unwrap().len(), 64);
    assert_eq!(success(save("1")), first);
    let before = std::fs::read(&store).unwrap();
    let rejected = save("0");
    assert!(!rejected.status.success() && rejected.stdout.is_empty());
    assert_eq!(std::fs::read(&store).unwrap(), before);

    let resolved = success(run(
        dir.path(),
        &[
            "cart-preferences",
            "--store",
            store.to_str().unwrap(),
            "--event-url",
            "https://ticketmaster.cz/event/renamed/88770901?language=en",
            "--expected-revision",
            "1",
        ],
    ));
    assert_eq!(resolved["revision"], 1);
    assert_eq!(
        resolved["effective_preferences"]["source"],
        "event_override"
    );
    assert_eq!(
        resolved["effective_preferences"]["preferences"]["quantity"],
        3
    );

    let mut changed = document.clone();
    changed["defaults"]["quantity"] = json!(5);
    std::fs::write(&source, serde_json::to_vec(&changed).unwrap()).unwrap();
    let second = success(save("1"));
    assert_eq!(second["revision"], 2);
    assert_ne!(second["document_hash"], first["document_hash"]);
    let stale_read = run(
        dir.path(),
        &[
            "cart-preferences",
            "--store",
            store.to_str().unwrap(),
            "--export",
            "--expected-revision",
            "1",
        ],
    );
    assert!(!stale_read.status.success() && stale_read.stdout.is_empty());
    let exported = success(run(
        dir.path(),
        &[
            "cart-preferences",
            "--store",
            store.to_str().unwrap(),
            "--export",
            "--expected-revision",
            "2",
        ],
    ));
    assert_eq!(exported, changed);
    assert!(exported.get("revision").is_none());
    assert_eq!(exported["schema_version"], 1);
    assert!(!dir.path().join("data").exists());
}

#[test]
fn invalid_preference_commands_do_not_create_state_or_echo_document_contents() {
    let dir = tempfile::tempdir().unwrap();
    let source = dir.path().join("invalid.json");
    let store = dir.path().join("absent/preferences.sqlite");
    std::fs::write(
        &source,
        br#"{"schema_version":99,"private":"private-document-marker"}"#,
    )
    .unwrap();
    for args in [
        vec!["cart-preferences", "--store", store.to_str().unwrap()],
        vec![
            "cart-preferences-save",
            source.to_str().unwrap(),
            "--store",
            store.to_str().unwrap(),
            "--expected-revision",
            "0",
        ],
        vec![
            "cart-preferences",
            "--file",
            source.to_str().unwrap(),
            "--store",
            store.to_str().unwrap(),
        ],
        vec!["cart-preferences", "--export"],
    ] {
        let result = run(dir.path(), &args);
        assert!(!result.status.success() && result.stdout.is_empty());
        assert!(!String::from_utf8_lossy(&result.stderr).contains("private-document-marker"));
        assert!(!store.parent().unwrap().exists());
        assert!(!dir.path().join("data").exists());
    }
}

#[test]
fn trigger_cli_uses_latest_observation_and_versioned_preferences_without_actions() {
    let dir = tempfile::tempdir().unwrap();
    let preferences_path = dir.path().join("preferences.sqlite");
    ticket_watch::preference_store::import(
        &preferences_path,
        &PreferencesDocument::default(),
        0,
        chrono::Utc::now().timestamp_millis(),
    )
    .unwrap();
    let watch = Watch {
        id: "preview-event".into(),
        name: "Synthetic event".into(),
        artist: "Synthetic artist".into(),
        country: "CZ".into(),
        url: "https://www.ticketmaster.cz/event/synthetic/12345678".into(),
        ..Default::default()
    };
    let mut config = Config {
        database: dir.path().join("monitor.sqlite"),
        map_database: dir.path().join("unused-map.sqlite"),
        watches: vec![watch.clone()],
        ..Default::default()
    };
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    let mut saved = Store::open(&config.database).unwrap();
    saved.reconcile(std::slice::from_ref(&watch)).unwrap();
    let observation = Observation {
        watch_id: watch.id.clone(),
        observed_at: chrono::Utc::now().timestamp(),
        availability: Availability::Available,
        evidence: Evidence::Offers,
        complete: true,
        offers: vec![Offer {
            key: "do-not-publish-private-offer-key".into(),
            quantity: Some(2),
            quantity_kind: QuantityKind::Listing,
            price_minor: Some(100000),
            currency: Some("CZK".into()),
            price_scope: PriceScope::PerTicket,
            fees: FeeScope::Unknown,
            section: Some("A".into()),
            row: Some("1".into()),
            ticket_type: Some("Standard".into()),
            ..Default::default()
        }],
        ticket_categories: vec![],
        adapter: None,
        detail: "do-not-publish-private-detail".into(),
    };
    saved.apply(&watch, &observation).unwrap();
    drop(saved);
    let preview = |revision: &str| {
        run(
            dir.path(),
            &[
                "--config",
                config_path.to_str().unwrap(),
                "cart-trigger-preview",
                "--watch",
                &watch.id,
                "--preferences-store",
                preferences_path.to_str().unwrap(),
                "--expected-revision",
                revision,
            ],
        )
    };
    let before_monitor = std::fs::read(&config.database).unwrap();
    let before_preferences = std::fs::read(&preferences_path).unwrap();
    let candidate = success(preview("1"));
    assert_eq!(candidate["needs_fresh_native_preparation"], true);
    assert_eq!(candidate["reservation_authorized"], false);
    assert_eq!(candidate["preference_revision"], 1);
    assert_eq!(
        candidate["effective_preferences"]["preferences"]["require_adjacent"],
        true
    );
    let serialized = candidate.to_string();
    assert!(!serialized.contains("do-not-publish-private"));
    assert_eq!(std::fs::read(&config.database).unwrap(), before_monitor);
    assert_eq!(
        std::fs::read(&preferences_path).unwrap(),
        before_preferences
    );

    let stale_revision = preview("2");
    assert!(!stale_revision.status.success() && stale_revision.stdout.is_empty());
    let mut saved = Store::open(&config.database).unwrap();
    let blocked = Observation {
        availability: Availability::Blocked,
        evidence: Evidence::None,
        offers: vec![],
        ..observation
    };
    saved.apply(&watch, &blocked).unwrap();
    drop(saved);
    assert_eq!(
        success(preview("1"))["needs_fresh_native_preparation"],
        false
    );

    // A changed configured event cannot reuse the older observation's watch ID.
    config.watches[0].url = "https://www.ticketmaster.cz/event/different/12345679".into();
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    let changed = success(preview("1"));
    assert_eq!(changed["needs_fresh_native_preparation"], false);
    assert_eq!(changed["reasons"], json!(["watch_configuration_changed"]));
    config.database = dir.path().join("absent/monitor.sqlite");
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    let missing = preview("1");
    assert!(!missing.status.success() && missing.stdout.is_empty());
    assert!(!config.database.parent().unwrap().exists());
    assert!(!config.map_database.exists());
    assert!(!dir.path().join("data").exists());
}
