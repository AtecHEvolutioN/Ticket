# One native Czech cart attempt

`cart-attempt` connects the saved ticket preferences, a dedicated browser session
and the durable attempt journal to one **Find tickets** action. That action may
reserve tickets immediately. The command never pays, accepts non-adjacent seats,
solves verification, or retries the reservation.

This is an experimental source command, newer than the 0.7.0-test downloads.
Build current Rust source and install the browser dependencies first. The Android
APK is unchanged; export its preferences to use them with this command.

## Configure the attempt

Use `cart-catalog --preferences ticket-preferences.json` to inspect the current
configured categories and prices. `cart-prepare` remains a separate, offline
rehearsal: its network-fenced context cannot be resumed to submit tickets.

```bash
./target/release/ticket-watch cart-attempt --headed --execute \
  --url PUBLIC_CZECH_EVENT_URL \
  --event-start EXPECTED_RFC3339_PERFORMANCE_TIME \
  --category-id CATEGORY_ID --price-id PRICE_ID \
  --unit-price-minor CONFIGURED_CZK_PRICE_IN_MINOR_UNITS \
  --native-section-id NATIVE_AREA_ID \
  --preferences ticket-preferences.json \
  --attempt-id UNIQUE_APPLICATION_ATTEMPT_ID \
  --journal data/cart-attempts.sqlite \
  --evidence-dir data/cart-evidence \
  --observe-secs 120
```

The placeholders must come from the intended event's current configuration.
For example, `171200` means 1,712 CZK per ticket; it is a configured rollup, not
a verified complete group total. The loaded page must match the expected event,
performance instant, category, exact price and quantity. Changed or unrecognized
controls stop the attempt. Only the validated Czech `STANDARD` filter mechanism
is currently supported.

Without `--preferences`, the defaults are two tickets, required adjacency,
cheapest eligible group, CZK and no total cap. Saved defaults and complete event
overrides can change these settings. `--native-section-id` may be repeated or
omitted for all areas compatible with the selected price. Native area IDs are
not seat-section or row labels.

Configured prices do not prove remaining stock, exact seats, complete fees or
cheapest available inventory. The command selects the explicit configured choice;
it does not automatically try other prices or create competing holds. For KoRn,
the inspected restricted-view category was cheaper than ordinary seating. Type
filters must express any desired exclusion rather than silently skipping it.

## Dispatch and recovery

The retained browser applies the quantity and exact price/area controls, then
rechecks the same document immediately before dispatch. Its preflight expires
after five seconds. Rust persists the native request and dispatch marker before
sending the one-shot command to activate Find tickets. Both the journal and the
browser latch prevent another submission for that attempt.

Use the same journal for all attempts. A reused attempt ID returns the existing
record without opening another browser. Another active attempt for that event
blocks a new search, including after a timeout or restart. Creating a different
journal would lose this protection; it is not a recovery procedure.

After submission, the command only observes the same browser for the configured
period (120 seconds by default, up to 1,800). Ctrl+C during this observation ends
the wait. Browser closure does not release a provider hold or unlock the attempt.
The final output reports the durable state and whether browser closure succeeded.

```bash
./target/release/ticket-watch cart-journal --file data/cart-attempts.sqlite
./target/release/ticket-watch cart-journal --file data/cart-attempts.sqlite \
  --history-attempt UNIQUE_APPLICATION_ATTEMPT_ID
```

Checkout navigation, a selected-ticket counter and SDK success are insufficient
to report a confirmed hold. The initial native adapter deliberately records
uncertain outcomes while retaining private evidence for receipt validation.
Actual event/date, exact seats and quantity, affirmative adjacency, complete
fees, hold identity and expiry still need a validated native receipt contract.
Cheapest-group proof is a separate requirement. An empty page or elapsed local
timer cannot release an uncertain attempt.

A recognized visible native rejection is exposed separately as
`native_signal: "insufficient_tickets_reported"`. This means Ticketmaster reported
insufficient tickets for the selected quantity/type. It is not an event-wide
sold-out result, proof that every price is unavailable, or permission to unlock
the attempt. Fresh uncertain observations can retain missing event/date claims
while preserving the same attempt/session lineage; they stay locked and keep
their missing-evidence review reasons.

## Browser ownership and private evidence

The browser uses ordinary Ticketmaster UI and narrowly scoped checkout navigation
rules derived from [the reviewed public SDK](czech-checkout-source.md). Login,
verification, queue, non-adjacent warnings and unrecognized screens stop automated
actions. This command does not provide remote browser access or synchronize a
cart into Android. A headed browser is visible only on the display where it runs.

Private captures may contain checkout tokens and session information. They are
bounded and written with owner-only permissions under the evidence directory;
the default `data/` directory is excluded from Git. Keep this directory private.
Console and journal outputs use application correlation IDs and public event
identities, rather than raw checkout URLs or provider cart identifiers.

Automatic monitoring-to-cart dispatch, authenticated receipt extraction, usable
customer cart handoff and verified Discord hold notifications remain unfinished.
No exact-second availability or reservation latency guarantee is implied by this
command.

## Recorded native test

The [2026-11-10 report](evidence/native-cart-attempt-2026-11-10.json) records one
Find tickets action for two KoRn tickets in the lowest inspected configured
restricted-view category: 1,712 CZK each, native area `4P`. Ticketmaster returned
GraphQL `U402` with `reserveTickets: null` and displayed an insufficient-tickets
message for that selection. No cart, complete total or adjacent group was verified.
The attempt remains locked. Re-running its application ID returned the existing
journal without starting another browser or submitting another search.

The journal committed 1.434 seconds before the click attempt; the rejection body
was captured 1.830 seconds after the click attempt. These measurements exclude
browser startup/preparation and do not measure stock-change detection. The live
capture also exposed a passive reconciliation issue with missing event/date
claims; that was fixed with local regression tests, without replaying a native
reservation. A successful hold receipt and usable customer handoff remain untested.
