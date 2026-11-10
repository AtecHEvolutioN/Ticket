//! Read-only supervision of displayed-price observation freshness.
//!
//! A healthy snapshot does not prove that a monitor process is running, that
//! Discord accepted an alert, or that any ticket can currently be purchased.
use std::{path::Path, time::Duration};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, Transaction, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::config::{Config, MapWatch};

const MAX_WATCH_JSON: usize = 65_536;
const MAX_AGE_SECS: u64 = 31_536_000;
const TABLES: [&str; 7] = [
    "map_scan_meta",
    "map_scan_scopes",
    "map_scan_quotes",
    "map_monitor_meta",
    "map_monitor_watches",
    "map_monitor_cooldowns",
    "map_monitor_outbox",
];

#[derive(Debug, Serialize)]
pub struct HealthReport {
    pub healthy: bool,
    pub status: &'static str,
    pub scope: &'static str,
    pub checked_at_ms: i64,
    pub max_age_secs: u64,
    pub enabled_watches: usize,
    pub checks: Vec<WatchHealth>,
    pub cooldowns: Vec<CooldownHealth>,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
    pub delivery_verified: bool,
    pub process_running_verified: bool,
}

#[derive(Debug, Serialize)]
pub struct WatchHealth {
    pub id: String,
    pub healthy: bool,
    pub issues: Vec<&'static str>,
    pub stored_health: Option<String>,
    pub last_success_at_ms: Option<i64>,
    pub age_ms: Option<i64>,
    pub last_observed_at_ms: Option<i64>,
    pub next_due_ms: Option<i64>,
    /// Pending rows whose source observations are within the alert age limit.
    /// This count does not validate delivery eligibility or Discord credentials.
    pub pending_alerts: u64,
    /// Expired by source age, but not yet marked expired by the monitor.
    pub expired_pending_alerts: u64,
    pub expired_alerts: u64,
}

#[derive(Debug, Serialize)]
pub struct CooldownHealth {
    pub scope: String,
    pub until_ms: i64,
    pub remaining_ms: i64,
}

struct SavedWatch {
    raw: Option<String>,
    fingerprint: Option<String>,
    enabled: i64,
    initialized: i64,
    health: Option<String>,
    next_due_ms: i64,
    last_observed_at_ms: Option<i64>,
    last_success_at_ms: Option<i64>,
}

/// Inspect one SQLite read transaction without creating or migrating state.
/// Missing initialization is a structured unhealthy result; invalid schemas and
/// inaccessible or locked databases are errors, never a healthy fallback.
pub fn inspect(config: &Config, max_age_secs: u64, now_ms: i64) -> Result<HealthReport> {
    crate::config::validate(config)?;
    ensure!(
        now_ms > 0 && (1..=MAX_AGE_SECS).contains(&max_age_secs),
        "Health checks require a positive time and max age of 1..31536000 seconds"
    );
    let enabled: Vec<_> = config
        .map_watches
        .iter()
        .filter(|watch| watch.enabled)
        .collect();
    let mut report = HealthReport {
        healthy: false,
        status: "inactive",
        scope: "displayed_price_observations",
        checked_at_ms: now_ms,
        max_age_secs,
        enabled_watches: enabled.len(),
        checks: Vec::new(),
        cooldowns: Vec::new(),
        stock_verified: false,
        adjacency_verified: false,
        delivery_verified: false,
        process_running_verified: false,
    };
    if enabled.is_empty() {
        return Ok(report);
    }
    if !existing_file(&config.map_database)? {
        report.status = "not_initialized";
        report.checks = enabled.iter().map(|watch| missing(watch)).collect();
        return Ok(report);
    }
    let mut connection = Connection::open_with_flags(
        &config.map_database,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .context("Open existing map database for health inspection")?;
    connection.busy_timeout(Duration::from_millis(250))?;
    let tx = connection.transaction_with_behavior(TransactionBehavior::Deferred)?;
    validate_schema(&tx)?;
    let max_age_ms = (max_age_secs * 1000) as i64;
    let alert_cutoff = now_ms.saturating_sub((config.alert_max_age_secs * 1000) as i64);
    for expected in enabled {
        report.checks.push(inspect_watch(
            &tx,
            expected,
            now_ms,
            max_age_ms,
            alert_cutoff,
        )?);
    }
    report.cooldowns = cooldowns(&tx, now_ms)?;
    report.healthy = report.checks.iter().all(|check| check.healthy);
    report.status = if report.healthy {
        "healthy"
    } else {
        "unhealthy"
    };
    tx.commit()?;
    Ok(report)
}

fn existing_file(path: &Path) -> Result<bool> {
    match std::fs::metadata(path) {
        Ok(metadata) => {
            ensure!(metadata.is_file(), "Map database path is not a file");
            Ok(true)
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error).context("Inspect map database path"),
    }
}

fn validate_schema(tx: &Transaction<'_>) -> Result<()> {
    let mut statement = tx.prepare("SELECT CASE WHEN length(CAST(name AS BLOB))<=128 THEN name ELSE NULL END FROM sqlite_master WHERE type='table' AND name NOT LIKE 'sqlite_%' LIMIT 8")?;
    let tables = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        tables.len() == TABLES.len() && tables.iter().all(|name| TABLES.contains(&name.as_str())),
        "Health inspection requires a dedicated map database with schema version 2"
    );
    let version: i64 = tx
        .query_row("SELECT version FROM map_scan_meta WHERE id=1", [], |row| {
            row.get(0)
        })
        .context("Read map database schema version")?;
    ensure!(
        version == 2,
        "Health inspection does not migrate unsupported map database schemas"
    );
    Ok(())
}

fn missing(watch: &MapWatch) -> WatchHealth {
    WatchHealth {
        id: watch.id.clone(),
        healthy: false,
        issues: vec!["not_initialized"],
        stored_health: None,
        last_success_at_ms: None,
        age_ms: None,
        last_observed_at_ms: None,
        next_due_ms: None,
        pending_alerts: 0,
        expired_pending_alerts: 0,
        expired_alerts: 0,
    }
}

fn inspect_watch(
    tx: &Transaction<'_>,
    expected: &MapWatch,
    now_ms: i64,
    max_age_ms: i64,
    alert_cutoff: i64,
) -> Result<WatchHealth> {
    let rows = {
        let mut statement = tx.prepare("SELECT CASE WHEN length(CAST(watch_json AS BLOB))<=?2 THEN watch_json ELSE NULL END,
            CASE WHEN length(CAST(fingerprint AS BLOB))=64 THEN fingerprint ELSE NULL END,
            enabled,initialized,CASE WHEN length(CAST(health AS BLOB))<=32 THEN health ELSE NULL END,
            next_due_ms,last_observed_at_ms,last_success_at_ms FROM map_monitor_watches WHERE id=?1 LIMIT 2")?;
        statement
            .query_map(params![expected.id, MAX_WATCH_JSON], |row| {
                Ok(SavedWatch {
                    raw: row.get(0)?,
                    fingerprint: row.get(1)?,
                    enabled: row.get(2)?,
                    initialized: row.get(3)?,
                    health: row.get(4)?,
                    next_due_ms: row.get(5)?,
                    last_observed_at_ms: row.get(6)?,
                    last_success_at_ms: row.get(7)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    ensure!(
        rows.len() <= 1,
        "Map database has duplicate watch identities"
    );
    let Some(saved) = rows.into_iter().next() else {
        return Ok(missing(expected));
    };
    let mut check = missing(expected);
    check.issues.clear();
    check.stored_health = saved.health;
    check.last_success_at_ms = saved.last_success_at_ms;
    check.last_observed_at_ms = saved.last_observed_at_ms;
    check.next_due_ms = Some(saved.next_due_ms);
    match saved
        .raw
        .as_deref()
        .and_then(|raw| serde_json::from_str::<MapWatch>(raw).ok())
    {
        Some(stored) if stored == *expected => {}
        Some(_) => check.issues.push("config_changed"),
        None => check.issues.push("invalid_state"),
    }
    if saved.raw.as_deref().is_none_or(|raw| {
        saved.fingerprint.as_deref()
            != Some(format!("{:x}", Sha256::digest(raw.as_bytes())).as_str())
    }) || ![0, 1].contains(&saved.enabled)
        || ![0, 1].contains(&saved.initialized)
        || saved.next_due_ms <= 0
    {
        check.issues.push("invalid_state");
    }
    if saved.enabled == 0 {
        check.issues.push("disabled");
    }
    if saved.initialized == 0 {
        check.issues.push("unobserved");
    }
    match check.stored_health.as_deref() {
        Some("ok") => {}
        Some("blocked") => check.issues.push("blocked"),
        Some("unknown") => check.issues.push("unknown"),
        Some("unobserved") => check.issues.push("unobserved"),
        _ => check.issues.push("invalid_state"),
    }
    match saved.last_success_at_ms {
        Some(at) if at <= 0 => check.issues.push("invalid_state"),
        Some(at) if at > now_ms => check.issues.push("future_timestamp"),
        Some(at) => {
            let age = now_ms - at;
            check.age_ms = Some(age);
            if age > max_age_ms {
                check.issues.push("stale");
            }
        }
        None => check.issues.push("unobserved"),
    }
    match saved.last_observed_at_ms {
        Some(at) if at > now_ms => check.issues.push("future_timestamp"),
        Some(at) if at <= 0 || saved.last_success_at_ms.is_some_and(|success| success > at) => {
            check.issues.push("invalid_state")
        }
        None if saved.last_success_at_ms.is_some() => check.issues.push("invalid_state"),
        _ => {}
    }
    let counts: (u64,u64,u64) = tx.query_row("SELECT COUNT(CASE WHEN state='pending' AND observed_at_ms>=?2 THEN 1 END),
        COUNT(CASE WHEN state='pending' AND observed_at_ms<?2 THEN 1 END),COUNT(CASE WHEN state='expired' THEN 1 END)
        FROM map_monitor_outbox WHERE watch_id=?1", params![expected.id,alert_cutoff], |row| Ok((row.get(0)?,row.get(1)?,row.get(2)?)))?;
    (
        check.pending_alerts,
        check.expired_pending_alerts,
        check.expired_alerts,
    ) = counts;
    check.issues.sort_unstable();
    check.issues.dedup();
    check.healthy = check.issues.is_empty();
    Ok(check)
}

fn cooldowns(tx: &Transaction<'_>, now_ms: i64) -> Result<Vec<CooldownHealth>> {
    let mut statement = tx.prepare("SELECT CASE WHEN length(CAST(scope AS BLOB))<=64 THEN scope ELSE NULL END,until_ms FROM map_monitor_cooldowns ORDER BY scope LIMIT 3")?;
    let entries = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ensure!(
        entries.len() <= 2
            && entries.iter().all(
                |(scope, until)| ["host:ticketmaster.cz", "discord:delivery"]
                    .contains(&scope.as_str())
                    && *until > 0
            ),
        "Invalid saved map cooldowns"
    );
    Ok(entries
        .into_iter()
        .map(|(scope, until_ms)| CooldownHealth {
            scope,
            until_ms,
            remaining_ms: until_ms.saturating_sub(now_ms).max(0),
        })
        .collect())
}
