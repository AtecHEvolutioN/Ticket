# Run map monitoring as a Linux service

The [map service](../deploy/ticket-watch-map.service) is an opt-in systemd
template. It runs `map-run` in preview mode, with a dedicated account and durable
state. It has not been installed, enabled or started on a production host.
The existing `ticket-watch.service` remains the separate inventory monitor.

This service observes displayed Czech resale seat prices. It does not establish
available quantities, adjacent groups or successful carting. An active service
does not prove that Ticketmaster is accessible or that Discord delivery works.

## Paths and prerequisites

| Purpose | Service path or identity |
| --- | --- |
| Unit | `ticket-watch-map.service` |
| Unprivileged user and group | `ticketwatch-map` |
| Code, Node dependencies and release binary | `/opt/ticket-watch` |
| Private configuration | `/etc/ticket-watch/map.toml` |
| Optional secret environment file | `/etc/ticket-watch/map.env` |
| Durable history, queue, cooldowns and lock | `/var/lib/ticket-watch-map` |
| Browser cache | `/var/cache/ticket-watch-map` |
| Logs | systemd journal, identifier `ticket-watch-map` |

Build the release binary with `cargo build --release --locked`. Install Node.js
20 or newer, run `npm ci` in the deployed checkout, and install Chromium and its
native dependencies. The example uses `/usr/bin/node` and `/usr/bin/chromium`;
adjust these absolute paths to match the host. The service account must be able
to read the checkout, worker and Node modules and execute both binaries. Keep the
application files owned by the deployment administrator rather than writable by
the service account.

The template is headless. A previous headed capture does not establish that the
same page will work in this service. Validate ordinary headless access on the
chosen host. Headed operation needs an existing graphical session, its display
and authentication access, and a reviewed service environment. The template does
not install or start Xvfb. A browser downloaded into another user's home is not
the configured system Chromium.

The unit uses a private temporary directory and private state permissions. It
does not add untested namespace, syscall or Chromium sandbox overrides. Validate
any additional host hardening with the real browser before deploying it.

## Prepare and preview

The following are administrator setup commands for the chosen host, not actions
performed by this repository:

```bash
sudo useradd --system --user-group --home-dir /var/lib/ticket-watch-map \
  --no-create-home --shell /usr/sbin/nologin ticketwatch-map
sudo install -d -o root -g ticketwatch-map -m 0750 /etc/ticket-watch
sudo install -d -o ticketwatch-map -g ticketwatch-map -m 0750 \
  /var/lib/ticket-watch-map /var/cache/ticket-watch-map
sudo install -o root -g ticketwatch-map -m 0640 \
  deploy/map-service.example.toml /etc/ticket-watch/map.toml
```

If the account already exists, reuse it after checking its identity and
permissions. Edit the installed configuration to select the event and numbered
section. The shipped watch is disabled deliberately; enable a reviewed watch
before starting the service. All database and browser paths in the service
example are absolute. The `database` path is an unused, separate inventory path;
`map_database` is the file used by this service.

Run static validation and a manual preview as the same account that will own the
service. The preview makes website requests and saves state but sends no messages:

```bash
sudo -u ticketwatch-map /opt/ticket-watch/target/release/ticket-watch \
  --config /etc/ticket-watch/map.toml validate
sudo -u ticketwatch-map /opt/ticket-watch/target/release/ticket-watch \
  --config /etc/ticket-watch/map.toml map-run --once
sudo -u ticketwatch-map /opt/ticket-watch/target/release/ticket-watch \
  --config /etc/ticket-watch/map.toml map-health --max-age-secs 180
```

The same state is used after service startup, so an immediate run can skip a
watch while its interval or provider cooldown remains active. Do not delete the
database or repeatedly use diagnostic fetch commands to defeat that pacing.

After reviewing the preview, install and start the preview service explicitly:

```bash
sudo install -o root -g root -m 0644 \
  deploy/ticket-watch-map.service /etc/systemd/system/ticket-watch-map.service
sudo systemd-analyze verify /etc/systemd/system/ticket-watch-map.service
sudo systemctl daemon-reload
sudo systemctl start ticket-watch-map.service
sudo journalctl -u ticket-watch-map.service -f
```

Use `systemctl enable ticket-watch-map.service` separately if startup at boot is
wanted. The unit uses `Type=exec`; systemd starting the executable is not source
readiness. With no enabled watches, the runner reconciles disabled state and
exits successfully; it does not remain active.

## Check operation and freshness

Check both the running process and the saved source evidence:

```bash
systemctl is-active --quiet ticket-watch-map.service
sudo -u ticketwatch-map /opt/ticket-watch/target/release/ticket-watch \
  --config /etc/ticket-watch/map.toml map-health --max-age-secs 180
```

Require success from both commands in external supervision. `map-health` emits
JSON and exits zero only when all enabled configured watches have matching,
successful observations within the requested age. It exits nonzero for absent
state, no enabled watches, changed configuration, blocked/unknown observations,
or stale evidence. Configuration and malformed-database errors also exit
nonzero. It does not fetch a page, use a bot token, migrate the schema or modify
saved state.

The report includes `status`, `healthy`, `checked_at_ms`, `max_age_secs`,
`enabled_watches`, per-watch `checks`, and saved `cooldowns`. Each check includes
its issues, source age, last success, next due time and pending/expired alert
counts. Its scope is `displayed_price_observations`; stock, adjacency, delivery
and running-process verification remain false. A stopped process may still have
fresh saved evidence briefly, which is why the process check is separate.

Choose the age threshold for the actual schedule. Captures are serialized and
all Czech watches share host spacing. With several watches, a 180-second age
threshold may be shorter than one full rotation. The default health threshold
is 120 seconds, not a latency guarantee. Provider backoff can intentionally make
evidence stale: investigate the reported block or cooldown rather than restarting
the service in a loop. Source failures do not mean tickets are sold out.

The journal contains JSON lifecycle records:

```json
{"event":"map_monitor_started","timestamp_ms":1794296702120,"process_id":1234,"enabled_watches":1,"delivery_mode":"preview_only"}
{"event":"map_monitor_stopped","timestamp_ms":1794296762120,"process_id":1234,"reason":"shutdown"}
```

`map_monitor_started` means configuration was accepted, state reconciled and the
fetcher initialized. Chromium starts lazily, so the record does not prove browser
startup, current prices or Discord access. `map_monitor_stopped` is emitted after
network tasks and the browser worker are cleaned up; its reason is `shutdown`,
`once_complete` or `failed`. Early initialization failures appear on stderr.
Scan summaries and preview payloads are separate JSON records. Logs can contain
selected events, seat prices and configured mention IDs; treat journal access
as operational access.

## Enable Discord deliberately

Configure the numeric channel and allowed mention IDs in `map.toml`. Put the bot
token in `/etc/ticket-watch/map.env`, owned by root with mode `0600`. The file uses
systemd environment assignments such as `DISCORD_BOT_TOKEN=...`, without `export`
or shell commands. Edit it privately; never place the token in TOML, command-line
arguments, the unit, screenshots or Git. The service manager reads this optional
file before switching to the unprivileged account. The configured bot-token
variable is removed from the browser subprocess environment.

Install the [delivery override](../deploy/ticket-watch-map.send-discord.conf.example)
only when real messages to the configured channel are intended:

```bash
sudo install -d -m 0755 /etc/systemd/system/ticket-watch-map.service.d
sudo install -o root -g root -m 0644 \
  deploy/ticket-watch-map.send-discord.conf.example \
  /etc/systemd/system/ticket-watch-map.service.d/20-discord.conf
sudo systemctl daemon-reload
sudo systemctl restart ticket-watch-map.service
```

The override adds `--send-discord`. Missing credentials fail before browser or
state work. Remove the override, reload systemd and restart to return to preview.
Preview does not acknowledge pending messages; still-current messages for an
unchanged destination can become eligible for delivery. Changing the destination
cancels its previous pending messages and resets the watch baseline. Discord
delivery still needs its own real-channel validation; `map-health` cannot prove
that a user received a notification.

## Stop, recover and upgrade

`systemctl stop ticket-watch-map.service`, SIGTERM and Ctrl-C use the same cleanup
path. The runner cancels outstanding captures, stops the worker and releases its
database lock. The unit signals the Rust parent first and allows 30 seconds for
cleanup before systemd's final termination of the full service control group. Do not manually kill
only Chromium while leaving its monitor active.

Failures restart after 30 seconds, with at most three startup attempts in five
minutes. Persistent configuration or credential errors need repair; after fixing
them, use `systemctl reset-failed ticket-watch-map.service` and start again.
Blocked pages are recorded with backoff and ordinarily keep the process alive.
Saved provider and Discord cooldowns survive restarts, including exhausted
Discord buckets reported with successful delivery responses.

Stop the service before replacing its binary or changing configuration. Keep
`/var/lib/ticket-watch-map` across upgrades; it contains history, nonce-bearing
pending messages and pacing. Back up using SQLite's backup API, or copy the
database and any WAL files only after a clean stop. Keep backups private and
restore configuration and state consistently. Restoring an old outbox can repeat
an alert after Discord's limited deduplication window. Run health inspection
before deciding whether to resume delivery.

Only one map runner should own this database. The separately named inventory
service is not a shared website rate limiter; coordinate it and other clients
before polling the same host. Starting more instances does not improve the
website's source freshness or guarantee notification within one second.

## Validation performed

The [validation report](evidence/service-and-date-validation-2026-11-10.json)
records 327 passing Rust tests and a health check against earlier real scan state.
The old observation was correctly reported stale, and its unsent notification
was counted as expired by age without changing state. Formatting and Clippy pass.

The unit was parsed with systemd-analyze 257 in an isolated temporary validation
directory, with its executable path substituted for the local built binary and
minimal dependency fixtures. This validates the unit syntax, not installation,
host permissions or boot behavior. The subprocess regression starts the real CLI
with a synthetic hanging worker, sends SIGTERM or SIGINT, and checks successful exit,
lifecycle records, worker cleanup and released database locking. It requires
permission to signal local child processes. No service was started and no
Discord message was sent during these checks.
