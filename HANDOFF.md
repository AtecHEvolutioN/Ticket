# Ticket Watch handoff — 10 November 2026

## Current goal and stopping point

Build a fast Ticketmaster monitor with Discord availability/asking-price alerts,
history charts and eventually verified carting. The latest scope is **all artists
and returned event types on Ticketmaster.nl**, across Dutch cities/venues. The
user chose an **always-on server**, then asked to finish, commit to GitHub and
write this handoff. **No paid server has been created or purchased.**

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

The viewer/assets and health endpoint were verified HTTP200. Health reports
`configured` with `watcher_status: unverified`. The viewer reads JSON files
locally or embedded standalone HTML; it has **no live database connection**.
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

**Scale limits:** the initial query covers at most ten 100-event pages and the
example retains at most 1,000 events. This is not exhaustive NL coverage. The
current conservative 60-second host interval could take over 16 hours for a
1,000-event sweep. A one-second country-wide detection claim is not justified.
Date/location partitioning, priority scheduling and measured capacity remain work.

**Netherlands limits:** tested support includes metadata, displayed category
prices, some explicit resale listings and overview maps. Dutch section/seat
hovering and carting remain unvalidated. The new per-seat history uses the
validated Czech extraction path; it does not make Dutch seat extraction work.
See [the NL audit](docs/netherlands-mapping.md).

## Always-on server preparation

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

- Rust: **546 tests passed**; formatting, Clippy `--all-targets -D warnings` and
  optimized release build passed.
- Discord/scripts/viewer: **65 tests passed**.
- New real-Chromium regression passed: SVG visibility, candles, mobile layout,
  safe imports and empty-file reset, with no external requests.
- Offline parser → SQLite → JSON/HTML → Chromium rehearsal passed with 361
  synthetic samples, separate primary/resale/unknown series and 15 gap markers.
  Standalone export, final responsive resize and invalid-file rejection passed.
- Feature CI [42857781689](https://github.com/AtecHEvolutioN/Ticket/actions/runs/42857781689)
  passed both jobs, including the full existing browser suite.
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
3. Partition all-artist discovery beyond the deep-page ceiling; add inventory
   priorities for imminent onsales/recent changes and report freshness/coverage.
4. Run a measured preview/soak, verify delivery to the intended routes, then enable
   startup at boot. Check restart/stop behavior, disk growth, consistent SQLite
   backup/restore and off-host backup. No scheduled backups are configured yet.
5. Add an authenticated read-only history interface for live charts. Keep private
   watchlists/session data out of public Vercel storage and URLs.
6. Validate Dutch seat extraction separately before displaying per-seat stock or
   attempting carts. Actual transactions require separate verified receipts and
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
