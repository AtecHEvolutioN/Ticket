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

#[test]
fn map_inspection_and_saved_layouts_keep_events_and_inventory_separate() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = include_str!("fixtures/seat-map-regional.html");
    let file = directory.path().join("event.html");
    let url = "https://www.ticketmaster.cz/event/synthetic-concert/1234567890";
    std::fs::write(&file, fixture).unwrap();
    let run = |extra: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .env_remove("TICKETMASTER_API_KEY")
            .env_remove("DISCORD_BOT_TOKEN")
            .args([
                "map-inspect",
                "--url",
                url,
                "--html",
                file.to_str().unwrap(),
            ])
            .args(extra)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let preview = run(&[]);
    assert_eq!(preview["transport_binding"], "unverified_saved_html");
    assert_eq!(preview["map"]["geometry_verified"], false);
    assert_eq!(preview["map"]["stock_verified"], false);
    assert_eq!(preview["map"]["adjacency_verified"], false);
    assert!(preview["saved_venue"].is_null());
    assert!(!directory.path().join("data").exists());
    let first = run(&["--save-layout"]);
    let repeated = run(&["--save-layout"]);
    assert_eq!(first["saved_venue"], repeated["saved_venue"]);
    assert_eq!(first["saved_venue"]["layouts"].as_array().unwrap().len(), 1);
    assert!(first["saved_venue"]["layouts"][0]["verified_at"].is_null());
    assert_eq!(
        first["saved_venue"]["provider_venue_id"],
        "Z598xSyntheticVenue"
    );
    {
        let mut store =
            ticket_watch::store::Store::open(&directory.path().join("data/ticket.sqlite")).unwrap();
        let mut preset = store.venue_list().unwrap().remove(0);
        preset.layouts[0].name = "My reviewed arrangement".into();
        preset.layouts[0].verified_at = Some(1_800_000_000);
        store.venue_upsert(&preset).unwrap();
    }
    let refreshed = run(&["--save-layout"]);
    assert_eq!(
        refreshed["saved_venue"]["layouts"][0]["name"],
        "My reviewed arrangement"
    );
    assert_eq!(
        refreshed["saved_venue"]["layouts"][0]["verified_at"],
        1_800_000_000
    );
    std::fs::write(
        &file,
        fixture.replace("synthetic-concert", "renamed-concert"),
    )
    .unwrap();
    let renamed = run(&["--save-layout"]);
    assert_eq!(
        renamed["saved_venue"]["layouts"][0]["source_event_urls"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    std::fs::write(&file, fixture.replace("map-a.png", "map-b.png")).unwrap();
    let second = run(&["--save-layout"]);
    assert_eq!(first["saved_venue"]["id"], second["saved_venue"]["id"]);
    assert_eq!(
        second["saved_venue"]["layouts"].as_array().unwrap().len(),
        2
    );
    assert_ne!(
        first["map"]["static_map"]["layout_fingerprint"],
        second["map"]["static_map"]["layout_fingerprint"]
    );
    let connection =
        rusqlite::Connection::open(directory.path().join("data/ticket.sqlite")).unwrap();
    for table in ["watches", "watch_observations", "outbox"] {
        let count: i64 = connection
            .query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 0);
    }
}

#[test]
fn map_image_existing_output_is_not_overwritten_or_downloaded() {
    let directory = tempfile::tempdir().unwrap();
    let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/seat-map-regional.html");
    let image = directory.path().join("map.png");
    std::fs::write(&image, b"keep original").unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .current_dir(directory.path())
        .args([
            "map-inspect",
            "--url",
            "https://www.ticketmaster.cz/event/synthetic-concert/1234567890",
            "--html",
            fixture.to_str().unwrap(),
            "--map-image",
            image.to_str().unwrap(),
        ])
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("already exists"));
    assert_eq!(std::fs::read(&image).unwrap(), b"keep original");
    assert!(!directory.path().join("data").exists());
}

#[test]
fn map_geometry_export_is_bound_to_the_event_and_never_creates_stock() {
    let directory = tempfile::tempdir().unwrap();
    let file = directory.path().join("event.html");
    let output_file = directory.path().join("sections.svg");
    let base = include_str!("fixtures/seat-map-regional.html");
    let geometry = include_str!("fixtures/map-geometry.html");
    let start = geometry.find("<svg ").unwrap();
    let end = geometry.find("</svg>").unwrap() + "</svg>".len();
    std::fs::write(
        &file,
        base.replace("</aside>", &format!("{}</aside>", &geometry[start..end])),
    )
    .unwrap();
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .args([
                "map-inspect",
                "--url",
                "https://www.ticketmaster.cz/event/synthetic-concert/1234567890",
                "--html",
                file.to_str().unwrap(),
                "--geometry-svg",
                output_file.to_str().unwrap(),
            ])
            .output()
            .unwrap()
    };
    let result = run();
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let value: serde_json::Value = serde_json::from_slice(&result.stdout).unwrap();
    assert_eq!(
        value["map"]["rendered_geometry"]["sections"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_eq!(
        value["map"]["rendered_geometry"]["individual_seats_verified"],
        false
    );
    assert_eq!(value["map"]["stock_verified"], false);
    assert_eq!(value["exported_geometry"]["format"], "sanitized_svg");
    let svg = std::fs::read_to_string(&output_file).unwrap();
    assert!(svg.contains("A1") && svg.contains("Floor"));
    assert!(!svg.contains("<script") && !svg.contains("data-active"));
    assert!(!run().status.success());
    assert_eq!(std::fs::read_to_string(&output_file).unwrap(), svg);
    assert!(!directory.path().join("data").exists());
    std::fs::remove_file(&output_file).unwrap();
    std::fs::write(&file, base).unwrap();
    let missing = run();
    assert!(!missing.status.success());
    assert!(String::from_utf8_lossy(&missing.stderr).contains("No captured section geometry"));
    assert!(!output_file.exists());
}

#[test]
fn map_section_flags_require_a_live_section_target_before_any_fetch() {
    let directory = tempfile::tempdir().unwrap();
    let url = "https://www.ticketmaster.cz/event/example/12345678";
    for extra in [
        vec!["--seat-id", "seat_1"],
        vec!["--section-id", "s_1", "--html", "missing.html"],
        vec![
            "--section-id",
            "s_1",
            "--seat-id",
            "seat_1",
            "--html",
            "missing.html",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .args(["map-inspect", "--url", url])
            .args(extra)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("--section-id") || error.contains("--html"));
        assert!(!error.contains("Open HTML") && !error.contains("Browser worker"));
        assert!(output.stdout.is_empty());
    }
    assert!(!directory.path().join("data").exists());
}

#[test]
fn map_section_cli_validates_hover_evidence_without_turning_it_into_stock() {
    let directory = tempfile::tempdir().unwrap();
    let svg = |html: &str| {
        let start = html.find("<svg ").unwrap();
        let end = html.find("</svg>").unwrap() + "</svg>".len();
        html[start..end].to_owned()
    };
    let overview = svg(include_str!("fixtures/map-geometry.html"))
        .replace("s_a", "s_12")
        .replace("A1", "110");
    let seats = svg(include_str!("fixtures/seat-geometry.html"));
    let page = include_str!("fixtures/seat-map-regional.html")
        .replace("</aside>", &format!("{overview}{seats}</aside>"));
    std::fs::write(directory.path().join("event.html"), page).unwrap();
    std::fs::create_dir(directory.path().join("browser")).unwrap();
    std::fs::write(
        directory.path().join("browser/worker.cjs"),
        r#"
        const fs=require('node:fs');
        require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
            const r=JSON.parse(line);
            if(r.adapter!=='ticketmaster'||r.display!=='map'||r.map_section_id!=='s_12'||r.map_seat_id!=='seat-2') {
                process.stdout.write(JSON.stringify({id:r.id,error:'invalid_request'})+'\n'); return;
            }
            process.stdout.write(JSON.stringify({id:r.id,status:200,adapter_state:'ready',
                final_url:r.url,html:fs.readFileSync('event.html','utf8'),
                map_inspection:JSON.parse(fs.readFileSync('inspection.json','utf8'))})+'\n');
        });
        "#,
    )
    .unwrap();
    let mut inspection = serde_json::json!({
        "section_id":"s_12", "section_name":"110", "seat_id":"seat-2",
        "coverage":"section_only", "stock_verified":false, "adjacency_verified":false,
        "tooltip":{
            "zone":"1. POSCHODÍ", "section":"110", "row":"12", "seat":"2",
            "ticket_type":"Ověřené přeprodávané vstupenky", "restriction":null,
            "price_text":"1 234,50 Kč za kus"
        }
    });
    let run = || {
        Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .args([
                "map-inspect",
                "--url",
                "https://www.ticketmaster.cz/event/synthetic-concert/1234567890",
                "--section-id",
                "s_12",
                "--seat-id",
                "seat-2",
            ])
            .output()
            .unwrap()
    };
    let evidence_file = directory.path().join("inspection.json");
    std::fs::write(&evidence_file, serde_json::to_vec(&inspection).unwrap()).unwrap();
    let output = run();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let observed = &report["section_inspection"];
    assert_eq!(observed["rendered_seat_count"], 3);
    assert_eq!(observed["displayed_quote"]["price_minor"], 123450);
    assert_eq!(observed["displayed_quote"]["price_basis"], "per_ticket");
    assert_eq!(observed["displayed_quote"]["fees_complete"], false);
    assert!(observed["displayed_quote"]["available_quantity"].is_null());
    assert_eq!(observed["stock_verified"], false);
    assert_eq!(observed["adjacency_verified"], false);
    assert_eq!(report["map"]["stock_verified"], false);
    assert!(report["saved_venue"].is_null());
    inspection["tooltip"]["row"] = "99".into();
    std::fs::write(&evidence_file, serde_json::to_vec(&inspection).unwrap()).unwrap();
    let mismatch = run();
    assert!(!mismatch.status.success());
    assert!(String::from_utf8_lossy(&mismatch.stderr).contains("does not match hovered seat"));
    assert!(mismatch.stdout.is_empty());
    assert!(!directory.path().join("data").exists());
}

#[test]
fn map_scan_requires_a_section_and_bounded_limit_without_offline_or_seat_options() {
    let directory = tempfile::tempdir().unwrap();
    for extra in [
        vec![],
        vec!["--section-id", "s_12", "--limit", "0"],
        vec!["--section-id", "s_12", "--limit", "51"],
        vec!["--section-id", "s_12", "--html", "missing.html"],
        vec!["--section-id", "s_12", "--seat-id", "seat-2"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .args([
                "map-scan",
                "--url",
                "https://www.ticketmaster.cz/event/example/12345678",
            ])
            .args(extra)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    assert!(!directory.path().join("data").exists());
}

#[test]
fn map_scan_cli_discovers_prices_and_records_only_validated_observations() {
    let directory = tempfile::tempdir().unwrap();
    let svg =
        |html: &str| html[html.find("<svg ").unwrap()..html.find("</svg>").unwrap() + 6].to_owned();
    let overview = svg(include_str!("fixtures/map-geometry.html"))
        .replace("s_a", "s_12")
        .replace("A1", "110");
    let seats = svg(include_str!("fixtures/seat-geometry.html"));
    let page = include_str!("fixtures/seat-map-regional.html")
        .replace("</aside>", &format!("{overview}{seats}</aside>"));
    std::fs::write(directory.path().join("event.html"), page).unwrap();
    std::fs::create_dir(directory.path().join("browser")).unwrap();
    std::fs::write(directory.path().join("browser/worker.cjs"), r#"
        const fs=require('node:fs');
        require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
            const r=JSON.parse(line), now=Date.now();
            if(r.adapter!=='ticketmaster'||r.display!=='map'||r.map_section_id!=='s_12'||r.map_scan_limit!==3||r.map_seat_id!==undefined){
                process.stdout.write(JSON.stringify({id:r.id,error:'invalid_request'})+'\n');return;
            }
            const tip=JSON.parse(fs.readFileSync('tooltip.json','utf8'));
            const scan={section_id:'s_12',section_name:'110',limit:3,
                started_at_ms:now-2,finished_at_ms:now,candidate_count:1,visible_candidate_count:1,
                results:[{seat_id:'seat-2',row:'12',seat:'2',observed_at_ms:now-1,tooltip:tip,outcome:'quoted'}],
                stop_reason:'visible_candidates_exhausted',coverage:'partial_rendered_section',
                stock_verified:false,adjacency_verified:false};
            process.stdout.write(JSON.stringify({id:r.id,status:200,adapter_state:'ready',elapsed_ms:2,
                final_url:r.url,html:fs.readFileSync('event.html','utf8'),map_scan:scan})+'\n');
        });
    "#).unwrap();
    let mut tooltip = serde_json::json!({"zone":"1. POSCHODÍ","section":"110","row":"12","seat":"2",
        "ticket_type":"Ověřené přeprodávané vstupenky","restriction":null,"price_text":"1 234,50 Kč za kus"});
    let tooltip_file = directory.path().join("tooltip.json");
    std::fs::write(&tooltip_file, serde_json::to_vec(&tooltip).unwrap()).unwrap();
    let run = |state: Option<&str>| {
        let mut command = Command::new(env!("CARGO_BIN_EXE_ticket-watch"));
        command
            .current_dir(directory.path())
            .env_remove("TICKETMASTER_API_KEY")
            .env_remove("DISCORD_BOT_TOKEN")
            .args([
                "--config",
                "missing-config.toml",
                "map-scan",
                "--url",
                "https://www.ticketmaster.cz/event/synthetic-concert/1234567890",
                "--section-id",
                "s_12",
                "--limit",
                "3",
            ]);
        if let Some(path) = state {
            command.args(["--state", path]);
        }
        command.output().unwrap()
    };
    let check = |output: std::process::Output| {
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice::<serde_json::Value>(&output.stdout).unwrap()
    };
    let first = check(run(None));
    assert_eq!(first["source"], "live_browser");
    assert_eq!(first["transport_binding"], "observed_main_document");
    assert_eq!(first["elapsed_ms"], 2);
    assert_eq!(first["scan"]["quoted_count"], 1);
    assert_eq!(
        first["scan"]["observations"][0]["quote"]["price_minor"],
        123450
    );
    assert_eq!(first["scan"]["stock_verified"], false);
    assert!(first["state_changes"].is_null());
    assert!(!directory.path().join("data").exists());
    let recorded = check(run(Some("scan.sqlite")));
    assert_eq!(
        recorded["state_changes"]["changes"][0]["kind"],
        "first_observed"
    );
    let repeated = check(run(Some("scan.sqlite")));
    assert!(
        repeated["state_changes"]["changes"]
            .as_array()
            .unwrap()
            .is_empty()
    );
    tooltip["row"] = "99".into();
    std::fs::write(&tooltip_file, serde_json::to_vec(&tooltip).unwrap()).unwrap();
    let invalid = run(Some("rejected.sqlite"));
    assert!(!invalid.status.success());
    assert!(invalid.stdout.is_empty());
    assert!(!directory.path().join("rejected.sqlite").exists());
    assert!(!directory.path().join("data").exists());
}

#[test]
fn new_status_commands_do_not_create_missing_databases() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::write(
        directory.path().join("monitor.toml"),
        "map_database='absent.sqlite'\n",
    )
    .unwrap();
    for args in [
        vec!["--config", "monitor.toml", "map-status"],
        vec![
            "--config",
            "missing.toml",
            "cart-journal",
            "--file",
            "absent.sqlite",
        ],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .args(args)
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(output.stdout.is_empty());
        assert!(!directory.path().join("absent.sqlite").exists());
        assert!(!directory.path().join("data").exists());
    }
}

#[test]
fn cart_controls_cli_requires_explicit_demo_and_never_creates_cart_state() {
    let directory = tempfile::tempdir().unwrap();
    let url = "https://www.ticketmaster.cz/event/synthetic-vstupenky/1234567890";
    for extra in [
        vec![],
        vec!["--demo-quantity", "0"],
        vec!["--demo-quantity", "21"],
        vec!["--demo-quantity", "2", "--html", "offline.html"],
    ] {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .args(["cart-controls", "--url", url, "--category-id", "2"])
            .args(extra)
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
        assert!(output.stdout.is_empty());
    }
    std::fs::create_dir(directory.path().join("browser")).unwrap();
    std::fs::write(
        directory.path().join("page.html"),
        include_str!("fixtures/cart-catalog-cz.html"),
    )
    .unwrap();
    std::fs::write(directory.path().join("browser/worker.cjs"),r#"
        const fs=require('node:fs');
        require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
            const r=JSON.parse(line),now=Date.now();
            if(r.display!=='cart_controls'||r.adapter!=='ticketmaster'||r.cart_category_id!=='2'||r.cart_demo_quantity!==2||r.map_section_id!==undefined){
                process.stdout.write(JSON.stringify({id:r.id,error:'invalid_request'})+'\n');return;
            }
            const controls={event_url:r.url,event_id:'1234567890',category_id:'2',category_title:'Sezení',demo_quantity:2,
                initial_quantity:0,final_quantity:0,started_at_ms:now-100,finished_at_ms:now,
                network_disabled_before_changes:true,form_restored:true,
                panels:[{kind:'price',heading:'Ceny',opened:true,disabled:false,options:[{checkbox_id:'price-1',label:'1 889,00 Kč',checked:false,disabled:false}]},
                    {kind:'section',heading:'Všechny sekce',opened:false,disabled:true,options:[]}],
                stock_verified:false,adjacency_verified:false,fees_complete:false,hold_verified:false,reservation_attempted:false};
            process.stdout.write(JSON.stringify({id:r.id,status:200,final_url:r.url,adapter_state:'ready',elapsed_ms:100,
                html:fs.readFileSync('page.html','utf8'),cart_controls:controls})+'\n');
        });
    "#).unwrap();
    let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .current_dir(directory.path())
        .env_remove("TICKETMASTER_API_KEY")
        .env_remove("DISCORD_BOT_TOKEN")
        .args([
            "--config",
            "missing.toml",
            "cart-controls",
            "--url",
            url,
            "--category-id",
            "2",
            "--demo-quantity",
            "2",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let output: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(output["source"], "live_browser_offline_form_demo");
    assert_eq!(output["report"]["controls"]["demo_quantity"], 2);
    assert_eq!(output["report"]["catalog"]["current_selected_count"], 0);
    assert_eq!(output["report"]["purchase_quantity_configured"], false);
    assert_eq!(output["report"]["reservation_authorized"], false);
    assert!(!directory.path().join("data").exists());
}

#[test]
fn map_run_cli_previews_then_respects_restart_pacing_and_disabled_watches() {
    let directory = tempfile::tempdir().unwrap();
    let svg =
        |html: &str| html[html.find("<svg ").unwrap()..html.find("</svg>").unwrap() + 6].to_owned();
    let overview = svg(include_str!("fixtures/map-geometry.html"))
        .replace("s_a", "s_12")
        .replace("A1", "110");
    let seats = svg(include_str!("fixtures/seat-geometry.html"));
    let page = include_str!("fixtures/seat-map-regional.html")
        .replace("</aside>", &format!("{overview}{seats}</aside>"));
    std::fs::write(directory.path().join("event.html"), page).unwrap();
    std::fs::create_dir(directory.path().join("browser")).unwrap();
    std::fs::write(directory.path().join("browser/worker.cjs"), r#"
        const fs=require('node:fs');
        require('node:readline').createInterface({input:process.stdin}).on('line',line=>{
            const r=JSON.parse(line), now=Date.now();
            fs.appendFileSync('requests.log','scan\n');
            if(r.adapter!=='ticketmaster'||r.display!=='map'||r.map_section_id!=='s_12'||r.map_scan_limit!==3){
                process.stdout.write(JSON.stringify({id:r.id,error:'invalid_request'})+'\n');return;
            }
            const scan={section_id:'s_12',section_name:'110',limit:3,
                started_at_ms:now-2,finished_at_ms:now,candidate_count:1,visible_candidate_count:1,
                results:[{seat_id:'seat-2',row:'12',seat:'2',observed_at_ms:now-1,outcome:'quoted',
                    tooltip:{zone:'1. POSCHODÍ',section:'110',row:'12',seat:'2',
                        ticket_type:'Ověřené přeprodávané vstupenky',restriction:null,
                        price_text:'1 234,50 Kč za kus'}}],
                stop_reason:'visible_candidates_exhausted',coverage:'partial_rendered_section',
                stock_verified:false,adjacency_verified:false};
            process.stdout.write(JSON.stringify({id:r.id,status:200,adapter_state:'ready',elapsed_ms:2,
                final_url:r.url,html:fs.readFileSync('event.html','utf8'),map_scan:scan})+'\n');
        });
    "#).unwrap();
    let config = r#"
        map_database='monitor.sqlite'
        per_host_interval_secs=3600
        [discord]
        token_env='TICKET_TEST_MISSING_MAP_DISCORD_TOKEN'
        [[map_watches]]
        id='synthetic-section'
        name='Synthetic section 110'
        url='https://www.ticketmaster.cz/event/synthetic-concert/1234567890'
        section_id='s_12'
        limit=3
        interval_secs=3600
        enabled=true
        notify_initial=true
    "#;
    let config_path = directory.path().join("monitor.toml");
    std::fs::write(&config_path, config).unwrap();
    let health = || {
        Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .env_remove("TICKET_TEST_MISSING_MAP_DISCORD_TOKEN")
            .args([
                "--config",
                "monitor.toml",
                "map-health",
                "--max-age-secs",
                "120",
            ])
            .output()
            .unwrap()
    };
    let missing = health();
    assert!(!missing.status.success());
    let missing: serde_json::Value = serde_json::from_slice(&missing.stdout).unwrap();
    assert_eq!(missing["status"], "not_initialized");
    assert!(!directory.path().join("monitor.sqlite").exists());
    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
            .current_dir(directory.path())
            .env_remove("TICKETMASTER_API_KEY")
            .env_remove("DISCORD_BOT_TOKEN")
            .env_remove("TICKET_TEST_MISSING_MAP_DISCORD_TOKEN")
            .args(["--config", "monitor.toml"])
            .args(args)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::Deserializer::from_slice(&output.stdout)
            .into_iter::<serde_json::Value>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let first = run(&["map-run", "--once"]);
    let preview = first
        .iter()
        .find(|entry| entry.get("outbox_id").is_some())
        .unwrap();
    assert_eq!(preview["mode"], "preview_only");
    let payload = preview["payload"].to_string();
    assert!(payload.contains("1 234,50 Kč za kus"), "{payload}");
    assert!(payload.contains("unverified"), "{payload}");
    let status = run(&["map-status"]);
    assert_eq!(status[0]["watches"][0]["health"], "ok");
    assert_eq!(status[0]["watches"][0]["pending_alerts"], 1);
    let current = health();
    assert!(
        current.status.success(),
        "{}",
        String::from_utf8_lossy(&current.stderr)
    );
    let current: serde_json::Value = serde_json::from_slice(&current.stdout).unwrap();
    assert_eq!(current["healthy"], true);
    assert_eq!(current["stock_verified"], false);
    assert_eq!(current["delivery_verified"], false);
    assert_eq!(current["process_running_verified"], false);
    assert!(
        status[0]["cooldowns"]["ticketmaster_cz_until_ms"]
            .as_i64()
            .unwrap()
            > status[0]["watches"][0]["last_observed_at_ms"]
                .as_i64()
                .unwrap()
    );

    let repeated = run(&["map-run", "--once"]);
    assert!(repeated.iter().any(|entry| entry["completed_watches"] == 0));
    let second_preview = repeated
        .iter()
        .find(|entry| entry.get("outbox_id").is_some())
        .unwrap();
    assert_eq!(preview["outbox_id"], second_preview["outbox_id"]);
    assert_eq!(
        std::fs::read_to_string(directory.path().join("requests.log")).unwrap(),
        "scan\n"
    );

    std::fs::write(
        &config_path,
        config.replace("enabled=true", "enabled=false"),
    )
    .unwrap();
    run(&["map-run", "--once"]);
    let disabled = run(&["map-status"]);
    assert_eq!(disabled[0]["watches"][0]["enabled"], false);
    assert_eq!(disabled[0]["watches"][0]["pending_alerts"], 0);
    let inactive = health();
    assert!(!inactive.status.success());
    let inactive: serde_json::Value = serde_json::from_slice(&inactive.stdout).unwrap();
    assert_eq!(inactive["status"], "inactive");
    assert_eq!(
        std::fs::read_to_string(directory.path().join("requests.log")).unwrap(),
        "scan\n"
    );
    assert!(!directory.path().join("data/ticket.sqlite").exists());
}
