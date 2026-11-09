use std::process::Command;

#[test]
fn venue_presets_import_idempotently_and_match_without_a_key_or_network() {
    let directory = tempfile::tempdir().unwrap();
    let seed = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("examples/venue-presets.json");
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .env_remove("TICKETMASTER_API_KEY")
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let first = run(&["venues", "import", seed.to_str().unwrap()]);
    let second = run(&["venues", "import", seed.to_str().unwrap()]);
    assert_eq!(first, second);
    assert_eq!(run(&["venues", "list"]).as_array().unwrap().len(), 21);
    let venue = run(&[
        "venues",
        "match",
        "--site",
        "www.ticketmaster.cz",
        "--name",
        "O2 arena",
        "--city",
        "Praha 9",
        "--country",
        "CZ",
    ]);
    assert_eq!(venue["name"], "O2 arena");
    assert_eq!(venue["layouts"], serde_json::json!([]));
    assert_eq!(
        run(&["venues", "show", venue["id"].as_str().unwrap()]),
        venue
    );
    assert!(
        run(&[
            "venues",
            "match",
            "--site",
            "ticketmaster.cz",
            "--name",
            "O2 arena",
            "--city",
            "Another city",
            "--country",
            "CZ"
        ])
        .is_null()
    );
}

#[test]
fn venue_import_rejects_inventory_fields_without_modifying_database() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("unsafe.json");
    std::fs::write(&file, r#"[{"site_host":"ticketmaster.cz","name":"Arena","city":"Prague","country":"CZ","availability":"available"}]"#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .current_dir(directory.path())
        .args(["venues", "import", file.to_str().unwrap()])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let store =
        ticket_watch::store::Store::open(&directory.path().join("data/ticket.sqlite")).unwrap();
    assert!(store.venue_list().unwrap().is_empty());
}

#[test]
fn discovery_setup_does_not_require_a_watch_configuration() {
    let directory = tempfile::tempdir().unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .current_dir(directory.path())
        .env_remove("TICKETMASTER_API_KEY")
        .args(["artists", "--query", "KoRn"])
        .output()
        .unwrap();
    assert!(!output.status.success());
    let diagnostic = String::from_utf8_lossy(&output.stderr);
    assert!(
        diagnostic.contains("Set TICKETMASTER_API_KEY"),
        "{diagnostic}"
    );
    assert!(!diagnostic.contains("Read configuration"));
}

#[test]
fn offline_replay_proves_alerts_dedup_and_unknown_recovery() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .current_dir(root)
        .args([
            "--config",
            "examples/offline-demo.toml",
            "replay",
            "--watch",
            "demo",
            "tests/fixtures/synthetic-unavailable.html",
            "tests/fixtures/synthetic-offers.html",
            "tests/fixtures/synthetic-offers.html",
            "tests/fixtures/synthetic-blocked.html",
            "tests/fixtures/synthetic-offers.html",
            "tests/fixtures/synthetic-unavailable.html",
            "tests/fixtures/synthetic-offers.html",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let values: Vec<serde_json::Value> = serde_json::Deserializer::from_slice(&output.stdout)
        .into_iter()
        .collect::<Result<_, _>>()
        .unwrap();
    let changes: Vec<u64> = values
        .iter()
        .filter_map(|v| v.get("new_alerts").and_then(|v| v.as_u64()))
        .collect();
    assert_eq!(changes, vec![0, 1, 0, 0, 0, 0, 1]);
    assert_eq!(values.iter().filter(|v| v.get("mode").is_some()).count(), 2);
    assert!(
        values
            .iter()
            .filter_map(|v| v.get("payload"))
            .all(|v| v["allowed_mentions"]["parse"] == serde_json::json!([]))
    );
}

#[test]
fn shipped_configuration_uses_generic_adapter_and_requires_event_selection() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let cfg = ticket_watch::config::load(&root.join("config.example.toml")).unwrap();
    assert_eq!(cfg.watches[0].id, "selected-event");
    assert!(!cfg.watches[0].enabled);
    assert_eq!(
        cfg.watches[0].parser.adapter,
        ticket_watch::config::PageAdapter::Ticketmaster
    );
    assert!(!cfg.watches[0].parser.selectors_verified);
    assert!(!cfg.watches[0].notify_metadata);
    let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .current_dir(root)
        .args(["--config", "config.example.toml", "validate"])
        .output()
        .unwrap();
    assert!(output.status.success());
}

#[test]
fn hybrid_example_requires_explicit_setup_and_exposes_catalog_commands() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = ticket_watch::config::load(&root.join("examples/hybrid.example.toml")).unwrap();
    assert_eq!(config.subscriptions.len(), 1);
    assert!(!config.subscriptions[0].enabled);
    assert!(!config.profiles[0].parser.selectors_verified);
    let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .args(["--help"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("sync"));
    assert!(help.contains("catalog"));
}

#[test]
fn cart_catalog_is_explicitly_configuration_only_without_monitor_state() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/cart-catalog-cz.html");
    let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .current_dir(directory.path())
        .env_remove("TICKETMASTER_API_KEY")
        .env_remove("DISCORD_BOT_TOKEN")
        .args([
            "cart-catalog",
            "--url",
            "https://www.ticketmaster.cz/event/synthetic-vstupenky/1234567890",
            "--html",
            fixture.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(value["source"], "saved_html");
    assert_eq!(value["transport_binding"], "unverified_saved_html");
    assert_eq!(value["reservation_attempted"], false);
    assert_eq!(value["catalog"]["evidence"], "configuration_only");
    assert_eq!(value["catalog"]["stock_verified"], false);
    assert_eq!(value["catalog"]["hold_verified"], false);
    assert!(!directory.path().join("data").exists());
}

#[test]
fn cart_catalog_rejects_unsupported_events_before_browser_start() {
    let directory = tempfile::tempdir().unwrap();
    for url in [
        "https://www.ticketmaster.pl/event/synthetic/1234567890",
        "https://www.ticketmaster.cz/artist/korn-vstupenky/1234567890",
        "https://www.ticketmaster.cz/checkout/1234567890",
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .args(["cart-catalog", "--url", url])
            .output()
            .unwrap();
        assert!(!output.status.success());
        let detail = String::from_utf8_lossy(&output.stderr);
        assert!(
            detail.contains("public Ticketmaster Czech event URL"),
            "{detail}"
        );
    }
    assert!(!directory.path().join("data").exists());
}

#[test]
fn cart_plan_refuses_missing_quantity_and_oversized_input_without_side_effects() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("quotes.json");
    let event_url = "https://www.ticketmaster.cz/event/synthetic/1234567890";
    let at = chrono::Utc::now().timestamp_millis();
    let request = serde_json::json!({
        "intent": {"event_url": event_url},
        "context": {"session_id":"synthetic-session", "document_id":"synthetic-document"},
        "snapshot": {
            "event_url":event_url, "session_id":"synthetic-session",
            "document_id":"synthetic-document", "requested_quantity":2,
            "observed_at_ms":at, "expires_at_ms":at + 5000,
            "coverage":"complete_for_request", "groups":[]
        }
    });
    let run = || {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .env_remove("TICKETMASTER_API_KEY")
            .env_remove("DISCORD_BOT_TOKEN")
            .args(["cart-plan", file.to_str().unwrap()])
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    std::fs::write(&file, serde_json::to_vec(&request).unwrap()).unwrap();
    let missing = run();
    assert_eq!(missing["outcome"], "refused");
    assert_eq!(missing["reason"], "missing_quantity");
    assert_eq!(missing["reservation_performed"], false);
    assert_eq!(missing["reservation_authorized"], false);
    assert!(missing["selected"].is_null());
    std::fs::write(&file, vec![b'x'; ticket_watch::cart::MAX_INPUT_BYTES + 1]).unwrap();
    assert_eq!(run()["reason"], "input_too_large");
    assert!(!directory.path().join("data").exists());
}
