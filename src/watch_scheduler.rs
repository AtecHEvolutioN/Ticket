//! Bounded priority scheduling. Selection never changes request eligibility,
//! configured intervals, host pacing, provider cooldowns or concurrency limits.
use crate::{
    catalog::Snapshot,
    config::{Config, Watch},
    hybrid,
    model::{Availability, Evidence, Observation},
};
use anyhow::Result;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    time::Instant,
};

pub const SALE_BEFORE_SECS: i64 = 15 * 60;
pub const SALE_AFTER_SECS: i64 = 30 * 60;
pub const RECENT_CHANGE_SECS: i64 = 10 * 60;
pub const FAIR_EVERY: u8 = 4;
const TIMING_CAPACITY: usize = 1000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Priority {
    SaleWindow,
    RecentlyChanged,
    Manual,
    Normal,
}

struct TrackedWatch {
    configuration: Vec<u8>,
    host: String,
    manual: bool,
    sales: Vec<i64>,
    last_observed_at: Option<i64>,
    last_availability: Option<Availability>,
    last_signature: Option<String>,
    changed_at: Option<i64>,
}

#[derive(Debug, Clone)]
pub struct DueWatch {
    pub index: usize,
    pub watch_id: String,
    pub host: String,
    pub due: Instant,
}

#[derive(Debug, Clone, Serialize)]
pub struct Choice {
    pub index: usize,
    pub priority: Priority,
    /// True when an oldest-due slot overrides priority, globally or for this host.
    pub fairness_slot: bool,
}

#[derive(Default)]
pub struct Scheduler {
    watches: HashMap<String, TrackedWatch>,
    global_dispatches: u8,
    host_dispatches: HashMap<String, u8>,
    timings: VecDeque<TimingSample>,
}

impl Scheduler {
    pub fn new(
        config: &Config,
        snapshot: &Snapshot,
        watches: &[Watch],
        stored_status: &[Value],
    ) -> Result<Self> {
        let mut scheduler = Self::default();
        scheduler.reconcile(config, snapshot, watches)?;
        for status in stored_status {
            let Some(id) = status["watch_id"].as_str() else {
                continue;
            };
            if let Some(watch) = scheduler.watches.get_mut(id) {
                watch.last_observed_at = status["last_observed_at"].as_i64();
                watch.last_availability =
                    serde_json::from_value(status["last_availability"].clone()).ok();
                watch.last_signature = status["last_signature"]
                    .as_str()
                    .filter(|value| {
                        value.len() == 64 && value.bytes().all(|byte| byte.is_ascii_hexdigit())
                    })
                    .map(str::to_owned);
            }
        }
        Ok(scheduler)
    }

    pub fn reconcile(
        &mut self,
        config: &Config,
        snapshot: &Snapshot,
        watches: &[Watch],
    ) -> Result<()> {
        let manual: HashSet<_> = config
            .watches
            .iter()
            .filter(|watch| watch.enabled)
            .map(|watch| watch.id.as_str())
            .collect();
        let mut sales: HashMap<String, Vec<i64>> = HashMap::new();
        for event in snapshot.selections.iter().flat_map(|s| &s.metadata) {
            let Some(url) = event["url"].as_str() else {
                continue;
            };
            if let Ok(url) = hybrid::canonical_event_url(url) {
                let starts = sales.entry(url).or_default();
                // Cached metadata uses public_sale/presales. The sales fallback
                // supports older normalized Discovery snapshots.
                let public = event
                    .get("public_sale")
                    .filter(|v| v.is_object())
                    .or_else(|| event.pointer("/sales/public"));
                if let Some(public) = public.filter(|v| v["startTBD"] != true)
                    && let Some(start) = timestamp(&public["startDateTime"])
                {
                    starts.push(start);
                }
                if let Some(presales) = event
                    .get("presales")
                    .filter(|v| v.is_array())
                    .or_else(|| event.pointer("/sales/presales"))
                    .and_then(Value::as_array)
                {
                    starts.extend(
                        presales
                            .iter()
                            .take(100)
                            .filter_map(|sale| timestamp(&sale["startDateTime"])),
                    );
                }
                starts.sort_unstable();
                starts.dedup();
                starts.truncate(101);
            }
        }
        let active: HashSet<_> = watches.iter().map(|watch| watch.id.as_str()).collect();
        self.watches.retain(|id, _| active.contains(id.as_str()));
        for watch in watches {
            // Keep the same baseline identity as durable storage: intervals and
            // generated titles can change without invalidating an observation.
            let configuration = crate::store::fingerprint(watch)?.into_bytes();
            let host = crate::config::validate_public_url(&watch.url)?
                .host_str()
                .expect("validated public host")
                .to_owned();
            if self
                .watches
                .get(&watch.id)
                .is_none_or(|old| old.configuration != configuration)
            {
                self.watches.insert(
                    watch.id.clone(),
                    TrackedWatch {
                        configuration,
                        host,
                        manual: false,
                        sales: vec![],
                        last_observed_at: None,
                        last_availability: None,
                        last_signature: None,
                        changed_at: None,
                    },
                );
            }
            let tracked = self.watches.get_mut(&watch.id).expect("inserted watch");
            tracked.manual = manual.contains(watch.id.as_str());
            // Standalone scheduler callers may supply non-catalog paths, which do
            // not have a catalog event identity or an associated sale schedule.
            // The public host/path validation above remains mandatory.
            tracked.sales = hybrid::canonical_event_url(&watch.url)
                .ok()
                .and_then(|url| sales.get(&url))
                .cloned()
                .unwrap_or_default();
        }
        let hosts: HashSet<_> = self
            .watches
            .values()
            .map(|watch| watch.host.as_str())
            .collect();
        self.host_dispatches
            .retain(|host, _| hosts.contains(host.as_str()));
        Ok(())
    }

    pub fn host(&self, watch_id: &str) -> Option<&str> {
        self.watches.get(watch_id).map(|watch| watch.host.as_str())
    }

    pub fn priority(&self, watch_id: &str, now: i64) -> Priority {
        let Some(watch) = self.watches.get(watch_id) else {
            return Priority::Normal;
        };
        if watch.sales.iter().any(|start| {
            now >= start.saturating_sub(SALE_BEFORE_SECS)
                && now <= start.saturating_add(SALE_AFTER_SECS)
        }) {
            Priority::SaleWindow
        } else if watch
            .changed_at
            .is_some_and(|at| now >= at && now.saturating_sub(at) <= RECENT_CHANGE_SECS)
        {
            Priority::RecentlyChanged
        } else if watch.manual {
            Priority::Manual
        } else {
            Priority::Normal
        }
    }

    /// Candidates must already satisfy all watch/host cooldowns and be idle.
    /// Every fourth dispatch takes the oldest eligible watch, regardless of
    /// priority. Per-host slots prevent interleaved other hosts from consuming
    /// all of a particular host's fairness allocation.
    pub fn choose(&self, candidates: &[DueWatch], now: i64) -> Option<Choice> {
        let oldest = |watch: &DueWatch| (watch.due, watch.watch_id.clone());
        if self.global_dispatches == FAIR_EVERY - 1 {
            let selected = candidates.iter().min_by_key(|watch| oldest(watch))?;
            return Some(Choice {
                index: selected.index,
                priority: self.priority(&selected.watch_id, now),
                fairness_slot: true,
            });
        }
        let mut host_choices: BTreeMap<&str, &DueWatch> = BTreeMap::new();
        for candidate in candidates {
            let fairness = self
                .host_dispatches
                .get(&candidate.host)
                .copied()
                .unwrap_or(0)
                == FAIR_EVERY - 1;
            let candidate_key = (
                if fairness {
                    Priority::Normal
                } else {
                    self.priority(&candidate.watch_id, now)
                },
                oldest(candidate),
            );
            host_choices
                .entry(&candidate.host)
                .and_modify(|old| {
                    let old_key = (
                        if fairness {
                            Priority::Normal
                        } else {
                            self.priority(&old.watch_id, now)
                        },
                        oldest(old),
                    );
                    if candidate_key < old_key {
                        *old = candidate;
                    }
                })
                .or_insert(candidate);
        }
        let selected = host_choices
            .values()
            .min_by_key(|watch| (self.priority(&watch.watch_id, now), oldest(watch)))?;
        Some(Choice {
            index: selected.index,
            priority: self.priority(&selected.watch_id, now),
            fairness_slot: self
                .host_dispatches
                .get(&selected.host)
                .copied()
                .unwrap_or(0)
                == FAIR_EVERY - 1,
        })
    }

    /// Called only after the caller has durably reserved the host pacing slot.
    pub fn dispatched(&mut self, host: &str) {
        self.global_dispatches = (self.global_dispatches + 1) % FAIR_EVERY;
        let count = self.host_dispatches.entry(host.to_owned()).or_default();
        *count = (*count + 1) % FAIR_EVERY;
    }

    pub fn observed(&mut self, observation: &Observation) -> Result<()> {
        // Storage ignores observations older than its current timestamp. Keep
        // scheduling/freshness aligned, including a saved future timestamp after
        // clock skew, instead of fabricating a fresh observation or price change.
        if self
            .watches
            .get(&observation.watch_id)
            .is_some_and(|watch| {
                watch
                    .last_observed_at
                    .is_some_and(|at| observation.observed_at < at)
            })
        {
            return Ok(());
        }
        let signature = observation_signature(observation)?;
        if let Some(watch) = self.watches.get_mut(&observation.watch_id) {
            watch.last_observed_at = Some(observation.observed_at);
            watch.last_availability = Some(observation.availability);
            if let Some(signature) = signature {
                if watch
                    .last_signature
                    .as_ref()
                    .is_some_and(|previous| *previous != signature)
                {
                    watch.changed_at = Some(observation.observed_at);
                }
                watch.last_signature = Some(signature);
            }
        }
        Ok(())
    }

    pub fn record_timing(&mut self, sample: TimingSample) {
        if self.timings.len() == TIMING_CAPACITY {
            self.timings.pop_front();
        }
        self.timings.push_back(sample);
    }

    pub fn report(
        &self,
        now: i64,
        instant: Instant,
        due: &[DueWatch],
        host_due: &HashMap<String, Instant>,
        in_flight: usize,
        per_host_interval_secs: u64,
    ) -> Value {
        let mut by_host: BTreeMap<&str, usize> = BTreeMap::new();
        for watch in self.watches.values() {
            *by_host.entry(&watch.host).or_default() += 1;
        }
        let observation_ages: Vec<_> = self
            .watches
            .values()
            .filter_map(|watch| watch.last_observed_at)
            .filter(|at| *at >= 0 && *at <= now)
            .map(|at| now.saturating_sub(at) as u64)
            .collect();
        let gates = due
            .iter()
            .filter(|watch| {
                host_due
                    .get(&watch.host)
                    .is_some_and(|until| *until > instant)
            })
            .count();
        json!({
            "kind":"scheduler_status", "at":now,
            "watches":self.watches.len(), "in_flight":in_flight,
            "due_waiting":due.len(), "due_host_gated":gates,
            "oldest_due_wait_ms":due.iter().map(|watch| millis(instant.saturating_duration_since(watch.due))).max(),
            "never_observed":self.watches.values().filter(|watch|watch.last_observed_at.is_none()).count(),
            "clock_invalid":self.watches.values().filter(|watch|watch.last_observed_at.is_some_and(|at|at < 0 || at > now)).count(),
            "latest_blocked":self.watches.values().filter(|watch|watch.last_availability == Some(Availability::Blocked)).count(),
            "latest_unknown":self.watches.values().filter(|watch|watch.last_availability == Some(Availability::Unknown)).count(),
            "observation_age_secs":distribution(observation_ages),
            "freshness_notice":"Observation age includes blocked/unknown reads; it does not prove fresh inventory.",
            "per_host":by_host.iter().map(|(host, count)|json!({"host":host,"watches":count,
                "configured_spacing_secs":per_host_interval_secs,
                "single_sweep_spacing_floor_secs":(*count as u64).saturating_sub(1).saturating_mul(per_host_interval_secs)})).collect::<Vec<_>>(),
            "priority_policy":{"sale_before_secs":SALE_BEFORE_SECS,"sale_after_secs":SALE_AFTER_SECS,
                "recent_change_secs":RECENT_CHANGE_SECS,"oldest_due_every_dispatches":FAIR_EVERY},
            "timing_sample_limit":TIMING_CAPACITY,
            "timing_notice":"Rolling process-local timings include blocked/failed attempts. Fetch begins at Fetcher invocation (including browser contention). These are not stock-change-to-alert or Discord acknowledgement measurements.",
            "fetch_to_observation_ms":distribution(self.timings.iter().map(|s|s.fetch_to_observation_ms).collect()),
            "parse_ms":distribution(self.timings.iter().filter_map(|s|s.parse_ms).collect()),
            "durable_apply_ms":distribution(self.timings.iter().map(|s|s.durable_apply_ms).collect()),
            "observation_to_recorded_ms":distribution(self.timings.iter().map(|s|s.observation_to_recorded_ms).collect()),
            "fetch_to_recorded_ms":distribution(self.timings.iter().map(|s|s.fetch_to_recorded_ms).collect()),
            "due_lateness_ms":distribution(self.timings.iter().map(|s|s.due_lateness_ms).collect()),
        })
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct TimingSample {
    pub fetch_to_observation_ms: u64,
    pub transport_reported_ms: Option<u64>,
    pub parse_ms: Option<u64>,
    pub observation_to_recorded_ms: u64,
    pub durable_apply_ms: u64,
    pub fetch_to_recorded_ms: u64,
    pub due_lateness_ms: u64,
}

pub fn millis(duration: std::time::Duration) -> u64 {
    duration.as_millis().min(u64::MAX as u128) as u64
}

fn distribution(mut values: Vec<u64>) -> Value {
    values.sort_unstable();
    let quantile = |percent: usize| {
        values
            .get((values.len() * percent).div_ceil(100).saturating_sub(1))
            .copied()
    };
    json!({"samples":values.len(), "p50":quantile(50), "p95":quantile(95), "p99":quantile(99), "max":values.last()})
}

fn timestamp(value: &Value) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value.as_str()?)
        .ok()
        .map(|time| time.timestamp())
}

pub(crate) fn observation_signature(observation: &Observation) -> Result<Option<String>> {
    if observation.availability == Availability::Blocked
        || observation.evidence == Evidence::Metadata
        || (observation.availability == Availability::Unknown
            && observation.offers.is_empty()
            && observation.ticket_categories.is_empty())
    {
        return Ok(None);
    }
    let mut offers = observation
        .offers
        .iter()
        .map(serde_json::to_vec)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    let mut categories = observation
        .ticket_categories
        .iter()
        .map(serde_json::to_vec)
        .collect::<std::result::Result<Vec<_>, _>>()?;
    offers.sort_unstable();
    categories.sort_unstable();
    Ok(Some(format!(
        "{:x}",
        Sha256::digest(serde_json::to_vec(&(
            observation.availability,
            observation.evidence,
            observation.complete,
            offers,
            categories,
        ))?)
    )))
}
