//! Compact observations for price and observed-listing charts. These are
//! snapshots, not transactions, sales volume or an event-wide stock estimate.
use std::{
    collections::{BTreeMap, BTreeSet},
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::{
    config::Watch,
    model::{
        Availability, Evidence, FeeScope, InventoryKind, Observation, PriceScope, QuantityKind,
    },
};

pub const TABLE_NAMES: &[&str] = &[
    "market_history_meta",
    "market_events",
    "market_sources",
    "market_samples",
];
pub const MAX_EXPORT_BYTES: usize = 16 * 1024 * 1024;
pub const MAX_EXPORT_SAMPLES: usize = 10_000;
const MAX_EXPORT_EVENTS: usize = 2_000;
const MAX_EXPORT_SOURCES: usize = 5_000;
const MAX_EXPORT_SERIES: usize = 200_000;
const MAX_INTERVAL_SECS: u64 = 31_536_000;
const MAX_SAMPLE_BYTES: usize = 1024 * 1024;
const MAX_ENTRIES: usize = 20_000;
const MAX_SERIES: usize = 2_000;
const MAX_NUMBER: i64 = crate::cart::MAX_EXACT_INTEGER as i64;
const MAX_TIMESTAMP: i64 = MAX_NUMBER / 1000;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SeriesKind {
    Offer,
    DisplayedCategory,
    DisplayedSeat,
}

#[derive(Debug, Clone)]
pub struct EventMetadata {
    pub event_url: String,
    pub name: String,
    pub artist: String,
    pub country: String,
    pub city: Option<String>,
}

#[derive(Debug, Clone)]
pub struct SourceMetadata {
    pub watch_id: String,
    /// The actual observation scope, such as event_page or selected map section.
    pub scope: String,
    /// Zero denotes an unscheduled, manually captured map sample.
    pub interval_secs: u64,
    /// Hash of configuration affecting source coverage; never exported.
    pub config_fingerprint: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistorySeat {
    /// Stable hash of provider seat identity; never the raw provider identifier.
    pub seat_key: String,
    pub section: String,
    pub row: String,
    pub number: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HistoryEntry {
    /// Used only to deduplicate one sample; never persisted or exported.
    pub entry_id: String,
    pub ticket_type: Option<String>,
    pub inventory_kind: InventoryKind,
    pub currency: Option<String>,
    pub price_scope: PriceScope,
    pub fees: FeeScope,
    pub quantity_kind: QuantityKind,
    pub kind: SeriesKind,
    #[serde(default)]
    pub seat: Option<HistorySeat>,
    pub min_price_minor: Option<i64>,
    pub max_price_minor: Option<i64>,
    pub quantity: Option<u32>,
}

#[derive(Debug, Clone)]
pub struct SampleInput {
    pub observed_at: i64,
    pub availability: Availability,
    pub evidence: Evidence,
    pub complete: bool,
    pub entries: Vec<HistoryEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryExport {
    pub schema_version: u32,
    pub generated_at: i64,
    pub events: Vec<HistoryEvent>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistoryEvent {
    pub event_id: String,
    pub event_url: String,
    pub name: String,
    pub artist: String,
    pub country: String,
    pub city: Option<String>,
    pub first_observed_at: i64,
    pub last_observed_at: i64,
    pub sources: Vec<HistorySource>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistorySource {
    pub source_id: String,
    pub watch_id: String,
    pub scope: String,
    pub interval_secs: u64,
    pub samples: Vec<HistorySample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistorySample {
    pub observed_at: i64,
    pub availability: Availability,
    pub evidence: Evidence,
    pub complete: bool,
    pub series: Vec<HistorySeries>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HistorySeries {
    pub series_id: String,
    pub ticket_type: Option<String>,
    pub inventory_kind: InventoryKind,
    pub currency: Option<String>,
    pub price_scope: PriceScope,
    /// Bundle size for comparable total-price quotes. Not a stock estimate.
    #[serde(default)]
    pub price_quantity: Option<u32>,
    pub fees: FeeScope,
    pub quantity_kind: QuantityKind,
    pub kind: SeriesKind,
    #[serde(default)]
    pub seat: Option<HistorySeat>,
    pub min_price_minor: Option<i64>,
    pub max_price_minor: Option<i64>,
    /// Present only if every entry has one exact price, never a range midpoint.
    pub median_price_minor: Option<f64>,
    pub observed_count: u32,
    /// Sum of explicitly reported positive quantities across distinct entries
    /// in this sample. Listings may overlap; this is not total event stock.
    pub reported_quantity: Option<u64>,
}

fn digest(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}
fn valid_time(value: i64) -> bool {
    (1..=MAX_TIMESTAMP).contains(&value)
}
fn label(raw: &str) -> String {
    // Configured labels may be shown in a public export, never raw diagnostics.
    if raw.len() > 4096 || raw.contains("://") || raw.contains("?token=") {
        return String::new();
    }
    raw.chars()
        .filter(|c| !c.is_control())
        .take(256)
        .collect::<String>()
        .trim()
        .to_owned()
}

fn event_identity(event: &EventMetadata) -> Result<(String, String)> {
    ensure!(
        event.event_url.len() <= 4096 && !event.event_url.chars().any(char::is_control),
        "Invalid history event URL"
    );
    let url = crate::hybrid::canonical_event_url(&event.event_url)?;
    let id = if let Some((host, id)) = crate::event_identity::identity(&event.event_url) {
        format!("tm:{host}:{id}")
    } else {
        // Generic public /event/ fixture routes are legitimate watcher targets,
        // but never share identity with a recognized provider event ID.
        format!("url:{}", digest(url.as_bytes()))
    };
    Ok((id, url))
}

fn schema_present(connection: &Connection) -> Result<bool> {
    let mut statement = connection.prepare("SELECT name FROM sqlite_master WHERE type='table' AND name IN ('market_history_meta','market_events','market_sources','market_samples')")?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    if names.is_empty() {
        return Ok(false);
    }
    ensure!(
        names.len() == TABLE_NAMES.len(),
        "Incomplete market history schema"
    );
    let version: Option<i64> = connection
        .query_row(
            "SELECT schema_version FROM market_history_meta WHERE id=1",
            [],
            |row| row.get(0),
        )
        .optional()?;
    ensure!(
        version == Some(1),
        "Unsupported market history schema version"
    );
    Ok(true)
}

/// Additive initialization; compatible with an enclosing caller transaction.
/// The schema guard runs before any mutation and the savepoint is atomic.
pub fn initialize(connection: &Connection) -> Result<()> {
    if schema_present(connection)? {
        return Ok(());
    }
    connection.execute_batch("SAVEPOINT market_history_initialize")?;
    let result = connection.execute_batch(
        "CREATE TABLE market_history_meta (id INTEGER PRIMARY KEY CHECK(id=1), schema_version INTEGER NOT NULL);
         CREATE TABLE market_events (event_id TEXT PRIMARY KEY, event_url TEXT NOT NULL, name TEXT NOT NULL, artist TEXT NOT NULL, country TEXT NOT NULL, city TEXT, first_observed_at INTEGER NOT NULL, last_observed_at INTEGER NOT NULL);
         CREATE TABLE market_sources (source_id TEXT PRIMARY KEY, event_id TEXT NOT NULL REFERENCES market_events(event_id), watch_id TEXT NOT NULL, scope TEXT NOT NULL, interval_secs INTEGER NOT NULL);
         CREATE TABLE market_samples (id INTEGER PRIMARY KEY, source_id TEXT NOT NULL REFERENCES market_sources(source_id), observed_at INTEGER NOT NULL, payload_hash TEXT NOT NULL, sample_json TEXT NOT NULL, UNIQUE(source_id,observed_at,payload_hash));
         CREATE INDEX market_samples_time ON market_samples(observed_at,source_id);
         INSERT INTO market_history_meta(id,schema_version) VALUES (1,1);"
    );
    match result {
        Ok(()) => {
            connection.execute_batch("RELEASE market_history_initialize")?;
            Ok(())
        }
        Err(error) => {
            connection.execute_batch(
                "ROLLBACK TO market_history_initialize; RELEASE market_history_initialize",
            )?;
            Err(error.into())
        }
    }
}

fn group_id(entry: &HistoryEntry) -> Result<String> {
    let dimensions = (
        &entry.ticket_type,
        entry.inventory_kind,
        &entry.currency,
        entry.price_scope,
        price_quantity(entry),
        entry.fees,
        entry.quantity_kind,
        &entry.kind,
    );
    // Preserve schema-v1 aggregate IDs written before optional per-seat views.
    // Missing seat and explicit null both retain their original series identity.
    let bytes = if let Some(seat) = &entry.seat {
        serde_json::to_vec(&(dimensions, seat))?
    } else {
        serde_json::to_vec(&dimensions)?
    };
    Ok(digest(&bytes))
}

fn price_quantity(entry: &HistoryEntry) -> Option<u32> {
    if entry.price_scope == PriceScope::Total
        && entry.kind == SeriesKind::Offer
        && entry.quantity_kind == QuantityKind::Listing
    {
        entry.quantity.filter(|quantity| *quantity > 0)
    } else {
        None
    }
}

fn seat_label(raw: &str) -> Option<String> {
    if raw.len() > 256 || raw.chars().any(char::is_control) || raw.contains("://") {
        return None;
    }
    let normalized = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if matches!(
        normalized.to_ascii_lowercase().as_str(),
        "" | "unknown" | "?" | "-" | "n/a" | "null" | "none" | "undefined"
    ) {
        return None;
    }
    Some(normalized)
}

fn normalized_seat(seat: &HistorySeat) -> Option<HistorySeat> {
    if seat.seat_key.len() != 64 || !seat.seat_key.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return None;
    }
    Some(HistorySeat {
        seat_key: seat.seat_key.to_ascii_lowercase(),
        section: seat_label(&seat.section)?,
        row: seat_label(&seat.row)?,
        number: seat_label(&seat.number)?,
    })
}

fn normalize_entry(entry: &HistoryEntry) -> Option<HistoryEntry> {
    if entry.entry_id.is_empty() || entry.entry_id.len() > 4096 {
        return None;
    }
    if entry
        .currency
        .as_ref()
        .is_some_and(|value| value.len() != 3 || !value.bytes().all(|b| b.is_ascii_uppercase()))
    {
        return None;
    }
    if entry
        .min_price_minor
        .is_some_and(|price| !(0..=MAX_NUMBER).contains(&price))
        || entry
            .max_price_minor
            .is_some_and(|price| !(0..=MAX_NUMBER).contains(&price))
        || matches!((entry.min_price_minor,entry.max_price_minor),(Some(min),Some(max)) if min>max)
        || entry.min_price_minor.is_some() != entry.max_price_minor.is_some()
    {
        return None;
    }
    let mut entry = entry.clone();
    if let Some(seat) = &entry.seat {
        if entry.kind != SeriesKind::DisplayedSeat {
            return None;
        }
        entry.seat = Some(normalized_seat(seat)?);
    }
    entry.ticket_type = entry
        .ticket_type
        .as_deref()
        .map(label)
        .filter(|v| !v.is_empty());
    // Selection capacity and displayed-seat counts are not remaining inventory.
    if entry.kind != SeriesKind::Offer
        || !matches!(
            entry.quantity_kind,
            QuantityKind::Listing | QuantityKind::Remaining
        )
    {
        entry.quantity = None;
    }
    if entry.kind != SeriesKind::Offer && entry.quantity_kind != QuantityKind::Selectable {
        entry.quantity_kind = QuantityKind::Unknown;
    }
    if entry.price_scope == PriceScope::Total && price_quantity(&entry).is_none() {
        entry.min_price_minor = None;
        entry.max_price_minor = None;
    }
    Some(entry)
}

fn aggregate(input: &SampleInput) -> Result<HistorySample> {
    ensure!(
        valid_time(input.observed_at),
        "Invalid history observation timestamp"
    );
    ensure!(
        input.entries.len() <= MAX_ENTRIES,
        "History sample exceeds entry limit"
    );
    let mut unique: BTreeMap<(String, String), Option<HistoryEntry>> = BTreeMap::new();
    for raw in &input.entries {
        if raw.entry_id.is_empty() || raw.entry_id.len() > 4096 {
            continue;
        }
        if input.availability == Availability::Blocked {
            continue;
        }
        if raw.kind == SeriesKind::Offer
            && (input.availability != Availability::Available || input.evidence != Evidence::Offers)
        {
            continue;
        }
        let mut normalized = normalize_entry(raw);
        if let Some(entry) = &mut normalized
            && let Some(seat) = &entry.seat
        {
            // One physical seat is one observation even if a capture
            // repeats it under multiple transient provider row IDs.
            entry.entry_id = seat.seat_key.clone();
        }
        let identity = if let Some(seat) = raw.seat.as_ref().filter(|seat| {
            raw.kind == SeriesKind::DisplayedSeat
                && seat.seat_key.len() == 64
                && seat.seat_key.bytes().all(|byte| byte.is_ascii_hexdigit())
        }) {
            (
                "individual_displayed_seat".into(),
                seat.seat_key.to_ascii_lowercase(),
            )
        } else {
            (serde_json::to_string(&raw.kind)?, raw.entry_id.clone())
        };
        unique
            .entry(identity)
            .and_modify(|previous| {
                if previous.as_ref() != normalized.as_ref() {
                    *previous = None;
                }
            })
            .or_insert(normalized);
    }
    let mut groups: BTreeMap<String, Vec<HistoryEntry>> = BTreeMap::new();
    for entry in unique.into_values().flatten() {
        groups.entry(group_id(&entry)?).or_default().push(entry);
    }
    ensure!(
        groups.len() <= MAX_SERIES,
        "History sample exceeds series limit"
    );
    let mut series = Vec::with_capacity(groups.len());
    for (series_id, entries) in groups {
        let first = &entries[0];
        let min_price_minor = entries.iter().filter_map(|e| e.min_price_minor).min();
        let max_price_minor = entries.iter().filter_map(|e| e.max_price_minor).max();
        let median_price_minor = if entries
            .iter()
            .all(|e| e.min_price_minor.is_some() && e.min_price_minor == e.max_price_minor)
        {
            let mut prices = entries
                .iter()
                .map(|e| e.min_price_minor.unwrap())
                .collect::<Vec<_>>();
            prices.sort_unstable();
            let middle = prices.len() / 2;
            if prices.len() % 2 == 1 {
                Some(prices[middle] as f64)
            } else {
                let sum = prices[middle - 1] + prices[middle];
                // An odd half-minor-unit median above the exact f64 boundary
                // cannot be represented faithfully; leave it unknown.
                if sum % 2 == 0 {
                    Some((sum / 2) as f64)
                } else if sum <= MAX_NUMBER {
                    Some(sum as f64 / 2.0)
                } else {
                    None
                }
            }
        } else {
            None
        };
        let reported_quantity = if first.kind == SeriesKind::Offer
            && matches!(
                first.quantity_kind,
                QuantityKind::Listing | QuantityKind::Remaining
            ) {
            entries.iter().try_fold(0u64, |sum, e| {
                e.quantity
                    .filter(|q| *q > 0)
                    .and_then(|q| sum.checked_add(u64::from(q)))
                    .filter(|sum| *sum <= crate::cart::MAX_EXACT_INTEGER)
            })
        } else {
            None
        };
        series.push(HistorySeries {
            series_id,
            ticket_type: first.ticket_type.clone(),
            inventory_kind: first.inventory_kind,
            currency: first.currency.clone(),
            price_scope: first.price_scope,
            price_quantity: price_quantity(first),
            fees: first.fees,
            quantity_kind: first.quantity_kind,
            kind: first.kind.clone(),
            seat: first.seat.clone(),
            min_price_minor,
            max_price_minor,
            median_price_minor,
            observed_count: entries.len() as u32,
            reported_quantity,
        });
    }
    Ok(HistorySample {
        observed_at: input.observed_at,
        availability: input.availability,
        evidence: input.evidence,
        complete: input.complete,
        series,
    })
}

/// Persist one compact normalized snapshot in the caller's transaction.
/// Exact replay is ignored; distinct samples in the same second remain ordered.
pub fn record_sample(
    transaction: &Transaction<'_>,
    event: &EventMetadata,
    source: &SourceMetadata,
    input: &SampleInput,
) -> Result<()> {
    let (event_id, event_url) = event_identity(event)?;
    ensure!(
        !source.watch_id.is_empty()
            && source.watch_id.len() <= 256
            && !source.watch_id.chars().any(char::is_control),
        "Invalid history watch identity"
    );
    ensure!(
        !source.scope.is_empty()
            && source.scope.len() <= 1024
            && !source.scope.chars().any(char::is_control)
            && !source.scope.contains("://"),
        "Invalid history source scope"
    );
    ensure!(
        source.interval_secs <= MAX_INTERVAL_SECS,
        "Invalid history source interval"
    );
    ensure!(
        source.config_fingerprint.len() == 64
            && source
                .config_fingerprint
                .bytes()
                .all(|b| b.is_ascii_hexdigit()),
        "Invalid history source fingerprint"
    );
    let source_id = digest(&serde_json::to_vec(&(
        &event_id,
        &source.watch_id,
        &source.scope,
        source.interval_secs,
        &source.config_fingerprint,
    ))?);
    let sample = aggregate(input)?;
    let raw = serde_json::to_string(&sample)?;
    ensure!(
        raw.len() <= MAX_SAMPLE_BYTES,
        "History sample exceeds byte limit"
    );
    transaction.execute("INSERT INTO market_events(event_id,event_url,name,artist,country,city,first_observed_at,last_observed_at) VALUES (?1,?2,?3,?4,?5,?6,?7,?7) ON CONFLICT(event_id) DO UPDATE SET event_url=excluded.event_url,name=CASE WHEN excluded.name<>'' THEN excluded.name ELSE name END,artist=CASE WHEN excluded.artist<>'' THEN excluded.artist ELSE artist END,country=CASE WHEN excluded.country<>'' THEN excluded.country ELSE country END,city=CASE WHEN excluded.city IS NOT NULL AND excluded.city<>'' THEN excluded.city ELSE city END,first_observed_at=min(first_observed_at,excluded.first_observed_at),last_observed_at=max(last_observed_at,excluded.last_observed_at)",params![event_id,event_url,label(&event.name),label(&event.artist),label(&event.country),event.city.as_deref().map(label),input.observed_at])?;
    transaction.execute("INSERT OR IGNORE INTO market_sources(source_id,event_id,watch_id,scope,interval_secs) VALUES (?1,?2,?3,?4,?5)",params![source_id,event_id,source.watch_id,source.scope,source.interval_secs])?;
    transaction.execute("INSERT OR IGNORE INTO market_samples(source_id,observed_at,payload_hash,sample_json) VALUES (?1,?2,?3,?4)",params![source_id,input.observed_at,digest(raw.as_bytes()),raw])?;
    Ok(())
}

/// Record all observed rows, independent of notification price/quantity filters.
/// Zero is the existing offline-replay marker and never becomes a live-history point.
pub fn record_observation(
    transaction: &Transaction<'_>,
    watch: &Watch,
    observation: &Observation,
) -> Result<()> {
    ensure!(
        observation.watch_id == watch.id,
        "History observation belongs to a different watch"
    );
    if observation.observed_at == 0 {
        return Ok(());
    }
    ensure!(
        observation
            .offers
            .len()
            .saturating_add(observation.ticket_categories.len())
            <= MAX_ENTRIES,
        "History sample exceeds entry limit"
    );
    let event = EventMetadata {
        event_url: watch.url.clone(),
        name: watch.name.clone(),
        artist: watch.artist.clone(),
        country: watch.country.clone(),
        city: watch.city.clone(),
    };
    let source = SourceMetadata {
        watch_id: watch.id.clone(),
        scope: "event_page".into(),
        interval_secs: watch.interval_secs,
        config_fingerprint: digest(&serde_json::to_vec(watch)?),
    };
    let entries = observation
        .offers
        .iter()
        .map(|offer| HistoryEntry {
            entry_id: offer.key.clone(),
            ticket_type: offer.ticket_type.clone(),
            inventory_kind: offer.inventory_kind,
            currency: offer.currency.clone(),
            price_scope: offer.price_scope,
            fees: offer.fees,
            quantity_kind: offer.quantity_kind,
            kind: SeriesKind::Offer,
            seat: None,
            min_price_minor: offer.price_minor,
            max_price_minor: offer.price_minor,
            quantity: offer.quantity,
        })
        .chain(
            observation
                .ticket_categories
                .iter()
                .map(|category| HistoryEntry {
                    entry_id: category.key.clone(),
                    ticket_type: Some(category.ticket_type.clone()),
                    inventory_kind: category.inventory_kind,
                    currency: category.currency.clone(),
                    price_scope: PriceScope::Unknown,
                    fees: FeeScope::Unknown,
                    quantity_kind: if category.selectable_quantity.is_some() {
                        QuantityKind::Selectable
                    } else {
                        QuantityKind::Unknown
                    },
                    kind: SeriesKind::DisplayedCategory,
                    seat: None,
                    min_price_minor: category.price_min_minor,
                    max_price_minor: category.price_max_minor,
                    quantity: None,
                }),
        )
        .collect();
    record_sample(
        transaction,
        &event,
        &source,
        &SampleInput {
            observed_at: observation.observed_at,
            availability: observation.availability,
            evidence: observation.evidence,
            complete: observation.complete,
            entries,
        },
    )
}

/// Read an existing database only. A legacy database without history exports
/// no events; it is never migrated or backfilled by this operation.
pub fn read_export(
    path: &Path,
    generated_at: i64,
    from: Option<i64>,
    to: Option<i64>,
) -> Result<HistoryExport> {
    read_export_filtered(path, generated_at, from, to, None)
}

/// Restrict the SQL query to one canonical event before applying export limits.
pub fn read_export_event(
    path: &Path,
    generated_at: i64,
    from: Option<i64>,
    to: Option<i64>,
    event_url: &str,
) -> Result<HistoryExport> {
    let event = EventMetadata {
        event_url: event_url.into(),
        name: String::new(),
        artist: String::new(),
        country: String::new(),
        city: None,
    };
    let (event_id, _) = event_identity(&event)?;
    read_export_filtered(path, generated_at, from, to, Some(&event_id))
}

fn read_export_filtered(
    path: &Path,
    generated_at: i64,
    from: Option<i64>,
    to: Option<i64>,
    event_filter: Option<&str>,
) -> Result<HistoryExport> {
    ensure!(valid_time(generated_at), "Invalid history export timestamp");
    let start = from.unwrap_or(0);
    let end = to.unwrap_or(generated_at);
    ensure!(
        start >= 0 && end >= start && end <= MAX_TIMESTAMP,
        "Invalid history export time range"
    );
    let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .context("Open existing history database read-only")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    let transaction = connection.transaction()?;
    let mut export = HistoryExport {
        schema_version: 1,
        generated_at,
        events: vec![],
    };
    if !schema_present(&transaction)? {
        return Ok(export);
    }
    let mut statement=transaction.prepare("SELECT CASE WHEN length(CAST(e.event_id AS BLOB))<=256 THEN e.event_id END,CASE WHEN length(CAST(e.event_url AS BLOB))<=4096 THEN e.event_url END,CASE WHEN length(CAST(e.name AS BLOB))<=1024 THEN e.name END,CASE WHEN length(CAST(e.artist AS BLOB))<=1024 THEN e.artist END,CASE WHEN length(CAST(e.country AS BLOB))<=1024 THEN e.country END,CASE WHEN length(CAST(e.city AS BLOB))<=1024 THEN e.city END,e.first_observed_at,e.last_observed_at,CASE WHEN length(CAST(s.source_id AS BLOB))<=64 THEN s.source_id END,CASE WHEN length(CAST(s.watch_id AS BLOB))<=256 THEN s.watch_id END,CASE WHEN length(CAST(s.scope AS BLOB))<=1024 THEN s.scope END,s.interval_secs,p.observed_at,CASE WHEN length(CAST(p.sample_json AS BLOB))<=?3 THEN p.sample_json ELSE NULL END FROM market_samples p JOIN market_sources s ON s.source_id=p.source_id JOIN market_events e ON e.event_id=s.event_id WHERE p.observed_at>=?1 AND p.observed_at<=?2 AND (?5 IS NULL OR e.event_id=?5) ORDER BY e.event_id,s.source_id,p.observed_at,p.id LIMIT ?4")?;
    let mut rows = statement.query(params![
        start,
        end,
        MAX_SAMPLE_BYTES as i64,
        (MAX_EXPORT_SAMPLES + 1) as i64,
        event_filter
    ])?;
    let mut event_indices = BTreeMap::new();
    let mut source_indices = BTreeMap::new();
    let mut byte_count = 0usize;
    let mut sample_count = 0usize;
    let mut series_count = 0usize;
    while let Some(row) = rows.next()? {
        sample_count += 1;
        ensure!(
            sample_count <= MAX_EXPORT_SAMPLES,
            "History export exceeds sample limit; choose a narrower time range"
        );
        let raw: Option<String> = row.get(13)?;
        let raw = raw.context("Stored history sample exceeds byte limit")?;
        byte_count = byte_count
            .checked_add(raw.len())
            .context("History export size overflow")?;
        ensure!(
            byte_count <= MAX_EXPORT_BYTES,
            "History export exceeds byte limit; choose a narrower time range"
        );
        let sample: HistorySample =
            serde_json::from_str(&raw).context("Invalid stored history sample")?;
        ensure!(
            sample.observed_at == row.get::<_, i64>(12)? && valid_time(sample.observed_at),
            "Invalid stored history sample timestamp"
        );
        validate_sample(&sample)?;
        series_count += sample.series.len();
        ensure!(
            series_count <= MAX_EXPORT_SERIES,
            "History export exceeds series point limit; choose a narrower time range"
        );
        let event_id: String = row.get(0)?;
        let source_id: String = row.get(8)?;
        let event_index = if let Some(index) = event_indices.get(&event_id) {
            *index
        } else {
            let event = HistoryEvent {
                event_id: event_id.clone(),
                event_url: row.get(1)?,
                name: row.get(2)?,
                artist: row.get(3)?,
                country: row.get(4)?,
                city: row.get(5)?,
                first_observed_at: row.get(6)?,
                last_observed_at: row.get(7)?,
                sources: vec![],
            };
            let checked = EventMetadata {
                event_url: event.event_url.clone(),
                name: event.name.clone(),
                artist: event.artist.clone(),
                country: event.country.clone(),
                city: event.city.clone(),
            };
            let (checked_id, checked_url) = event_identity(&checked)?;
            ensure!(
                checked_id == event_id
                    && checked_url == event.event_url
                    && valid_time(event.first_observed_at)
                    && valid_time(event.last_observed_at)
                    && event.first_observed_at <= event.last_observed_at,
                "Invalid stored history event"
            );
            ensure!(
                [
                    event.name.as_str(),
                    event.artist.as_str(),
                    event.country.as_str(),
                    event.city.as_deref().unwrap_or("")
                ]
                .iter()
                .all(|value| value.len() <= 1024 && label(value) == *value),
                "Invalid stored history metadata"
            );
            let index = export.events.len();
            ensure!(
                index < MAX_EXPORT_EVENTS,
                "History export exceeds event limit; select one event"
            );
            byte_count += serde_json::to_vec(&event)?.len();
            ensure!(
                byte_count <= MAX_EXPORT_BYTES,
                "History export exceeds byte limit; choose a narrower time range"
            );
            export.events.push(event);
            event_indices.insert(event_id, index);
            index
        };
        let source_index = if let Some(index) = source_indices.get(&source_id) {
            *index
        } else {
            let source = HistorySource {
                source_id: source_id.clone(),
                watch_id: row.get(9)?,
                scope: row.get(10)?,
                interval_secs: row.get(11)?,
                samples: vec![],
            };
            ensure!(
                source.source_id.len() == 64
                    && source.source_id.bytes().all(|b| b.is_ascii_hexdigit())
                    && !source.watch_id.is_empty()
                    && source.watch_id.len() <= 256
                    && !source.watch_id.chars().any(char::is_control)
                    && !source.scope.is_empty()
                    && source.scope.len() <= 1024
                    && !source.scope.chars().any(char::is_control)
                    && !source.scope.contains("://")
                    && source.interval_secs <= MAX_INTERVAL_SECS,
                "Invalid stored history source"
            );
            let index = export.events[event_index].sources.len();
            ensure!(
                source_indices.len() < MAX_EXPORT_SOURCES,
                "History export exceeds source limit; choose a narrower time range"
            );
            byte_count += serde_json::to_vec(&source)?.len();
            ensure!(
                byte_count <= MAX_EXPORT_BYTES,
                "History export exceeds byte limit; choose a narrower time range"
            );
            export.events[event_index].sources.push(source);
            source_indices.insert(source_id, index);
            index
        };
        export.events[event_index].sources[source_index]
            .samples
            .push(sample);
    }
    ensure!(
        serde_json::to_vec(&export)?.len() <= MAX_EXPORT_BYTES,
        "History export exceeds byte limit; choose a narrower time range"
    );
    Ok(export)
}

fn validate_sample(sample: &HistorySample) -> Result<()> {
    ensure!(
        sample.series.len() <= MAX_SERIES,
        "Stored history exceeds series limit"
    );
    let mut seen = BTreeSet::new();
    for series in &sample.series {
        let entry = HistoryEntry {
            entry_id: "validation".into(),
            ticket_type: series.ticket_type.clone(),
            inventory_kind: series.inventory_kind,
            currency: series.currency.clone(),
            price_scope: series.price_scope,
            fees: series.fees,
            quantity_kind: series.quantity_kind,
            kind: series.kind.clone(),
            seat: series.seat.clone(),
            min_price_minor: series.min_price_minor,
            max_price_minor: series.max_price_minor,
            quantity: series.price_quantity,
        };
        ensure!(
            normalize_entry(&entry).as_ref() == Some(&entry)
                && price_quantity(&entry) == series.price_quantity
                && group_id(&entry)? == series.series_id
                && seen.insert(&series.series_id),
            "Invalid stored history series identity or price"
        );
        ensure!(
            (1..=MAX_ENTRIES as u32).contains(&series.observed_count),
            "Invalid stored history observed count"
        );
        ensure!(
            series.seat.is_none() || series.observed_count == 1,
            "An individual seat series must contain one observed seat"
        );
        ensure!(
            series
                .median_price_minor
                .is_none_or(|value| value.is_finite()
                    && value >= 0.0
                    && value <= MAX_NUMBER as f64
                    && series
                        .min_price_minor
                        .is_some_and(|min| value >= min as f64)
                    && series
                        .max_price_minor
                        .is_some_and(|max| value <= max as f64)),
            "Invalid stored history median"
        );
        ensure!(
            series.reported_quantity.is_none_or(|value| value > 0
                && value <= crate::cart::MAX_EXACT_INTEGER
                && series.kind == SeriesKind::Offer
                && matches!(
                    series.quantity_kind,
                    QuantityKind::Listing | QuantityKind::Remaining
                )),
            "Invalid stored history quantity"
        );
        ensure!(
            sample.availability != Availability::Blocked
                && (series.kind != SeriesKind::Offer
                    || (sample.availability == Availability::Available
                        && sample.evidence == Evidence::Offers)),
            "Stored history has unsupported available offers"
        );
    }
    Ok(())
}
