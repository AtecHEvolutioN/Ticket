//! Durable observations and an at-least-once notification outbox.
//!
//! Unknown observations affect monitoring health, never the last valid inventory.
//! All state transitions and their outbound notifications commit in one transaction.

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    path::Path,
    time::Duration,
};

use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, Transaction, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use crate::{
    config::{DiscordConfig, Watch},
    model::{
        Alert, Availability, Evidence, Observation, Offer, PendingAlert, PriceScope, QuantityKind,
    },
    venues::{self, VenuePreset},
};

const MAX_PENDING: i64 = 10_000;
const MAX_CATALOG_BYTES: usize = 16 * 1024 * 1024;
const CATALOG_BUDGET_WINDOW_SECS: i64 = 86_400;

pub struct Store {
    connection: Connection,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct InventoryState {
    initialized: bool,
    qualifying_available: bool,
    generation: i64,
    offers: BTreeMap<String, KnownOffer>,
    #[serde(default)]
    strong_evidence_seen: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct KnownOffer {
    offer: Offer,
    qualifies: Option<bool>,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent).context("create state directory")?;
        }
        let connection = Connection::open(path).context("open state database")?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection
            .execute_batch(
                "PRAGMA journal_mode=WAL;
             PRAGMA synchronous=FULL;
             PRAGMA foreign_keys=ON;
             CREATE TABLE IF NOT EXISTS metadata (
                 key TEXT PRIMARY KEY, value TEXT NOT NULL
             );
             INSERT OR IGNORE INTO metadata(key, value)
                 VALUES ('instance', lower(hex(randomblob(16))));
             CREATE TABLE IF NOT EXISTS cooldowns (
                 scope TEXT PRIMARY KEY, until_timestamp INTEGER NOT NULL
             );
             CREATE TABLE IF NOT EXISTS catalog_snapshot (
                 id INTEGER PRIMARY KEY CHECK (id = 1), snapshot_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS catalog_requests (
                 id INTEGER PRIMARY KEY, reserved_at INTEGER NOT NULL
             );
             CREATE INDEX IF NOT EXISTS catalog_requests_time ON catalog_requests(reserved_at);
             CREATE TABLE IF NOT EXISTS venue_presets (
                 id TEXT PRIMARY KEY,
                 site_host TEXT NOT NULL,
                 provider_venue_id TEXT,
                 match_key TEXT NOT NULL,
                 preset_json TEXT NOT NULL
             );
             CREATE UNIQUE INDEX IF NOT EXISTS venue_provider_identity
                 ON venue_presets(site_host, provider_venue_id)
                 WHERE provider_venue_id IS NOT NULL;
             CREATE INDEX IF NOT EXISTS venue_name_identity ON venue_presets(match_key);
             CREATE TABLE IF NOT EXISTS watches (
                 id TEXT PRIMARY KEY,
                 fingerprint TEXT NOT NULL,
                 enabled INTEGER NOT NULL,
                 state_json TEXT NOT NULL,
                 observed_availability TEXT NOT NULL DEFAULT 'unobserved',
                 evidence TEXT NOT NULL DEFAULT 'none',
                 detail TEXT NOT NULL DEFAULT '',
                 last_observed_at INTEGER,
                 last_valid_at INTEGER
             );
             CREATE TABLE IF NOT EXISTS watch_observations (
                 watch_id TEXT PRIMARY KEY REFERENCES watches(id),
                 observation_json TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS outbox (
                 id INTEGER PRIMARY KEY AUTOINCREMENT,
                 watch_id TEXT NOT NULL REFERENCES watches(id),
                 generation INTEGER NOT NULL,
                 nonce TEXT NOT NULL UNIQUE,
                 payload_json TEXT NOT NULL,
                 state TEXT NOT NULL DEFAULT 'pending'
                     CHECK (state IN ('pending','delivered','cancelled','expired')),
                 attempts INTEGER NOT NULL DEFAULT 0,
                 not_before INTEGER NOT NULL,
                 created_at INTEGER NOT NULL,
                 delivered_message_id TEXT,
                 last_error TEXT,
                 UNIQUE(watch_id, generation)
             );
             CREATE INDEX IF NOT EXISTS outbox_due ON outbox(state, not_before, id);
             CREATE INDEX IF NOT EXISTS outbox_watch ON outbox(watch_id, state);",
            )
            .context("initialize state database")?;
        Ok(Self { connection })
    }

    /// Offline metadata lookup. A venue preset never supplies event inventory.
    pub fn venue_list(&self) -> Result<Vec<VenuePreset>> {
        let mut statement = self
            .connection
            .prepare("SELECT preset_json FROM venue_presets ORDER BY site_host, id")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.map(|row| read_venue(&row?)).collect()
    }

    pub fn venue_get(&self, id: &str) -> Result<Option<VenuePreset>> {
        venue_by_id(&self.connection, id)
    }

    /// IDs take precedence. Without a provider ID, an ambiguous exact-name
    /// lookup fails instead of choosing between distinct same-name venues.
    pub fn venue_lookup(&self, query: &VenuePreset) -> Result<Option<VenuePreset>> {
        find_venue(&self.connection, &query.normalized()?)
    }

    /// Preserve the local ID, existing layouts omitted from the input, and
    /// source history. Explicitly supplied layout IDs replace only that layout.
    /// Promoting an exact, unambiguous name match to a provider ID keeps its ID.
    pub fn venue_upsert(&mut self, preset: &VenuePreset) -> Result<VenuePreset> {
        self.venue_import(std::slice::from_ref(preset))?
            .pop()
            .context("Venue upsert returned no preset")
    }

    /// Validate and import the complete batch atomically, including conflicts.
    pub fn venue_import(&mut self, presets: &[VenuePreset]) -> Result<Vec<VenuePreset>> {
        ensure!(
            presets.len() <= venues::MAX_VENUES,
            "Too many venue presets"
        );
        let normalized = presets
            .iter()
            .map(VenuePreset::normalized)
            .collect::<Result<Vec<_>>>()?;
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let mut saved = Vec::with_capacity(normalized.len());
        for preset in normalized {
            saved.push(save_venue(&transaction, preset)?);
        }
        transaction.commit()?;
        Ok(saved)
    }

    /// The caller supplies the normalized Discovery event, not raw API data.
    pub fn venue_register_event(&mut self, event: &Value) -> Result<Vec<VenuePreset>> {
        self.venue_import(&venues::from_discovery_event(event)?)
    }

    /// Reconcile before polling or dispatching so removed/edited watches cannot ping.
    pub fn reconcile(&mut self, watches: &[Watch]) -> Result<()> {
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let ids: HashSet<&str> = watches.iter().map(|w| w.id.as_str()).collect();
        ensure!(ids.len() == watches.len(), "duplicate watch identifiers");
        let existing: Vec<String> = {
            let mut statement = transaction.prepare("SELECT id FROM watches")?;
            statement
                .query_map([], |row| row.get(0))?
                .collect::<rusqlite::Result<_>>()?
        };
        for id in existing {
            if !ids.contains(id.as_str()) {
                transaction.execute("UPDATE watches SET enabled=0 WHERE id=?1", [&id])?;
                cancel_pending(&transaction, &id)?;
            }
        }
        for watch in watches {
            ensure_watch(&transaction, watch)?;
        }
        transaction.commit()?;
        Ok(())
    }

    /// Bind queued work to the configured channel/mention targets, without storing secrets.
    /// Call before watch reconciliation and dispatch at startup.
    pub fn reconcile_delivery(&mut self, delivery: &DiscordConfig) -> Result<()> {
        let fingerprint = format!("{:x}", Sha256::digest(serde_json::to_vec(delivery)?));
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous: Option<String> = transaction
            .query_row(
                "SELECT value FROM metadata WHERE key='delivery_fingerprint'",
                [],
                |row| row.get(0),
            )
            .optional()?;
        if previous.as_deref() != Some(fingerprint.as_str()) {
            // Rows from an old destination (or an unbound pre-upgrade queue) cannot be retargeted.
            transaction.execute(
                "UPDATE outbox SET state='cancelled' WHERE state='pending'",
                [],
            )?;
            transaction.execute(
                "INSERT INTO metadata(key,value) VALUES ('delivery_fingerprint',?1)
                 ON CONFLICT(key) DO UPDATE SET value=excluded.value",
                [fingerprint],
            )?;
        }
        transaction.commit()?;
        Ok(())
    }

    pub fn apply(&mut self, watch: &Watch, observation: &Observation) -> Result<usize> {
        ensure!(
            observation.watch_id == watch.id,
            "observation/watch identifier mismatch"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        if !watch.enabled {
            transaction.commit()?;
            return Ok(0);
        }
        let registered: Option<(String, bool)> = transaction
            .query_row(
                "SELECT fingerprint,enabled FROM watches WHERE id=?1",
                [&watch.id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        if let Some((registered_fingerprint, enabled)) = registered {
            // An old in-flight result must not revive a removed/paused watch.
            if !enabled {
                transaction.commit()?;
                return Ok(0);
            }
            ensure!(
                registered_fingerprint == fingerprint(watch)?,
                "watch configuration changed; reconcile before applying new observations"
            );
        } else {
            ensure_watch(&transaction, watch)?;
        }
        let (state_json, last_observed_at): (String, Option<i64>) = transaction.query_row(
            "SELECT state_json, last_observed_at FROM watches WHERE id=?1",
            [&watch.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if last_observed_at.is_some_and(|last| observation.observed_at < last) {
            transaction.commit()?;
            return Ok(0);
        }
        let trusted = match observation.evidence {
            Evidence::Offers | Evidence::ExplicitStatus => true,
            Evidence::Metadata => watch.notify_metadata,
            Evidence::None => false,
        };
        let health = if observation.evidence == Evidence::Metadata && !watch.notify_metadata {
            "metadata_only"
        } else {
            availability_name(observation.availability)
        };
        transaction.execute(
            "UPDATE watches SET observed_availability=?2, evidence=?3, detail=?4,
                 last_observed_at=?5 WHERE id=?1",
            params![
                watch.id,
                health,
                evidence_name(observation.evidence),
                bounded(&observation.detail),
                observation.observed_at
            ],
        )?;
        let snapshot = serde_json::to_string(observation)?;
        ensure!(
            snapshot.len() <= 16 * 1024 * 1024,
            "observation exceeds storage limit"
        );
        transaction.execute(
            "INSERT INTO watch_observations(watch_id,observation_json) VALUES (?1,?2)
             ON CONFLICT(watch_id) DO UPDATE SET observation_json=excluded.observation_json",
            params![watch.id, snapshot],
        )?;
        if !trusted
            || matches!(
                observation.availability,
                Availability::Unknown | Availability::Blocked
            )
        {
            transaction.commit()?;
            return Ok(0);
        }
        if observation.availability == Availability::Unavailable
            && observation.evidence == Evidence::Offers
            && !observation.complete
        {
            // Absence in a partial inventory response is not negative evidence.
            transaction.commit()?;
            return Ok(0);
        }

        let mut state: InventoryState =
            serde_json::from_str(&state_json).context("read stored inventory state")?;
        // A cached metadata label must not erase stronger, offer-level evidence.
        if observation.evidence == Evidence::Metadata && state.strong_evidence_seen {
            transaction.commit()?;
            return Ok(0);
        }
        if matches!(
            observation.evidence,
            Evidence::Offers | Evidence::ExplicitStatus
        ) {
            state.strong_evidence_seen = true;
        }
        let was_initialized = state.initialized;
        let was_available = state.qualifying_available;
        let mut candidate_offers = Vec::new();
        let mut visible_matches = Vec::new();

        match observation.availability {
            Availability::Unavailable => {
                state.offers.clear();
                state.qualifying_available = false;
                cancel_pending(&transaction, &watch.id)?;
            }
            Availability::Available => {
                // A partial response cannot erase offers absent from that response.
                if observation.evidence == Evidence::Offers {
                    let seen: HashSet<&str> =
                        observation.offers.iter().map(|o| o.key.as_str()).collect();
                    if seen.len() != observation.offers.len() {
                        bail!("ambiguous duplicate offer identifiers");
                    }
                    if observation.complete {
                        state.offers.retain(|key, _| seen.contains(key.as_str()));
                    }
                    for offer in &observation.offers {
                        ensure!(!offer.key.is_empty(), "empty offer identifier");
                        let current_match = matches_filters(watch, offer);
                        let previous = state.offers.get(&offer.key);
                        if current_match == Some(true) {
                            visible_matches.push(offer.clone());
                            let newly_qualifying =
                                previous.is_none_or(|old| old.qualifies != Some(true));
                            let changed = watch.notify_offer_changes
                                && previous.is_some_and(|old| {
                                    old.qualifies == Some(true)
                                        && meaningful_change(&old.offer, offer)
                                });
                            if newly_qualifying || changed {
                                candidate_offers.push(offer.clone());
                            }
                        }
                        // Missing filter evidence is unknown, not a negative transition.
                        let qualifies =
                            current_match.or_else(|| previous.and_then(|old| old.qualifies));
                        state.offers.insert(
                            offer.key.clone(),
                            KnownOffer {
                                offer: offer.clone(),
                                qualifies,
                            },
                        );
                    }
                    state.qualifying_available = state
                        .offers
                        .values()
                        .any(|known| known.qualifies == Some(true))
                        || (!observation.complete && was_available);
                    // Empty offers from an incomplete response are not enough to establish a new baseline.
                    if observation.offers.is_empty() && !observation.complete && !was_initialized {
                        transaction.commit()?;
                        return Ok(0);
                    }
                } else if !has_detail_filters(watch) {
                    state.qualifying_available = true;
                } else {
                    // Event-level metadata cannot evaluate quantity or price predicates.
                    transaction.commit()?;
                    return Ok(0);
                }
            }
            Availability::Unknown | Availability::Blocked => unreachable!(),
        }
        state.initialized = true;
        if !state.qualifying_available {
            cancel_pending(&transaction, &watch.id)?;
        }

        let should_notify = state.qualifying_available
            && ((!was_initialized && watch.notify_initial)
                || (was_initialized && !was_available)
                || (was_initialized && !candidate_offers.is_empty()));
        let mut inserted = 0;
        if should_notify {
            let pending_count: i64 = transaction.query_row(
                "SELECT count(*) FROM outbox WHERE state='pending'",
                [],
                |row| row.get(0),
            )?;
            ensure!(
                pending_count < MAX_PENDING,
                "notification outbox is full; observation not committed"
            );
            state.generation = state
                .generation
                .checked_add(1)
                .context("notification generation overflow")?;
            let kind = if !was_initialized {
                "initial_available"
            } else if !was_available {
                "available"
            } else {
                "offers_changed"
            };
            let offers = if !was_initialized || !was_available {
                visible_matches
            } else {
                candidate_offers
            };
            let payload = Alert {
                watch_id: watch.id.clone(),
                event_name: watch.name.clone(),
                artist: watch.artist.clone(),
                location: watch
                    .city
                    .as_ref()
                    .map(|city| format!("{city}, {}", watch.country))
                    .unwrap_or_else(|| watch.country.clone()),
                event_url: watch.url.clone(),
                observed_at: observation.observed_at,
                kind: kind.into(),
                detail: bounded(&observation.detail),
                offers,
            };
            let instance: String = transaction.query_row(
                "SELECT value FROM metadata WHERE key='instance'",
                [],
                |row| row.get(0),
            )?;
            let digest = format!(
                "{:x}",
                Sha256::digest(format!("{instance}:{}:{}", watch.id, state.generation).as_bytes())
            );
            let nonce = &digest[..24];
            inserted = transaction.execute(
                "INSERT INTO outbox(watch_id,generation,nonce,payload_json,not_before,created_at)
                     VALUES (?1,?2,?3,?4,?5,?5)",
                params![
                    watch.id,
                    state.generation,
                    nonce,
                    serde_json::to_string(&payload)?,
                    observation.observed_at
                ],
            )?;
        }
        cancel_obsolete_offers(&transaction, &watch.id, &state)?;
        transaction.execute(
            "UPDATE watches SET state_json=?2,last_valid_at=?3 WHERE id=?1",
            params![
                watch.id,
                serde_json::to_string(&state)?,
                observation.observed_at
            ],
        )?;
        transaction.commit()?;
        Ok(inserted)
    }

    /// Single-dispatcher API: caller must hold the service's process lock.
    pub fn pending(&self, now: i64, limit: usize) -> Result<Vec<PendingAlert>> {
        let mut statement = self.connection.prepare(
            "SELECT o.id,o.nonce,o.payload_json,o.attempts FROM outbox o
             JOIN watches w ON w.id=o.watch_id
             WHERE o.state='pending' AND w.enabled=1 AND o.not_before<=?1
             ORDER BY o.id LIMIT ?2",
        )?;
        let rows = statement.query_map(
            params![now, limit.min(MAX_PENDING as usize) as i64],
            |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, u32>(3)?,
                ))
            },
        )?;
        let mut pending = Vec::new();
        for row in rows {
            let (id, nonce, payload_json, attempts) = row?;
            pending.push(PendingAlert {
                id,
                nonce,
                payload: serde_json::from_str(&payload_json)?,
                attempts,
            });
        }
        Ok(pending)
    }

    pub fn delivered(&mut self, id: i64, message_id: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE outbox SET state='delivered',delivered_message_id=?2,last_error=NULL
                 WHERE id=?1 AND state='pending'",
            params![id, bounded(message_id)],
        )?;
        Ok(())
    }

    pub fn retry(&mut self, id: i64, not_before: i64, error: &str) -> Result<()> {
        self.connection.execute(
            "UPDATE outbox SET attempts=attempts+1,not_before=?2,last_error=?3
                 WHERE id=?1 AND state='pending'",
            params![id, not_before, bounded(error)],
        )?;
        Ok(())
    }

    pub fn status(&self) -> Result<Vec<Value>> {
        let mut statement = self.connection.prepare(
            "SELECT w.id,w.enabled,w.observed_availability,w.evidence,w.detail,
                 w.last_observed_at,w.last_valid_at,w.state_json,
                 (SELECT count(*) FROM outbox o WHERE o.watch_id=w.id AND o.state='pending')
             FROM watches w ORDER BY w.id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, bool>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, String>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, Option<i64>>(6)?,
                row.get::<_, String>(7)?,
                row.get::<_, i64>(8)?,
            ))
        })?;
        let mut result = Vec::new();
        for row in rows {
            let (
                id,
                enabled,
                health,
                evidence,
                detail,
                last_observed_at,
                last_valid_at,
                state_json,
                pending,
            ) = row?;
            let state: InventoryState = serde_json::from_str(&state_json)?;
            let latest: Option<String> = self
                .connection
                .query_row(
                    "SELECT observation_json FROM watch_observations WHERE watch_id=?1",
                    [&id],
                    |row| row.get(0),
                )
                .optional()?;
            let latest = latest
                .map(|value| serde_json::from_str::<Value>(&value))
                .transpose()?;
            result.push(json!({
                "watch_id": id, "enabled": enabled, "health": health, "evidence": evidence,
                "detail": detail, "last_observed_at": last_observed_at, "last_valid_at": last_valid_at,
                "last_valid_matching_availability": state.initialized.then_some(state.qualifying_available),
                "known_offers": state.offers.len(), "pending_alerts": pending,
                "latest_observation": latest,
            }));
        }
        Ok(result)
    }

    pub fn expire_before(&mut self, cutoff: i64) -> Result<usize> {
        let transaction = self.connection.transaction()?;
        let count = transaction.execute(
            "UPDATE outbox SET state='expired' WHERE state='pending' AND created_at<?1",
            [cutoff],
        )?;
        // Keep terminal delivery history for a bounded period; AUTOINCREMENT preserves IDs.
        transaction.execute(
            "DELETE FROM outbox WHERE state!='pending' AND created_at<?1",
            [cutoff.saturating_sub(30 * 86_400)],
        )?;
        transaction.commit()?;
        Ok(count)
    }

    /// Persist host/provider cooldowns so restarting cannot shorten a wait.
    pub fn set_cooldown(&mut self, scope: &str, until: i64) -> Result<()> {
        ensure!(
            !scope.is_empty() && scope.len() <= 255,
            "invalid cooldown scope"
        );
        self.connection.execute(
            "INSERT INTO cooldowns(scope,until_timestamp) VALUES (?1,?2)
             ON CONFLICT(scope) DO UPDATE SET until_timestamp=max(until_timestamp,excluded.until_timestamp)",
            params![scope, until],
        )?;
        Ok(())
    }

    pub fn cooldowns(&self) -> Result<HashMap<String, i64>> {
        let mut statement = self
            .connection
            .prepare("SELECT scope,until_timestamp FROM cooldowns")?;
        let values = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?;
        Ok(values.collect::<rusqlite::Result<_>>()?)
    }

    pub fn cooldown(&self, scope: &str) -> Result<Option<i64>> {
        Ok(self
            .connection
            .query_row(
                "SELECT until_timestamp FROM cooldowns WHERE scope=?1",
                [scope],
                |row| row.get(0),
            )
            .optional()?)
    }

    /// Cached Discovery selection state, never evidence of ticket inventory.
    /// The catalog layer validates the shape and binds it to its configuration.
    pub fn catalog_load(&self) -> Result<Option<Value>> {
        let stored: Option<(i64, Option<String>)> = self
            .connection
            .query_row(
                "SELECT length(CAST(snapshot_json AS BLOB)),
                    CASE WHEN length(CAST(snapshot_json AS BLOB))<=?1 THEN snapshot_json END
                 FROM catalog_snapshot WHERE id=1",
                [MAX_CATALOG_BYTES as i64],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .optional()?;
        let Some((size, raw)) = stored else {
            return Ok(None);
        };
        ensure!(
            size >= 0 && size <= MAX_CATALOG_BYTES as i64,
            "stored catalog snapshot exceeds the 16 MiB limit"
        );
        let raw = raw.context("stored catalog snapshot is missing")?;
        serde_json::from_str(&raw)
            .map(Some)
            .context("decode cached catalog snapshot")
    }

    /// Replace the cache atomically; a rejected snapshot preserves the previous one.
    pub fn catalog_save(&mut self, snapshot: &Value) -> Result<()> {
        let raw = serde_json::to_string(snapshot)?;
        ensure!(
            raw.len() <= MAX_CATALOG_BYTES,
            "catalog snapshot exceeds the 16 MiB limit"
        );
        self.connection.execute(
            "INSERT INTO catalog_snapshot(id,snapshot_json) VALUES (1,?1)
             ON CONFLICT(id) DO UPDATE SET snapshot_json=excluded.snapshot_json",
            [raw],
        )?;
        Ok(())
    }

    /// Reserve before every catalog HTTP attempt, including attempts that fail.
    /// This is a process-independent rolling 24-hour local budget, not the account quota.
    pub fn reserve_catalog_request(&mut self, now: i64, max_per_day: u32) -> Result<bool> {
        ensure!(now >= 0, "catalog request timestamp must be nonnegative");
        ensure!(
            (1..=5_000).contains(&max_per_day),
            "catalog request budget must be 1–5000"
        );
        let transaction = self
            .connection
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        transaction.execute(
            "DELETE FROM catalog_requests WHERE reserved_at<=?1",
            [now.saturating_sub(CATALOG_BUDGET_WINDOW_SECS)],
        )?;
        // Include future-dated reservations after a clock rollback rather than resetting the budget.
        let count: i64 =
            transaction.query_row("SELECT count(*) FROM catalog_requests", [], |row| {
                row.get(0)
            })?;
        if count >= i64::from(max_per_day) {
            transaction.commit()?;
            return Ok(false);
        }
        transaction.execute(
            "INSERT INTO catalog_requests(reserved_at) VALUES (?1)",
            [now],
        )?;
        transaction.commit()?;
        Ok(true)
    }
}

fn read_venue(raw: &str) -> Result<VenuePreset> {
    ensure!(
        raw.len() <= venues::MAX_PRESET_BYTES,
        "Saved venue preset is too large"
    );
    serde_json::from_str::<VenuePreset>(raw)
        .context("Read saved venue preset")?
        .normalized()
}

fn venue_by_id(connection: &Connection, id: &str) -> Result<Option<VenuePreset>> {
    let raw: Option<String> = connection
        .query_row(
            "SELECT preset_json FROM venue_presets WHERE id=?1",
            [id],
            |row| row.get(0),
        )
        .optional()?;
    raw.as_deref().map(read_venue).transpose()
}

fn compatible_identity(existing: &VenuePreset, incoming: &VenuePreset) -> Result<()> {
    ensure!(
        existing.site_host == incoming.site_host,
        "Venue ID belongs to another Ticketmaster site"
    );
    ensure!(
        existing.country == incoming.country
            && existing.city.to_lowercase() == incoming.city.to_lowercase(),
        "Venue ID has conflicting city or country"
    );
    if let (Some(old), Some(new)) = (&existing.provider_venue_id, &incoming.provider_venue_id) {
        ensure!(old == new, "Conflicting provider venue IDs");
    }
    Ok(())
}

fn find_venue(connection: &Connection, incoming: &VenuePreset) -> Result<Option<VenuePreset>> {
    if !incoming.id.is_empty()
        && let Some(found) = venue_by_id(connection, &incoming.id)?
    {
        compatible_identity(&found, incoming)?;
        return Ok(Some(found));
    }
    // Exported IDs are opaque: an earlier name-based key may have been
    // promoted to a provider ID and renamed. Preserve it on a fresh import.
    if let Some(provider_id) = &incoming.provider_venue_id {
        let raw: Option<String> = connection
            .query_row(
                "SELECT preset_json FROM venue_presets WHERE site_host=?1 AND provider_venue_id=?2",
                params![incoming.site_host, provider_id],
                |row| row.get(0),
            )
            .optional()?;
        if let Some(raw) = raw {
            let found = read_venue(&raw)?;
            compatible_identity(&found, incoming)?;
            return Ok(Some(found));
        }
    }
    let mut statement =
        connection.prepare("SELECT preset_json FROM venue_presets WHERE match_key=?1")?;
    let rows = statement.query_map([incoming.match_key()], |row| row.get::<_, String>(0))?;
    let mut candidates = Vec::new();
    for row in rows {
        let found = read_venue(&row?)?;
        if incoming.provider_venue_id.is_none() || found.provider_venue_id.is_none() {
            candidates.push(found);
        }
    }
    ensure!(
        candidates.len() <= 1,
        "Ambiguous venue name; supply the provider venue ID or local preset ID"
    );
    Ok(candidates.pop())
}

fn append_venue_sources(existing: &mut Vec<String>, incoming: Vec<String>) {
    let mut seen: HashSet<_> = existing
        .iter()
        .filter_map(|url| crate::event_identity::identity(url))
        .collect();
    for url in incoming {
        if let Some(identity) = crate::event_identity::identity(&url)
            && seen.insert(identity)
        {
            existing.push(url);
        }
    }
    // Bounded provenance is reference history, not a complete event catalog.
    if existing.len() > 100 {
        existing.drain(..existing.len() - 100);
    }
}

fn save_venue(connection: &Connection, incoming: VenuePreset) -> Result<VenuePreset> {
    let mut saved = if let Some(mut existing) = find_venue(connection, &incoming)? {
        // Metadata-only Discovery refreshes never erase or implicitly verify a layout.
        existing.name = incoming.name;
        existing.city = incoming.city;
        if incoming.provider_venue_id.is_some() {
            existing.provider_venue_id = incoming.provider_venue_id;
        }
        append_venue_sources(&mut existing.source_event_urls, incoming.source_event_urls);
        for layout in incoming.layouts {
            if let Some(index) = existing.layouts.iter().position(|old| old.id == layout.id) {
                existing.layouts[index] = layout;
            } else {
                existing.layouts.push(layout);
            }
        }
        existing
    } else {
        let count: i64 =
            connection.query_row("SELECT COUNT(*) FROM venue_presets", [], |row| row.get(0))?;
        ensure!(
            count < venues::MAX_VENUES as i64,
            "Venue preset database limit reached"
        );
        incoming
    };
    if saved.id.is_empty() {
        saved.id = saved.generated_id();
        ensure!(
            venue_by_id(connection, &saved.id)?.is_none(),
            "Venue reference collides with a renamed venue; supply the official venue ID"
        );
    }
    saved = saved.normalized()?;
    connection.execute(
        "INSERT INTO venue_presets(id, site_host, provider_venue_id, match_key, preset_json)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(id) DO UPDATE SET site_host=excluded.site_host,
         provider_venue_id=excluded.provider_venue_id, match_key=excluded.match_key,
         preset_json=excluded.preset_json",
        params![
            saved.id,
            saved.site_host,
            saved.provider_venue_id,
            saved.match_key(),
            serde_json::to_string(&saved)?
        ],
    )?;
    Ok(saved)
}

fn ensure_watch(transaction: &Transaction<'_>, watch: &Watch) -> Result<()> {
    let fingerprint = fingerprint(watch)?;
    let existing: Option<(String, String, bool)> = transaction
        .query_row(
            "SELECT fingerprint,state_json,enabled FROM watches WHERE id=?1",
            [&watch.id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    match existing {
        None => {
            transaction.execute(
                "INSERT INTO watches(id,fingerprint,enabled,state_json) VALUES (?1,?2,?3,?4)",
                params![
                    watch.id,
                    fingerprint,
                    watch.enabled,
                    serde_json::to_string(&InventoryState::default())?
                ],
            )?;
        }
        Some((previous, state_json, enabled))
            if previous != fingerprint || enabled != watch.enabled =>
        {
            cancel_pending(transaction, &watch.id)?;
            transaction.execute(
                "DELETE FROM watch_observations WHERE watch_id=?1",
                [&watch.id],
            )?;
            let old: InventoryState = serde_json::from_str(&state_json)?;
            let state = InventoryState {
                generation: old.generation,
                ..InventoryState::default()
            };
            transaction.execute(
                "UPDATE watches SET fingerprint=?2,enabled=?3,state_json=?4,
                     observed_availability='unobserved',evidence='none',detail='',
                     last_observed_at=NULL,last_valid_at=NULL WHERE id=?1",
                params![
                    watch.id,
                    fingerprint,
                    watch.enabled,
                    serde_json::to_string(&state)?
                ],
            )?;
        }
        _ => {}
    }
    if !watch.enabled {
        cancel_pending(transaction, &watch.id)?;
    }
    Ok(())
}

pub(crate) fn fingerprint(watch: &Watch) -> Result<String> {
    let mut value = serde_json::to_value(watch)?;
    if let Some(object) = value.as_object_mut() {
        object.remove("interval_secs");
        if watch.id.starts_with("auto-") {
            // Catalog titles/date labels can be corrected without changing inventory identity.
            // Preserve the baseline so notify_initial does not repeat on a metadata refresh.
            object.remove("name");
        }
    }
    Ok(format!("{:x}", Sha256::digest(serde_json::to_vec(&value)?)))
}

fn cancel_pending(transaction: &Transaction<'_>, watch_id: &str) -> Result<()> {
    transaction.execute(
        "UPDATE outbox SET state='cancelled' WHERE watch_id=?1 AND state='pending'",
        [watch_id],
    )?;
    Ok(())
}

fn cancel_obsolete_offers(
    transaction: &Transaction<'_>,
    watch_id: &str,
    state: &InventoryState,
) -> Result<()> {
    let rows: Vec<(i64, String)> = {
        let mut statement = transaction
            .prepare("SELECT id,payload_json FROM outbox WHERE watch_id=?1 AND state='pending'")?;
        statement
            .query_map([watch_id], |row| Ok((row.get(0)?, row.get(1)?)))?
            .collect::<rusqlite::Result<_>>()?
    };
    for (id, payload_json) in rows {
        let mut alert: Alert = serde_json::from_str(&payload_json)?;
        if alert.offers.is_empty() {
            continue;
        }
        let before = alert.offers.len();
        alert.offers.retain(|offer| {
            state
                .offers
                .get(&offer.key)
                .is_some_and(|known| known.qualifies == Some(true))
        });
        if alert.offers.is_empty() {
            transaction.execute("UPDATE outbox SET state='cancelled' WHERE id=?1", [id])?;
        } else if alert.offers.len() != before {
            transaction.execute(
                "UPDATE outbox SET payload_json=?2 WHERE id=?1",
                params![id, serde_json::to_string(&alert)?],
            )?;
        }
    }
    Ok(())
}

fn has_detail_filters(watch: &Watch) -> bool {
    watch.min_quantity.is_some() || watch.max_price_minor.is_some() || watch.currency.is_some()
}

fn matches_filters(watch: &Watch, offer: &Offer) -> Option<bool> {
    let mut unknown = false;
    if let Some(minimum) = watch.min_quantity {
        if matches!(
            offer.quantity_kind,
            QuantityKind::Listing | QuantityKind::Remaining
        ) {
            match offer.quantity {
                Some(quantity) if quantity < minimum => return Some(false),
                Some(_) => {}
                None => unknown = true,
            }
        } else {
            // A dropdown maximum or an unclassified number is not offered stock.
            unknown = true;
        }
    }
    if let Some(currency) = &watch.currency {
        match &offer.currency {
            Some(actual) if !actual.eq_ignore_ascii_case(currency) => return Some(false),
            Some(_) => {}
            None => unknown = true,
        }
    }
    if let Some(maximum) = watch.max_price_minor {
        if offer.price_scope == PriceScope::PerTicket {
            match offer.price_minor {
                Some(price) if price > maximum => return Some(false),
                Some(_) => {}
                None => unknown = true,
            }
        } else {
            unknown = true;
        }
    }
    if offer.quantity == Some(0) {
        return Some(false);
    }
    if unknown { None } else { Some(true) }
}

fn meaningful_change(previous: &Offer, current: &Offer) -> bool {
    fn changed<T: PartialEq>(left: &Option<T>, right: &Option<T>) -> bool {
        matches!((left, right), (Some(a), Some(b)) if a != b)
    }
    (previous.quantity_kind == current.quantity_kind
        && matches!(
            current.quantity_kind,
            QuantityKind::Listing | QuantityKind::Remaining
        )
        && changed(&previous.quantity, &current.quantity))
        || (previous.price_scope == current.price_scope
            && current.price_scope != PriceScope::Unknown
            && changed(&previous.price_minor, &current.price_minor))
        || changed(&previous.currency, &current.currency)
        || changed(&previous.ticket_type, &current.ticket_type)
        || changed(&previous.inventory, &current.inventory)
}

fn bounded(value: &str) -> String {
    value
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .chars()
        .take(512)
        .collect()
}

fn availability_name(value: Availability) -> &'static str {
    match value {
        Availability::Available => "available",
        Availability::Unavailable => "unavailable",
        Availability::Unknown => "unknown",
        Availability::Blocked => "blocked",
    }
}

fn evidence_name(value: Evidence) -> &'static str {
    match value {
        Evidence::Metadata => "metadata",
        Evidence::ExplicitStatus => "explicit_status",
        Evidence::Offers => "offers",
        Evidence::None => "none",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn watch() -> Watch {
        serde_json::from_value(json!({
            "id":"korn-pl", "name":"KoRn", "artist":"KoRn", "country":"PL",
            "city":"Krakow", "url":"https://www.ticketmaster.pl/event/korn-bilety/496543017",
            "enabled":true, "mode":"http", "interval_secs":60,
            "notify_initial":false, "notify_metadata":false, "notify_offer_changes":false,
            "min_quantity":null, "max_price_minor":null, "currency":null, "parser":{}
        }))
        .unwrap()
    }

    fn observation(at: i64, availability: Availability) -> Observation {
        Observation {
            ticket_categories: vec![],
            adapter: None,
            watch_id: "korn-pl".into(),
            observed_at: at,
            availability,
            evidence: Evidence::ExplicitStatus,
            complete: true,
            offers: vec![],
            detail: "synthetic fixture".into(),
        }
    }

    fn offer(key: &str) -> Offer {
        Offer {
            key: key.into(),
            section: Some(key.into()),
            row: None,
            seats: None,
            ticket_type: Some("Standard".into()),
            inventory: Some("primary".into()),
            price_text: Some("100.00 PLN".into()),
            price_minor: Some(10_000),
            currency: Some("PLN".into()),
            quantity: Some(2),
            quantity_kind: QuantityKind::Remaining,
            price_scope: PriceScope::PerTicket,
            ..Offer::default()
        }
    }

    fn offers(at: i64, values: Vec<Offer>, complete: bool) -> Observation {
        Observation {
            ticket_categories: vec![],
            adapter: None,
            evidence: Evidence::Offers,
            offers: values,
            complete,
            ..observation(at, Availability::Available)
        }
    }

    #[test]
    fn restart_preserves_dedup_and_pending_nonce() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("state.sqlite");
        let watch = watch();
        let mut store = Store::open(&path).unwrap();
        store
            .apply(&watch, &observation(1, Availability::Unavailable))
            .unwrap();
        assert_eq!(
            store
                .apply(&watch, &observation(2, Availability::Available))
                .unwrap(),
            1
        );
        let first = store.pending(2, 10).unwrap().remove(0);
        assert!(first.nonce.len() <= 25);
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(
            store
                .apply(&watch, &observation(3, Availability::Available))
                .unwrap(),
            0
        );
        let recovered = store.pending(3, 10).unwrap().remove(0);
        assert_eq!(first.nonce, recovered.nonce);
        store.delivered(recovered.id, "1234").unwrap();
        assert!(store.pending(3, 10).unwrap().is_empty());
    }

    #[test]
    fn unknown_and_blocked_do_not_manufacture_restock() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let watch = watch();
        assert_eq!(
            store
                .apply(&watch, &observation(1, Availability::Available))
                .unwrap(),
            0
        );
        store
            .apply(&watch, &Observation::unknown(&watch.id, 2, "timeout"))
            .unwrap();
        store
            .apply(&watch, &observation(3, Availability::Blocked))
            .unwrap();
        assert_eq!(
            store
                .apply(&watch, &observation(4, Availability::Available))
                .unwrap(),
            0
        );
        assert!(store.pending(4, 10).unwrap().is_empty());
        store
            .apply(&watch, &observation(5, Availability::Unavailable))
            .unwrap();
        assert_eq!(
            store
                .apply(&watch, &observation(6, Availability::Available))
                .unwrap(),
            1
        );
    }

    #[test]
    fn initial_notifications_are_explicitly_opt_in() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.notify_initial = true;
        assert_eq!(
            store
                .apply(&watch, &observation(1, Availability::Available))
                .unwrap(),
            1
        );
        assert_eq!(
            store.pending(1, 1).unwrap()[0].payload.kind,
            "initial_available"
        );
    }

    #[test]
    fn offers_change_while_event_remains_available_and_partial_snapshots_preserve_absent() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let watch = watch();
        store
            .apply(&watch, &offers(1, vec![offer("A")], true))
            .unwrap();
        assert_eq!(
            store
                .apply(&watch, &offers(2, vec![offer("B")], false))
                .unwrap(),
            1
        );
        assert_eq!(store.pending(2, 10).unwrap()[0].payload.offers[0].key, "B");
        assert_eq!(
            store
                .apply(&watch, &offers(3, vec![offer("A")], false))
                .unwrap(),
            0
        );
        assert_eq!(
            store
                .apply(&watch, &offers(4, vec![offer("A"), offer("B")], true))
                .unwrap(),
            0
        );
        assert_eq!(store.status().unwrap()[0]["known_offers"], 2);
    }

    #[test]
    fn complete_removal_cancels_pending_and_reappearance_is_a_new_occurrence() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.notify_initial = true;
        store
            .apply(&watch, &offers(1, vec![offer("A")], true))
            .unwrap();
        let old_nonce = store.pending(1, 1).unwrap()[0].nonce.clone();
        store.apply(&watch, &offers(2, vec![], true)).unwrap();
        assert!(store.pending(2, 10).unwrap().is_empty());
        store
            .apply(&watch, &offers(3, vec![offer("A")], true))
            .unwrap();
        assert_ne!(store.pending(3, 1).unwrap()[0].nonce, old_nonce);
    }

    #[test]
    fn purchase_limits_and_total_prices_cannot_satisfy_ticket_filters() {
        let mut watch = watch();
        watch.min_quantity = Some(2);
        watch.max_price_minor = Some(10_000);
        watch.currency = Some("PLN".into());
        let mut value = offer("A");
        assert_eq!(matches_filters(&watch, &value), Some(true));
        value.quantity_kind = QuantityKind::Selectable;
        assert_eq!(matches_filters(&watch, &value), None);
        value.quantity_kind = QuantityKind::Unknown;
        assert_eq!(matches_filters(&watch, &value), None);
        value.quantity_kind = QuantityKind::Listing;
        value.price_scope = PriceScope::Total;
        assert_eq!(matches_filters(&watch, &value), None);
        value.price_scope = PriceScope::Unknown;
        assert_eq!(matches_filters(&watch, &value), None);
        value.price_scope = PriceScope::PerTicket;
        assert_eq!(matches_filters(&watch, &value), Some(true));
    }

    #[test]
    fn changing_selectable_capacity_is_not_a_stock_change() {
        let mut prior = offer("A");
        prior.quantity_kind = QuantityKind::Selectable;
        let mut current = prior.clone();
        current.quantity = Some(8);
        assert!(!meaningful_change(&prior, &current));
        prior.quantity_kind = QuantityKind::Listing;
        current.quantity_kind = QuantityKind::Listing;
        assert!(meaningful_change(&prior, &current));
    }

    #[test]
    fn detail_filters_do_not_accept_unknown_values_or_metadata() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.min_quantity = Some(2);
        watch.max_price_minor = Some(10_000);
        watch.currency = Some("PLN".into());
        watch.notify_initial = true;
        store
            .apply(&watch, &observation(1, Availability::Available))
            .unwrap();
        assert!(store.pending(1, 10).unwrap().is_empty());
        let mut missing = offer("A");
        missing.quantity = None;
        store
            .apply(&watch, &offers(2, vec![missing], true))
            .unwrap();
        assert!(store.pending(2, 10).unwrap().is_empty());
        assert_eq!(
            store
                .apply(&watch, &offers(3, vec![offer("A")], true))
                .unwrap(),
            1
        );
    }

    #[test]
    fn unknown_quantity_recovery_does_not_generate_another_ping() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.min_quantity = Some(2);
        store
            .apply(&watch, &offers(1, vec![offer("A")], true))
            .unwrap();
        let mut missing = offer("A");
        missing.quantity = None;
        store
            .apply(&watch, &offers(2, vec![missing], false))
            .unwrap();
        assert_eq!(
            store
                .apply(&watch, &offers(3, vec![offer("A")], true))
                .unwrap(),
            0
        );
    }

    #[test]
    fn metadata_is_health_only_unless_explicitly_enabled() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.notify_initial = true;
        let mut value = observation(1, Availability::Available);
        value.evidence = Evidence::Metadata;
        assert_eq!(store.apply(&watch, &value).unwrap(), 0);
        assert_eq!(store.status().unwrap()[0]["health"], "metadata_only");
        watch.notify_metadata = true;
        store.reconcile(std::slice::from_ref(&watch)).unwrap();
        assert_eq!(store.apply(&watch, &value).unwrap(), 1);
    }

    #[test]
    fn edits_disable_remove_and_old_observations_are_safe() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.notify_initial = true;
        store
            .apply(&watch, &observation(10, Availability::Available))
            .unwrap();
        store
            .apply(&watch, &observation(9, Availability::Unavailable))
            .unwrap();
        assert_eq!(store.pending(10, 10).unwrap().len(), 1);
        watch.interval_secs = 120;
        store.reconcile(&[watch.clone()]).unwrap();
        assert_eq!(store.pending(10, 10).unwrap().len(), 1);
        watch.name = "Different name".into();
        store.reconcile(&[watch.clone()]).unwrap();
        assert!(store.pending(10, 10).unwrap().is_empty());
        store
            .apply(&watch, &observation(11, Availability::Available))
            .unwrap();
        store.reconcile(&[]).unwrap();
        assert!(store.pending(11, 10).unwrap().is_empty());
        assert_eq!(store.status().unwrap()[0]["enabled"], false);
        watch.enabled = false;
        assert_eq!(
            store
                .apply(&watch, &observation(12, Availability::Available))
                .unwrap(),
            0
        );
    }

    #[test]
    fn generated_watch_name_corrections_preserve_baseline_but_price_filters_reset_it() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.id = "auto-synthetic-stable-event".into();
        watch.notify_initial = true;
        watch.max_price_minor = Some(20_000);
        watch.currency = Some("PLN".into());
        let mut available = offers(1, vec![offer("A")], true);
        available.watch_id = watch.id.clone();
        assert_eq!(store.apply(&watch, &available).unwrap(), 1);
        let pending = store.pending(1, 10).unwrap().remove(0);
        store.delivered(pending.id, "synthetic-message").unwrap();

        watch.name = "Catalog corrected the event title and date".into();
        store.reconcile(std::slice::from_ref(&watch)).unwrap();
        assert_eq!(
            store.status().unwrap()[0]["last_valid_matching_availability"],
            true
        );
        available.observed_at = 2;
        assert_eq!(store.apply(&watch, &available).unwrap(), 0);
        assert!(store.pending(2, 10).unwrap().is_empty());

        watch.max_price_minor = Some(5_000);
        store.reconcile(std::slice::from_ref(&watch)).unwrap();
        assert!(store.status().unwrap()[0]["last_valid_matching_availability"].is_null());
        available.observed_at = 3;
        assert_eq!(store.apply(&watch, &available).unwrap(), 0);
        assert_eq!(
            store.status().unwrap()[0]["last_valid_matching_availability"],
            false
        );
    }

    #[test]
    fn retries_expiry_and_delivery_are_durable() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.notify_initial = true;
        store
            .apply(&watch, &observation(1, Availability::Available))
            .unwrap();
        let row = store.pending(1, 1).unwrap().remove(0);
        store.retry(row.id, 20, "temporary failure").unwrap();
        assert!(store.pending(19, 10).unwrap().is_empty());
        assert_eq!(store.pending(20, 1).unwrap()[0].attempts, 1);
        assert_eq!(store.expire_before(2).unwrap(), 1);
        assert!(store.pending(20, 10).unwrap().is_empty());
    }

    #[test]
    fn weaker_metadata_cannot_overwrite_explicit_inventory() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.notify_metadata = true;
        store
            .apply(&watch, &offers(1, vec![offer("A")], true))
            .unwrap();
        let mut stale = observation(2, Availability::Unavailable);
        stale.evidence = Evidence::Metadata;
        store.apply(&watch, &stale).unwrap();
        assert_eq!(
            store.status().unwrap()[0]["last_valid_matching_availability"],
            true
        );
        assert_eq!(
            store
                .apply(&watch, &offers(3, vec![offer("A")], true))
                .unwrap(),
            0
        );
    }

    #[test]
    fn cooldowns_survive_restart_and_only_extend() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("state.sqlite");
        let mut store = Store::open(&path).unwrap();
        store.set_cooldown("www.ticketmaster.pl", 100).unwrap();
        store.set_cooldown("www.ticketmaster.pl", 50).unwrap();
        assert_eq!(store.cooldowns().unwrap()["www.ticketmaster.pl"], 100);
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.cooldowns().unwrap()["www.ticketmaster.pl"], 100);
        store.set_cooldown("www.ticketmaster.pl", 200).unwrap();
        assert_eq!(store.cooldowns().unwrap()["www.ticketmaster.pl"], 200);
    }

    #[test]
    fn partial_offers_cannot_reset_coarse_positive_state() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let watch = watch();
        store
            .apply(&watch, &observation(1, Availability::Available))
            .unwrap();
        store.apply(&watch, &offers(2, vec![], false)).unwrap();
        let mut partial_negative = observation(3, Availability::Unavailable);
        partial_negative.evidence = Evidence::Offers;
        partial_negative.complete = false;
        store.apply(&watch, &partial_negative).unwrap();
        assert_eq!(
            store
                .apply(&watch, &observation(4, Availability::Available))
                .unwrap(),
            0
        );
        assert_eq!(
            store.status().unwrap()[0]["last_valid_matching_availability"],
            true
        );
    }

    #[test]
    fn complete_negative_offer_snapshot_cancels_coarse_alerts() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut watch = watch();
        watch.notify_initial = true;
        store
            .apply(&watch, &observation(1, Availability::Available))
            .unwrap();
        store.apply(&watch, &offers(2, vec![], true)).unwrap();
        assert!(store.pending(2, 10).unwrap().is_empty());
    }

    #[test]
    fn stale_inflight_configuration_cannot_undo_reconcile() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        let mut original = watch();
        original.notify_initial = true;
        store.reconcile(std::slice::from_ref(&original)).unwrap();
        let mut updated = original.clone();
        updated.min_quantity = Some(3);
        store.reconcile(std::slice::from_ref(&updated)).unwrap();
        assert!(
            store
                .apply(&original, &offers(1, vec![offer("A")], true))
                .is_err()
        );
        store.reconcile(&[]).unwrap();
        assert_eq!(
            store
                .apply(&updated, &offers(2, vec![offer("A")], true))
                .unwrap(),
            0
        );
        assert_eq!(store.status().unwrap()[0]["enabled"], false);
        assert!(store.pending(3, 10).unwrap().is_empty());
    }

    #[test]
    fn changed_delivery_config_cannot_retarget_pending_alerts() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("state.sqlite");
        let mut store = Store::open(&path).unwrap();
        let mut delivery = DiscordConfig {
            channel_id: Some("12345".into()),
            ..DiscordConfig::default()
        };
        store.reconcile_delivery(&delivery).unwrap();
        let mut watch = watch();
        watch.notify_initial = true;
        store
            .apply(&watch, &observation(1, Availability::Available))
            .unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        store.reconcile_delivery(&delivery).unwrap();
        assert_eq!(store.pending(1, 10).unwrap().len(), 1);
        delivery.mention_user_ids.push("98765".into());
        store.reconcile_delivery(&delivery).unwrap();
        assert!(store.pending(2, 10).unwrap().is_empty());
        store
            .apply(&watch, &observation(2, Availability::Unavailable))
            .unwrap();
        store
            .apply(&watch, &observation(3, Availability::Available))
            .unwrap();
        assert_eq!(store.pending(3, 10).unwrap().len(), 1);
        delivery.channel_id = Some("54321".into());
        store.reconcile_delivery(&delivery).unwrap();
        assert!(store.pending(3, 10).unwrap().is_empty());
    }

    #[test]
    fn single_scope_cooldown_is_optional_and_persistent() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        assert_eq!(store.cooldown("discord:delivery").unwrap(), None);
        store.set_cooldown("discord:delivery", 100).unwrap();
        assert_eq!(store.cooldown("discord:delivery").unwrap(), Some(100));
    }

    #[test]
    fn catalog_cache_survives_restart_and_rejects_oversize_without_replacement() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("state.sqlite");
        let mut store = Store::open(&path).unwrap();
        assert!(store.catalog_load().unwrap().is_none());
        let snapshot = json!({"version":1, "subscriptions":{"korn-pl":{"fingerprint":"selected", "events":[]}}});
        store.catalog_save(&snapshot).unwrap();
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert_eq!(store.catalog_load().unwrap(), Some(snapshot.clone()));
        let oversize = Value::String("x".repeat(MAX_CATALOG_BYTES));
        assert!(store.catalog_save(&oversize).is_err());
        assert_eq!(store.catalog_load().unwrap(), Some(snapshot));
        store
            .catalog_save(&json!({"version":1, "subscriptions":{}}))
            .unwrap();
        assert_eq!(
            store.catalog_load().unwrap().unwrap()["subscriptions"],
            json!({})
        );
    }

    #[test]
    fn corrupt_or_oversized_stored_catalog_is_never_returned() {
        let mut store = Store::open(Path::new(":memory:")).unwrap();
        store.catalog_save(&json!({"version":1})).unwrap();
        store
            .connection
            .execute(
                "UPDATE catalog_snapshot SET snapshot_json='not JSON' WHERE id=1",
                [],
            )
            .unwrap();
        assert!(store.catalog_load().is_err());
        store
            .connection
            .execute(
                "UPDATE catalog_snapshot SET snapshot_json=?1 WHERE id=1",
                ["x".repeat(MAX_CATALOG_BYTES + 1)],
            )
            .unwrap();
        assert!(store.catalog_load().is_err());
    }

    #[test]
    fn rolling_catalog_budget_survives_restart_and_only_releases_expired_reservations() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("state.sqlite");
        let mut store = Store::open(&path).unwrap();
        assert!(store.reserve_catalog_request(1_000, 2).unwrap());
        assert!(store.reserve_catalog_request(1_001, 2).unwrap());
        assert!(!store.reserve_catalog_request(1_002, 2).unwrap());
        drop(store);
        let mut store = Store::open(&path).unwrap();
        assert!(!store.reserve_catalog_request(1_003, 2).unwrap());
        assert!(!store.reserve_catalog_request(1_000 + 86_399, 2).unwrap());
        assert!(store.reserve_catalog_request(1_000 + 86_400, 2).unwrap());
        assert!(!store.reserve_catalog_request(1_000 + 86_400, 2).unwrap());
        assert!(store.reserve_catalog_request(1_001 + 86_400, 2).unwrap());
        assert_eq!(
            store
                .connection
                .query_row("SELECT count(*) FROM catalog_requests", [], |row| row
                    .get::<_, i64>(0))
                .unwrap(),
            2
        );
    }

    #[test]
    fn catalog_budget_is_shared_across_connections_and_does_not_reset_on_clock_rollback() {
        let directory = TempDir::new().unwrap();
        let path = directory.path().join("state.sqlite");
        let mut first = Store::open(&path).unwrap();
        let mut second = Store::open(&path).unwrap();
        assert!(first.reserve_catalog_request(2_000, 1).unwrap());
        assert!(!second.reserve_catalog_request(2_001, 1).unwrap());
        assert!(!second.reserve_catalog_request(10, 1).unwrap());
        assert!(second.reserve_catalog_request(2_000 + 86_400, 1).unwrap());
        assert!(!first.reserve_catalog_request(2_000 + 86_400, 1).unwrap());
        assert!(first.reserve_catalog_request(100, 0).is_err());
        assert!(first.reserve_catalog_request(100, 5_001).is_err());
        assert!(first.reserve_catalog_request(-1, 1).is_err());
    }
}
