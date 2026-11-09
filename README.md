# Ticket Watch

Rust monitoring that combines **Ticketmaster Discovery API searches** with **website availability checks**. Choose artists and locations; Discovery finds matching events, and the website adapter checks those events for observable ticket availability before sending Discord alerts.

**Current status:** selected events use a reusable Ticketmaster layout adapter, with no artist names, event IDs, or generated CSS classes in its extraction rules. It reads displayed categories/ranges and explicit resale listing quantities from the recognized regional layout. Browser mode opens known best-available tabs automatically. Unknown layouts and blocked access remain explicit; this is not verified support for every Ticketmaster market. See [event adapters](docs/event-adapters.md) and the [21-event, 12-country coverage report](docs/coverage-2026-11-09.md). No production service or live Discord delivery has been deployed.

## Downloads

The [0.6.0-test release](https://github.com/AtecHEvolutioN/Ticket/releases/tag/v0.6.0-test)
contains the tested builds and SHA-256 checksums:

- [Android APK](https://github.com/AtecHEvolutioN/Ticket/raw/refs/heads/main/downloads/ticket-watch-android.apk) — Android 8.0 or newer; manual page checks and venue presets.
- [Linux x86_64 bot](https://github.com/AtecHEvolutioN/Ticket/raw/refs/heads/main/downloads/ticket-watch-bot-linux-x86_64.tar.gz) — continuous monitoring after configuration; glibc 2.39 or newer.
- [21-venue starter database](https://github.com/AtecHEvolutioN/Ticket/raw/refs/heads/main/downloads/venue-presets.sqlite) — a fresh database with venue metadata and no saved watches. To add venues to an existing monitor, import [the JSON seed](examples/venue-presets.json) using [the venue commands](docs/venue-presets.md).

Release verification: 164 Rust tests, Clippy, 26 browser tests, and 291 offline Android
assertions passed locally. Android verification used an API 28 emulator; see
[test details](android/TESTING.md). The released APK retains the certificate used
for earlier local test builds; freshly built CI artifacts use ephemeral signing.

## Try it without credentials

An Android test client is available in [android/](android/README.md). It supports
artist search, saved watches, foreground page checks, and local demo notifications.
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

- Reusable venue presets in SQLite, with scoped venue identity and separate named layout references. Discovery catalog refreshes register venue metadata; [venue commands](docs/venue-presets.md) import, list, inspect, and match presets without network access. The starter catalog contains 21 venues with no invented seating layouts.
- [Map scraping](docs/seat-maps.md): `map-inspect` extracts bound static map references and configured zones, downloads public images on request, and optionally saves separate draft layouts at a shared venue. Map drawings never become live seat or adjacency evidence. Available in current source; the APK is unchanged.
- Built-in layout recognition for any selected event, plus verified custom CSS profiles. JSON-LD metadata is labeled separately and does not alert by default.
- Displayed ticket categories and price ranges stay separate from confirmed resale listings; status includes the latest observation.
- Each offer requires a visible, enabled purchase signal. Browser captures preserve computed visibility and disabled controls, and fetched event IDs must match the watched event after redirects.
- Prices carry currency, per-ticket/total/unknown basis, and fee scope. Quantities distinguish listing, reported remaining, selectable order capacity, and unknown; they are never added into an event-wide stock total.
- A local browser capture command and offline `profile-check` report help validate event-specific extraction without sending messages or modifying monitoring state.
- A [manual verification extension point](docs/manual-intervention.md) keeps interactive capture in the same browser while an operator handles a challenge. Background checks report a fixed blocked reason and back off. This source change is newer than the 0.6.0-test downloads.
- Optional quantity and price filters, offer-change detection, and a quiet initial baseline by default.
- Per-host pacing, bounded concurrency, backoff, and saved host cooldowns.
- Atomic state/outbox persistence, restart deduplication, stale-alert expiry, and explicit Discord mentions.
- Direct HTTP or explicitly selected browser mode; HTTP 403 does not trigger an automatic browser retry.
- [Czech access diagnosis](docs/czech-access.md): specific protection-block diagnostics and a bounded wait for the website's normal initial identification navigation. Headed Chromium reached the selected KoRn event and exposed four ticket categories in the recorded test.
- [Czech cart preparation](docs/czech-carting.md): `cart-catalog` inspects configured prices and quantity controls; `cart-plan` evaluates supplied adjacent-group quotes offline. Neither command reserves tickets. These source commands are newer than the 0.6.0-test downloads.
- Artist/location subscriptions, bounded Discovery pagination, a persisted generated watchlist, and background catalog refresh. Discovery request budgets and provider cooldowns are separate from website pacing.
- Read-only API metadata stays separate from observed website inventory; generated watches cannot enable metadata-based stock alerts.

Selection currently uses TOML and the CLI. Discord slash commands, scheduled-sale timers, automatic carting, and multi-node operation are not implemented. Successful parsing of one market does not establish worldwide coverage or a latency guarantee.

## Development and operations

```bash
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
```

Optional browser transport: Node.js 20+, `npm ci`, then `npm test`. Install Chromium using `npm run browser:install` or configure an installed executable. Local HTTP tests require permission to bind loopback ports; tests do not send live Ticketmaster or Discord requests.

Operational instructions: [docs/running.md](docs/running.md). Example service: [deploy/ticket-watch.service](deploy/ticket-watch.service). The earlier [architecture](docs/architecture.md), [website feasibility report](docs/website-monitoring.md), and [screenshot reference](docs/alert-reference.md) describe design goals and evidence; not every planned feature is implemented.
