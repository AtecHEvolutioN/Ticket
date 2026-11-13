# Bounded country discovery

All-artist subscriptions can search more than the first 1,000 events by enabling
dated partitions. The Netherlands examples enable this configuration:

```toml
[catalog]
refresh_interval_secs = 900
max_requests_per_refresh = 40
max_requests_per_day = 4000
max_events = 10000
country_partitioning = true
country_horizon_days = 730
country_window_days = 30
```

Only `all_artists = true` subscriptions use these partitions. Artist searches
keep their existing paging behavior. The default is disabled for compatibility
with existing configurations; a disabled partitioner leaves old catalog
fingerprints unchanged. The prepared service subscription remains disabled until
an operator configures and activates it.

Each scan fixes a logical UTC horizon starting at the current UTC day and lasting
the configured number of days. It initially divides that horizon into windows.
A response reporting more than 1,000 events, more than ten pages, or a saturated
last page causes the current window to split into smaller date ranges. Ordinary
pages within each window remain bounded to page 0–9 and 100 events per page.

The [Discovery API documentation](https://developer.ticketmaster.com/products-and-docs/apis/discovery-api/v2/)
describes `startDateTime` and `endDateTime` as “after” and “before” without a clear
inclusivity guarantee. Each wire query therefore extends one second before and
after its logical window. Exact day/split boundaries remain inside strict
filters; canonical event identities deduplicate overlapping results. Dated
queries explicitly use `includeTBA=no` and `includeTBD=no`. The API response
records both logical and wire date bounds.

`catalog` reports the persisted `country_scan` horizon, pending window/page
cursors, completed windows and incomplete windows. Refresh and rolling daily
request budgets still apply, including failed requests. Restart or a failed page
resumes the saved cursor. A bounded scan never retires an unseen cached event
merely because it did not appear in the horizon: its date could have moved or
become unknown. Explicit cancellation/past-date evidence can still retire a watch.

`complete: true` means the configured **dated scope** was exhausted within the
retention limits. It does not mean every Ticketmaster event was found. Gaps remain
explicit for:

- Undated/TBA/TBD events and dates outside the horizon.
- More than 1,000 results in a one-second window, including conservative overlap.
- The 8,192 created-window limit, request budgets, provider failures or invalid
  pagination.
- The configurable retained-event limit (up to 10,000) and the catalog's bounded
  serialized storage. Losing retained capacity keeps the scan incomplete even
  when its final API page succeeds.

After a completed scan (including one with explicit gaps), the next permitted
refresh starts a new horizon. Pending scans resume first. To recover persistent
capacity gaps, narrow the country/city scope or raise the configured retention
ceiling within the supported bounds and measure the resulting storage use.
Changed scope/cap/window settings invalidate the selection fingerprint, while
saved provider cooldowns and rolling request usage remain in the database.
Do not delete state to retry a quota-limited scan.

The country query does not establish available tickets. Website observations
remain separate, with their own pacing, access, freshness and inventory evidence.
Retaining 10,000 events is a storage ceiling, not a promise to check all of them
quickly. Increasing catalog capacity does not increase permitted request rates.

Offline tests cover a 1,500-event split, duplicate/boundary identities, restart and
failure cursors, rolling budgets, same-time saturation, capacity loss and legacy
fingerprints. The request tests verify the encoded date filters against a local
HTTP fixture. These tests do not certify current account quotas or exhaustive
live country coverage.
