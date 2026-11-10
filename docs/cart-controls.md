# Inspect Czech quantity-dependent controls

`cart-controls` loads a public Czech event in an ordinary browser, then disables
networking before a local form demonstration. It reads the native price/section
filter choices for one unambiguous ticket category and demo quantity, closes the
panels and restores the quantity to zero. It never activates Find tickets,
selects seats or confirms a reservation.

First use `cart-catalog` to inspect category IDs. For the supported Czech layout:

```bash
cargo build --release --locked
./target/release/ticket-watch cart-catalog --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657
./target/release/ticket-watch cart-controls --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --category-id 2 --demo-quantity 2
```

The example quantity is explicitly a form demo; it is not a saved purchase
instruction. Choose an ID from the current catalog. The inspector rejects a
selected display category that merges several metadata ticket types. An unrelated
merged row may remain present only with quantity zero. Unknown layouts and access
blocks stop the inspection. This command does not bypass them or retry a blocked
page. It does not use the monitoring database, a Ticketmaster key or Discord.

The browser context is isolated from the monitor. It blocks service workers and
WebSocket connections from startup. Before changing the form, it blocks outgoing
HTTP routes, enters offline mode and cancels or waits for initial-load traffic
to end. A lost completion signal prevents the demo. Unexpected pages, document
changes, challenges, altered configuration or unrecognized controls also prevent
a successful report. This is ordinary browser network isolation, not an exported
login/session or a claim about arbitrary browser network protocols.

The JSON includes the final configuration catalog and a separate controls report:

- `demo_quantity` records the quantity used while inspecting filters.
- `initial_quantity` and `final_quantity` must both be zero.
- `panels` contain observed checkbox IDs, labels, checked/disabled state and
  whether a panel could be opened. The inspector does not select checkboxes.
- `form_restored=true` requires closed panels, the original filter state and zero
  quantity. A final capture and document check must still match.
- `purchase_quantity_configured`, `reservation_authorized`, stock, adjacency,
  complete-fee and hold verification remain false.

Native checkbox IDs are rendered DOM IDs, not configured price IDs. Labels and
category prices describe configuration; no available adjacent group or cheapest
purchasable total is inferred. A saved catalog does not establish a live offer.
The [reservation-flow investigation](czech-reservation-flow.md) explains why Find
tickets itself cannot be used as a read-only availability check.

Captures and inspection are bounded to the request deadline. Restore is required
for success; a failed demo closes its isolated context instead of leaving a live
selected form. There is no `--html` simulation option for this command. Offline
parser fixtures are tests and cannot stand in for a live capture.

## Validated native result

The [sanitized live result](evidence/native-controls-2026-11-10.json) records a
successful headed KoRn inspection on 2026-11-10. Category 2 (`Sezení`) with demo
quantity 2 exposed five price choices: 2,361; 2,243; 2,125; 2,125; and 1,889 CZK,
plus section filters `1. POSCHODÍ` and `4. POSCHODÍ`. Equal prices had different
category labels. These are configured choices, not available-ticket quotes.

The full capture took 10,184 ms; the offline controls phase took 3,012 ms. Both
panels closed and the quantity returned to zero. The original live mismatch came
from a description button nested in an unrelated Platinum title; a narrowly
matched parser fix and regression cover that observed shape.

All 343 Rust tests and 96 browser tests passed, with no browser skips. Tests cover
network fencing, WebSockets, unexpected pages, request completion, document and
metadata changes, ambiguous types, restoration and journal recovery constraints.
The native reservation submission and authenticated receipt adapter remain
unfinished. This inspector cannot be switched back online to submit an order.
