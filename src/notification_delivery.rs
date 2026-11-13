//! Durable, bounded Discord delivery for the ordinary event monitor.
//! Polling owns its own task; this module keeps the existing outbox, freshness,
//! nonce and preview behavior while retaining provider cooldowns across restarts.
use std::{collections::HashSet, time::Instant};

use anyhow::{Result, bail};

use crate::{
    config::Config,
    database::Db,
    discord::{self, Delivery, Discord},
};

fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

/// Observation timestamps have only whole-second precision. Report an interval,
/// not an invented subsecond stock-change timestamp. Clock-invalid ages stay null.
fn observation_age_range(observed_at: i64, at_ms: i64) -> Option<(u64, u64)> {
    if observed_at < 0 || at_ms < 0 {
        return None;
    }
    let beginning = observed_at.checked_mul(1_000)?;
    let upper = at_ms.checked_sub(beginning)?;
    if upper < 0 {
        return None;
    }
    Some((upper.saturating_sub(999).max(0) as u64, upper as u64))
}

fn delivery_timing(
    alert: &crate::model::PendingAlert,
    dispatch_at_ms: i64,
    response_at_ms: i64,
    send_elapsed_ms: u128,
    outcome: &str,
) -> serde_json::Value {
    let clock_valid = response_at_ms >= dispatch_at_ms;
    let age = clock_valid
        .then(|| observation_age_range(alert.payload.observed_at, response_at_ms))
        .flatten();
    serde_json::json!({
        "kind": "discord_delivery_timing",
        "outbox_id": alert.id,
        "watch_id": alert.payload.watch_id,
        "outcome": outcome,
        "previous_failed_attempts": alert.attempts,
        "observed_at_seconds": alert.payload.observed_at,
        "dispatch_started_at_ms": dispatch_at_ms,
        "response_at_ms": response_at_ms,
        "send_elapsed_ms": send_elapsed_ms.min(u64::MAX as u128) as u64,
        "observation_to_response_ms": age.map(|(minimum, maximum)| serde_json::json!({
            "minimum": minimum, "maximum": maximum
        })),
        "acknowledged": outcome == "accepted",
        "stock_change_time_known": false,
        "notice": "Observation-to-response includes queue/retry delay and second-resolution observation time; it is not stock-change detection latency."
    })
}

fn print_json(value: &impl serde::Serialize) -> Result<()> {
    println!("{}", serde_json::to_string_pretty(value)?);
    Ok(())
}

/// Dispatch at most 32 currently eligible alerts, or print newly queued previews.
pub async fn dispatch_batch(
    db: &Db,
    config: &Config,
    sender: &mut Option<Discord>,
    shown: &mut HashSet<i64>,
) -> Result<()> {
    let cutoff = now() - config.alert_max_age_secs as i64;
    db.call(move |s| s.expire_before(cutoff)).await?;
    if let Some(sender) = sender.as_mut() {
        for _ in 0..32 {
            // Polling may cancel an older alert while Discord is busy. Re-read each send candidate.
            let at = now();
            let max_age = config.alert_max_age_secs as i64;
            let pending = db
                .call(move |s| {
                    s.expire_before(at - max_age)?;
                    if s.cooldown("discord:delivery")?
                        .is_some_and(|until| until > at)
                    {
                        return Ok(Vec::new());
                    }
                    s.pending(at, 1)
                })
                .await?;
            let Some(alert) = pending.into_iter().next() else {
                break;
            };
            let id = alert.id;
            let dispatch_at_ms = chrono::Utc::now().timestamp_millis();
            let send_started = Instant::now();
            let delivered = sender.send(&alert).await;
            let send_elapsed_ms = send_started.elapsed().as_millis();
            let response_at_ms = chrono::Utc::now().timestamp_millis();
            let outcome = match &delivered {
                Ok(Delivery::Sent { .. }) => "accepted",
                Ok(Delivery::Retry { .. }) => "retry_or_ambiguous",
                Ok(Delivery::Permanent { .. }) => "rejected",
                Err(_) => "error",
            };
            eprintln!(
                "{}",
                delivery_timing(
                    &alert,
                    dispatch_at_ms,
                    response_at_ms,
                    send_elapsed_ms,
                    outcome
                )
            );
            // Successful responses can exhaust Discord's bucket too. Persist
            // the limiter before acknowledging a message or returning an error,
            // even when this is the only pending alert. The journal stores whole
            // seconds, so round conservatively across its timestamp boundary.
            if let Some(delay) = sender.cooldown_secs() {
                let until = now()
                    .saturating_add(delay.min(i64::MAX as u64) as i64)
                    .saturating_add(1);
                db.call(move |s| s.set_cooldown("discord:delivery", until))
                    .await?;
            }
            match delivered? {
                Delivery::Sent { message_id } => {
                    db.call(move |s| s.delivered(id, &message_id)).await?;
                    eprintln!("Discord accepted alert {id}");
                }
                Delivery::Retry { after_secs, reason } => {
                    let until = now().saturating_add(after_secs.max(1).min(i64::MAX as u64) as i64);
                    db.call(move |s| {
                        s.set_cooldown("discord:delivery", until)?;
                        s.retry(id, until, &reason)
                    })
                    .await?;
                    break;
                }
                Delivery::Permanent { reason } => {
                    let diagnostic = reason.clone();
                    let until = now() + 60;
                    db.call(move |s| s.retry(id, until, &reason)).await?;
                    bail!("Discord delivery stopped: {diagnostic}");
                }
            }
        }
    } else {
        let at = now();
        let pending = db.call(move |s| s.pending(at, 10_000)).await?;
        let still_pending: HashSet<i64> = pending.iter().map(|a| a.id).collect();
        shown.retain(|id| still_pending.contains(id));
        for alert in pending {
            if shown.insert(alert.id) {
                print_json(
                    &serde_json::json!({"mode":"preview_only", "outbox_id":alert.id,
                    "payload":discord::preview(&alert, &config.discord)}),
                )?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use serde_json::Value;
    use tokio::{
        io::{AsyncReadExt, AsyncWriteExt},
        net::TcpListener,
        sync::Mutex,
        task::JoinHandle,
    };

    use super::*;
    use crate::{
        config::{DiscordConfig, Watch},
        model::{Availability, Evidence, Observation, PendingAlert},
        store::Store,
    };

    #[test]
    fn timing_keeps_second_resolution_and_clock_uncertainty_explicit() {
        assert_eq!(observation_age_range(100, 100_000), Some((0, 0)));
        assert_eq!(observation_age_range(100, 100_850), Some((0, 850)));
        assert_eq!(observation_age_range(100, 102_250), Some((1_251, 2_250)));
        assert_eq!(observation_age_range(101, 100_999), None);
        assert_eq!(observation_age_range(-1, 100_999), None);
        assert_eq!(observation_age_range(i64::MAX, i64::MAX), None);
    }

    #[test]
    fn delivery_timing_does_not_turn_retries_into_acknowledgements_or_log_payloads() {
        let alert = PendingAlert {
            id: 7,
            nonce: "private-nonce-marker".into(),
            payload: crate::model::Alert {
                watch_id: "synthetic-watch".into(),
                event_name: "private-name-marker".into(),
                artist: String::new(),
                location: String::new(),
                event_url: "https://fixture.invalid/private-url-marker".into(),
                observed_at: 100,
                kind: "available".into(),
                detail: "private-detail-marker".into(),
                offers: vec![],
            },
            attempts: 2,
        };
        let retry = delivery_timing(&alert, 101_100, 102_250, 1_150, "retry_or_ambiguous");
        assert_eq!(retry["acknowledged"], false);
        assert_eq!(retry["stock_change_time_known"], false);
        assert_eq!(retry["observation_to_response_ms"]["minimum"], 1_251);
        assert!(!retry.to_string().contains("private-"));
        let accepted = delivery_timing(&alert, 101_100, 102_250, 1_150, "accepted");
        assert_eq!(accepted["acknowledged"], true);
        let skew = delivery_timing(&alert, 102_500, 102_250, 1_150, "accepted");
        assert!(skew["observation_to_response_ms"].is_null());
        assert_eq!(skew["send_elapsed_ms"], 1_150);
    }

    struct Fixture {
        endpoint: String,
        requests: Arc<Mutex<Vec<Value>>>,
        task: JoinHandle<()>,
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl Fixture {
        async fn new(status: &str, headers: &str, body: &str) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
            let endpoint = format!("http://{}/messages", listener.local_addr().unwrap());
            let response = format!(
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n{body}",
                body.len()
            );
            let requests = Arc::new(Mutex::new(Vec::new()));
            let seen = requests.clone();
            let task = tokio::spawn(async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let mut chunk = [0_u8; 4096];
                    loop {
                        let count = socket.read(&mut chunk).await.unwrap();
                        assert!(count > 0 && bytes.len() + count <= 65_536);
                        bytes.extend_from_slice(&chunk[..count]);
                        if let Some(end) = bytes.windows(4).position(|w| w == b"\r\n\r\n") {
                            let headers = String::from_utf8_lossy(&bytes[..end]).to_lowercase();
                            assert!(!headers.contains("authorization:"));
                            let length: usize = headers
                                .lines()
                                .find_map(|line| line.strip_prefix("content-length: "))
                                .unwrap()
                                .parse()
                                .unwrap();
                            if bytes.len() >= end + 4 + length {
                                seen.lock().await.push(
                                    serde_json::from_slice(&bytes[end + 4..end + 4 + length])
                                        .unwrap(),
                                );
                                break;
                            }
                        }
                    }
                    socket.write_all(response.as_bytes()).await.unwrap();
                }
            });
            Self {
                endpoint,
                requests,
                task,
            }
        }

        fn sender(&self, config: &Config) -> Option<Discord> {
            Some(Discord::for_test(&config.discord, self.endpoint.clone()))
        }
    }

    fn config(path: std::path::PathBuf) -> Config {
        Config {
            database: path,
            discord: DiscordConfig {
                channel_id: Some("123456789012345678".into()),
                token_env: "UNUSED_LOCAL_DELIVERY_FIXTURE_TOKEN".into(),
                ..Default::default()
            },
            ..Default::default()
        }
    }

    fn enqueue(config: &Config, id: &str) -> PendingAlert {
        let watch = Watch {
            id: id.into(),
            name: "Synthetic event".into(),
            artist: "Synthetic artist".into(),
            url: "https://www.ticketmaster.cz/event/synthetic/12345678".into(),
            notify_initial: true,
            ..Default::default()
        };
        let mut store = Store::open(&config.database).unwrap();
        store.reconcile_delivery(&config.discord).unwrap();
        assert_eq!(
            store
                .apply(
                    &watch,
                    &Observation {
                        watch_id: id.into(),
                        observed_at: now(),
                        availability: Availability::Available,
                        evidence: Evidence::ExplicitStatus,
                        complete: true,
                        offers: vec![],
                        ticket_categories: vec![],
                        adapter: None,
                        detail: "synthetic local fixture".into(),
                    }
                )
                .unwrap(),
            1
        );
        store
            .pending(now(), 10)
            .unwrap()
            .into_iter()
            .find(|alert| alert.payload.watch_id == id)
            .unwrap()
    }

    #[tokio::test]
    async fn single_success_exhausted_bucket_is_durable_before_ack_and_gates_a_restarted_sender() {
        let dir = tempfile::tempdir().unwrap();
        let config = config(dir.path().join("monitor.sqlite"));
        let original = enqueue(&config, "first");
        let fixture = Fixture::new(
            "201 Created",
            "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 120\r\n",
            r#"{"id":"123456789012345678"}"#,
        )
        .await;
        let db = Db::open(&config.database).unwrap();
        let mut sender = fixture.sender(&config);
        tokio::time::timeout(
            Duration::from_secs(3),
            dispatch_batch(&db, &config, &mut sender, &mut HashSet::new()),
        )
        .await
        .unwrap()
        .unwrap();
        let saved = Store::open(&config.database).unwrap();
        assert!(saved.cooldown("discord:delivery").unwrap().unwrap() > now() + 115);
        assert!(saved.pending(now(), 10).unwrap().is_empty());
        let connection = rusqlite::Connection::open(&config.database).unwrap();
        let accepted: (String, String, u32) = connection
            .query_row(
                "SELECT state,delivered_message_id,attempts FROM outbox WHERE id=?1",
                [original.id],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .unwrap();
        assert_eq!(
            accepted,
            ("delivered".into(), "123456789012345678".into(), 0)
        );
        drop(connection);
        drop(saved);
        drop(db);
        drop(sender);

        // Both process-local limiter and database handle are replaced. There was
        // no second pending alert during the successful send to manufacture a
        // Retry response: this must rely on the persisted successful cooldown.
        let waiting = enqueue(&config, "after-restart");
        let reopened = Db::open(&config.database).unwrap();
        let mut fresh_sender = fixture.sender(&config);
        tokio::time::timeout(
            Duration::from_secs(3),
            dispatch_batch(&reopened, &config, &mut fresh_sender, &mut HashSet::new()),
        )
        .await
        .unwrap()
        .unwrap();
        let pending = Store::open(&config.database)
            .unwrap()
            .pending(now(), 10)
            .unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].nonce, waiting.nonce);
        assert_eq!(
            pending[0].attempts, 0,
            "a persisted cooldown is not a new HTTP attempt"
        );
        let requests = fixture.requests.lock().await;
        assert_eq!(requests.len(), 1);
        assert_eq!(
            requests[0]["nonce"],
            discord::preview(&original, &config.discord)["nonce"]
        );
        assert_eq!(requests[0]["enforce_nonce"], true);
    }

    #[tokio::test]
    async fn retry_and_permanent_results_keep_nonce_and_cooldown_without_echoing_response_bodies() {
        for (status, permanent) in [("503 Service Unavailable", false), ("403 Forbidden", true)] {
            let dir = tempfile::tempdir().unwrap();
            let config = config(dir.path().join("monitor.sqlite"));
            let original = enqueue(&config, "retry-fixture");
            let fixture = Fixture::new(
                status,
                "X-RateLimit-Remaining: 0\r\nX-RateLimit-Reset-After: 120\r\n",
                r#"{"message":"private-response-marker"}"#,
            )
            .await;
            let db = Db::open(&config.database).unwrap();
            let mut sender = fixture.sender(&config);
            let result = tokio::time::timeout(
                Duration::from_secs(3),
                dispatch_batch(&db, &config, &mut sender, &mut HashSet::new()),
            )
            .await
            .unwrap();
            if permanent {
                let error = result.unwrap_err().to_string();
                assert!(error.contains("Discord delivery stopped"));
                assert!(!error.contains("private-response-marker"));
            } else {
                result.unwrap();
            }
            let saved = Store::open(&config.database).unwrap();
            assert!(saved.cooldown("discord:delivery").unwrap().unwrap() > now() + 115);
            let pending = saved.pending(i64::MAX, 10).unwrap();
            assert_eq!(pending.len(), 1);
            assert_eq!(pending[0].nonce, original.nonce);
            assert_eq!(pending[0].attempts, 1);
            assert_eq!(fixture.requests.lock().await.len(), 1);
            let connection = rusqlite::Connection::open(&config.database).unwrap();
            let error: String = connection
                .query_row(
                    "SELECT last_error FROM outbox WHERE id=?1",
                    [original.id],
                    |row| row.get(0),
                )
                .unwrap();
            assert!(!error.contains("private-response-marker"));
        }
    }
}
