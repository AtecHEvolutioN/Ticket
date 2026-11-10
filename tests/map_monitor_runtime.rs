use std::{path::Path, time::Duration};

use rusqlite::Connection;
use ticket_watch::{
    config::{Config, MapWatch},
    map_monitor::{run, run_with_shutdown},
    map_monitor_store::MapMonitorStore,
};

fn fixture(directory: &Path) -> Config {
    fn svg(input: &str) -> &str {
        &input[input.find("<svg").unwrap()..input.rfind("</svg>").unwrap() + 6]
    }
    let overview = include_str!("fixtures/map-geometry.html")
        .replace("s_a", "s_12")
        .replace("A1", "110");
    let html = include_str!("fixtures/seat-map-regional.html").replace(
        "</aside>",
        &format!(
            "{}{}</aside>",
            svg(&overview),
            svg(include_str!("fixtures/seat-geometry.html"))
        ),
    );
    std::fs::write(directory.join("page.html"), html).unwrap();
    std::fs::write(directory.join("mode"), "ready").unwrap();
    std::fs::write(directory.join("worker.cjs"), r#"
const fs = require('node:fs'), path = require('node:path');
const file = name => path.join(__dirname, name);
fs.writeFileSync(file('pid'), String(process.pid));
require('node:readline').createInterface({input:process.stdin}).on('line', line => {
  const r=JSON.parse(line), mode=fs.readFileSync(file('mode'),'utf8');
  fs.appendFileSync(file('requests'), JSON.stringify(r)+'\n');
  if(mode==='hang') { setTimeout(()=>{},30000); return; }
  const now=Date.now();
  const scan={section_id:r.map_section_id,section_name:'110',limit:r.map_scan_limit,
    started_at_ms:now-10,finished_at_ms:now,candidate_count:1,visible_candidate_count:1,
    results:[{seat_id:'seat-2',row:'12',seat:'2',observed_at_ms:now-1,outcome:'quoted',
      tooltip:{zone:'1. POSCHODÍ',section:'110',row:'12',seat:'2',
        ticket_type:'Ověřené přeprodávané vstupenky',restriction:null,price_text:'1 234,50 Kč za kus'}}],
    stop_reason:'visible_candidates_exhausted',coverage:'partial_rendered_section',
    stock_verified:false,adjacency_verified:false};
  const blocked=mode==='blocked', invalid=mode==='invalid';
  if(invalid) scan.candidate_count=2;
  process.stdout.write(JSON.stringify({id:r.id,status:blocked?403:200,
    adapter_state:blocked?'blocked':'ready',retry_after_secs:blocked?120:undefined,
    html:blocked?'':fs.readFileSync(file('page.html'),'utf8'),final_url:r.url,
    map_scan:blocked?undefined:scan})+'\n');
});
"#).unwrap();
    Config {
        database: directory.join("inventory.sqlite"),
        map_database: directory.join("map.sqlite"),
        request_timeout_secs: 2,
        per_host_interval_secs: 1,
        blocked_retry_secs: 60,
        unknown_retry_secs: 1,
        browser: ticket_watch::config::BrowserConfig {
            script: directory.join("worker.cjs"),
            ..Default::default()
        },
        map_watches: vec![MapWatch {
            id: "example".into(),
            name: "Synthetic concert".into(),
            url: "https://www.ticketmaster.cz/event/synthetic-concert/1234567890".into(),
            section_id: "s_12".into(),
            interval_secs: 1,
            notify_initial: true,
            ..Default::default()
        }],
        ..Default::default()
    }
}

fn requests(directory: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(directory.join("requests"))
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn release_due(config: &Config) {
    let connection = Connection::open(&config.map_database).unwrap();
    connection
        .execute("UPDATE map_monitor_watches SET next_due_ms=1", [])
        .unwrap();
    connection
        .execute("UPDATE map_monitor_cooldowns SET until_ms=1", [])
        .unwrap();
}

#[tokio::test]
async fn blocked_restart_preserves_prices_and_last_success_and_honors_retry_after() {
    let dir = tempfile::tempdir().unwrap();
    let config = fixture(dir.path());
    run(config.clone(), true, false).await.unwrap();
    let first = MapMonitorStore::open(&config.map_database)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(first[0].health, "ok");
    assert_eq!(first[0].pending_alerts, 1);
    release_due(&config);
    std::fs::write(dir.path().join("mode"), "blocked").unwrap();
    run(config.clone(), true, false).await.unwrap();
    let store = MapMonitorStore::open(&config.map_database).unwrap();
    let blocked = store.status().unwrap();
    assert_eq!(blocked[0].health, "blocked");
    assert_eq!(blocked[0].last_success_at_ms, first[0].last_success_at_ms);
    assert_eq!(blocked[0].pending_alerts, 0);
    assert!(
        store.cooldown("host:ticketmaster.cz").unwrap().unwrap()
            >= blocked[0].last_observed_at_ms.unwrap() + 120_000
    );
    let connection = Connection::open(&config.map_database).unwrap();
    let count: i64 = connection
        .query_row("SELECT COUNT(*) FROM map_scan_quotes", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
    // Make the watch due while preserving the host cooldown: a restart still skips it.
    connection
        .execute("UPDATE map_monitor_watches SET next_due_ms=1", [])
        .unwrap();
    drop(connection);
    drop(store);
    run(config.clone(), true, false).await.unwrap();
    assert_eq!(requests(dir.path()).len(), 2);
}

#[tokio::test]
async fn invalid_capture_is_unknown_and_does_not_replace_successful_history() {
    let dir = tempfile::tempdir().unwrap();
    let config = fixture(dir.path());
    run(config.clone(), true, false).await.unwrap();
    let first = MapMonitorStore::open(&config.map_database)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(first[0].health, "ok");
    release_due(&config);
    std::fs::write(dir.path().join("mode"), "invalid").unwrap();
    run(config.clone(), true, false).await.unwrap();
    let after = MapMonitorStore::open(&config.map_database)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(after[0].health, "unknown");
    assert_eq!(after[0].last_success_at_ms, first[0].last_success_at_ms);
    assert_eq!(after[0].pending_alerts, 0);
}

#[tokio::test]
async fn once_uses_stable_due_order_and_skips_same_host_until_spacing_elapsed() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fixture(dir.path());
    config.map_watches[0].id = "z-last".into();
    let mut first = config.map_watches[0].clone();
    first.id = "a-first".into();
    first.section_id = "s_other".into();
    config.map_watches.push(first);
    run(config.clone(), true, false).await.unwrap();
    let sent = requests(dir.path());
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["map_section_id"], "s_other");
    let status = MapMonitorStore::open(&config.map_database)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(status[1].id, "z-last");
    assert_eq!(status[1].health, "unobserved");
}

#[tokio::test]
async fn missing_live_credentials_fail_before_any_state_or_browser_work() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = fixture(dir.path());
    config.discord.channel_id = Some("12345678".into());
    config.discord.token_env = format!("TICKET_TEST_ABSENT_TOKEN_{}", std::process::id());
    assert!(std::env::var_os(&config.discord.token_env).is_none());
    assert!(
        run(config.clone(), true, true)
            .await
            .unwrap_err()
            .to_string()
            .contains("token")
    );
    assert!(!config.map_database.exists());
    assert!(!config.map_database.with_extension("lock").exists());
    assert!(requests(dir.path()).is_empty());
}

#[tokio::test]
async fn cancellation_kills_worker_and_releases_exclusive_database_lock() {
    let dir = tempfile::tempdir().unwrap();
    let config = fixture(dir.path());
    std::fs::write(dir.path().join("mode"), "hang").unwrap();
    let started = dir.path().join("requests");
    let competing = config.clone();
    let shutdown = async move {
        tokio::time::timeout(Duration::from_secs(4), async {
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .unwrap();
        let error = run(competing, true, false).await.unwrap_err();
        assert!(error.to_string().contains("Another monitor"));
    };
    tokio::time::timeout(
        Duration::from_secs(8),
        run_with_shutdown(config.clone(), false, false, shutdown),
    )
    .await
    .unwrap()
    .unwrap();
    let pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
    #[cfg(target_os = "linux")]
    assert!(!Path::new(&format!("/proc/{pid}")).exists());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(config.map_database.with_extension("lock"))
        .unwrap();
    lock.try_lock()
        .expect("monitor must release its exclusive lock");
    assert_eq!(requests(dir.path()).len(), 1);
}

#[tokio::test]
async fn continuous_scans_reuse_one_worker_and_resume_from_completion() {
    let dir = tempfile::tempdir().unwrap();
    let config = fixture(dir.path());
    let path = dir.path().to_owned();
    let shutdown = async move {
        loop {
            if requests(&path).len() >= 2 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    };
    tokio::time::timeout(
        Duration::from_secs(8),
        run_with_shutdown(config, false, false, shutdown),
    )
    .await
    .unwrap()
    .unwrap();
    let sent = requests(dir.path());
    assert_eq!(sent.len(), 2);
    assert_eq!(
        sent[1]["id"].as_u64().unwrap(),
        sent[0]["id"].as_u64().unwrap() + 1
    );
}

#[cfg(unix)]
#[tokio::test]
async fn sigterm_stops_cli_cleans_browser_and_emits_lifecycle_records() {
    assert_signal_stops_cli("-TERM").await;
}

#[cfg(unix)]
#[tokio::test]
async fn sigint_stops_cli_cleans_browser_and_emits_lifecycle_records() {
    assert_signal_stops_cli("-INT").await;
}

#[cfg(unix)]
async fn assert_signal_stops_cli(signal: &str) {
    let dir = tempfile::tempdir().unwrap();
    let config = fixture(dir.path());
    std::fs::write(dir.path().join("mode"), "hang").unwrap();
    let config_path = dir.path().join("config.toml");
    std::fs::write(&config_path, toml::to_string(&config).unwrap()).unwrap();
    let stdout_path = dir.path().join("stdout");
    let stderr_path = dir.path().join("stderr");
    let mut child = tokio::process::Command::new(env!("CARGO_BIN_EXE_ticket-watch"))
        .arg("--config")
        .arg(&config_path)
        .arg("map-run")
        .stdout(std::fs::File::create(&stdout_path).unwrap())
        .stderr(std::fs::File::create(&stderr_path).unwrap())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let pid = child.id().unwrap();
    tokio::time::timeout(Duration::from_secs(4), async {
        while requests(dir.path()).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let browser_pid = std::fs::read_to_string(dir.path().join("pid")).unwrap();
    assert!(
        tokio::process::Command::new("kill")
            .args([signal, &pid.to_string()])
            .status()
            .await
            .unwrap()
            .success()
    );
    let result = tokio::time::timeout(Duration::from_secs(8), child.wait()).await;
    let stderr = std::fs::read_to_string(&stderr_path).unwrap();
    let stdout = std::fs::read_to_string(&stdout_path).unwrap();
    if result.is_err() {
        let _ = child.kill().await;
    }
    let status = result
        .unwrap_or_else(|_| {
            panic!("{signal} must take the graceful cleanup path; stdout={stdout}; stderr={stderr}")
        })
        .unwrap();
    assert!(status.success(), "{stderr}");
    let records: Vec<serde_json::Value> = stdout
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let started = records
        .iter()
        .find(|item| item["event"] == "map_monitor_started")
        .unwrap();
    assert_eq!(started["process_id"], pid);
    assert_eq!(started["enabled_watches"], 1);
    assert_eq!(started["delivery_mode"], "preview_only");
    assert_eq!(records.last().unwrap()["event"], "map_monitor_stopped");
    assert_eq!(records.last().unwrap()["reason"], "shutdown");
    #[cfg(target_os = "linux")]
    assert!(!Path::new(&format!("/proc/{browser_pid}")).exists());
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(config.map_database.with_extension("lock"))
        .unwrap();
    lock.try_lock()
        .expect("Shutdown signal must release the database lock");
}
