# Scan displayed resale prices

`map-scan` automatically discovers resale seat candidates in one selected Czech
numbered section and reads supported hover cards in a single browser session.
It opens the section once and hovers a bounded number of seats. It never clicks
a seat, changes ticket quantities, searches for a group or creates a reservation.
This source feature is newer than the Android APK.

```bash
cargo build --release --locked

# One scan; no database or Discord messages.
./target/release/ticket-watch map-scan --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --section-id s_128 --limit 20

# Remember observations and compare with later runs.
./target/release/ticket-watch map-scan --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --section-id s_128 --limit 20 --state data/seat-prices.sqlite
```

Section IDs come from `map-inspect`'s overview geometry; see
[seat inspection](seat-inspection.md). No individual seat ID, API key, Discord
token or watch configuration is needed. The example section ID is an observed
KoRn section, not a hardcoded target. The command supports the observed Czech
resale layout; it does not establish support for every event or standard ticket.

## Bounds and coverage

The limit defaults to 20 and must be between 1 and 50. Discovery is bounded to
2,000 resale candidates. Candidates are ordered by seat ID, and only supported
candidates painted within the current viewport are eligible for hovering. The
section scan has a bounded time budget inside the command's overall browser
deadline. It does not pan through the entire venue or claim a cheapest group.
The normal initial identification navigation may take up to 15 seconds within
the existing 45-second request deadline. Retry-After cooldowns remain enforced;
incomplete identification still stops the command without updating history.

The report separates total DOM candidates, visible candidates, attempted hovers,
successful price observations and unattempted candidates. A fixed stop reason
distinguishes exhausting visible candidates, the seat limit and the time budget.
Missing or unreadable tooltips stay unknown. An empty result does not mean sold
out. Candidate counts are not remaining stock or purchasable quantities.

Each result has its own observation timestamp. Earlier prices are observations
from earlier hovers, not an atomic inventory snapshot or a promise they remain
current when the scan ends. Original document, map, section and candidate nodes
are checked throughout and after capture; identity changes, replaced nodes,
access challenges or modal interference invalidate the scan. Supported tooltip
fields must be visible and match the hovered seat.

Validated prices retain their currency and per-ticket basis. Complete checkout
fees, available quantity, stock and adjacency remain unverified. Coverage is
always partial. These observations are kept separate from the monitor's
confirmed inventory and notification outbox.

## Optional observation history

`--state` opts into a separate SQLite history. Changes and the updated comparison
baseline commit in one transaction. Failed validation does not create history;
a failed transaction does not partially update the baseline. Without this option,
the command writes no database.
Use a dedicated history file: unrelated application databases are refused.
History is bounded to 10,000 observed seat identities per event/section; reaching
that limit fails atomically rather than discarding prior observations.

The comparison reports first observations, price changes, restriction/detail
changes and refreshed baselines. A first observation is not a restock. Missing
seats and failed hovers never delete earlier observations or mark tickets sold
out. Comparison baselines older than 15 minutes are refreshed rather than used
to announce a price change. Replaying the exact same scan is idempotent;
conflicting or overlapping older observations are rejected.

Run the command again to obtain another scan and its changes. This is a one-pass
command: it does not schedule polling, send Discord messages, or initiate carting.
Use the [section monitor](map-monitor.md) for scheduling and explicit Discord
delivery of displayed-price observations. Fresh purchasability/group validation
is still required before any reservation flow. No exact-second delivery guarantee
is established.

## Live validation

On 2026-11-10, the optimized command scanned KoRn section 110 without a seat ID.
It automatically found one supported resale candidate and read section 110,
row 12, seat 3: restricted view, 2,438 CZK per ticket. The first successful run
saved a first-observation record. A second process read the same price and
returned no changes, confirming that the persisted comparison survived restart.

The two browser captures took 8,023 ms and 8,193 ms; the section scan portions
took 1,673 ms and 1,615 ms. These are individual observations including their
respective page-loading and UI costs, not polling or notification guarantees.
An earlier attempt stopped during identification with HTTP 401 and wrote no
history. The longer bounded grace was then tested with a local delayed-navigation
fixture; successful live access still does not prove continuous access.

The [sanitized evidence](evidence/section-scan-2026-11-10.json) records these checks.
Local verification passed 262 Rust tests and 80 browser tests with none skipped,
plus formatting, Clippy and the optimized build. No Discord message or reservation
was sent.
