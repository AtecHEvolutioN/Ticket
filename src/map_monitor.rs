//! Scheduled display-price scans with independent, durable Discord delivery.
//!
//! Source observations remain separate from confirmed ticket inventory. One
//! browser capture runs at a time; an in-flight capture does not block delivery.
use std::{collections::HashSet, future::Future, path::Path, sync::Arc, time::Duration};

use anyhow::{Context, Result, bail};
use serde_json::json;
use tokio::{sync::oneshot, task::JoinSet};

use crate::{
    config::{self, Config, MapWatch},
    discord::{self, Delivery, Discord},
    fetch::Fetcher,
    map_monitor_store::MapMonitorStore,
    map_scan::{SectionScan, validate_scan},
    seat_map::extract_seat_map,
};

const TICK: Duration = Duration::from_millis(500);
const MAX_PENDING: usize = 10_000;

type Work = Box<dyn FnOnce(&mut MapMonitorStore) + Send>;

/// SQLite operations run away from the browser and Discord network tasks.
#[derive(Clone)]
struct MonitorDb {
    tx: std::sync::mpsc::Sender<Work>,
}

impl MonitorDb {
    fn open(path: &Path) -> Result<Self> {
        let mut store = MapMonitorStore::open(path)?;
        let (tx, rx) = std::sync::mpsc::channel::<Work>();
        std::thread::Builder::new()
            .name("map-monitor-database".into())
            .spawn(move || {
                for work in rx {
                    work(&mut store);
                }
            })?;
        Ok(Self { tx })
    }

    async fn call<R: Send + 'static>(
        &self,
        f: impl FnOnce(&mut MapMonitorStore) -> Result<R> + Send + 'static,
    ) -> Result<R> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(Box::new(move |store| {
                let _ = tx.send(f(store));
            }))
            .map_err(|_| anyhow::anyhow!("Map database worker stopped"))?;
        rx.await
            .context("Map database operation did not complete")?
    }
}

#[derive(Debug)]
struct ScanAttempt {
    scan: Option<SectionScan>,
    health: &'static str,
    retry_after_secs: Option<u64>,
    elapsed_ms: Option<u64>,
    detail: String,
}

/// Run continuously, or inspect each currently due watch at most once.
/// Live Discord delivery is opt-in; preview mode never reads a bot token.
pub async fn run(config: Config, once: bool, send: bool) -> Result<()> {
    run_with_shutdown(config, once, send, async {
        let _ = tokio::signal::ctrl_c().await;
    })
    .await
}

/// Embedding hook for orderly shutdown; cancellation stops both network tasks.
pub async fn run_with_shutdown(
    config: Config,
    once: bool,
    send: bool,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<()> {
    config::validate(&config)?;
    // Missing live credentials must fail before opening state or a browser.
    let sender = if send && config.map_watches.iter().any(|watch| watch.enabled) {
        Some(Discord::new(&config.discord)?)
    } else {
        None
    };
    run_prepared(config, once, sender, shutdown).await
}

async fn run_prepared(
    config: Config,
    once: bool,
    mut sender: Option<Discord>,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<()> {
    let _lock = lock_database(&config.map_database)?;
    let db = MonitorDb::open(&config.map_database)?;
    let watches = config.map_watches.clone();
    let route = config.discord.clone();
    db.call(move |store| store.reconcile(&watches, &route, now_ms()))
        .await?;
    if !config.map_watches.iter().any(|watch| watch.enabled) {
        println!(
            "{}",
            json!({"mode":if sender.is_some() {"live_delivery"} else {"preview_only"},
            "completed_watches":0, "enabled_watches":0})
        );
        return Ok(());
    }
    let fetcher = Arc::new(Fetcher::new(&config)?);
    let mut fetches = JoinSet::new();
    let mut deliveries = JoinSet::new();
    let live = sender.is_some();
    let mut done = HashSet::new();
    let mut shown = HashSet::new();
    let mut in_flight_delivery = None;
    let max_age_ms = milliseconds(config.alert_max_age_secs);
    tokio::pin!(shutdown);
    let outcome = async {
        loop {
            // Re-read persisted due times and cooldowns on every scheduling pass.
            if fetches.is_empty() {
                let excluded = done.clone();
                let selected = db.call(move |store| {
                    let at = now_ms();
                    for watch in store.due(at, MAX_PENDING)? {
                        if once && excluded.contains(&watch.id) { continue; }
                        let scope = host_scope(&watch.url)?;
                        if store.cooldown(&scope)?.is_some_and(|until| until > at) { continue; }
                        return Ok(Some((watch, scope)));
                    }
                    Ok(None)
                }).await?;
                if let Some((watch, scope)) = selected {
                    // Commit pacing before starting the request, including on restart.
                    let until = add_seconds(now_ms(), config.per_host_interval_secs);
                    db.call(move |store| store.set_cooldown(&scope, until)).await?;
                    let client = fetcher.clone();
                    fetches.spawn(async move {
                        let attempt = observe(&client, &watch).await;
                        (watch, attempt)
                    });
                }
            }

            if deliveries.is_empty() {
                let pending = db.call(move |store| store.pending(now_ms(), max_age_ms, MAX_PENDING)).await?;
                if let Some(mut client) = sender.take() {
                    if let Some(alert) = pending.into_iter().next() {
                        let id = alert.id;
                        // A process crash during HTTP delivery leaves a retryable
                        // entry with its original nonce and a short quiet period.
                        db.call(move |store| store.retry(id, add_seconds(now_ms(), 20),
                            "Discord delivery started; acceptance may be ambiguous")).await?;
                        in_flight_delivery = Some(id);
                        deliveries.spawn(async move {
                            let delivered = client.send(&alert).await;
                            (client, id, delivered)
                        });
                    } else {
                        sender = Some(client);
                    }
                } else if !live {
                    let active: HashSet<_> = pending.iter().map(|alert| alert.id).collect();
                    shown.retain(|id| active.contains(id));
                    for alert in pending {
                        if shown.insert(alert.id) {
                            println!("{}", json!({"mode":"preview_only", "outbox_id":alert.id,
                                "payload":discord::preview(&alert, &config.discord)}));
                        }
                    }
                }
            }

            if once && fetches.is_empty() && deliveries.is_empty() {
                let status = db.call(|store| store.status()).await?;
                let skipped: Vec<_> = status.iter().filter(|item| item.enabled && !done.contains(&item.id))
                    .map(|item| item.id.clone()).collect();
                println!("{}", json!({"mode":if live {"live_delivery"} else {"preview_only"},
                    "completed_watches":done.len(), "skipped_due_or_cooldown":skipped}));
                break;
            }

            tokio::select! {
                _ = &mut shutdown => { eprintln!("Stopping map monitor"); break; }
                completed = fetches.join_next(), if !fetches.is_empty() => {
                    let (watch, attempt) = completed.context("Map fetch task disappeared")?.context("Map fetch task failed")?;
                    done.insert(watch.id.clone());
                    let at = now_ms();
                    let wait = poll_delay(&config, &watch, attempt.health, attempt.retry_after_secs);
                    let next_due = add_seconds(at, wait);
                    if attempt.health == "blocked" || attempt.retry_after_secs.is_some_and(|delay| delay > 0) {
                        let scope = host_scope(&watch.url)?;
                        let cooldown = if attempt.health == "blocked" { wait } else { attempt.retry_after_secs.unwrap_or(0) };
                        db.call(move |store| store.set_cooldown(&scope, add_seconds(at, cooldown))).await?;
                    }
                    let id = watch.id.clone();
                    let summary = if let Some(scan) = attempt.scan {
                        let quoted = scan.quoted_count;
                        let candidates = scan.candidate_count;
                        let recorded = db.call(move |store| store.record_success(&id, &scan, at, next_due, max_age_ms)).await?;
                        json!({"watch_id":watch.id,"health":"ok","observed_at_ms":at,
                            "candidate_count":candidates,"quoted_count":quoted,"enqueued":recorded.enqueued,
                            "elapsed_ms":attempt.elapsed_ms,"next_due_ms":next_due})
                    } else {
                        let health = attempt.health;
                        db.call(move |store| store.record_failure(&id, health, at, next_due)).await?;
                        json!({"watch_id":watch.id,"health":attempt.health,"observed_at_ms":at,
                            "detail":attempt.detail,"elapsed_ms":attempt.elapsed_ms,"next_due_ms":next_due})
                    };
                    println!("{summary}");
                }
                completed = deliveries.join_next(), if !deliveries.is_empty() => {
                    let (client, id, delivered) = completed.context("Discord delivery task disappeared")?.context("Discord delivery task failed")?;
                    // Successful responses can exhaust the Discord bucket too.
                    // Persist its reset before acknowledging the queued message.
                    if let Some(delay) = client.cooldown_secs() {
                        let until = add_seconds(now_ms(), delay);
                        db.call(move |store| store.set_cooldown("discord:delivery", until)).await?;
                    }
                    sender = Some(client);
                    in_flight_delivery = None;
                    match delivered? {
                        Delivery::Sent { message_id } => {
                            db.call(move |store| store.delivered(id, &message_id)).await?;
                            eprintln!("Discord accepted displayed-price alert {id}");
                        }
                        Delivery::Retry { after_secs, reason } => {
                            let until = add_seconds(now_ms(), after_secs.max(1));
                            db.call(move |store| {
                                store.set_cooldown("discord:delivery", until)?;
                                store.retry(id, until, &reason)
                            }).await?;
                        }
                        Delivery::Permanent { reason } => {
                            db.call(move |store| store.retry(id, add_seconds(now_ms(), 60), "Discord delivery requires configuration attention")).await?;
                            bail!("Discord delivery stopped: {reason}");
                        }
                    }
                }
                _ = tokio::time::sleep(TICK) => {}
            }
        }
        Ok(())
    }.await;

    fetches.abort_all();
    while fetches.join_next().await.is_some() {}
    deliveries.abort_all();
    while deliveries.join_next().await.is_some() {}
    if let Some(id) = in_flight_delivery {
        let _ = db
            .call(move |store| {
                store.retry(
                    id,
                    add_seconds(now_ms(), 20),
                    "Discord delivery interrupted; acceptance may be ambiguous",
                )
            })
            .await;
    }
    fetcher.shutdown().await;
    outcome
}

async fn observe(fetcher: &Fetcher, watch: &MapWatch) -> ScanAttempt {
    let page = match fetcher
        .fetch_map_scan(&watch.url, &watch.section_id, watch.limit)
        .await
    {
        Ok(page) => page,
        Err(_) => {
            return ScanAttempt {
                scan: None,
                health: "unknown",
                retry_after_secs: None,
                elapsed_ms: None,
                detail: "Map capture failed; displayed prices remain unknown".into(),
            };
        }
    };
    let blocked =
        matches!(page.status, 401 | 403 | 429) || page.adapter_state.as_deref() == Some("blocked");
    let scan = if !blocked
        && (200..300).contains(&page.status)
        && page.adapter_state.as_deref() == Some("ready")
    {
        (|| {
            let raw = page
                .map_scan
                .as_ref()
                .context("Missing map scan evidence")?;
            let snapshot = extract_seat_map(&watch.url, &page.final_url, &page.html)?;
            validate_scan(&snapshot, raw, now_ms())
        })()
        .ok()
    } else {
        None
    };
    let health = if blocked {
        "blocked"
    } else if scan.is_some() {
        "ok"
    } else {
        "unknown"
    };
    ScanAttempt {
        scan,
        health,
        retry_after_secs: page.retry_after_secs,
        elapsed_ms: Some(page.elapsed_ms),
        detail: format!("HTTP {}; {}", page.status, page.detail),
    }
}

fn poll_delay(config: &Config, watch: &MapWatch, health: &str, retry: Option<u64>) -> u64 {
    let backoff = match health {
        "blocked" => config.blocked_retry_secs,
        "unknown" => config.unknown_retry_secs,
        _ => 0,
    };
    watch.interval_secs.max(backoff).max(retry.unwrap_or(0))
}

fn host_scope(url: &str) -> Result<String> {
    let (host, _) =
        crate::event_identity::identity(url).context("Map watch event identity missing")?;
    Ok(format!("host:{host}"))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
fn milliseconds(seconds: u64) -> i64 {
    seconds.saturating_mul(1000).min(i64::MAX as u64) as i64
}
fn add_seconds(at: i64, seconds: u64) -> i64 {
    at.saturating_add(milliseconds(seconds))
}

fn lock_database(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path.with_extension("lock"))?;
    file.try_lock()
        .context("Another monitor is already using this map database")?;
    Ok(file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    #[test]
    fn retry_backoffs_are_completion_based_bounded_and_preserve_provider_delay() {
        let config = Config::default();
        let watch = MapWatch::default();
        assert_eq!(poll_delay(&config, &watch, "ok", None), 60);
        assert_eq!(poll_delay(&config, &watch, "unknown", None), 120);
        assert_eq!(poll_delay(&config, &watch, "blocked", Some(5)), 900);
        assert_eq!(poll_delay(&config, &watch, "blocked", Some(1200)), 1200);
        assert_eq!(add_seconds(i64::MAX - 10, u64::MAX), i64::MAX);
    }

    #[tokio::test]
    async fn discord_delivers_during_a_hanging_browser_capture() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("worker.cjs");
        std::fs::write(
            &script,
            r#"
            const fs=require('node:fs'),path=require('node:path');
            require('node:readline').createInterface({input:process.stdin}).on('line',()=>{
                fs.writeFileSync(path.join(__dirname,'started'),'yes');
                setTimeout(()=>{},30000);
            });
        "#,
        )
        .unwrap();
        let config = Config {
            database: dir.path().join("inventory.sqlite"),
            map_database: dir.path().join("map.sqlite"),
            browser: crate::config::BrowserConfig {
                script,
                ..Default::default()
            },
            map_watches: vec![MapWatch {
                id: "fixture".into(),
                name: "Fixture event".into(),
                url: "https://www.ticketmaster.cz/event/synthetic/12345678".into(),
                section_id: "s_12".into(),
                notify_initial: true,
                ..Default::default()
            }],
            ..Default::default()
        };
        let now = now_ms();
        let scan: SectionScan = serde_json::from_value(json!({
            "event_url":config.map_watches[0].url,"event_id":"12345678","site_host":"ticketmaster.cz",
            "section_id":"s_12","section_name":"110","started_at_ms":now-10,"finished_at_ms":now,
            "coverage":"partial_rendered_section","limit":20,"candidate_count":1,"visible_candidate_count":1,
            "attempted_count":1,"unattempted_count":0,"quoted_count":1,"stop_reason":"visible_candidates_exhausted",
            "stock_verified":false,"adjacency_verified":false,"observations":[{
                "seat_id":"seat-2","row":"12","seat":"2","observed_at_ms":now-1,"outcome":"quoted",
                "quote":{"evidence":"rendered_seat_hover","seat_id":"seat-2","zone":"1. POSCHODÍ",
                    "section":"110","row":"12","seat":"2","ticket_type":"Ověřené přeprodávané vstupenky",
                    "restriction":null,"price_minor":100000,"currency":"CZK","price_basis":"per_ticket",
                    "price_text":"1 000,00 Kč za kus","fees_complete":false,"available_quantity":null}
            }]
        })).unwrap();
        let mut store = MapMonitorStore::open(&config.map_database).unwrap();
        store
            .reconcile(&config.map_watches, &config.discord, now)
            .unwrap();
        assert_eq!(
            store
                .record_success("fixture", &scan, now, now, 300_000)
                .unwrap()
                .enqueued,
            1
        );
        drop(store);
        let server = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}/messages", server.local_addr().unwrap());
        let sender = Discord::for_test(&config.discord, endpoint);
        let started = dir.path().join("started");
        let (request_seen, received) = oneshot::channel();
        let server = tokio::spawn(async move {
            let (mut socket, _) = server.accept().await.unwrap();
            let mut request = Vec::new();
            let mut chunk = [0_u8; 4096];
            loop {
                let size = socket.read(&mut chunk).await.unwrap();
                assert!(size > 0);
                request.extend_from_slice(&chunk[..size]);
                if let Some(end) = request.windows(4).position(|s| s == b"\r\n\r\n") {
                    let headers = String::from_utf8_lossy(&request[..end]).to_lowercase();
                    let length: usize = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length: "))
                        .unwrap()
                        .parse()
                        .unwrap();
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            while !started.exists() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let request = String::from_utf8(request).unwrap();
            assert!(request.contains("Displayed resale price"));
            assert!(request.contains("enforce_nonce"));
            assert!(!request.to_lowercase().contains("authorization:"));
            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 14\r\nX-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 120\r\nConnection: close\r\n\r\n{\"id\":\"12345\"}").await.unwrap();
            request_seen.send(()).unwrap();
        });
        let path = config.map_database.clone();
        let shutdown = async move {
            received.await.unwrap();
            loop {
                let connection = rusqlite::Connection::open(&path).unwrap();
                let delivered: i64 = connection
                    .query_row(
                        "SELECT COUNT(*) FROM map_monitor_outbox WHERE state='delivered'",
                        [],
                        |r| r.get(0),
                    )
                    .unwrap();
                if delivered == 1 {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        };
        tokio::time::timeout(
            Duration::from_secs(8),
            run_prepared(config.clone(), false, Some(sender), shutdown),
        )
        .await
        .expect("Discord must not wait for the browser deadline")
        .unwrap();
        server.await.unwrap();
        let saved = MapMonitorStore::open(&config.map_database).unwrap();
        assert!(saved.cooldown("discord:delivery").unwrap().unwrap() > now_ms() + 100_000);
    }
}
