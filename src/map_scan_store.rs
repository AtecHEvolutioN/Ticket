//! Optional display-price history, separate from inventory and notification state.
//!
//! A partial scan can add observations and compare fresh displayed prices. Missing
//! or unreadable seats never remove a baseline or become an unavailable signal.
use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::map_inspection::DisplayedSeatQuote;
use crate::map_scan::{SectionScan, validate_displayed_quote, validate_report};

pub const COMPARISON_MAX_AGE_MS: i64 = 15 * 60 * 1000;
const MAX_QUOTES_PER_SCOPE: usize = 10_000;
const MAX_QUOTE_BYTES: usize = 16 * 1024;

pub struct MapScanStore {
    connection: Connection,
}

#[derive(Debug, Clone, Copy, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ChangeKind {
    FirstObserved,
    PriceChanged,
    DetailsChanged,
    BaselineRefreshed,
}

#[derive(Debug, Serialize)]
pub struct SeatChange {
    pub kind: ChangeKind,
    pub seat_id: String,
    pub observed_at_ms: i64,
    pub previous_observed_at_ms: Option<i64>,
    pub previous_price_minor: Option<i64>,
    pub current: DisplayedSeatQuote,
}

#[derive(Debug, Serialize)]
pub struct ScanChanges {
    pub event_url: String,
    pub section_id: String,
    pub finished_at_ms: i64,
    pub comparison_max_age_ms: i64,
    pub duplicate_scan: bool,
    pub changes: Vec<SeatChange>,
    pub stock_verified: bool,
    pub adjacency_verified: bool,
}

impl MapScanStore {
    /// Open a dedicated history database; refuse unrelated application databases.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).context("Create map history directory")?;
        }
        let connection = Connection::open(path).context("Open map price history")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        let tables = {
            let mut statement = connection.prepare(
                "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
            )?;
            statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?
        };
        ensure!(
            tables.is_empty()
                || (tables.len() == 3
                    && tables.iter().all(|name| [
                        "map_scan_meta",
                        "map_scan_scopes",
                        "map_scan_quotes"
                    ]
                    .contains(&name.as_str()))),
            "Use a dedicated map price history database"
        );
        if !tables.is_empty() {
            let version: i64 = connection
                .query_row(
                    "SELECT version FROM map_scan_meta WHERE id = 1",
                    [],
                    |row| row.get(0),
                )
                .context("Read map price history version")?;
            ensure!(version == 1, "Unsupported map price history version");
        }
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             BEGIN IMMEDIATE;
             CREATE TABLE IF NOT EXISTS map_scan_meta (
                 id INTEGER PRIMARY KEY CHECK (id = 1), version INTEGER NOT NULL
             );
             INSERT OR IGNORE INTO map_scan_meta (id, version) VALUES (1, 1);
             CREATE TABLE IF NOT EXISTS map_scan_scopes (
                 site_host TEXT NOT NULL, event_id TEXT NOT NULL, section_id TEXT NOT NULL,
                 finished_at_ms INTEGER NOT NULL, report_hash TEXT NOT NULL,
                 PRIMARY KEY (site_host, event_id, section_id)
             );
             CREATE TABLE IF NOT EXISTS map_scan_quotes (
                 site_host TEXT NOT NULL, event_id TEXT NOT NULL, section_id TEXT NOT NULL,
                 seat_id TEXT NOT NULL, observed_at_ms INTEGER NOT NULL, quote_json TEXT NOT NULL,
                 PRIMARY KEY (site_host, event_id, section_id, seat_id),
                 FOREIGN KEY (site_host, event_id, section_id)
                     REFERENCES map_scan_scopes(site_host, event_id, section_id)
             );
             COMMIT;",
            )
            .context("Initialize map price history")?;
        Ok(Self { connection })
    }

    /// Atomically update only successful displayed quotes, never missing seats.
    /// Exact replay is idempotent; older or overlapping batches are rejected.
    pub fn record(&mut self, scan: &SectionScan) -> Result<ScanChanges> {
        validate_report(scan)?;
        let mut normalized = scan.clone();
        normalized
            .observations
            .sort_by(|a, b| a.seat_id.cmp(&b.seat_id));
        let hash = format!("{:x}", Sha256::digest(serde_json::to_vec(&normalized)?));
        let mut result = ScanChanges {
            event_url: scan.event_url.clone(),
            section_id: scan.section_id.clone(),
            finished_at_ms: scan.finished_at_ms,
            comparison_max_age_ms: COMPARISON_MAX_AGE_MS,
            duplicate_scan: false,
            changes: Vec::new(),
            stock_verified: false,
            adjacency_verified: false,
        };
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous_scope: Option<(i64, String)> = transaction
            .query_row(
                "SELECT finished_at_ms,
                 CASE WHEN length(CAST(report_hash AS BLOB)) <= 64 THEN report_hash ELSE NULL END
             FROM map_scan_scopes WHERE site_host = ?1 AND event_id = ?2 AND section_id = ?3",
                params![scan.site_host, scan.event_id, scan.section_id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()
            .context("Read prior map scan")?;
        if let Some((finished, previous_hash)) = &previous_scope {
            ensure!(
                *finished > 0
                    && previous_hash.len() == 64
                    && previous_hash.bytes().all(|b| b.is_ascii_hexdigit()),
                "Invalid saved map scan metadata"
            );
            if scan.finished_at_ms == *finished && *previous_hash == hash {
                result.duplicate_scan = true;
                transaction.commit()?;
                return Ok(result);
            }
            ensure!(
                scan.finished_at_ms > *finished && scan.started_at_ms >= *finished,
                "Map scan is older than or overlaps saved history"
            );
        }
        let mut count: usize = transaction.query_row(
            "SELECT COUNT(*) FROM map_scan_quotes WHERE site_host = ?1 AND event_id = ?2 AND section_id = ?3",
            params![scan.site_host, scan.event_id, scan.section_id], |row| row.get(0),
        )?;
        ensure!(
            count <= MAX_QUOTES_PER_SCOPE,
            "Map price history exceeds its section limit"
        );
        transaction.execute(
            "INSERT INTO map_scan_scopes (site_host, event_id, section_id, finished_at_ms, report_hash)
             VALUES (?1, ?2, ?3, ?4, ?5)
             ON CONFLICT(site_host, event_id, section_id) DO UPDATE SET
                 finished_at_ms = excluded.finished_at_ms, report_hash = excluded.report_hash",
            params![scan.site_host, scan.event_id, scan.section_id, scan.finished_at_ms, hash],
        )?;
        for observation in &normalized.observations {
            let Some(quote) = &observation.quote else {
                continue;
            };
            let stored: Option<(i64, String)> = transaction
                .query_row(
                    "SELECT observed_at_ms,
                    CASE WHEN length(CAST(quote_json AS BLOB)) <= ?5 THEN quote_json ELSE NULL END
                 FROM map_scan_quotes
                 WHERE site_host = ?1 AND event_id = ?2 AND section_id = ?3 AND seat_id = ?4",
                    params![
                        scan.site_host,
                        scan.event_id,
                        scan.section_id,
                        observation.seat_id,
                        MAX_QUOTE_BYTES
                    ],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .context("Read saved displayed quote")?;
            let previous = stored
                .map(|(observed_at, raw)| -> Result<_> {
                    let quote: DisplayedSeatQuote =
                        serde_json::from_str(&raw).context("Invalid saved displayed quote")?;
                    validate_displayed_quote(&quote)?;
                    ensure!(
                        quote.seat_id == observation.seat_id
                            && observed_at > 0
                            && previous_scope
                                .as_ref()
                                .is_some_and(|(finished, _)| observed_at <= *finished),
                        "Saved displayed quote has inconsistent identity or time"
                    );
                    Ok((observed_at, quote))
                })
                .transpose()?;
            let changes = if let Some((observed_at, prior)) = &previous {
                if observation.observed_at_ms - *observed_at > COMPARISON_MAX_AGE_MS
                    || !same_identity(prior, quote)
                {
                    vec![ChangeKind::BaselineRefreshed]
                } else {
                    let mut changes = Vec::new();
                    if prior.price_minor != quote.price_minor {
                        changes.push(ChangeKind::PriceChanged);
                    }
                    if prior.restriction != quote.restriction {
                        changes.push(ChangeKind::DetailsChanged);
                    }
                    changes
                }
            } else {
                ensure!(
                    count < MAX_QUOTES_PER_SCOPE,
                    "Map price history section limit reached"
                );
                count += 1;
                vec![ChangeKind::FirstObserved]
            };
            for kind in changes {
                result.changes.push(SeatChange {
                    kind,
                    seat_id: observation.seat_id.clone(),
                    observed_at_ms: observation.observed_at_ms,
                    previous_observed_at_ms: previous.as_ref().map(|(at, _)| *at),
                    previous_price_minor: previous.as_ref().map(|(_, quote)| quote.price_minor),
                    current: quote.clone(),
                });
            }
            let raw = serde_json::to_string(quote)?;
            ensure!(
                raw.len() <= MAX_QUOTE_BYTES,
                "Displayed quote exceeds history limit"
            );
            transaction.execute(
                "INSERT INTO map_scan_quotes (site_host, event_id, section_id, seat_id, observed_at_ms, quote_json)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6)
                 ON CONFLICT(site_host, event_id, section_id, seat_id) DO UPDATE SET
                     observed_at_ms = excluded.observed_at_ms, quote_json = excluded.quote_json",
                params![scan.site_host, scan.event_id, scan.section_id, observation.seat_id, observation.observed_at_ms, raw],
            )?;
        }
        transaction.commit()?;
        Ok(result)
    }
}

fn same_identity(left: &DisplayedSeatQuote, right: &DisplayedSeatQuote) -> bool {
    left.seat_id == right.seat_id
        && left.zone == right.zone
        && left.section == right.section
        && left.row == right.row
        && left.seat == right.seat
        && left.ticket_type == right.ticket_type
        && left.currency == right.currency
        && left.price_basis == right.price_basis
        && left.evidence == right.evidence
}
