# Local cart review and recovery

The optional operator mode keeps one native browser open for manual verification
and cart review. It uses the same saved preferences and one-dispatch journal as
`cart-attempt`. Run it from an interactive terminal on the Unix desktop where
Chromium will appear:

```bash
./target/release/ticket-watch cart-attempt --operator --headed --execute \
  --url PUBLIC_CZECH_EVENT_URL \
  --event-start EXPECTED_RFC3339_PERFORMANCE_TIME \
  --category-id CATEGORY_ID --price-id PRICE_ID \
  --unit-price-minor CONFIGURED_CZK_PRICE_IN_MINOR_UNITS \
  --native-section-id NATIVE_AREA_ID \
  --preferences ticket-preferences.json \
  --attempt-id UNIQUE_APPLICATION_ATTEMPT_ID \
  --journal data/cart-attempts.sqlite \
  --operator-timeout-secs 600
```

Build the current source first. This mode is newer than the 0.7.0-test downloads;
the Android APK is unchanged. Use the event's current configured choices, as
described in [the native attempt guide](native-cart-attempt.md).

## Terminal commands

The browser prepares the saved selection and continues into the one authorized
Find tickets action when ready. If a recognized verification screen interrupts
preparation, complete it yourself in that browser and type a command in the
terminal:

| Command | Before dispatch | After dispatch |
| --- | --- | --- |
| `resume` | Recheck the current page and selection, then continue the authorized attempt if valid | Observe the current session |
| `status` | Show the retained preparation state | Observe or show the retained result |
| `close` | End the browser session | End the browser session without clearing the journal lock |

An acknowledgement is not evidence that verification succeeded. Resume checks
the actual page, response status, event identity, date and native controls. It
does not reload or open another event. Initial verification can finish in a new
document within the same browser before the first preflight exists. Once a
preflight is established, its document and configuration must remain consistent.
Partial or changed selections are rejected rather than reset automatically.

Let the bot activate Find tickets in this session. The reviewed Find control is
blocked before authorization. After the journal records dispatch, a single-use
permission admits at most one activation; a simultaneous manual click cannot
produce a second bot click reaching the native handler. The final verified form
is sealed before that permission is armed, and intervening ticket-control changes
invalidate it. Unexpected native reservation requests disable automatic
submission. No CAPTCHA solver or automatic acceptance of non-adjacent seats is
connected. Resuming after dispatch never starts another reservation.

The default operator window is ten minutes; the CLI accepts 30–1,800 seconds.
The deadline includes preparation and manual waiting. `--observe-secs` belongs to
the separate timed mode and cannot be combined with `--operator`. The browser
stays open for review when observation reaches a terminal state. After transport
loss the operator remains open, but cannot confirm that the browser is still
running. `close`, terminal EOF, Ctrl+C, SIGTERM or the deadline end the session.
Cleanup is bounded, and `browser_closed=false` reports unconfirmed cleanup.
Closing a browser neither releases provider tickets nor resolves an uncertain
attempt.

Status events are written to stderr as JSON lines; the final report is written
to stdout. `dispatch_recorded` means the durable dispatch marker exists, not
that the provider confirmed a cart. No terminal command contents, credentials,
raw browser errors or checkout tokens are included in these status events.

If the local clock moves behind the last journal update or becomes invalid,
observation reports `native_clock_needs_review` and preserves the existing
attempt and event lock. It cannot accept a future receipt by advancing the local
time artificially. After correcting the clock, a fresh passive observation in
the retained session can continue reconciliation without another Find action.

This is local browser ownership. Android synchronization, remote browser access,
portable checkout links and verified cart receipts remain separate unfinished
work. The navigation policy still covers only the reviewed Czech routes.

## Inspect an interrupted attempt

```bash
./target/release/ticket-watch cart-recovery --file data/cart-attempts.sqlite
./target/release/ticket-watch cart-recovery --file data/cart-attempts.sqlite \
  --attempt-id APPLICATION_ATTEMPT_ID
```

This reads the existing journal without starting a browser, migrating the database,
expiring a hold or clearing an event lock. It reports the retained state, evidence
age, current event-lock owner and applicable recovery actions. Inspecting an older
terminal attempt still identifies a newer active attempt for the same event.
Browser liveness remains unknown: a database record cannot prove that its browser
still exists. A new browser cannot reconstruct the original session from these
records.

An elapsed recorded hold deadline is reported as eligible for the existing
journal's expiry rules only when the retained evidence qualifies. Inspection does
not execute expiry or independently prove provider-side release. Unknown or
mismatched holds do not acquire a guessed expiry.

Recovery also checks the persisted timeline: creation, dispatch, receipt
observation (when present), latest update and inspection time must be in order.
Invalid or inconsistent timestamps produce `journal_clock_needs_review`, retain
the evidence and existing lock state, and suppress expiry or fresh-intent
recommendations. Inspection does not repair the journal automatically.

For a plan that was recorded but **never dispatched**, the explicit cleanup command
is:

```bash
./target/release/ticket-watch cart-abandon-prepared \
  --file data/cart-attempts.sqlite --attempt-id APPLICATION_ATTEMPT_ID
```

The transaction rechecks that dispatch never started. A concurrent dispatch wins
or abandonment wins; both cannot succeed. Missing, empty or unrelated journals
and inconsistent target attempts are rejected. This command cannot abandon an uncertain native attempt,
release tickets, or authorize another search.

The recorded KoRn attempt remains `needs_reconciliation`. Its insufficient-tickets
message is a selection-specific diagnostic; it does not qualify for this cleanup
command. The original live journal has not been rewritten or unlocked.

## Passive request evidence

The native session now arms a bounded request ledger immediately before the
attempted Find action. Application request IDs connect request, response and
completion observations. The ledger records timing, the originating dispatch
document ID, recognized operation shape and allowlisted selection fields. That
document ID does not independently prove each request's initiator. Known requests
that started before arming are excluded; unavailable timing remains explicit.

The four reviewed SDK query documents have exact normalized fingerprints. Changed
queries, aliases, additional operations and unsupported variable shapes remain
unrecognized or partial. HTTP success is separate from GraphQL errors, polling,
provisional responses and checkout-handoff shapes. These are diagnostic
associations with the dispatch window, not proof of causality, complete capture,
adjacency, a confirmed hold, or absence of a hold.

Raw request bodies, authorization values, query tokens and provider cart/session
identifiers are excluded from the ledger. Observed `acceptSplit` is retained as
a request flag; it never proves seat adjacency. The ledger remains private with
the other bounded session evidence and does not change journal eligibility.

Document snapshots omit sign-in pages and strip form values from a detached copy
of the page. Raw HTML responses and unrelated account/payment API responses are
not retained. Reviewed reservation JSON responses can still contain private
cart/session details; all session evidence stays outside Git in the private
evidence directory.
