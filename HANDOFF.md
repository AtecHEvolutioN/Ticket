# Ticket Watch handoff — 13 November 2026

## Current goal and stopping point

Build a fast Ticketmaster monitor with Discord availability/asking-price alerts,
history charts and eventually verified carting. The latest scope is **all artists
and returned event types on Ticketmaster.nl**, across Dutch cities/venues. The
user chose an **always-on server**, then requested broader/faster discovery,
authenticated live charts and Dutch seat/cart validation. **No paid server has
been created or purchased.** Latest changes implement date partitions, priority
scheduling, read-only chart connections and limited Dutch seat extraction.
Two headed Dutch seat checks validated explicit primary/resale quotes; three
headless reads returned HTTP403. No Dutch reservation has been attempted.

Do not infer an actual sale price from an asking price, a disappearing listing,
or a seat becoming unobservable. Track those as observations. Confirmed sales
need transaction evidence. Reserved-seat history is useful where identities and
quotes are validated; map colors alone do not prove inventory or sales.

## Repository and deployments

- Repository: <https://github.com/AtecHEvolutioN/Ticket>, branch `main`.
- `45fec0fe31f8974a5b51f5685c917b1c530f785a`: safe NL Discord layout provisioning.
- `e228079895746cda48ad352b66346a54a37a72a9`: event/seat asking-price history,
  chart viewer and bounded all-artist country discovery. This is the last
  feature deployment verified before this handoff/deployment-preparation commit.
- Vercel project root: `apps/discord`.
- Viewer: <https://ticket-discord.vercel.app/market.html>.
- Interactions: <https://ticket-discord.vercel.app/api/interactions>.
- Health: <https://ticket-discord.vercel.app/api/health>.
- Dashboard: <https://vercel.com/andrejs-projects-e8a3d8d0/ticket-discord>.
- Vercel Git deployments work. The Vercel connector previously could not see
  this project despite team authorization; do not repeat that authorization
  blindly. Four non-secret Discord settings are in `apps/discord/vercel.json`.

The prior viewer/assets and health endpoint were verified HTTP200. Health reports
`configured` with `watcher_status: unverified`. The viewer reads JSON files
locally or embedded standalone HTML, and now supports an operator-configured
authenticated live endpoint. **No production worker/API endpoint is connected.**
Its built-in demo is explicitly synthetic. `/ticket ping`, `/ticket help` and
`/ticket chart` are registered; command success is not watcher readiness.

## Discord server: actually created

- Application: `1569720679757127740`.
- Guild: `1569717598709350491`.
- Allowed operator: `133350475020894208`.
- Existing general channel: `1569717599724634306`.
- New category **TICKETMASTER NL**: `1569753960397541380`.

| Channel | ID |
| --- | --- |
| nl-upcoming-sales | 1569753963245338686 |
| nl-presales | 1569753968161198181 |
| nl-on-sale | 1569753969780072580 |
| nl-primary | 1569753970929434736 |
| nl-resale | 1569753971738804334 |
| nl-price-changes | 1569753972741509140 |
| nl-event-updates | 1569753973525839892 |
| nl-charts | 1569753974444269738 |
| bot-status | 1569753975182332027 |

[Layout evidence](docs/evidence/discord-nl-layout-2026-11-10.json) and
[channel purposes](docs/discord-netherlands.md) are committed. The provisioning
run [42857120073](https://github.com/AtecHEvolutioN/Ticket/actions/runs/42857120073)
confirmed all ten creations (category plus nine channels). No messages/pings,
permission overwrites, renames or deletions were performed. Existing channels
were preserved. Do not rerun creation just to check status; `--inspect` is read-only.

Updated command registration succeeded in
[42858004317](https://github.com/AtecHEvolutioN/Ticket/actions/runs/42858004317).
Both setup workflows consume the existing `DISCORD_SETUP_BOT_TOKEN` GitHub
Actions secret only in their final step. Its value has not been retrieved.

**Feeds are not yet connected.** The Rust sender currently has one configured
destination. Creating nine channels does not implement multi-channel routing,
catalog announcements, presale timers or live chart publishing.

On November 13 the user requested a desktop demonstration. **One preview message
was delivered** to `bot-status`, confirmed by the Discord response and successful
[workflow 43458151314](https://github.com/AtecHEvolutioN/Ticket/actions/runs/43458151314).
[Open the message](https://discord.com/channels/1569717598709350491/1569753975182332027/1570830552636858460).
It contains clearly dated recorded primary/resale quotes, chart links and the
inactive watcher/carting status, with mentions suppressed. See
[delivery evidence](docs/evidence/discord-preview-2026-11-13.json).
Do not rerun this workflow just to inspect the message.

## Implemented history and discovery

- `src/market_history.rs`: additive SQLite history tables, compact samples,
  atomic recording with normal/map state, independent of alert delivery. Exact
  replays deduplicate; different samples in the same second remain ordered.
- Grouping preserves sale channel, ticket type, currency, price basis, fee scope,
  quantity semantics and source configuration. Total prices need a known
  **listing bundle** size; remaining-stock quantity cannot establish that size.
- Primary/resale/unknown classification requires explicit evidence. Standard,
  VIP and Official Platinum do not establish the sale channel.
- Supported map quotes retain an aggregate series and separate section/row/seat
  histories with scoped hashed identities. Missing/blocked quotes leave gaps;
  known displayed prices can remain continuous while stock is unknown.
- `src/market_chart.rs` and `history-export`: read-only JSON/standalone HTML
  exports; no provider request, database migration or delivery. Existing output
  filenames are never overwritten. Bounds: 16 MiB, 10,000 samples, 2,000 events,
  5,000 sources, 200,000 series points. Narrow date/event selection for more data.
- Viewer: event/source/series selectors, asking-price lines/OHLC candles,
  last-observation count bars, responsive mobile SVG, local import and inspection.
  Counts are never summed across repeated polls, sources or aggregate/seat views.
- History begins when this version records data; it is not reconstructed before
  then. Data is retained after a watch is removed. Automatic end-of-event
  retirement/retention and large-history rollups are not implemented.
- `all_artists = true` with a required `country` permits country discovery and
  forbids an `attraction_id` in the same subscription. Existing artist selection
  remains strict and old false/default serialization preserves fingerprints.
- `examples/netherlands-all.example.toml` retains actual artists and published
  public-sale/presale metadata. A capped/resumed/failed scan remains explicitly
  incomplete; it must not silently evict unseen cached matches.

**Broader discovery:** [dated partitions](docs/discovery-partitions.md) split
saturated country searches, persist window/page cursors and preserve cached
matches during partial scans. NL examples cover a 730-day dated horizon and
retain up to 10,000 events. Each query still stops at ten 100-event pages.
One-second wire overlap protects date boundaries. TBA/TBD/undated events,
out-of-horizon dates, saturated one-second windows and budget/storage limits
remain explicit gaps. Completion refers only to the configured dated scope.

**Priority and timing:** [scheduling](docs/scheduling.md) favors sale windows,
recent changes and manual watches, with every-fourth-dispatch oldest-due fairness.
Host pacing, cooldowns and concurrency limits are unchanged. Runtime reports
show backlog, blocked/unknown reads, clock-invalid timestamps and rolling
fetch/parse/storage timing percentiles. Delivery logs separately measure dispatch
to response and second-resolution observation-age bounds. None measures an
unknown upstream stock-change time. A 60-second host interval still means over
16 hours for a 1,000-event sweep; one-second country-wide detection is not proven.

**Live chart connection:** [the history API](docs/live-history-api.md) uses fixed
existing database paths, a separate bearer token, exact CORS origins, loopback
binding and the read-only Rust exporter. Exports are bounded, serialized and
cancelled on timeout/disconnect. The viewer keeps the token only in memory and
preserves selections while refreshing. Errors retain the last good snapshot
with stale/error status. Production still needs the host, private token, TLS
reverse proxy and actual-host read-only WAL validation.

**Netherlands limits:** tested support includes metadata, displayed category
prices, some explicit resale listings and overview maps. Guarded read-only
section/seat hovering now has **limited live validation**: Monster Jam at GelreDome
exposed primary section101/row11/seat213 at EUR89.95; New Wave at Ziggo Dome exposed
resale section209/row8/seat292 at EUR73.90. These were ordinary headed Chromium
checks followed by Rust validation. Three headless reads returned HTTP403;
unattended access still needs a host test. Fees, stock, adjacency and presale
eligibility remain unknown. No seats were selected or reserved. Dutch automatic
section scans and carting remain disabled. The cart request diagnostic receives the user's actual
adjacency preference from the durable dispatch binding; it does not manufacture
proof of adjacent seats or a successful hold.
See [the NL audit](docs/netherlands-mapping.md) and
[live seat evidence](docs/netherlands-seat-validation.md).

## Always-on server preparation

The user now has an existing Proxmox Debian 13 VM and confirmed guest-agent
command execution from its node Shell. Available resources were checked; host
addresses and application inventory remain in chat. The isolated
[Proxmox installer](docs/proxmox-deployment.md) is prepared for this path.
It installs the v0.8.0-server-test package, limits the worker to one concurrent
check/one CPU/1536 MiB and leaves the service stopped. No target-host installation
or startup has yet been confirmed. Obtain the installation log before claiming
deployment. Credentials and source-access validation are the next steps.

Desktop setup: [Linux launcher](docs/linux-desktop.md) installs/reuses Discord
and opens the existing server and chart viewer. The operator-triggered
`discord-preview.yml` workflow verifies application/channel identity, then posts
one clearly labelled recorded-evidence preview in `bot-status`. It uses the
existing setup secret only in its final step. This does not start the watcher.
The official Discord launcher was installed under
`/workspace/.ticket-desktop/ticket-watch/discord/Discord/discord` in this cloud
environment. Its native updater rejected the environment's HTTPS inspection
certificate (`UnknownIssuer`), so the full desktop app was not bootstrapped here.
TLS verification was preserved. There is no active DISPLAY/Wayland socket or
desktop-control tool in this session; this does not install or open an app on the
user's physical PC. The Linux launcher is intended to run in their desktop terminal.

See [the deployment guide](docs/netherlands-server.md). Prepared files:

- `deploy/prepare-nl-server.sh`: default `--plan`; `--check` offline prerequisites;
  explicit local-root `--apply` creates account/private paths/new files and reloads
  systemd. No package installation, secrets loading, overwrite, service startup,
  provider request or Discord delivery.
- `deploy/nl-service.example.toml`: NL scope, initially disabled, destination unset.
- `deploy/ticket-watch-nl.service`: unprivileged preview process, persistent state
  and cache, automatic failure restart, graceful SIGINT stop.
- `deploy/ticket-watch-nl.send-discord.conf.example`: inactive explicit delivery
  override. It does not supply multi-channel routing.
- `deploy/test-nl-deployment.py`: offline preparation checks. Full systemd/runtime
  validation must run on the actual target; this sandbox cannot initialize the
  systemd manager used by `systemd-analyze`.
- `deploy/package-bot.sh`: includes chart assets needed by Rust `include_str!`,
  CSP config for the browser test, build metadata and per-file/archive checksums.
  Packaged public files/directories have service-readable permissions. No local
  configuration, secret files, raw captures, databases or node_modules are bundled.

Suggested starting size: Debian13 x86_64, 2 vCPU, 4 GB RAM, persistent SSD.
DigitalOcean Amsterdam Basic Regular was checked at **USD24/month base** for
4 GiB/2 vCPU/80 GiB, before tax/backups/extra usage. This is a proposal, not an
approved purchase or a performance guarantee. The preparation is provider-neutral.
No cloud account, IP address, SSH access or privately supplied runtime secrets
are connected yet. Do not provision paid resources based solely on this handoff.

Runtime secrets required: `TICKETMASTER_API_KEY` and `DISCORD_BOT_TOKEN`, privately
installed on the host. Neither is set in this execution environment. Do not
paste keys into chat or copy credentials into Git/public logs.

## Verification completed

- Rust: **584 tests passed**; formatting, Clippy `--all-targets -D warnings` and
  optimized release build passed.
- Discord/scripts/viewer/history API: **89 tests passed**, including four preview
  delivery guards. The latest desktop change reran all 76 Discord/script/viewer
  checks; the 13 unchanged API checks passed in the preceding feature validation.
  [CI for the desktop code](https://github.com/AtecHEvolutioN/Ticket/actions/runs/43458074680)
  passed both jobs.
- Full local Chromium suite: **169 tests passed**, including authenticated live
  chart transport, mobile layout, native-cart diagnostics and offline parsing.
- Real SQLite WAL writer → release exporter → authenticated API → Chromium
  refresh passed with synthetic data. A committed new price appeared while the
  selected series remained selected. Invalid auth was rejected; no token was
  written into URL/storage/cookies. Standalone embedded HTML retained its
  no-network policy (served on loopback because managed Chrome blocks file URLs).
- Offline parser → SQLite → JSON/HTML → Chromium rehearsal passed with 361
  synthetic samples, separate primary/resale/unknown series and 15 gap markers.
  Standalone export, final responsive resize and invalid-file rejection passed.
- Previous feature CI [42857781689](https://github.com/AtecHEvolutioN/Ticket/actions/runs/42857781689)
  passed both jobs. Check the newest commit's GitHub run for this revision's result.
- NL preparation: six checks passed, one explicit systemd-runtime check skipped
  because of the sandbox; no real host install/start was tested.
- Linux bundle: archive and per-file checksums, service-readable permissions and
  packaged offline replay passed. The extracted source passed offline locked
  `cargo check --all-targets`, including the embedded chart assets.

Use targeted checks for new changes; do not make provider calls just to rerun
offline tests. Previously published Android artifacts remain unchanged. A newly
built local worker bundle is not automatically a published GitHub Release.

## Next work, in order

1. Confirm provider/budget, provision the approved VM, establish private access,
   install the reviewed checkout/bundle and private environment. Validate ordinary
   headless website access before enabling a long-running watcher.
2. Implement durable channel routing/publishers for catalog schedules, sale-open
   status, original/resale observations, price changes and event updates. Keep
   unknown provenance out of primary/resale-only channels; do not duplicate nine
   independent scrapers. Preserve outbox identity, cooldowns and deduplication.
3. Validate the partitioned catalog and priority scheduler on the actual account
   and host. Measure freshness/latency under real pacing and coverage gaps. Expand
   undated discovery separately if required; do not imply exhaustive coverage.
4. Run a measured preview/soak, verify delivery to the intended routes, then enable
   startup at boot. Check restart/stop behavior, disk growth, consistent SQLite
   backup/restore and off-host backup. No scheduled backups are configured yet.
5. Install the read-only history service, private token and HTTPS reverse proxy.
   Verify the real watcher update reaches a chart and that no credentials appear
   in URLs/logs. Keep SQLite and private state off public Vercel storage.
6. Broaden Dutch seat validation before claiming general coverage or per-seat
   purchasable stock. A Dutch live cart test still needs an event URL and maximum total
   EUR including fees (default two adjacent tickets); user clarification is pending.
   Actual transactions require separate verified receipts and
   accounting; listing churn is not a completed-sale ledger.

## Preserve these existing boundaries

- Czech attempt `korn-cz-native-20261110-01` remains unresolved in
  `data/cart-attempts.sqlite`. Preserve its event lock and evidence. Do not retry,
  unlock it, replace its journal or infer release from elapsed time.
- No CAPTCHA solver, queue/protection bypass, automatic buying, resale publishing
  or repricing was added. Manual challenge/session handoff remains the boundary.
- Default cart preference is two adjacent cheapest eligible tickets, editable by
  the user. Unknown fees/quantity/adjacency cannot become verified eligibility.
- Private captures, live databases and session material stay ignored/unpublished.
- The user has repeatedly authorized GitHub commits/pushes. Finish concrete
  authorized work without repeatedly asking permission; paid provisioning still
  needs a specific approved resource/cost and actual account access.

Local developer toolchain if continuing in this workspace:

```sh
cd /workspace/ticket
export RUSTUP_HOME=/workspace/.ticket-rustup
export CARGO_HOME=/workspace/.ticket-cargo
/workspace/.ticket-cargo/bin/cargo test --locked
node --test --test-isolation=none apps/discord/test/*.test.js apps/discord/scripts/*.test.mjs
TICKET_BROWSER_TESTS=1 CHROMIUM_PATH=/usr/bin/chromium \
  node --test --test-isolation=none browser/market-chart.test.cjs
python3 deploy/test-nl-deployment.py
```

Local loopback/browser tests may need the execution sandbox's normal approval.
Do not run `prepare-nl-server.sh --apply` in the shared development workspace.
