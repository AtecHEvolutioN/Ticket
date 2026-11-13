# Priority checks and measured freshness

The ordinary `run` monitor prioritizes due website checks using published sale
times and observations. Discovery is still metadata; a sale starting does not
prove available inventory or eligibility for a presale.

## Selection policy

Checks become eligible through the existing configured watch interval, idle
worker state, global concurrency limit and shared host pacing/cooldown gates.
Priority does not shorten any of those limits or retry blocked pages sooner.
Among eligible watches, the order is:

1. Published public-sale or presale start from 15 minutes before through 30
   minutes after its timestamp. Missing, TBD or invalid public-sale times do not
   establish this priority.
2. A changed explicit observation within the last 10 minutes, including displayed
   offers/categories. The first observation is a baseline, and changed diagnostic
   text, offer ordering or a failed check does not count as a ticket-price change.
3. Enabled manual watches in the configuration.
4. Other discovered watches.

Within a priority class the oldest due watch wins, with a stable watch-ID tie
break. Every fourth dispatch selects the oldest eligible watch regardless of
priority. A corresponding per-host counter preserves that host's fairness slot
when requests for other hosts interleave. Only actual dispatches advance these
counters. Priority state and counters are process-local; persisted observations
seed baselines/freshness after restart, and existing provider cooldowns remain
durable. A restart does not invent a recent price change.
Restart seeding reads only active watch IDs and retains compact timestamp,
availability and signature fields; it does not retain historical raw observations
in the scheduler. An observation older than the saved timestamp cannot reset
health or manufacture a recent change, including after clock skew.
Baseline identity matches durable watch storage: changing an interval or a
generated event title preserves it; changing inventory filters resets it.

This improves which events get the available request slots. It does not increase
the provider request allowance. At a 60-second host interval, even one urgent
watch cannot be promised second-level checks; more urgent watches share the same
capacity. A 1,000-event single-host sweep has at least 59,940 seconds between its
first and last starts, before response times, other priority work and backoff.

## Runtime reports

Every 60 seconds, the worker writes a `scheduler_status` JSON line to stderr
(the service journal). It reports:

- Total watches, in-flight checks, due backlog and how many due checks are gated
  by host pacing/backoff.
- Oldest due wait, never-observed count, latest blocked/unknown counts and
  p50/p95/p99/max observation ages. Future or negative saved timestamps count as
  `clock_invalid` and are excluded from age quantiles, not treated as age zero.
- Per-host watch counts, configured spacing and a single-sweep spacing lower
  bound. This bound is not a forecast or a fresh-inventory guarantee.
- Bounded rolling timing distributions from the last 1,000 accepted completed
  observations in this process. Zero samples produce null quantiles.

Observation age includes unsuccessful and blocked attempts. Read the unknown and
blocked counts alongside freshness; a recent failed read does not prove fresh
stock. Due backlog excludes already running work; `in_flight` reports that work
separately. Retired or edited configurations discard obsolete in-flight inventory
while retaining any provider backoff, as before.

Each accepted observation also includes monotonic elapsed measurements in its
stdout JSON:

| Field | Measured interval |
| --- | --- |
| `fetch_to_observation_ms` | From Fetcher invocation through response classification/parsing; includes browser contention. |
| `transport_reported_ms` | Transport's own elapsed value, when returned. It is not interchangeable with the complete fetch interval. |
| `parse_ms` | Response classification/parsing; absent on transport failure. |
| `observation_to_recorded_ms` | From parsed observation to committed storage, including main-loop/DB waiting. |
| `durable_apply_ms` | Awaited storage operation, including database queue and commit. |
| `fetch_to_recorded_ms` | Fetch invocation through committed recording. |
| `due_lateness_ms` | How long the watch was due at dispatch selection. |

These timings do not know when Ticketmaster changed inventory. They must not be
presented as stock-change-to-alert latency. Discord dispatch/acknowledgement
timings are measured separately by the delivery worker. Long-running source
access and actual delivery still require a measured target-server run.

## Verification

`tests/watch_scheduler.rs` uses deterministic timestamps to cover sale windows,
fairness, manual/changed priority, restarts, edits, clock skew and bounded timing
quantiles. `tests/watch_scheduler_runtime.rs` exercises the real monitor with a
local browser-protocol stub: high concurrency still respects one host's interval,
and a provider block stops further same-host requests and survives restart. These
tests make no Ticketmaster or Discord requests.
