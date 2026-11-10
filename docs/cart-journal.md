# Durable cart-attempt journal

The journal is a foundation for a future Czech cart adapter. It records a plan
and its outcome across restarts; it performs no browser actions or requests.
There is no live reservation command connected to it.

Inspect an existing journal without creating or modifying it:

```bash
./target/release/ticket-watch cart-journal --file data/cart-attempts.sqlite
```

The Rust API is `cart_journal::CartJournal`. Its database is separate from both
inventory alerts and map observations. Unrelated database files are rejected.

## Reserving a fully quoted group

1. Call `begin` with an application-generated attempt ID and a fresh
   `CartPlanRequest`. The journal recomputes the planner decision. It requires an
   explicit quantity, matching expected/source performance instants,
   site-confirmed adjacency, complete CZK fees and a complete
   search that supports the cheapest group for that request. Displayed map prices
   do not qualify. Only one active attempt per event can exist in a journal.
2. Commit `mark_dispatch_started` immediately before any action that might reserve
   stock. Only the first caller receives `newly_started=true`; a duplicate call
   cannot justify sending the same action again. This value records ownership of
   a transition, not user authorization or verified source access.
3. Reconcile the actual native cart in the same application session. Record a
   matching `HoldReceipt`, or a `NoHoldReceipt` proving that the request finished
   without a hold. An empty cart while the request is pending is insufficient.
4. On a timeout or restart after dispatch, treat the attempt as
   `needs_reconciliation`. Do not retry the reservation blindly. A quote becoming
   stale does not release this guard. A confirmed hold can be marked expired
   only after its recorded hold expiry. A prepared but undispatched attempt can
   be explicitly abandoned without a reservation.

Receipts must match the attempt, event, performance instant, session, document and quote. Hold receipts
must also match the exact seats, quantity, total, currency and complete fees, and
have a plausible future expiry. Exact retries are idempotent; conflicting outcome
receipts are rejected. State changes and their audit entries commit together.

The planner takes `intent.expected_event_start` and `snapshot.event_start` as
RFC3339 timestamps with known explicit offsets. Equivalent offsets normalize to
the same UTC instant; fractional seconds remain exact. Date-only values, unknown
`-00:00` offsets, invalid dates and mismatched instants cannot qualify. Missing
dates permit only provisional offline diagnostics. New receipts must supply
`context.event_start` matching both dates in the persisted plan.

All of this validates **caller-supplied evidence**, not its authenticity. Reports
keep `reservation_authorized=false`, `source_authenticated=false` and `date_verified=false`, while
`event_start_claims_match` reports consistency of the supplied date claims. A
native receipt parser is still required before connecting the journal to real
cart actions.

Version 1 and 2 journals migrate to version 3 without rewriting historical evidence.
Undated prepared attempts may be abandoned but cannot dispatch. Existing confirmed
holds may expire by their recorded expiry. Undated uncertain attempts stay
blocked; the migration does not invent an expected date or unlock another attempt.

## A native search that may reserve

The Czech [Find tickets flow](czech-reservation-flow.md) can search and reserve in
one action. The journal now has a separate `native_search` workflow for this
sequence; it does not invent a complete quote before the site returns one.

1. `begin_search` records a `NativeSearchRequest`: explicit quantity, matching
   expected/source event starts, application session/document/configuration IDs,
   selected category and exact configured price filter, CZK currency and optional
   total ceiling. Both adjacent and cheapest requirements must be enabled.
   Configuration freshness is capped at five seconds. The configured unit price
   is a reference, not proof of stock, complete fees or the cheapest available group.
2. Commit `mark_search_dispatch_started` before Find tickets. The quoted and native
   workflows share the same event lock, so they cannot start competing holds.
   Only the first committed dispatch reports `newly_started=true`.
3. Supply the observed `NativeSearchOutcome` to `record_search_outcome`. The source
   form document and the outcome document have separate IDs because the native
   flow may navigate. Both remain bound to the same application session, attempt
   and configuration.
4. A matching hold must have the requested seats/quantity, affirmative adjacency,
   exact filters, complete total and an actual expiry. Classifying it as cheapest
   also requires complete **post-action** ranking evidence. The existing planner
   reevaluates that snapshot and its exact winning group. This never becomes a
   reconstructed pre-action quote.

Missing or mismatched claims are retained with explicit review reasons under
`needs_reconciliation`. Private checkout URLs are discarded instead of persisted
as event URLs. A valid matching hold with only unproven cheapest ranking can expire
at its confirmed deadline. A mismatched or unknown hold cannot release the event
using an unverified timer. An explicit terminal no-hold result can release a new
attempt only when its bindings and finality checks pass.

An outcome is immutable apart from exact idempotent replay. A recorded uncertain
or mismatched outcome cannot be overwritten with a generic no-hold claim; a
separate native reconciliation flow is still needed for those cases. An empty
cart, timeout, selection counter or expired configuration never proves release.

Inspection JSON includes `workflow`; native bindings appear under
`binding.request`. Old quoted binding fields retain their previous JSON shape.
Both workflows validate caller-supplied claims only. Neither is connected to a
live browser submission or an authenticated native receipt parser yet.

Use application correlation IDs or digests in journal records. Do not supply
cookies, authentication tokens, checkout URLs or private cart identifiers.

Tests cover concurrent writers, duplicate dispatch, restart recovery, stale and
incomplete plans, mismatched receipts, uncertain outcomes and transaction
rollback. These are synthetic consistency tests, not a live Ticketmaster hold.
