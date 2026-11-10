# Scheduled section scans and Discord price notifications

`map-run` repeatedly scans configured Czech map sections and queues changes in
displayed resale prices. It shares the existing Discord REST sender, while keeping
its history and queue separate from confirmed ticket inventory.

Start with the example configuration and inspect the exact message payload:

```bash
cargo build --release --locked
./target/release/ticket-watch --config config.map-monitor.example.toml validate
./target/release/ticket-watch --config config.map-monitor.example.toml map-run --once
./target/release/ticket-watch --config config.map-monitor.example.toml map-status
```

Without `--send-discord`, the runner previews messages in its output and does not
read the bot token or send messages. Previewing does not mark a notification as
delivered. To run continuously, omit `--once`; stop with Ctrl-C.

For delivery, set the Discord channel ID and desired user/role mention IDs in your
private configuration, supply `DISCORD_BOT_TOKEN` in the process environment, then
run with `map-run --send-discord`. The token stays out of configuration files,
browser subprocesses, saved state and message payloads. The bot needs access to
the configured channel and permission to send messages there.
Restart the runner after editing its configuration; it does not reload it while
running. A changed destination cancels old messages and establishes a fresh
baseline on the next successful scan.

The example uses the previously observed KoRn section. Replace its event URL,
section ID and name for another supported section. `map-inspect` lists section
IDs. Multiple enabled watches must have distinct event/section pairs.

## Notification meaning

Messages describe a displayed price observation, price change, or restriction
change. They carry the public event URL, section, row, seat, ticket type, displayed
per-ticket price and observation time. Quantity, complete checkout fees,
availability and adjacency remain unverified. A new observation is not a claim
that tickets just went on sale; no tickets are reserved by this monitor.

`notify_initial` controls whether the first baseline produces a notification.
It defaults to false; the example enables it so the preview has a useful initial
message. Later newly observed seats can produce observation messages. Price and
detail changes follow `notify_price_changes` and `notify_details_changes`.
Refreshing an old comparison baseline does not produce a price-change alert.
`notify_initial=true` also applies to a fresh watch baseline when one-off scans
have already populated its history. Previewing and later configuring a channel
does not replay messages for the previous destination.

## Scheduling and recovery

One runner owns the map database and reuses its browser process. Scans are
serialized, while Discord delivery can proceed independently of a slow browser
load. Watch intervals and host spacing both constrain scheduling. Missed polls
do not produce a catch-up burst. `--once` checks currently due watches and skips
those still subject to a saved interval or cooldown.

Blocks, unreadable pages and `Retry-After` responses preserve earlier price
history and update monitoring health. Cooldowns and next check times survive a
restart. An access failure never becomes a sold-out observation.

Price history and queued notifications commit together. Editing or disabling a
watch, or changing its Discord destination, cancels obsolete queued messages.
Delivery retries reuse a stable nonce. Expiry is based on the source observation
time, not the retry time. A permanent Discord error stops delivery for repair.
The queue retains at most 10,000 rows, pruning the oldest completed, cancelled or
expired entries. Pending messages are not discarded to make room. A full pending
queue stops a new scan's database commit so its price change is not silently lost.

`map-status` shows saved watch health, next check time, last successful scan,
pending-message counts and provider/Discord cooldown deadlines. The map database
is separate from `database`, which
belongs to the ordinary inventory monitor. Existing section-scan history files
are migrated when opened by the newer code.

These schedules are local to this runner. Coordinate other processes polling the
same website; this command is not a shared rate limiter for unrelated programs.
Polling, source refresh delays and access restrictions prevent an exact-second
notification guarantee. The APK is unchanged, and verified group selection and
the live cart adapter remain separate work.

## Validation on 2026-11-10

The [sanitized live preview](evidence/map-monitor-2026-11-10.json) found one
displayed resale quote in KoRn section 110: row 12, seat 3, restricted view,
2,438 CZK per ticket. Browser capture took 8,129 ms and produced one queued preview.
An immediate restart made no new capture and preserved the same message/nonce.
No Discord message was sent and no tickets were selected or reserved.

All 303 Rust tests passed, along with formatting, Clippy and the release build.
Local HTTP fixtures verify Discord retries, success-response cooldown persistence,
and delivery while a browser capture is stalled. Other regressions cover migration,
atomic history/outbox commits, stale messages, route edits, disabled watches and
cart-journal crash recovery. One successful live preview does not establish
sustained access, source refresh speed or production alert latency.
