# Authenticated live history charts

The chart viewer can read fresh snapshots from the persistent worker without
uploading SQLite files to Vercel. `server/history-api.cjs` is a separate Node.js
service that calls the Rust `history-export` command against a fixed list of
existing databases. It does not fetch tickets, start the watcher, create a
database, migrate schemas or send Discord messages.

The API and live viewer are implemented. **No worker VM, public history hostname,
TLS certificate or live data source has been provisioned by adding these files.**
The local integration tests use synthetic history and do not prove Ticketmaster
access or continuous collection.

## Connect the services

1. Install the reviewed worker and a current `ticket-watch` binary on the chosen
   host. Start collecting observations using the separately configured watcher.
   Existing history begins at the first recorded observation; it cannot recover
   prices from before collection began.
2. Copy [the environment example](../deploy/history-api.environment.example) to
   `/etc/ticket-watch-nl/history-api.environment`, owned by root with mode `0600`.
   Set a newly generated random `TICKET_HISTORY_TOKEN` privately. Use at least
   32 characters (32 random bytes encoded as hexadecimal is suitable). This is a
   read-only chart credential, separate from Ticketmaster and Discord credentials.
3. Set the fixed database list and exact browser-origin allowlist. These settings
   are JSON arrays. Paths must be absolute, existing regular files without
   symlinks. Do not configure cart journals or unrelated databases. Up to eight
   watcher/map history databases can be read, with sources kept separate.
4. Install the optional [systemd unit](../deploy/ticket-history-api.service).
   It uses the previously prepared `ticketwatch-nl` account, a private temporary
   directory and a read-only filesystem view of watcher state. The API listens
   only on loopback (`127.0.0.1:8787`, or explicitly configured `::1`). Starting
   this unit is a separate operator action; the NL preparation helper does not
   install or activate it.
5. Put an HTTPS reverse proxy in front of the loopback API. Restrict proxy routing
   to `/api/history` and `/api/health`; forward `Authorization` and `Origin`.
   Disable response caching and request/header logging for these routes. Never
   log the bearer token, return it to the viewer, or put it in a URL. A dedicated
   hostname such as `https://history.example.com` keeps this separate from the
   public Vercel application. Configure the actual hostname and TLS certificate
   on your approved host; no proxy or DNS record is created automatically.
6. Open `/market.html` on the Vercel viewer. Enter the HTTPS API origin and the
   read-only token in its live-data controls. The token stays in the open page's
   memory; it is not stored in the URL or browser storage. Select one event or a
   shorter timeframe when national history exceeds the export limit. Rotate the
   token by replacing the environment value and restarting this API service.

The service intentionally shares the watcher account so it can read its private
SQLite files, but systemd mounts the state read-only for this process. Confirm
read-only WAL access on the actual host while the watcher is writing. Do not
change the database to world-readable or copy only its main file while WAL
writes are active. The API never receives the watcher's provider or bot tokens;
the child exporter receives a minimal environment without the chart token.

For a local development viewer, explicitly allow its exact loopback origin in
`TICKET_HISTORY_ALLOWED_ORIGINS`. HTTP is supported only for loopback development;
remote chart connections require HTTPS. A file opened with `file://` has an
opaque origin and is not allowed for live access; use the hosted viewer instead.

## HTTP contract

Every data request requires `Authorization: Bearer <read-only-token>`. The API
compares a fixed-length hash using a constant-time comparison. CORS is restricted
to the configured exact origins; an approved preflight may request only GET and
the Authorization header. Cookie authentication and query-string tokens are not
supported. All responses have `Cache-Control: no-store`.

`GET /api/history` returns the existing history JSON schema version 1 unchanged.
Optional parameters:

| Parameter | Meaning |
| --- | --- |
| `event_url` | Public HTTPS Ticketmaster `/event/` URL; remove language/tracking query parameters and fragments. This filters existing data only and never causes a network request. |
| `from` | Inclusive RFC3339 timestamp with timezone; defaults to 48 hours before `to`. |
| `to` | Inclusive RFC3339 timestamp with timezone; defaults to now. |

The maximum requested window is 366 days. Unknown or repeated parameters are
rejected. The existing export limits remain 16 MiB, 10,000 samples, 2,000 events,
5,000 sources and 200,000 series points. Large exports fail explicitly; data is
never silently truncated or combined into a claim of total event inventory.
The viewer may refresh every 30 seconds; this reads the latest saved observations
and does not increase the Ticketmaster polling rate.

One export runs at a time, with at least five seconds between starts and a
30-second subprocess timeout. Responses are not cached across requests. A browser
disconnect kills its exporter before another export is admitted. Temporary
outputs are private and removed after success, failure, timeout or cancellation.

| Status | Meaning |
| --- | --- |
| 200 | Valid export; `events: []` means no observations in the requested window, including an old database without history tables. |
| 400 | Invalid filter or timeframe. |
| 401 / 403 | Invalid token / unapproved browser origin. |
| 404 `no_history` | No saved observations match the selected event and timeframe. |
| 413 | Export exceeds a bound; narrow the event/time selection. |
| 429 | Export busy or minimum interval active; honor `Retry-After`. |
| 503 | Configured database/binary missing, invalid history schema or other unavailable source. |
| 504 | Export timed out; narrow the selection and inspect the worker. |

Errors use `{ "error": { "code": "…", "message": "…" } }`. They omit raw
subprocess stderr, file paths, query values and credentials.

`GET /api/health` is also authenticated. It reports the API's last export state,
counts and latest observation age **within the last successful filtered request**.
That observation may be blocked or unknown. It is not whole-catalog freshness,
verified inventory, or watcher-process health. Before the first export the age
is null. Future observation timestamps produce null age and
`observation_clock_invalid: true`, rather than appearing fresh.

## Verification

```sh
node --test server/history-api.test.cjs
```

Tests cover authentication, CORS, duplicate headers, invalid filters, request
limits, subprocess timeouts/cancellation, missing-file behavior, token isolation
and real SQLite → Rust exporter → HTTP refreshes with unchanged database hashes.
Build `ticket-watch` first for the SQLite integration, or set
`TICKET_HISTORY_TEST_BINARY` to its absolute path. The integration fixture uses
Python 3's standard SQLite module. CI includes this test in the normal backend
suite. A loopback-restricted execution sandbox may require its normal test
permission before binding the local HTTP server.

These tests establish the local connection. Production acceptance still needs
the real host's read-only WAL behavior, HTTPS/origin configuration and an observed
worker update reaching the authenticated chart without exposing credentials.
