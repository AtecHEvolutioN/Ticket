# Czech ticket selection and cart readiness

The monitor can inspect event pages. The new source commands below add a
read-only Czech price catalog and an offline adjacent-group planner. They do not
reserve tickets, run automatically from a watch, or change the Android APK.

## Inspect the configured selection choices

Build the current source, then run on a graphical desktop:

```bash
./target/release/ticket-watch cart-catalog --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657
```

For offline inspection, replace `--headed` with `--html captures/event.html`.
This needs no configuration file, API key, database, or Discord token. Saved HTML
has unverified transport and age; its embedded event identity must still match.
The live command requires a successful, current event document from the browser.
Access blocks stop inspection instead of producing prices from an error page.

The output distinguishes configured price choices, category and section IDs,
purchase limits, and the currently selected count. It explicitly reports
`configuration_only`: none of these establishes remaining stock, adjacent seats,
or a reservation. Amounts use integer minor units: 188900 means 1,889 CZK.
Each configured price's `total_minor` combines its per-ticket face value and
listed service/upsell charges; it is not a verified all-in group quote.
Handling fees are reported separately. This catalog covers primary configuration;
recognized resale listing rows are counted separately and excluded from it.
A price level called “Platinum” is not automatically the same
as the site's official Platinum ticket type. Duplicate rendered titles can group
multiple metadata types; their shared quantity control is counted once.

## Evaluate an adjacent group

```bash
./target/release/ticket-watch cart-plan quotes.json
```

This command evaluates a supplied JSON request offline. It does not fetch quotes,
open a browser, write monitoring state, or create a cart. Its output is a decision
about the supplied claims, not proof that those claims came from Ticketmaster.
The Rust entry point is `cart::plan_json`; the typed request is `CartPlanRequest`.

The request must specify the quantity explicitly. A watch's minimum quantity is
an alert filter and cannot supply this purchase instruction. Eligible groups
need that exact number of unique seats, explicit site-confirmed adjacency, a
complete group total in CZK, and fresh matching event/session/document bindings.
Standing tickets and consecutive-looking seat numbers do not establish adjacency.
Unknown fees, stale quotes, category ranges, and quantity limits are insufficient.

The cheapest eligible total can be called cheapest only within a complete search
for the requested quantity. Partial or unresolved search coverage produces at
most a provisional cheapest-observed group. Even a successful plan does not
authorize the reservation step. An optional maximum total can constrain selection;
the user's current preference is cheapest, without a specified cap.

## Browser evidence still needed

[Section and seat inspection](seat-inspection.md) now reads explicit rendered
seat identities and an observed resale hover price without selecting the seat.
It keeps quantities, complete fees and adjacency unverified and does not feed
those display observations into the reservation planner.
The [section scanner](section-scans.md) can now discover resale seat IDs and
record displayed-price changes automatically within a selected section. Its
partial observations still do not establish a purchasable adjacent group.

Captured Czech pages for KoRn, Ross Noble, and Andrea Bocelli share the category
stepper and `findTicketsBtn` controls. Price and section filters are disabled until
a ticket type and quantity are chosen. The bundled Czech translations include a
non-adjacent-seat warning and hold-expiry text; translations alone prove neither
that a warning is currently displayed nor that a hold exists.

The first live control-inspection attempt on 2026-11-09 timed out during the
website's initial identification navigation, before changing any quantity.
A subsequent headed session reached the event. With network access then disabled,
a **demo quantity of one** enabled the native price and section filters. This was
local form research, not the user's purchase quantity or a ticket search.
The seating price filter displayed five choices (two at the same price):
2,361; 2,243; 2,125; 2,125; and 1,889 CZK. The section filter displayed
`1. POSCHODÍ` and `4. POSCHODÍ`. No `Najít vstupenky` action was submitted.
Opening a native filter changes the document's UI/fragment state; a future runner
must close it and revalidate the event before interpreting results.

The [sanitized evidence](evidence/czech-cart-controls-2026-11-09.json) records
the successful offline UI inspection and three saved-capture catalog checks.
Raw HTML and session data remain private. Earlier headed access evidence is in
[the access diagnosis](czech-access.md). Access remains intermittent; neither
these observations nor the 198 passing Rust tests establish unattended production
readiness. Format checks and Clippy also passed for this source change.

The next browser adapter needs a persistent session separate from the monitor's
short-lived page checks. In that session, it must:

1. Apply the explicitly requested quantity and validate the native exact-price
   filter and search order. Category minima overlap and cannot prove the cheapest
   available group.
2. Determine whether the native search itself creates a hold. Record an attempt
   durably **before** the first action that can reserve stock.
3. Reject non-adjacent results and re-read prices and availability after any
   [manual verification pause](manual-intervention.md). No solver is connected.
4. Verify the resulting cart's event, quantity, seat grouping, complete price and
   actual hold/expiry. A selection counter is not a hold confirmation.
5. Treat a timeout after submission as an uncertain outcome. Reconcile the same
   session before another attempt; do not blindly retry, duplicate or renew holds.
6. Notify the configured Discord destination after verified availability or a
   verified hold, using public event links. Payment remains with the user.

The cart runner needs narrowly validated navigation rules for observed Czech cart
routes; the existing monitor's public-event policy remains in place. Its durable
attempt journal must be separate from alert deduplication. Session ownership and
manual pauses must not stall checks for unrelated watched events.

Before production, validate this flow on more Czech layouts, exercise crash and
timeout recovery, and measure successful-check age, quote-to-action time, verified
hold time, and Discord acceptance latency. Source freshness, access restrictions,
and polling cadence prevent a guarantee of notification in the exact second stock
changes. The requested ticket quantity is still needed for the first live hold.
