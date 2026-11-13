# Event price and observed-volume history

Every accepted observation from the normal watcher is now recorded in its SQLite
database. Czech map scans record a separate displayed-seat history in their map
database. Recording is independent of alert delivery and notification filters:
unchanged samples and unsuccessful checks are retained, not just price alerts.
Database work stays in the existing transaction; chart rendering runs separately
so it does not delay the network polling loop with browser rendering or exports.

History starts with the first sample recorded by this version. It cannot reconstruct
prices from before collection started. Keep the event enabled through the period
you want to track, including the event itself. Removing a watch does not delete
its recorded history. Event completion is not inferred from a failed page, empty
scan or a change in ticket availability; automatic end-of-event scheduling is not
implemented yet. No retention deletion is applied automatically. Back up and
monitor disk use on the persistent host.

## What the charts measure

Each event has independent source histories. Event-page observations and different
map sections are not added together: they can cover overlapping inventory. A
changed watch configuration creates a separate source. Ticket type, original /
resale / unknown sale channel, currency, price basis, fee treatment, quantity
semantics and observation kind all stay separate.
Group-total prices also require an explicitly reported listing bundle size;
two-ticket and four-ticket totals have separate series. A remaining-stock count
does not establish a bundle size. Unproven bundle totals keep their observed-entry
counts but do not supply a numeric price chart.

| Measure | Meaning |
| --- | --- |
| Asking price | Minimum observed price within the selected comparable series at a sample time. It is not a transaction price. |
| Candlestick | Open, high, low and close of those minimum asking prices within a time bucket. It is not based on completed sales. |
| Observed entries | Distinct observed offers, displayed categories or displayed seat quotes, depending on the selected series. The bar uses the final sample in a bucket, never the sum of repeated checks. |
| Reported quantity | Sum of explicit listing/remaining quantities across observed entries in the selected sample, when every quantity is known. Listings may overlap. This is not total event stock or sold volume. |
| Displayed category / seat quote | A visible price observation. It does not prove a ticket can be purchased or that a map seat is still available. |

Missing prices, blocked checks, missing-series and long polling gaps break the price line. A
failed check does not become a zero-price or sold-out point. Quantity selectors
are not remaining tickets. A removed listing is not a confirmed sale. Unknown
currency, fee and price basis remain explicitly unknown. Prices with different
currencies or fee treatment are never combined or converted.

When a supported map exposes a validated seat quote, the export includes both
the aggregate displayed-seat series and a separate series labelled with its
section, row and seat number. Seat identity is hashed and scoped to that event
and section. Equal prices do not merge different seats. The viewer never adds
the individual series to the aggregate count. A missing seat leaves a gap; a
return adds a new observed quote. Neither transition proves a sale or a cart hold.
Known displayed prices can remain continuous while stock is unknown, with that
uncertainty visible. Unscheduled captures remain isolated observations.

Scheduled individual-seat collection currently supports the Czech map adapter.
[Manual Dutch inspection](netherlands-seat-validation.md) has validated one
primary and one resale seat quote at two venues, but Dutch automatic section
scans and history collection are not enabled. Adding history storage does not
establish available stock or general layout coverage.

The current sources do **not** provide verified market-wide sales volume. Future
confirmed purchase/resale receipts need a separate transaction ledger; scraping
listing changes is insufficient to fill that gap.

## Export and view

With an updated watcher collecting data:

```sh
# Existing normal watcher DB: standalone interactive HTML, opens offline.
ticket-watch history-export --database data/ticket.sqlite --output event-history.html

# JSON for the hosted viewer; the file is read locally and is not uploaded.
ticket-watch history-export --database data/ticket.sqlite --output event-history.json

# Combine event and map histories while keeping their sources separate.
ticket-watch history-export --database data/ticket.sqlite \
  --database data/map-monitor.sqlite --output combined-history.html

# Optional public event and inclusive timestamp filters.
ticket-watch history-export --database data/ticket.sqlite \
  --event-url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --from 2026-11-10T00:00:00Z --to 2026-11-11T00:00:00Z \
  --output korn-day-one.html
```

Use the actual configured database paths. The exporter is read-only: it does not
create a missing database, migrate an old one, fetch a page or send a Discord
message. Choose a new output filename each time; it never overwrites an existing
file. Exports have 16 MiB / 10,000-sample limits and refuse an oversized selection
instead of silently omitting history. Narrow the time window if required. The
output contains public event metadata and aggregate observations, not page captures,
cart tokens or raw offer identifiers. Keep exports private if your watchlist is
private.

Open **[/market.html](https://ticket-discord.vercel.app/market.html)**, or use
`/ticket chart` after the guild command registration is updated. Select a JSON
export, event, source and comparable ticket series. Choose the time window,
line/candlestick view and observed-entry/reported-quantity volume measure. The
synthetic demo can be opened explicitly and is labelled throughout. A standalone
HTML export contains the same viewer and its data with no external dependencies.

For an entirely offline parser → database → chart rehearsal with invented prices:

```sh
cargo run --example price_history_demo -- /tmp/ticket-history-demo
```

Use a new directory. This creates synthetic observations and both export formats,
including a blocked interval; it performs no provider requests or Discord sends.

## Authenticated live charts

The hosted viewer can connect to the worker's separate read-only history API.
Enter the HTTPS endpoint and private history token in the connection form, then
choose an optional event URL and history window. The viewer refreshes without
discarding the selected event/source/series. Pause or disconnect at any time.
The token stays in page memory, is sent only in an Authorization header, and is
never written into a URL, browser storage or exported history. Imported files
stay local; standalone embedded HTML exports keep networking disabled.

The API runs beside the persistent watcher, using fixed configured database
paths and the existing read-only exporter. It does not scrape Ticketmaster,
modify watches or send alerts. Export limits remain enforced; a large country
history may require an event filter or shorter time window. Freshness is based
on source observation timestamps, not the time the API generated its response.
A successful refresh does not turn blocked or unknown observations into stock.

See [live history setup](live-history-api.md) for the private token, exact CORS
origin allowlist, loopback service and HTTPS reverse proxy. The hosted viewer's
CSP permits HTTPS connections so an operator can configure their own endpoint;
narrow that directive to the deployed API origin when the host is known.

The connection is implemented and can be tested locally. A persistent production
watcher/API host is still required before real data updates continuously.
Vercel serves the viewer and commands; SQLite stays on the worker. `/ticket ping`
still checks only the command endpoint. No paid host is provisioned by this change.
