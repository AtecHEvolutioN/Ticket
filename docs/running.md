# Running Ticket Watch

## Requirements and setup

Use current stable Rust and normal native build tools; this project was tested with Rust 1.99.1. The application uses edition 2024 and standard-library file locking. A minimum supported Rust version has not been established for all dependencies. SQLite is bundled. The core HTTP/CLI path does not need Node or a browser.

```bash
cargo build --release --locked
cp config.example.toml config.local.toml
./target/release/ticket-watch --config config.local.toml validate
```

Run these examples from the repository root. Pass `--config config.local.toml` explicitly; the default is `config.toml`. Database and browser-script paths resolve relative to the configuration file. Local configuration, databases, captures, and `.env` files are ignored by Git. The application does not automatically load `.env`; inject secrets through your shell or service manager.

## Find an artist and choose locations

If your Discovery key is configured as the secret environment variable `TICKETMASTER_API_KEY`, use:

```bash
./target/release/ticket-watch artists --query KoRn
./target/release/ticket-watch events \
  --attraction-id YOUR_SELECTED_ATTRACTION_ID --country PL --city Krakow
```

These two commands need no configuration file. Choose the attraction ID returned by the first command. A website's numeric artist ID is not necessarily its Discovery ID. Each call fetches one page: up to 20 artists or 100 events. Use `--page 1`, `--page 2`, and so on for additional result pages; the default page is zero, and pagination bounds are enforced. The client paces requests at two per second. Standalone search commands do not participate in the persisted catalog request budget and cannot enforce account-wide limits shared with other applications.

These standalone searches return metadata and sale information, not confirmed remaining tickets. They do not alter the watchlist. Artist/location subscriptions below connect Discovery results to website monitoring. Never put the key in a command argument, configuration file, issue, or chat.

Without a Discovery key, find candidate links from an accessible artist page:

```bash
./target/release/ticket-watch --config config.local.toml discover \
  --url https://www.ticketmaster.co.uk/korn-tickets/artist/775700
```

Add `--html captures/artist.html` for offline extraction or `--browser` to explicitly select the browser for a live request. Review candidate URLs for the desired artist/location and ordinary admission versus parking, upgrades, or packages. This website-link discovery command is a manual search step. Use API subscriptions for automatic catalog refresh and enrollment.

Add a `[[watches]]` entry for each selected event. `artist`, `country`, and `city` label that event; they do not turn its URL into an automatic artist/location search. Duplicate enabled event URLs are rejected. Restart the monitor after changing configuration.

## Subscribe and combine the sources

Copy [examples/hybrid.example.toml](../examples/hybrid.example.toml) to `config.local.toml` at the repository root. Replace `YOUR_DISCOVERY_ATTRACTION_ID` with the selected artist ID and set the subscription's `enabled = true`.

Each `[[subscriptions]]` entry selects one `attraction_id`, optional `country` and `city`, and a named website `profile`. Add entries for additional artists or locations. Country codes use uppercase ISO-style codes such as `PL`; city matching uses the returned venue metadata. Omitting country broadens the API query, but does not make every market's website readable or supported. Explicit `exclude_keywords` can omit parking, upgrades, or other unwanted products by title; review these filters for the selected market and language.

Each `[[profiles]]` entry lists the exact public Ticketmaster hosts it applies to and supplies website fetching mode, interval, parser selectors, and optional quantity/price filters. Do not assume selectors from one country work in another. Missing location/artist identity, incompatible hosts, unsupported URLs, excluded titles, and incomplete catalog coverage are reported rather than treated as sold-out inventory.

```bash
./target/release/ticket-watch --config config.local.toml validate
./target/release/ticket-watch --config config.local.toml sync
./target/release/ticket-watch --config config.local.toml catalog
./target/release/ticket-watch --config config.local.toml run
```

`sync` performs bounded Discovery searches and saves generated website watches. It does not fetch event pages or send Discord messages, and cannot run while a monitor owns the same database. `catalog` inspects saved catalog state without a network request. `run` uses cached eligible watches, refreshes Discovery in the background, and updates the website monitoring set as matching events are discovered. Slow API searches do not delay an already-detected website inventory alert. `run --once` attempts the catalog refresh before checking the selected pages; it still respects saved refresh cadence and provider cooldowns.

Generated watch IDs start with `auto-`; that prefix is reserved. Manual watches can coexist with subscriptions. For overlapping canonical event URLs, a manual watch takes precedence, including a disabled manual watch that explicitly pauses the event. Otherwise the first matching subscription in configuration order wins. Filters are not merged, so avoid overlapping profiles with conflicting requirements. Changing subscriptions or profiles invalidates their saved configuration binding. Restart the monitor after editing configuration.

A complete catalog scan can retire events that no longer match without asserting that their tickets are sold out. Partial results or failed refreshes retain cached matches; saved pagination cursors let later refreshes continue a bounded scan. Check the catalog notes and `complete` flags to distinguish current complete results from retained or incomplete coverage.

The catalog's initial limits are a 900-second refresh interval, 20 API requests per refresh, 4,000 attempted requests per rolling 24 hours, and 1,000 generated events. The request ledger survives restarts and counts failed attempts. This is a local budget for one database, not Ticketmaster's account-wide quota: other applications and standalone search commands may use the same key. The client also honors provider cooldowns and its normal two-requests-per-second pacing. Bounded pagination or a budget limit can leave coverage incomplete; inspect `catalog` before assuming every matching event is monitored. A 12 MiB snapshot target limits local catalog growth: descriptive metadata or detailed skip diagnostics may be omitted first, and any reduction in saved watches is marked as incomplete coverage.

API catalog fields and website offers remain separate. Advertised Discovery price ranges are metadata, not observed ticket prices, and an `onsale` event status does not establish current stock. Generated watches keep `notify_metadata = false`. Website quantity filters require observed stock semantics; unknown fields cannot satisfy a filter. Combining sources does not guarantee exact remaining quantities or seat-level coverage for every event.

## Validate page extraction

The shipped watch is a disabled placeholder. Replace its URL and labels, enable it, and use `mode = "browser"` with `[watches.parser] adapter = "ticketmaster"`. The built-in adapter recognizes page structure for any selected event; it does not require new selectors for every artist. [Supported fields and limitations](event-adapters.md) describe which layouts are recognized. `inspect --url PUBLIC_EVENT_URL` performs one independent check without a configuration file.

```bash
# One diagnostic request, without changing saved inventory state or sending alerts.
./target/release/ticket-watch --config config.local.toml check --watch selected-event

# Save accessible page markup for local parser development.
./target/release/ticket-watch --config config.local.toml check --watch selected-event \
  --save-html captures/event.html

# Test extraction without another website request.
./target/release/ticket-watch --config config.local.toml check --watch selected-event \
  --html captures/event.html
```

Captures may contain session-related data. Keep them local and redact them before sharing. Payment/cart URLs and credential-bearing URLs are rejected as watch targets.

For recurring Czech section hover-price checks, use the separate
[`map-run` setup](map-monitor.md) and `config.map-monitor.example.toml`.
It previews displayed-price notifications by default and needs `--send-discord`
for live delivery. `map-status` reports its saved health and pending messages.
These observations do not establish available ticket counts or adjacent groups.
Use the [map service guide](map-service.md) for a dedicated systemd instance,
explicit Discord enablement, graceful stop and source freshness checks.

For custom profiles, set `adapter = "selectors"` and configure selectors against current markup you can access and inspect. Do not combine manual selectors with the built-in adapter. Status selectors must apply to the event's relevant sale scope. Offer selectors should identify actual offers with stable IDs. A verified profile with `offers_selector` also requires `offer_available_selector`: a visible, enabled purchasable signal relative to each row. A disabled button or price-only row cannot establish availability. A complete-snapshot selector must establish that the relevant listing is fully loaded: missing offers on a partial page are not sold-out evidence. A generic event-wide purchase button is insufficient.

Set `quantity_kind` to `listing`, `remaining`, `selectable`, or `unknown` according to the source. A quantity dropdown always becomes `selectable`, irrespective of the configured stock meaning. Only listing/reported remaining quantities can satisfy `min_quantity`. The older `quantity_is_stock = true` setting maps an otherwise unknown kind to `remaining`; prefer the explicit field for new profiles.

Prices use integer minor units, the configured currency, and decimal separator. Set `price_scope` to `per_ticket`, `total`, or `unknown`, and `fees` to `included`, `excluded`, or `unknown`. `max_price_minor` requires `currency` and an observed `per_ticket` price basis. Unknown or total prices cannot satisfy a per-ticket ceiling. Profiles created before these fields were added must establish their meanings explicitly; existing persisted offers default to unknown. Price and quantity meanings appear in Discord previews. Overlapping offer quantities are never summed into total event stock.

`notify_initial = false` establishes a quiet first baseline. Enable it to alert on matching inventory already present when a watch starts. `notify_offer_changes` adds alerts for supported changes in existing matching offers. `notify_metadata` stays off by default because JSON-LD availability is a page hint, not a demonstrated live inventory feed.

## Optional browser transport

```bash
npm ci
npm run browser:install
npm test
```

Use Node.js 20+. Set a watch or website profile's `mode = "browser"` when rendering is needed. The worker reuses one ordinary browser context and navigates directly to each selected event. With `adapter = "ticketmaster"`, it rejects a recognized nonessential-cookie banner and opens a known Polish, English, Czech, Spanish, or Dutch best-available display tab when needed. It never selects a ticket, changes quantity, submits Find tickets, or enters checkout. Set `[browser] headed = true` to use a graphical display. Queues, access blocks, unfamiliar layouts, and unsupported display labels remain explicit; there is no automatic retry using another browser identity.

For an installed Chromium, set `[browser].executable_path` to its absolute path or expose `CHROMIUM_PATH`; the configured path takes precedence. `browser.node` can be an absolute Node executable path. Browser dependencies and the worker script must be accessible to the service account.

The browser respects inherited proxy settings and TLS verification. A 403 does not automatically switch transports. Challenges and incomplete pages are unknown/blocked observations, never proof of sold-out inventory.

The [Czech access investigation](czech-access.md) found an initial HTTP 401 HTML
identification response that navigated normally to the same event with HTTP 200
in headed Chromium. For this exact marked response, the worker allows a bounded
five-second passive wait within the request deadline. A supplied `Retry-After`
must parse as zero to permit this wait. It uses the final committed document and does not reload
the page or retry a blocked request. Initial 403 and 429 responses retain backoff.
The observed 403 protection marker now produces a specific access-protection
diagnostic in both browser and direct HTTP modes. An unfinished identification
navigation retains the initial blocked 401, with an explicit incomplete reason
and no inventory HTML.

Recognized verification and queue messages include a fixed manual-intervention
reason in browser diagnostics. Both the built-in adapter and custom selectors
treat these pages as blocked. Background checks keep normal backoff and close the
page; they do not wait for a person. See [manual verification](manual-intervention.md)
for the operator interface extension point and its current limits.

Browser mode serializes a clone of the rendered page with computed-hidden and disabled
elements marked for the Rust parser; it leaves the live DOM unchanged. This makes CSS
visibility and disabled fieldsets consistent with extraction. Successful page redirects
must retain the watched event ID. A different event's offers cannot populate the watch.

## Capture a readable page and review the scraper profile

When a public event page is readable in an ordinary browser on your own computer,
use the local capture tool from a graphical desktop and interactive terminal:

```bash
npm ci
npm run browser:install
npm run capture -- --url https://www.ticketmaster.pl/event/korn-bilety/496543017 \
  --output captures/event.html
```

It opens a fresh browser context. When the selected event's offers are visible,
return to the terminal and press Enter. If a recognized verification or queue prompt
remains, the tool lets you handle it in the same browser and press Enter to recheck,
up to three times within the session timeout. It does not select tickets, reserve
seats, import personal sessions, or automate verification. A final HTTP error,
remaining challenge, or different event prevents capture. The browser closes on completion, cancellation,
or timeout. An existing Chromium can be selected with `--executable-path`; use
`--timeout-secs` for a bounded 30–1800 second session.

The tool saves `captures/event.html` and `captures/event.html.json` with private
file permissions. The sidecar records event identity, time, and response status;
it explicitly leaves `profile_verified = false`. Files are not overwritten unless
`--overwrite` is supplied. Captures may contain session-related markup: keep them
local and review/redact before sharing. A successful capture does not establish
sustained unattended access or a permitted polling rate.

Configure candidate row/field selectors in a manual watch while keeping
`selectors_verified = false`, then inspect the extraction offline:

```bash
./target/release/ticket-watch --config config.local.toml profile-check \
  --watch selected-event --html captures/event.html
```

The report includes raw selector counts and at most 20 **unverified candidates**.
It performs no network requests and does not open the monitor database, change the
configuration, or queue alerts. The sidecar is detected automatically; use
`--capture-json PATH` to select another one. A mismatched event, unsuccessful HTTP
response, or visible challenge is rejected. Without a sidecar, provenance remains
unverified. A locally editable sidecar is not independent proof of authenticity.

Compare candidate prices, quantity meanings, ticket types, and purchase controls
against the actual page before marking a profile verified. Generated subscription
watches should first be copied into a manual review entry. The built-in adapter handles the recognized regional layout; custom selector profiles still require independent validation.

## Preview and run

```bash
./target/release/ticket-watch --config config.local.toml run --once
./target/release/ticket-watch --config config.local.toml run
./target/release/ticket-watch --config config.local.toml status
./target/release/ticket-watch --config config.local.toml outbox
```

`run` defaults to preview-only delivery. It still fetches websites and saves observations/outbox entries. Previews remain pending and expire according to `alert_max_age_secs`; switching to sending can deliver still-current pending alerts. The README's `replay` example is completely offline, uses an in-memory database, and does not alter production state.

Default pacing values are engineering settings, not a Ticketmaster allowance or a latency promise. Effective frequency depends on watch interval, per-host interval, concurrency, response times, and backoff. Host pacing and cooldowns survive restarts; a one-shot run can skip a watch while a saved cooldown is active. `check` is a manual diagnostic, not the scheduler: do not repeatedly invoke it to defeat cooldowns.

Only one monitor can own a database. Use persistent local storage for SQLite and its WAL files. Stop gracefully with Ctrl+C. Back up through SQLite's backup facilities or stop the monitor before copying its database files.

## Discord delivery

Create a Discord bot application and add it to your chosen server. Grant View Channel, Send Messages, and Embed Links for the destination; administrator permission is unnecessary. Role mentions also depend on server permissions/settings, and device pings depend on recipient notification settings.

Set numeric `[discord].channel_id`, `mention_user_ids`, and/or `mention_role_ids`. Store the token in the `DISCORD_BOT_TOKEN` secret environment variable, or change `token_env` to another variable name. Never place the token itself in TOML. Previews need no token.

```bash
./target/release/ticket-watch --config config.local.toml run --send-discord
```

This flag sends real external messages. The sender uses explicit mentions, bounded payloads, persisted Discord cooldowns, and stable retry nonces. Discord's deduplication window is limited, so ambiguous timeouts do not have an exactly-once guarantee. Stale alerts expire and confirmed newer negative evidence can cancel unsent alerts. Changing delivery configuration cancels existing pending alerts on restart. Permanent credential/channel errors stop the monitor so configuration can be corrected.

There are no Discord slash commands, scheduled-sale timers, or cart/checkout automation in this version. Manage watches through TOML and restart after edits.

## Optional Linux service

The [service unit](../deploy/ticket-watch.service) is a template, not an installed deployment. It assumes:

- Code and release binary are at `/opt/ticket-watch`.
- An unprivileged `ticketwatch` user/group exists.
- `/opt/ticket-watch/config.local.toml` sets `database = "/var/lib/ticket-watch/ticket.sqlite"`.
- Optional `/etc/ticket-watch.env` contains secret environment assignments with restricted file permissions.
- Browser dependencies, if enabled, are accessible to the service account. Prefer an explicit system Chromium executable; a download in a developer's home may be inaccessible under the unit's restrictions.

The unit starts in preview mode. After validating the source and configuring Discord, explicitly append `--send-discord` to `ExecStart`. Install the unit under `/etc/systemd/system`, reload systemd, and start it using normal administrator procedures. Read logs using `journalctl -u ticket-watch`.

The service restarts on failure with a startup-rate limit to prevent a tight loop. Fix persistent configuration/credential errors before restarting a failed unit. Keep its state directory across upgrades. The example sends SIGINT for graceful shutdown and requires no administrator account for the application process.

No service installation, host purchase, or production deployment has been performed.

## Verification and remaining limits

```bash
cargo test --locked
cargo clippy --all-targets --locked -- -D warnings
cargo fmt --check
npm test
# Also exercise actual Chromium with local fixtures and the compiled Rust CLI:
TICKET_BROWSER_TESTS=1 CHROMIUM_PATH=/usr/bin/chromium npm test
```

Tests cover synthetic parser states, artist/location planning, catalog request accounting, transitions, restart deduplication, outbox handling, Discord payload/cooldown behavior, and local browser/HTTP mocks. Some tests need loopback-listener permission. They do not send live Ticketmaster or Discord requests or establish production access.

A single KoRn snapshot validates displayed section, row, price, type, and listing-count evidence. Automated extraction across available, unavailable, and blocked states, sustained polling, and live alerts still need validation. Historical architecture documents include future goals; the CLI and this runbook define current operational scope.
