//! Durable scheduling and displayed-price alerts, separate from inventory state.
use std::{collections::HashSet, path::Path};

use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::{
    config::{Config, DiscordConfig, MapWatch},
    map_inspection::DisplayedSeatQuote,
    map_scan::{SectionScan, validate_displayed_quote, validate_report},
    map_scan_store::{
        ChangeKind, MapScanStore, ScanChanges, market_event, market_sample, record_in_transaction,
    },
    market_history::{self, SampleInput, SourceMetadata},
    model::{
        Alert, Availability, Evidence, FeeScope, InventoryKind, Offer, PendingAlert, PriceScope,
        QuantityKind,
    },
};

const MAX_PENDING: usize = 10_000;
const MAX_JSON: usize = 65_536;
const MAX_INTERVAL_MS: i64 = 31_536_000_000;

pub struct MapMonitorStore {
    inner: MapScanStore,
    route: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MonitorStatus {
    pub id: String,
    pub enabled: bool,
    pub health: String,
    pub next_due_ms: i64,
    pub last_observed_at_ms: Option<i64>,
    pub last_success_at_ms: Option<i64>,
    pub pending_alerts: usize,
}

#[derive(Debug, Serialize)]
pub struct RecordOutcome {
    pub changes: ScanChanges,
    pub enqueued: usize,
}

impl MapMonitorStore {
    pub fn open(path: &Path) -> Result<Self> {
        Ok(Self {
            inner: MapScanStore::open(path)?,
            route: None,
        })
    }

    /// Revoke obsolete queue entries before dispatching any saved messages.
    pub fn reconcile(
        &mut self,
        watches: &[MapWatch],
        discord: &DiscordConfig,
        now_ms: i64,
    ) -> Result<()> {
        ensure!(now_ms > 0, "Invalid monitor time");
        crate::config::validate(&Config {
            map_watches: watches.to_vec(),
            discord: discord.clone(),
            ..Config::default()
        })?;
        let route = route_fingerprint(discord)?;
        let tx = self
            .inner
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<String> = tx
            .query_row(
                "SELECT value FROM map_monitor_meta WHERE key='route'",
                [],
                |r| r.get(0),
            )
            .optional()?;
        let route_changed = previous.as_deref() != Some(&route);
        if route_changed {
            tx.execute(
                "UPDATE map_monitor_outbox SET state='cancelled' WHERE state='pending'",
                [],
            )?;
            tx.execute("INSERT INTO map_monitor_meta(key,value) VALUES ('route',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [&route])?;
        }
        let ids: HashSet<_> = watches.iter().map(|watch| watch.id.as_str()).collect();
        let saved_ids = {
            let mut stmt = tx.prepare("SELECT id FROM map_monitor_watches")?;
            stmt.query_map([], |r| r.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        for id in saved_ids {
            if !ids.contains(id.as_str()) {
                tx.execute(
                    "UPDATE map_monitor_watches SET enabled=0 WHERE id=?1",
                    [&id],
                )?;
                tx.execute("UPDATE map_monitor_outbox SET state='cancelled' WHERE watch_id=?1 AND state='pending'", [&id])?;
            }
        }
        for watch in watches {
            let raw = serde_json::to_string(watch)?;
            ensure!(
                raw.len() <= MAX_JSON,
                "Map watch configuration is too large"
            );
            let fingerprint = digest(raw.as_bytes());
            let old: Option<(String, bool)> = tx
                .query_row(
                    "SELECT fingerprint,enabled FROM map_monitor_watches WHERE id=?1",
                    [&watch.id],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?;
            let changed = route_changed
                || old.as_ref().is_none_or(|(hash, enabled)| {
                    hash != &fingerprint || *enabled != watch.enabled
                });
            if changed || !watch.enabled {
                tx.execute("UPDATE map_monitor_outbox SET state='cancelled' WHERE watch_id=?1 AND state='pending'", [&watch.id])?;
            }
            tx.execute("INSERT INTO map_monitor_watches(id,fingerprint,route_fingerprint,watch_json,enabled,next_due_ms)
                VALUES (?1,?2,?3,?4,?5,?6)
                ON CONFLICT(id) DO UPDATE SET fingerprint=excluded.fingerprint,route_fingerprint=excluded.route_fingerprint,
                watch_json=excluded.watch_json,enabled=excluded.enabled,
                initialized=CASE WHEN ?7 THEN 0 ELSE initialized END,
                next_due_ms=CASE WHEN ?7 THEN excluded.next_due_ms ELSE next_due_ms END,
                health=CASE WHEN ?7 THEN 'unobserved' ELSE health END,
                last_observed_at_ms=CASE WHEN ?7 THEN NULL ELSE last_observed_at_ms END,
                last_success_at_ms=CASE WHEN ?7 THEN NULL ELSE last_success_at_ms END",
                params![watch.id,fingerprint,route,raw,watch.enabled,now_ms,changed])?;
        }
        tx.commit()?;
        self.route = Some(route);
        Ok(())
    }

    pub fn due(&self, now_ms: i64, limit: usize) -> Result<Vec<MapWatch>> {
        self.route
            .as_ref()
            .context("Reconcile map watches before polling")?;
        let mut stmt = self.inner.connection.prepare("SELECT CASE WHEN length(CAST(watch_json AS BLOB))<=?3 THEN watch_json ELSE NULL END FROM map_monitor_watches WHERE enabled=1 AND next_due_ms<=?1 ORDER BY next_due_ms,id LIMIT ?2")?;
        let rows = stmt.query_map(params![now_ms, limit.min(MAX_PENDING), MAX_JSON], |r| {
            r.get::<_, String>(0)
        })?;
        rows.map(|raw| parse_watch(&raw?)).collect()
    }

    pub fn status(&self) -> Result<Vec<MonitorStatus>> {
        let mut stmt = self.inner.connection.prepare("SELECT id,enabled,health,next_due_ms,last_observed_at_ms,last_success_at_ms,
            (SELECT COUNT(*) FROM map_monitor_outbox o WHERE o.watch_id=w.id AND o.state='pending') FROM map_monitor_watches w ORDER BY id")?;
        Ok(stmt
            .query_map([], |r| {
                Ok(MonitorStatus {
                    id: r.get(0)?,
                    enabled: r.get(1)?,
                    health: r.get(2)?,
                    next_due_ms: r.get(3)?,
                    last_observed_at_ms: r.get(4)?,
                    last_success_at_ms: r.get(5)?,
                    pending_alerts: r.get(6)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn record_success(
        &mut self,
        watch_id: &str,
        scan: &SectionScan,
        now_ms: i64,
        next_due_ms: i64,
        alert_max_age_ms: i64,
    ) -> Result<RecordOutcome> {
        validate_report(scan)?;
        times(now_ms, next_due_ms, alert_max_age_ms)?;
        ensure!(
            scan.started_at_ms >= now_ms.saturating_sub(crate::map_scan::MAX_SCAN_AGE_MS)
                && scan.finished_at_ms <= now_ms.saturating_add(5000),
            "Map scan is stale or in the future"
        );
        let route = self
            .route
            .as_ref()
            .context("Reconcile map watches before recording")?;
        let tx = self
            .inner
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (raw,fingerprint,saved_route,initialized,enabled): (String,String,String,bool,bool) = tx.query_row("SELECT CASE WHEN length(CAST(watch_json AS BLOB))<=?2 THEN watch_json ELSE NULL END,fingerprint,route_fingerprint,initialized,enabled FROM map_monitor_watches WHERE id=?1", params![watch_id,MAX_JSON], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?,r.get(4)?))).context("Map watch is not registered")?;
        let watch = parse_watch(&raw)?;
        ensure!(
            enabled
                && watch.enabled
                && saved_route == *route
                && fingerprint == digest(raw.as_bytes()),
            "Map watch configuration changed"
        );
        let (host, event) =
            crate::event_identity::identity(&watch.url).context("Invalid configured event")?;
        ensure!(
            scan.site_host == host
                && scan.event_id == event
                && scan.section_id == watch.section_id
                && scan.limit == watch.limit,
            "Map scan does not match its configured watch"
        );
        let changes = record_in_transaction(&tx, scan)?;
        market_history::record_sample(
            &tx,
            &market_event(&watch.url, Some(&watch.name)),
            &market_source(&watch, &fingerprint),
            &market_sample(scan)?,
        )?;
        let mut batches: Vec<(&str, Vec<(DisplayedSeatQuote, i64)>)> = Vec::new();
        for (kind, signal) in [
            (ChangeKind::FirstObserved, "displayed_price_observed"),
            (ChangeKind::PriceChanged, "displayed_price_changed"),
            (ChangeKind::DetailsChanged, "displayed_details_changed"),
        ] {
            let quotes = if !initialized {
                if watch.notify_initial && kind == ChangeKind::FirstObserved {
                    scan.observations
                        .iter()
                        .filter_map(|o| o.quote.clone().map(|q| (q, o.observed_at_ms)))
                        .collect()
                } else {
                    Vec::new()
                }
            } else if changes.duplicate_scan
                || (kind == ChangeKind::PriceChanged && !watch.notify_price_changes)
                || (kind == ChangeKind::DetailsChanged && !watch.notify_details_changes)
            {
                Vec::new()
            } else {
                changes
                    .changes
                    .iter()
                    .filter(|change| change.kind == kind)
                    .map(|c| (c.current.clone(), c.observed_at_ms))
                    .collect()
            };
            let fresh = quotes
                .into_iter()
                .filter(|(_, at)| *at >= now_ms.saturating_sub(alert_max_age_ms))
                .collect::<Vec<_>>();
            batches.push((signal, fresh));
        }
        let instance: String = tx.query_row(
            "SELECT value FROM map_monitor_meta WHERE key='instance'",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            instance.len() == 32 && instance.bytes().all(|b| b.is_ascii_hexdigit()),
            "Invalid map monitor instance"
        );
        let mut enqueued = 0;
        let pending: usize = tx.query_row(
            "SELECT COUNT(*) FROM map_monitor_outbox WHERE state='pending'",
            [],
            |r| r.get(0),
        )?;
        ensure!(
            pending <= MAX_PENDING,
            "Map notification outbox exceeds its limit"
        );
        for (signal, quotes) in batches {
            for chunk in quotes.chunks(6) {
                let observed_at_ms = chunk.iter().map(|(_, at)| *at).min().unwrap();
                let alert = Alert {
                    watch_id: watch.id.clone(),
                    event_name: watch.name.clone(),
                    artist: "Ticketmaster CZ".into(),
                    location: format!("Section {}", scan.section_name),
                    event_url: scan.event_url.clone(),
                    observed_at: observed_at_ms / 1000,
                    kind: signal.into(),
                    detail: "Displayed ticket price observation. Availability, purchasable quantity, complete checkout fees and adjacency are unverified. No tickets are reserved.".into(),
                    offers: chunk.iter().map(|(quote, _)| offer(quote)).collect(),
                };
                let payload = serde_json::to_string(&alert)?;
                ensure!(payload.len() <= MAX_JSON, "Map alert payload exceeds limit");
                let nonce = digest(&serde_json::to_vec(&(
                    &instance,
                    &watch.id,
                    &fingerprint,
                    route,
                    scan.finished_at_ms,
                    signal,
                    &payload,
                ))?);
                ensure!(
                    pending + enqueued < MAX_PENDING,
                    "Map notification outbox is full"
                );
                enqueued += tx.execute("INSERT OR IGNORE INTO map_monitor_outbox(watch_id,fingerprint,route_fingerprint,nonce,payload_json,not_before_ms,created_at_ms,observed_at_ms) VALUES (?1,?2,?3,?4,?5,?6,?6,?7)", params![watch.id,fingerprint,route,nonce,payload,now_ms,observed_at_ms])?;
            }
        }
        // Keep a bounded delivery history without evicting pending work. Scan
        // hashes still suppress exact report replay after terminal rows age out.
        let retained: usize =
            tx.query_row("SELECT COUNT(*) FROM map_monitor_outbox", [], |r| r.get(0))?;
        tx.execute("DELETE FROM map_monitor_outbox WHERE id IN (SELECT id FROM map_monitor_outbox WHERE state<>'pending' ORDER BY id LIMIT ?1)", [retained.saturating_sub(MAX_PENDING)])?;
        tx.execute("UPDATE map_monitor_watches SET initialized=1,health='ok',last_observed_at_ms=?2,last_success_at_ms=?2,next_due_ms=?3 WHERE id=?1",params![watch_id,scan.finished_at_ms,next_due_ms])?;
        tx.commit()?;
        Ok(RecordOutcome { changes, enqueued })
    }

    pub fn record_failure(
        &mut self,
        watch_id: &str,
        health: &str,
        now_ms: i64,
        next_due_ms: i64,
    ) -> Result<()> {
        times(now_ms, next_due_ms, 1)?;
        ensure!(
            ["unknown", "blocked"].contains(&health),
            "Invalid map monitor health"
        );
        let tx = self
            .inner
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let saved: Option<(String, String)> = tx.query_row(
            "SELECT CASE WHEN length(CAST(watch_json AS BLOB))<=?2 THEN watch_json ELSE NULL END,fingerprint FROM map_monitor_watches WHERE id=?1 AND enabled=1",
            params![watch_id, MAX_JSON],
            |row| Ok((row.get(0)?, row.get(1)?)),
        ).optional()?;
        if let Some((raw, fingerprint)) = saved {
            let watch = parse_watch(&raw)?;
            ensure!(
                watch.id == watch_id && fingerprint == digest(raw.as_bytes()),
                "Map watch configuration changed"
            );
            market_history::record_sample(
                &tx,
                &market_event(&watch.url, Some(&watch.name)),
                &market_source(&watch, &fingerprint),
                &SampleInput {
                    observed_at: now_ms / 1000,
                    availability: if health == "blocked" {
                        Availability::Blocked
                    } else {
                        Availability::Unknown
                    },
                    evidence: Evidence::None,
                    complete: false,
                    entries: Vec::new(),
                },
            )?;
        }
        tx.execute("UPDATE map_monitor_watches SET health=?2,last_observed_at_ms=?3,next_due_ms=?4 WHERE id=?1 AND enabled=1",params![watch_id,health,now_ms,next_due_ms])?;
        tx.execute(
            "UPDATE map_monitor_outbox SET state='cancelled' WHERE watch_id=?1 AND state='pending'",
            [watch_id],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn cooldown(&self, scope: &str) -> Result<Option<i64>> {
        Ok(self
            .inner
            .connection
            .query_row(
                "SELECT until_ms FROM map_monitor_cooldowns WHERE scope=?1",
                [scope],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn set_cooldown(&mut self, scope: &str, until_ms: i64) -> Result<()> {
        ensure!(
            ["host:ticketmaster.cz", "discord:delivery"].contains(&scope) && until_ms > 0,
            "Invalid monitor cooldown"
        );
        self.inner.connection.execute("INSERT INTO map_monitor_cooldowns(scope,until_ms) VALUES (?1,?2) ON CONFLICT(scope) DO UPDATE SET until_ms=MAX(until_ms,excluded.until_ms)",params![scope,until_ms])?;
        Ok(())
    }

    pub fn expire(&mut self, now_ms: i64, alert_max_age_ms: i64) -> Result<usize> {
        times(now_ms, now_ms, alert_max_age_ms)?;
        Ok(self.inner.connection.execute("UPDATE map_monitor_outbox SET state='expired' WHERE state='pending' AND observed_at_ms<?1",[now_ms.saturating_sub(alert_max_age_ms)])?)
    }

    pub fn pending(
        &mut self,
        now_ms: i64,
        alert_max_age_ms: i64,
        limit: usize,
    ) -> Result<Vec<PendingAlert>> {
        self.expire(now_ms, alert_max_age_ms)?;
        let route = self
            .route
            .as_ref()
            .context("Reconcile map watches before delivery")?;
        if self
            .cooldown("discord:delivery")?
            .is_some_and(|until| until > now_ms)
        {
            return Ok(Vec::new());
        }
        let tx = self
            .inner
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE map_monitor_outbox SET state='cancelled' WHERE state='pending' AND (route_fingerprint<>?1 OR NOT EXISTS(SELECT 1 FROM map_monitor_watches w WHERE w.id=watch_id AND w.enabled=1 AND w.fingerprint=map_monitor_outbox.fingerprint AND w.route_fingerprint=map_monitor_outbox.route_fingerprint))",[route])?;
        let rows = {
            let mut stmt=tx.prepare("SELECT o.id,o.nonce,CASE WHEN length(CAST(o.payload_json AS BLOB))<=?3 THEN o.payload_json ELSE NULL END,o.attempts,CASE WHEN length(CAST(w.watch_json AS BLOB))<=?3 THEN w.watch_json ELSE NULL END FROM map_monitor_outbox o JOIN map_monitor_watches w ON w.id=o.watch_id WHERE o.state='pending' AND o.not_before_ms<=?1 ORDER BY o.id LIMIT ?2")?;
            stmt.query_map(params![now_ms, limit.min(MAX_PENDING), MAX_JSON], |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, String>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, u32>(3)?,
                    r.get::<_, String>(4)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
        };
        let mut result = Vec::new();
        for (id, nonce, raw, attempts, watch_raw) in rows {
            let alert: Alert = serde_json::from_str(&raw).context("Invalid saved map alert")?;
            let encoded: serde_json::Value = serde_json::from_str(&raw)?;
            let watch = parse_watch(&watch_raw)?;
            ensure!(
                nonce.len() == 64
                    && nonce.bytes().all(|b| b.is_ascii_hexdigit())
                    && alert.watch_id == watch.id
                    && (1..=6).contains(&alert.offers.len())
                    && [
                        "displayed_price_observed",
                        "displayed_price_changed",
                        "displayed_details_changed"
                    ]
                    .contains(&alert.kind.as_str()),
                "Invalid saved map alert identity"
            );
            let identity =
                crate::event_identity::identity(&watch.url).context("Invalid saved map watch")?;
            ensure!(
                crate::event_identity::identity(&alert.event_url) == Some(identity.clone()),
                "Saved alert event mismatch"
            );
            let mut current = true;
            for (index, offered) in alert.offers.iter().enumerate() {
                let saved:Option<String>=tx.query_row("SELECT CASE WHEN length(CAST(quote_json AS BLOB))<=?5 THEN quote_json ELSE NULL END FROM map_scan_quotes WHERE site_host=?1 AND event_id=?2 AND section_id=?3 AND seat_id=?4",params![identity.0,identity.1,watch.section_id,offered.key,MAX_JSON],|r|r.get(0)).optional()?;
                let Some(saved) = saved else {
                    current = false;
                    break;
                };
                let quote: DisplayedSeatQuote = serde_json::from_str(&saved)?;
                validate_displayed_quote(&quote)?;
                let mut displayed = quote.clone();
                displayed.price_text = offered
                    .price_text
                    .clone()
                    .context("Saved map alert lacks its displayed price")?;
                displayed.price_minor = offered
                    .price_minor
                    .context("Saved map alert lacks its numeric price")?;
                validate_displayed_quote(&displayed)?;
                let mut normalized = offered.clone();
                normalized.price_text = Some(quote.price_text.clone());
                let mut expected = offer(&quote);
                if encoded["offers"][index].get("inventory_kind").is_none() {
                    // Legacy alerts remain Unknown without inventing a classification
                    // or cancelling an otherwise current, fully matching saved quote.
                    expected.inventory_kind = InventoryKind::Unknown;
                }
                if normalized != expected {
                    current = false;
                    break;
                }
            }
            if current {
                result.push(PendingAlert {
                    id,
                    nonce,
                    payload: alert,
                    attempts,
                });
            } else {
                tx.execute("UPDATE map_monitor_outbox SET state='cancelled' WHERE id=?1 AND state='pending'",[id])?;
            }
        }
        tx.commit()?;
        Ok(result)
    }

    pub fn delivered(&mut self, id: i64, message_id: &str) -> Result<()> {
        ensure!(
            !message_id.is_empty()
                && message_id.len() <= 20
                && message_id.parse::<u64>().is_ok_and(|n| n > 0),
            "Invalid Discord message ID"
        );
        self.inner.connection.execute("UPDATE map_monitor_outbox SET state='delivered',delivered_message_id=?2,last_error=NULL WHERE id=?1 AND state='pending'",params![id,message_id])?;
        Ok(())
    }

    pub fn retry(&mut self, id: i64, not_before_ms: i64, reason: &str) -> Result<()> {
        ensure!(not_before_ms > 0, "Invalid delivery retry time");
        // Never persist arbitrary transport text, which might contain credentials.
        let diagnostic = if reason.contains("rate") {
            "Discord rate limited"
        } else if reason.contains("progress") {
            "Discord delivery in progress; outcome may be ambiguous"
        } else {
            "Discord delivery retry; outcome may be ambiguous"
        };
        self.inner.connection.execute("UPDATE map_monitor_outbox SET attempts=MIN(attempts+1,4294967295),not_before_ms=MAX(not_before_ms,?2),last_error=?3 WHERE id=?1 AND state='pending'",params![id,not_before_ms,diagnostic])?;
        Ok(())
    }
}

fn parse_watch(raw: &str) -> Result<MapWatch> {
    ensure!(raw.len() <= MAX_JSON, "Saved map watch exceeds limit");
    let watch: MapWatch = serde_json::from_str(raw).context("Invalid saved map watch")?;
    crate::config::validate(&Config {
        map_watches: vec![watch.clone()],
        ..Config::default()
    })?;
    Ok(watch)
}
fn market_source(watch: &MapWatch, fingerprint: &str) -> SourceMetadata {
    SourceMetadata {
        watch_id: watch.id.clone(),
        scope: format!(
            "map_monitor:section:{}:limit:{}",
            watch.section_id, watch.limit
        ),
        interval_secs: watch.interval_secs,
        config_fingerprint: fingerprint.into(),
    }
}
fn times(now: i64, due: i64, age: i64) -> Result<()> {
    ensure!(
        now > 0 && due >= now && (1..=MAX_INTERVAL_MS).contains(&age),
        "Invalid monitor timing bounds"
    );
    Ok(())
}
fn digest(value: &[u8]) -> String {
    format!("{:x}", Sha256::digest(value))
}
fn route_fingerprint(config: &DiscordConfig) -> Result<String> {
    let mut normalized = config.clone();
    normalized.mention_user_ids.sort();
    normalized.mention_user_ids.dedup();
    normalized.mention_role_ids.sort();
    normalized.mention_role_ids.dedup();
    Ok(digest(&serde_json::to_vec(&normalized)?))
}
fn offer(quote: &DisplayedSeatQuote) -> Offer {
    Offer {
        key: quote.seat_id.clone(),
        section: Some(quote.section.clone()),
        row: Some(quote.row.clone()),
        seats: Some(quote.seat.clone()),
        ticket_type: Some(quote.ticket_type.clone()),
        inventory: quote.restriction.clone(),
        // All callers validate the strict rendered resale tooltip before conversion.
        // A restriction is retained above but never used to classify the sale.
        inventory_kind: InventoryKind::Resale,
        price_text: Some(quote.price_text.clone()),
        price_minor: Some(quote.price_minor),
        currency: Some(quote.currency.clone()),
        quantity: None,
        quantity_kind: QuantityKind::Unknown,
        price_scope: PriceScope::PerTicket,
        fees: FeeScope::Unknown,
    }
}
