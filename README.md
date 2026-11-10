# Ticket Watch

Rust monitoring that combines **Ticketmaster Discovery API searches** with **website availability checks**. Choose artists and locations; Discovery finds matching events, and the website adapter checks those events for observable ticket availability before sending Discord alerts.

**Current status:** selected events use reusable Ticketmaster layout adapters, with no artist names, event IDs, or generated CSS classes in their extraction rules. They read displayed categories/ranges and explicit resale listing quantities from recognized layouts. Browser mode opens known best-available tabs automatically. Unknown layouts and blocked access remain explicit; this is not verified support for every Ticketmaster market. The latest [92-event, 14-country coverage report](docs/coverage-2026-11-10.md) records 242 priced category rows across 75 events. See [event adapters](docs/event-adapters.md) for inventory limits. The continuous watcher and live Discord delivery have not been deployed; the optional command endpoint is described in the [Discord setup guide](docs/discord-setup.md).

## Downloads

For the current deployment state and next steps, read [HANDOFF.md](HANDOFF.md).
The [always-on Netherlands server guide](docs/netherlands-server.md) prepares a
dedicated worker; no paid server or live feed has been activated yet.

The [0.7.0-test release](https://github.com/AtecHEvolutioN/Ticket/releases/tag/v0.7.0-test)
contains the tested app/bot builds and SHA-256 checksums. The venue catalog is
maintained separately on `main`:

- [Android APK](https://github.com/AtecHEvolutioN/Ticket/raw/refs/tags/v0.7.0-test/downloads/ticket-watch-android.apk) — Android 8.0 or newer; editable ticket preferences, manual page checks and venue presets.
- [Linux x86_64 bot](https://github.com/AtecHEvolutioN/Ticket/raw/refs/tags/v0.7.0-test/downloads/ticket-watch-bot-linux-x86_64.tar.gz) — continuous monitoring after configuration, offline preference import and cart planning; glibc 2.39 or newer.
- [29-venue starter database](https://github.com/AtecHEvolutioN/Ticket/raw/refs/heads/main/downloads/venue-presets.sqlite) — venue metadata and six draft Dutch static layouts, with no saved watches. To add venues to an existing monitor, import [the JSON seed](examples/venue-presets.json) using [the venue commands](docs/venue-presets.md).

Release verification: 360 Rust tests, Clippy, and 364 offline Android assertions
passed, including actual Android export → Rust planner checks. Android verification
uses an API 28 emulator; see
[test details](android/TESTING.md). The released APK retains the certificate used
for earlier local test builds; freshly built CI artifacts use ephemeral signing.

## Try it without credentials

An Android test client is available in [android/](android/README.md). It supports
artist search, saved watches, foreground page checks, editable ticket preferences and local demo notifications.
It is separate from the always-on Rust monitor and does not bundle API credentials.

Install current stable Rust (tested with 1.99.1), then run from this repository:

```bash
cargo build --release --locked
./target/release/ticket-watch --config examples/offline-demo.toml replay --watch demo \
  tests/fixtures/synthetic-unavailable.html \
  tests/fixtures/synthetic-offers.html \
  tests/fixtures/synthetic-offers.html \
  tests/fixtures/synthetic-blocked.html \
  tests/fixtures/synthetic-offers.html \
  tests/fixtures/synthetic-unavailable.html \
  tests/fixtures/synthetic-offers.html
```

This uses **synthetic HTML**, an isolated in-memory database, and no network calls. It prints two Discord payload previews: the first matching availability transition and a later restock. Repeated offers and recovery from a blocked page do not create duplicate alerts. The fixture selectors are not Ticketmaster selectors.

## Inspect any selected event

```bash
./target/release/ticket-watch inspect --url 'YOUR_PUBLIC_TICKETMASTER_EVENT_URL'
# Optional: --headed on a graphical desktop, --currency PLN for ambiguous currency,
# or --html captures/event.html for an offline capture (observed_at = 0).
```

This command needs no configuration, API key, database, or Discord token. It reports ticket categories, observed offers, and an explicit adapter/availability result. Choose watches in `config.example.toml` or subscribe to selected artists below. The Android test app applies the same event-independent layout approach to foreground page checks.

## Combine Discovery and website monitoring

Configure your API key privately as `TICKETMASTER_API_KEY`, then find the artist's Discovery ID:

```bash
./target/release/ticket-watch artists --query KoRn
cp examples/hybrid.example.toml config.local.toml
```

In `config.local.toml`, set the subscription's `attraction_id`, choose its country/city, and enable it. The linked website profile specifies which hosts and extraction rules can be used. The example uses `adapter = "ticketmaster"` to recognize the supported layout on each matching event. No per-event selectors are required. Leave unfamiliar layouts unknown until an adapter is validated for them.

```bash
./target/release/ticket-watch --config config.local.toml validate
# API searches only: save matching website watches, without sending messages.
./target/release/ticket-watch --config config.local.toml sync
# Inspect the saved catalog, generated watches, and coverage diagnostics.
./target/release/ticket-watch --config config.local.toml catalog
# Refresh Discovery in the background and monitor selected event pages.
./target/release/ticket-watch --config config.local.toml run
```

Discovery supplies event identity, dates, venues, sale schedules, and advertised price ranges when available. Website observations supply availability and any observable section, row, seat, ticket type, price, or stock quantity. **An API `onsale` status or advertised price range never becomes proof of available inventory.** Missing fields remain unknown.

Use `run --send-discord` after configuring the bot token, destination, and reliable source extraction. Sending is disabled by default. For manually selected event URLs, the original `[[watches]]` configuration in [config.example.toml](config.example.toml) remains supported alongside subscriptions.

Catalog refreshes preserve matching cached watches through partial failures and resume saved pagination. Complete scans may retire outdated matches without treating them as sold out. See [running the monitor](docs/running.md) for overlap precedence, configuration, limits, and live validation.

## Implemented behavior

- [Netherlands Discord layout](docs/discord-netherlands.md): nine created channels separate sale schedules, observed original/resale offers, price changes, charts, event updates and monitor status. Country subscriptions can discover all returned artists; coverage and worker capacity limits remain explicit. Live feeds and multi-channel routing still need deployment.
- [Event price history and charts](docs/price-history.md): watcher and map snapshots persist independently of alerts. Export asking-price lines/candles and observed-entry counts, with separate ticket types, primary/resale channels, currencies and price/fee basis. `/ticket chart` opens the local-import viewer; it is not yet connected to a live watcher. Observed volume is not confirmed sales volume.
- [Ticket preferences](docs/ticket-preferences.md): two adjacent tickets and cheapest eligible total by default. Android saves global defaults and per-event overrides for quantity, optional total budget, currency, adjacency, ticket type, category ID, section and row. Export to `cart-plan --preferences` to apply the choices to supplied quotes. Preferences do not activate automatic carting or change availability-alert filters.
- [Saved preferences and trigger preview](docs/monitor-trigger-preview.md): a separate revisioned registry rejects stale edits and exports the existing Android format. `cart-trigger-preview` compares a current manual/generated watch's latest stored offers against those settings, with explicit missing evidence. It never reserves or authorizes a cart.
- Reusable venue presets in SQLite, with scoped venue identity and separate named layout references. Discovery catalog refreshes register venue metadata; [venue commands](docs/venue-presets.md) import, list, inspect, and match presets without network access. The starter catalog contains 29 venues and six draft Dutch static map references; no seating layout is invented.
- [Map scraping](docs/seat-maps.md): `map-inspect` preserves the native map view, extracts supported SVG section outlines, and exports a clean map with `--geometry-svg`. It also reads static references/configured zones, downloads public images and saves draft venue layouts on request. Neither outlines nor map colors establish live seats or adjacency. These map commands run in the Rust service, not the Android inspector.
- [Netherlands mapping](docs/netherlands-mapping.md): 11 events across ten venues were checked; seven SVG overviews expose 231 sections and 232 label placements. Dutch section-name aliases and repeated label positions are preserved, with clean arena SVG exports and draft venue presets. Section/seat interaction and carting remain Czech-only.
- [Czech seat inspection](docs/seat-inspection.md): optional `--section-id` opens a numbered section; `--seat-id` hovers an explicit resale seat to read its row, number, restrictions and per-ticket price. Rendered seat counts stay separate from available quantities. No seat selection or reservation occurs.
- [Section price scans](docs/section-scans.md): `map-scan` discovers resale seat candidates automatically, reads bounded hover prices in one browser session, and optionally records observation changes in a separate SQLite history. Missing seats remain unknown; these scans do not send alerts or create carts.
- [Scheduled section monitoring](docs/map-monitor.md): `map-run` schedules selected sections, persists cooldowns and queues displayed-price notifications for Discord. Preview is the default; `--send-discord` enables delivery to your configured channel. These messages do not claim verified stock or reserve tickets.
- [Map-monitor service setup](docs/map-service.md): dedicated systemd templates, graceful SIGTERM shutdown and `map-health` freshness checks support supervised operation. The templates are not installed or activated automatically.
- Built-in layout recognition for any selected event, plus verified custom CSS profiles. JSON-LD metadata is labeled separately and does not alert by default.
- Displayed ticket categories and price ranges stay separate from confirmed resale listings; status includes the latest observation.
- Each offer requires a visible, enabled purchase signal. Browser captures preserve computed visibility and disabled controls, and fetched event IDs must match the watched event after redirects.
- Prices carry currency, per-ticket/total/unknown basis, and fee scope. Quantities distinguish listing, reported remaining, selectable order capacity, and unknown; they are never added into an event-wide stock total.
- A local browser capture command and offline `profile-check` report help validate event-specific extraction without sending messages or modifying monitoring state.
- A [manual verification extension point](docs/manual-intervention.md) keeps interactive capture in the same browser while an operator handles a challenge. Background checks report a fixed blocked reason and back off.
- Optional quantity and price filters, offer-change detection, and a quiet initial baseline by default.
- Per-host pacing, bounded concurrency, backoff, and saved host cooldowns.
- Atomic state/outbox persistence, restart deduplication, stale-alert expiry, and explicit Discord mentions.
- Direct HTTP or explicitly selected browser mode; HTTP 403 does not trigger an automatic browser retry.
- [Czech access diagnosis](docs/czech-access.md): specific protection-block diagnostics and a bounded wait for the website's normal initial identification navigation. Headed Chromium reached the selected KoRn event and exposed four ticket categories in the recorded test.
- [Czech cart preparation](docs/czech-carting.md): `cart-catalog` inspects configured prices and quantity controls; `cart-plan` evaluates supplied group quotes offline using the requested filters and adjacency preference. Neither command reserves tickets.
- [Native controls inspection](docs/cart-controls.md): `cart-controls` uses a separate browser context, disables networking before a demo quantity change, reads price/section filters and restores zero selection. It never submits Find tickets.
- [Native preparation rehearsal](docs/native-preparation.md): `cart-prepare` uses saved quantity preferences and explicit configured price/area IDs, verifies the native filter selections offline and restores the form. `cart-catalog --preferences` ranks configured choices while keeping exact seat filters and live availability unverified. These commands require current Rust source and are newer than the 0.7.0-test downloads.
- [Native cart attempt](docs/native-cart-attempt.md): the experimental `cart-attempt --execute` command applies saved preferences in a retained Czech browser and journals one Find tickets action before submission. It observes the same session afterward and never retries or pays. Receipt verification and customer cart handoff remain unfinished; this command is newer than the 0.7.0-test downloads.
- [Local operator and recovery commands](docs/cart-operator.md): `cart-attempt --operator --headed` retains the original browser for manual verification and review, with `resume`, `status`, `close` and a bounded deadline. `cart-recovery` explains retained locks and evidence; `cart-abandon-prepared` cleans up only attempts that never dispatched. A bounded passive request ledger improves diagnostics without treating errors or checkout redirects as hold proof.
- A [recorded KoRn native test](docs/evidence/native-cart-attempt-2026-11-10.json) submitted two tickets at the inspected 1,712 CZK configured price. Ticketmaster reported insufficient tickets for that selection; no cart was verified and the uncertain event lock remains active. This does not establish an event-wide sellout.
- [Cart attempt journal](docs/cart-journal.md): durable plan/dispatch tracking and receipt checks prevent duplicate attempts after a restart. `cart-journal --file ...` inspects its state.
- The journal also supports a native search that may reserve before returning exact seats. Requested filters and post-action hold evidence remain separate, with a shared event lock across both workflows.
- Native recovery preserves prior outcomes in a versioned history and uses fresh evidence plus an expected-outcome fingerprint to prevent conflicting updates. Known holds require matching hold evidence or explicit correlated release. `cart-journal --history-attempt ...` reads the audit trail; the native adapter currently retains uncertainty until a receipt contract is validated.
- Performance timestamps are bound across the cart plan and receipts. The [Czech reservation-flow trace](docs/czech-reservation-flow.md) identifies Find tickets as a possible reservation action; a selection counter cannot confirm a hold.
- Artist/location subscriptions, bounded Discovery pagination, a persisted generated watchlist, and background catalog refresh. Discovery request budgets and provider cooldowns are separate from website pacing.
- Read-only API metadata stays separate from observed website inventory; generated watches cannot enable metadata-based stock alerts.

Server watch selection currently uses TOML and the CLI. Android preferences can be exported to the offline planner but are not synchronized with the server. The optional [Discord command endpoint](docs/discord-setup.md) runs on Vercel and provides signed `/ticket ping`, `/ticket help` and `/ticket chart`; it is separate from the persistent watcher. Remote watch/settings controls, scheduled-sale timers, monitor-triggered carting, and multi-node operation are not implemented. Successful parsing of one market does not establish worldwide coverage or a latency guarantee.

The [production roadmap](docs/production-roadmap.md) tracks the remaining Discord,
cart receipt, automation and deployment work, followed by the planned MCP market
research and resale experiment. The MCP and trading features are not implemented.

## Development and operations

```bash
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
```

Optional browser transport: Node.js 20+, `npm ci`, then `npm test`. Install Chromium using `npm run browser:install` or configure an installed executable. Local HTTP tests require permission to bind loopback ports; tests do not send live Ticketmaster or Discord requests.

Operational instructions: [docs/running.md](docs/running.md). Example service: [deploy/ticket-watch.service](deploy/ticket-watch.service). The earlier [architecture](docs/architecture.md), [website feasibility report](docs/website-monitoring.md), and [screenshot reference](docs/alert-reference.md) describe design goals and evidence; not every planned feature is implemented.
