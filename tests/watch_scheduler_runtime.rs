//! Exercise the real run loop against a local browser-protocol stub. No browser,
//! provider connection, Discord message or real credentials are used.
use std::{
    fs,
    process::{Command, Output, Stdio},
    thread,
    time::{Duration, Instant},
};
use tempfile::TempDir;
use ticket_watch::config::{Config, FetchMode};

fn fixture(block: bool) -> (TempDir, std::path::PathBuf, std::path::PathBuf) {
    let temp = TempDir::new().unwrap();
    let mut config: Config = toml::from_str(include_str!("../examples/offline-demo.toml")).unwrap();
    config.database = temp.path().join("state.sqlite");
    config.map_database = temp.path().join("map-unused.sqlite");
    config.per_host_interval_secs = 1;
    config.max_in_flight = 32;
    config.request_timeout_secs = 2;
    let prototype = config.watches.remove(0);
    config.watches = ["100", "200"]
        .into_iter()
        .map(|id| {
            let mut watch = prototype.clone();
            watch.id = id.into();
            watch.mode = FetchMode::Browser;
            watch.url = format!("https://www.ticketmaster.nl/event/{id}");
            watch
        })
        .collect();
    let log = temp.path().join("requests.jsonl");
    let worker = temp.path().join("worker.cjs");
    fs::write(&worker, format!(r#"
const fs = require('node:fs');
const lines = require('node:readline').createInterface({{input:process.stdin}});
lines.on('line', line => {{
    const request = JSON.parse(line);
    fs.appendFileSync({log},JSON.stringify({{url:request.url,at:Date.now()}})+'\n');
    process.stdout.write(JSON.stringify({{id:request.id,status:{status},html:{html},final_url:request.url,elapsed_ms:1,retry_after_secs:{retry}}})+'\n');
}});
"#, log=serde_json::to_string(&log).unwrap(), status=if block {429} else {200},
    html=serde_json::to_string(include_str!("fixtures/synthetic-unavailable.html")).unwrap(),
    retry=if block {"60"} else {"null"})).unwrap();
    config.browser.script = worker;
    let file = temp.path().join("config.toml");
    fs::write(&file, toml::to_string(&config).unwrap()).unwrap();
    (temp, file, log)
}

fn run(config: &std::path::Path) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .args(["--config", config.to_str().unwrap(), "run", "--once"])
        .env_remove("TICKETMASTER_API_KEY")
        .env_remove("DISCORD_BOT_TOKEN")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if child.try_wait().unwrap().is_some() {
            break;
        }
        if Instant::now() >= deadline {
            child.kill().unwrap();
            let output = child.wait_with_output().unwrap();
            panic!(
                "offline run did not complete: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        thread::sleep(Duration::from_millis(20));
    }
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn high_concurrency_still_respects_shared_host_pacing_and_reports_measured_timings() {
    let (_temp, config, log) = fixture(false);
    let output = run(&config);
    let requests: Vec<serde_json::Value> = fs::read_to_string(log)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(requests.len(), 2);
    assert!(requests[1]["at"].as_u64().unwrap() - requests[0]["at"].as_u64().unwrap() >= 900);
    let stdout = String::from_utf8(output.stdout).unwrap();
    let observations: Vec<serde_json::Value> = serde_json::Deserializer::from_str(&stdout)
        .into_iter::<serde_json::Value>()
        .map(Result::unwrap)
        .filter(|value| value.get("timing").is_some())
        .collect();
    assert_eq!(observations.len(), 2);
    for observation in observations {
        let timing = &observation["timing"];
        assert_eq!(observation["scheduling"]["priority"], "manual");
        assert!(
            timing["fetch_to_recorded_ms"].as_u64().unwrap()
                >= timing["fetch_to_observation_ms"].as_u64().unwrap()
        );
        assert!(
            timing["observation_to_recorded_ms"].as_u64().unwrap()
                >= timing["durable_apply_ms"].as_u64().unwrap()
        );
        assert_eq!(timing["transport_reported_ms"], 1);
    }
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("scheduler_status")
    );
}

#[test]
fn provider_block_stops_other_same_host_jobs_and_survives_restart() {
    let (_temp, config, log) = fixture(true);
    let first = run(&config);
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);
    assert!(
        String::from_utf8(first.stderr)
            .unwrap()
            .contains("host cooldown active")
    );
    let second = run(&config);
    assert_eq!(fs::read_to_string(&log).unwrap().lines().count(), 1);
    assert!(
        String::from_utf8(second.stderr)
            .unwrap()
            .contains("host cooldown active")
    );
}
