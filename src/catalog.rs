//! Discovery schedules work; only website observations enter the inventory state machine.
use crate::{
    config::{Config, Subscription, Watch},
    database::Db,
    discovery::Discovery,
    hybrid,
};
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};

const REFRESH_SCOPE: &str = "catalog:refresh";
const API_SCOPE: &str = "discovery:api";
fn now() -> i64 {
    chrono::Utc::now().timestamp()
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Selection {
    pub id: String,
    pub watches: Vec<Watch>,
    pub metadata: Vec<Value>,
    pub complete: bool,
    pub updated_at: Option<i64>,
    pub notes: Vec<String>,
    pub skipped: Vec<Value>,
    /// Resume a bounded multi-page scan across refreshes without starting at page zero.
    #[serde(default)]
    pub next_page: u32,
    /// Accepted watch IDs from every successfully read page of the current scan.
    #[serde(default)]
    pub scan_watch_ids: Vec<String>,
    /// A resumed scan cannot claim complete coverage after a prior page lost
    /// selected events to the configured storage/monitoring capacity.
    #[serde(default)]
    pub scan_capacity_limited: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize, Default)]
pub struct Snapshot {
    pub fingerprint: String,
    pub refreshed_at: Option<i64>,
    pub next_refresh_at: Option<i64>,
    pub next_subscription: usize,
    pub selections: Vec<Selection>,
    pub requests_attempted: usize,
    pub notes: Vec<String>,
}

pub fn fingerprint(config: &Config) -> Result<String> {
    Ok(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            &config.subscriptions,
            &config.profiles,
            config.catalog.max_events,
        ))?)
    ))
}

pub fn decode(config: &Config, value: Option<Value>) -> Result<Snapshot> {
    let expected = fingerprint(config)?;
    if let Some(value) = value {
        let snapshot: Snapshot = serde_json::from_value(value).context("Read catalog snapshot")?;
        if snapshot.fingerprint == expected {
            return Ok(snapshot);
        }
    }
    Ok(Snapshot {
        fingerprint: expected,
        ..Snapshot::default()
    })
}

pub async fn load(config: &Config, db: &Db) -> Result<Snapshot> {
    decode(config, db.call(|s| s.catalog_load()).await?)
}

/// Manual watches take precedence, then subscriptions in configuration order.
/// Including disabled manual entries lets an explicit pause override generated work.
pub fn effective_watches(config: &Config, snapshot: &Snapshot) -> Result<Vec<Watch>> {
    let mut watches = Vec::new();
    let mut seen = HashSet::new();
    for watch in &config.watches {
        if seen.insert(hybrid::canonical_event_url(&watch.url)?) {
            watches.push(watch.clone());
        }
    }
    for subscription in config.subscriptions.iter().filter(|s| s.enabled) {
        if let Some(selection) = snapshot.selections.iter().find(|s| s.id == subscription.id) {
            for watch in &selection.watches {
                if seen.insert(hybrid::canonical_event_url(&watch.url)?) {
                    watches.push(watch.clone());
                }
            }
        }
    }
    Ok(watches)
}

pub trait Source: Send {
    fn page(
        &mut self,
        subscription: &Subscription,
        page: u32,
    ) -> impl std::future::Future<Output = Result<Value>> + Send;
    fn cooldown_until(&self) -> Option<i64> {
        None
    }
    fn is_paused(&self) -> bool {
        false
    }
}
impl Source for Discovery {
    async fn page(&mut self, s: &Subscription, page: u32) -> Result<Value> {
        if s.all_artists {
            return self
                .events_country_page(
                    s.country
                        .as_deref()
                        .context("All-artist discovery requires a country")?,
                    s.city.as_deref(),
                    page,
                )
                .await;
        }
        self.events_page(
            &s.attraction_id,
            s.country.as_deref(),
            s.city.as_deref(),
            page,
        )
        .await
    }
    fn cooldown_until(&self) -> Option<i64> {
        Discovery::cooldown_until(self)
    }
    fn is_paused(&self) -> bool {
        Discovery::is_paused(self)
    }
}

fn metadata(event: &Value) -> Value {
    json!({"id":event["id"], "url":event["url"], "name":event["name"],
        "event_status":event["event_status"], "dates":event["dates"],
        "venue":event["venues"].as_array().and_then(|v| v.first()),
        "venues":event["venues"],
        "public_sale":event.pointer("/sales/public"),
        "presales":event.pointer("/sales/presales").and_then(Value::as_array).map(|sales|sales.iter().take(100).collect::<Vec<_>>()),
        "attractions":event["attractions"],
        "price_ranges":event["price_ranges"].as_array().map(|v|v.iter().take(10).collect::<Vec<_>>()),
        "source":"ticketmaster_discovery", "inventory_verified":false})
}

async fn save_backoff<S: Source>(source: &S, db: &Db) -> Result<()> {
    // Unknown upstream reset requires a manual correction/restart plus a conservative persisted pause.
    let until = match (source.cooldown_until(), source.is_paused()) {
        (Some(provider), true) => Some(provider.max(now() + 86_400)),
        (None, true) => Some(now() + 86_400),
        (provider, false) => provider,
    };
    if let Some(until) = until {
        db.call(move |s| s.set_cooldown(API_SCOPE, until)).await?;
    }
    Ok(())
}

/// A bounded refresh. Partial/error responses preserve previously selected events;
/// a complete search can retire watches, but never creates an unavailable observation.
pub async fn refresh<S: Source>(config: &Config, db: &Db, source: &mut S) -> Result<Snapshot> {
    let mut snapshot = load(config, db).await?;
    snapshot.notes.clear();
    snapshot.requests_attempted = 0;
    let at = now();
    let gate = db
        .call(|s| {
            Ok(s.cooldown(REFRESH_SCOPE)?
                .unwrap_or(0)
                .max(s.cooldown(API_SCOPE)?.unwrap_or(0)))
        })
        .await?;
    if gate > at {
        snapshot.next_refresh_at = Some(gate);
        snapshot
            .notes
            .push("Saved Discovery cadence/backoff is active; using cached events".into());
        return Ok(snapshot);
    }
    let subscriptions: Vec<_> = config.subscriptions.iter().filter(|s| s.enabled).collect();
    if subscriptions.is_empty() {
        bail!("No enabled artist/location or all-artist country subscriptions");
    }
    let next = at + config.catalog.refresh_interval_secs as i64;
    db.call(move |s| s.set_cooldown(REFRESH_SCOPE, next))
        .await?;
    snapshot.next_refresh_at = Some(next);
    let cached_watch_ids: HashSet<_> = snapshot
        .selections
        .iter()
        .flat_map(|selection| selection.watches.iter().map(|watch| watch.id.clone()))
        .collect();
    let mut selections: HashMap<_, _> = snapshot
        .selections
        .drain(..)
        .map(|s| (s.id.clone(), s))
        .collect();
    let start = snapshot.next_subscription % subscriptions.len();
    let mut global_stop = false;
    for offset in 0..subscriptions.len() {
        let index = (start + offset) % subscriptions.len();
        let sub = subscriptions[index];
        if global_stop || snapshot.requests_attempted >= config.catalog.max_requests_per_refresh {
            break;
        }
        snapshot.next_subscription = (index + 1) % subscriptions.len();
        let profile = config
            .profiles
            .iter()
            .find(|p| p.id == sub.profile)
            .context("Missing website profile")?;
        let mut selection = selections.remove(&sub.id).unwrap_or_else(|| Selection {
            id: sub.id.clone(),
            ..Selection::default()
        });
        if selection.next_page >= 10 {
            selection.next_page = 0;
            selection.scan_watch_ids.clear();
        }
        if selection.next_page == 0 {
            selection.scan_watch_ids.clear();
            selection.scan_capacity_limited = false;
        }
        let mut page = selection.next_page;
        let mut complete = false;
        let mut notes = Vec::new();
        if sub.all_artists {
            notes.push(format!(
                "Country scope {} has no artist filter. Coverage refers to returned Discovery metadata, not every website listing or verified ticket inventory.",
                sub.country.as_deref().unwrap_or("unknown")
            ));
        }
        let mut skipped = Vec::new();
        let mut observed = false;
        loop {
            if snapshot.requests_attempted >= config.catalog.max_requests_per_refresh {
                notes.push(
                    "Refresh request budget reached; retained cached matches and pagination cursor"
                        .into(),
                );
                break;
            }
            if source.is_paused() || source.cooldown_until().is_some_and(|until| until > now()) {
                save_backoff(source, db).await?;
                notes.push("Discovery provider backoff is active; retained cached matches".into());
                global_stop = true;
                break;
            }
            let quota = config.catalog.max_requests_per_day;
            let reserved = db
                .call(move |s| s.reserve_catalog_request(now(), quota))
                .await?;
            if !reserved {
                notes.push(
                    "Rolling 24-hour Discovery budget exhausted; retained cached matches".into(),
                );
                global_stop = true;
                break;
            }
            snapshot.requests_attempted += 1;
            let result = source.page(sub, page).await;
            save_backoff(source, db).await?;
            let response = match result {
                Ok(value) => value,
                Err(error) => {
                    notes.push(format!("Discovery refresh failed: {error}"));
                    break;
                }
            };
            let Some(items) = response["events"].as_array() else {
                notes.push("Discovery response has no event array; retained cached matches".into());
                break;
            };
            observed = true;
            let events: Vec<_> = items.iter().take(100).cloned().collect();
            let plan = hybrid::plan(sub, profile, &events);
            retire_explicit_events(&mut selection, &events, &plan.skipped);
            let selected: HashSet<_> = plan
                .watches
                .iter()
                .filter_map(|w| hybrid::canonical_event_url(&w.url).ok())
                .collect();
            for watch in plan.watches {
                if !selection.scan_watch_ids.contains(&watch.id) {
                    selection.scan_watch_ids.push(watch.id.clone());
                }
                selection.watches.retain(|old| old.id != watch.id);
                selection.watches.push(watch);
            }
            for event in events.iter().filter(|event| {
                event["url"]
                    .as_str()
                    .and_then(|url| hybrid::canonical_event_url(url).ok())
                    .is_some_and(|url| selected.contains(&url))
            }) {
                let mut item = metadata(event);
                let venue_event = event.clone();
                match db.call(move |s| s.venue_register_event(&venue_event)).await {
                    Ok(presets) => {
                        item["venue_preset_ids"] = json!(presets.iter().map(|p| &p.id).collect::<Vec<_>>());
                    }
                    Err(_) => notes.push(
                        "Venue reference metadata could not be safely matched; event monitoring is unchanged".into(),
                    ),
                }
                let identity = item["url"]
                    .as_str()
                    .and_then(|url| hybrid::canonical_event_url(url).ok());
                selection.metadata.retain(|old| {
                    old["id"] != item["id"]
                        && old["url"]
                            .as_str()
                            .and_then(|url| hybrid::canonical_event_url(url).ok())
                            != identity
                });
                selection.metadata.push(item);
            }
            skipped.extend(plan.skipped);
            if response["has_more"].as_bool() == Some(false) && items.len() <= 100 {
                // Absence only retires a watch after every page in this scan has succeeded,
                // including earlier pages persisted by preceding refreshes.
                let accepted: HashSet<_> = selection.scan_watch_ids.iter().collect();
                selection
                    .watches
                    .retain(|watch| accepted.contains(&watch.id));
                selection.next_page = 0;
                selection.scan_watch_ids.clear();
                complete = !selection.scan_capacity_limited;
                if selection.scan_capacity_limited {
                    notes.push("This scan previously reached the configured event limit; coverage remains incomplete even though its final API page succeeded. A new scan is required to recover omitted events.".into());
                }
                break;
            }
            if response["next_page"].as_u64() != Some(page as u64 + 1)
                || page == 9
                || items.len() > 100
            {
                notes.push(if sub.all_artists {
                    "Country discovery coverage is incomplete: the 1,000-result paging ceiling or invalid pagination was reached. This is not every event; date/location partitioning is required for exhaustive coverage. Cached matches were retained.".into()
                } else {
                    "Search pagination is incomplete; narrow artist/location filters".into()
                });
                selection.next_page = 0;
                selection.scan_watch_ids.clear();
                break;
            }
            page += 1;
            selection.next_page = page;
        }
        selection.complete = complete;
        selection.notes = notes;
        selection.skipped = skipped.into_iter().take(1000).collect();
        if observed {
            selection.updated_at = Some(at);
        }
        selections.insert(sub.id.clone(), selection);
    }
    let mut retained_urls = HashSet::new();
    // A capped partial refresh must not evict an existing watch merely because
    // an updated copy moved later in a page or another subscription found new
    // events first. Reserve capacity for surviving cached identities first.
    for sub in &subscriptions {
        if let Some(selection) = selections.get(&sub.id) {
            for watch in selection
                .watches
                .iter()
                .filter(|watch| cached_watch_ids.contains(&watch.id))
            {
                if retained_urls.len() < config.catalog.max_events
                    && let Ok(url) = hybrid::canonical_event_url(&watch.url)
                {
                    retained_urls.insert(url);
                }
            }
        }
    }
    for sub in subscriptions {
        let mut selection = selections.remove(&sub.id).unwrap_or_else(|| Selection {
            id: sub.id.clone(),
            ..Selection::default()
        });
        let before = selection.watches.len();
        selection.watches.retain(|watch| {
            let Ok(url) = hybrid::canonical_event_url(&watch.url) else {
                return false;
            };
            if retained_urls.contains(&url) {
                return true;
            }
            if retained_urls.len() >= config.catalog.max_events {
                return false;
            }
            retained_urls.insert(url);
            true
        });
        if selection.watches.len() < before {
            selection.complete = false;
            selection.scan_capacity_limited = true;
            selection
                .notes
                .push("Configured unique event limit reached; coverage is incomplete".into());
        }
        let urls: HashSet<_> = selection
            .watches
            .iter()
            .filter_map(|w| hybrid::canonical_event_url(&w.url).ok())
            .collect();
        selection.metadata.retain(|v| {
            v["url"]
                .as_str()
                .and_then(|u| hybrid::canonical_event_url(u).ok())
                .is_some_and(|u| urls.contains(&u))
        });
        snapshot.selections.push(selection);
    }
    let api_until = db.call(|s| s.cooldown(API_SCOPE)).await?;
    if let Some(until) = api_until {
        snapshot.next_refresh_at = Some(snapshot.next_refresh_at.unwrap_or(0).max(until));
    }
    snapshot.refreshed_at = Some(at);
    bound_snapshot(&mut snapshot)?;
    let saved = serde_json::to_value(&snapshot)?;
    db.call(move |s| s.catalog_save(&saved)).await?;
    Ok(snapshot)
}

/// Keep a margin below the store's hard 16 MiB limit. Prefer selected watches over
/// descriptive metadata, and preserve enough space to report every selection's gap.
fn bound_snapshot(snapshot: &mut Snapshot) -> Result<()> {
    const TARGET_BYTES: usize = 12 * 1024 * 1024;
    const ENVELOPE_RESERVE: usize = 16 * 1024;
    const EMPTY_SELECTION_RESERVE: usize = 1024;
    let count = snapshot.selections.len();
    let mut used = ENVELOPE_RESERVE;
    for (index, selection) in snapshot.selections.iter_mut().enumerate() {
        let reserved = (count - index - 1) * EMPTY_SELECTION_RESERVE;
        let budget = TARGET_BYTES.saturating_sub(used + reserved);
        let mut size = serde_json::to_vec(selection)?.len();
        if size > budget {
            selection.metadata.clear();
            selection
                .notes
                .push("Catalog storage limit: descriptive metadata omitted".into());
            size = serde_json::to_vec(selection)?.len();
        }
        if size > budget {
            selection.skipped.clear();
            selection
                .notes
                .push("Catalog storage limit: detailed skip diagnostics omitted".into());
            size = serde_json::to_vec(selection)?.len();
        }
        if size > budget {
            selection.complete = false;
            selection.next_page = 0;
            selection.scan_watch_ids.clear();
            selection.notes = vec![
                "Catalog storage limit reached; watch coverage is incomplete and scan will restart"
                    .into(),
            ];
            size = serde_json::to_vec(selection)?.len();
            while size > budget {
                let Some(watch) = selection.watches.pop() else {
                    break;
                };
                size = size.saturating_sub(serde_json::to_vec(&watch)?.len());
            }
            size = serde_json::to_vec(selection)?.len();
            anyhow::ensure!(
                size <= budget,
                "Catalog selection cannot fit bounded snapshot"
            );
        }
        used += size + 1;
    }
    Ok(())
}

fn retire_explicit_events(selection: &mut Selection, events: &[Value], skipped: &[Value]) {
    let retired_ids: HashSet<_> = skipped
        .iter()
        .filter(|entry| {
            matches!(
                entry["reason"].as_str(),
                Some("catalog marks event cancelled" | "catalog event date has passed")
            )
        })
        .filter_map(|entry| entry["event_id"].as_str())
        .collect();
    if retired_ids.is_empty() {
        return;
    }
    let retired_urls: HashSet<_> = events
        .iter()
        .chain(&selection.metadata)
        .filter(|event| {
            event["id"]
                .as_str()
                .is_some_and(|id| retired_ids.contains(id))
        })
        .filter_map(|event| {
            event["url"]
                .as_str()
                .and_then(|url| hybrid::canonical_event_url(url).ok())
        })
        .collect();
    let retired_watch_ids: HashSet<_> = selection
        .watches
        .iter()
        .filter(|watch| {
            hybrid::canonical_event_url(&watch.url)
                .ok()
                .is_some_and(|url| retired_urls.contains(&url))
        })
        .map(|watch| watch.id.clone())
        .collect();
    selection
        .watches
        .retain(|watch| !retired_watch_ids.contains(&watch.id));
    selection
        .scan_watch_ids
        .retain(|id| !retired_watch_ids.contains(id));
    selection.metadata.retain(|event| {
        !event["id"]
            .as_str()
            .is_some_and(|id| retired_ids.contains(id))
            && !event["url"]
                .as_str()
                .and_then(|url| hybrid::canonical_event_url(url).ok())
                .is_some_and(|url| retired_urls.contains(&url))
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapshot_limit_preserves_watch_scope_before_large_metadata() {
        let large_metadata = json!({"synthetic_details": "x".repeat(7 * 1024 * 1024)});
        let mut snapshot = Snapshot {
            selections: (0..2)
                .map(|index| Selection {
                    id: format!("selection-{index}"),
                    watches: vec![Watch {
                        id: format!("watch-{index}"),
                        ..Watch::default()
                    }],
                    metadata: vec![large_metadata.clone()],
                    complete: true,
                    ..Selection::default()
                })
                .collect(),
            ..Snapshot::default()
        };
        bound_snapshot(&mut snapshot).unwrap();
        assert!(serde_json::to_vec(&snapshot).unwrap().len() < 12 * 1024 * 1024);
        assert!(
            snapshot
                .selections
                .iter()
                .all(|selection| selection.watches.len() == 1 && selection.complete)
        );
        assert!(snapshot.selections[1].metadata.is_empty());
        assert!(
            snapshot.selections[1]
                .notes
                .iter()
                .any(|note| note.contains("metadata omitted"))
        );
    }
}
