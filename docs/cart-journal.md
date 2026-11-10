# Durable cart-attempt journal

The journal records a plan and its outcome across restarts; it performs no browser
actions or requests itself. The experimental [native cart command](native-cart-attempt.md)
now commits its dispatch marker before one native Find tickets action. Native
receipt extraction is still incomplete, so that adapter retains uncertain outcomes.

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
   site-confirmed adjacency when required, complete CZK fees and a complete
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
native receipt parser is still required to accept an actual verified cart outcome.

Version 1, 2 and 3 journals migrate to version 4 without rewriting historical evidence.
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
   total ceiling. Cheapest ranking is required; adjacency defaults to required
   and can be explicitly disabled through the saved selection preferences.
   Category/type and section/row filters are persisted with the request.
   Configuration freshness is capped at five seconds. The configured unit price
   is a reference, not proof of stock, complete fees or the cheapest available group.
2. Commit `mark_search_dispatch_started` before Find tickets. The quoted and native
   workflows share the same event lock, so they cannot start competing holds.
   Only the first committed dispatch reports `newly_started=true`.
3. Supply the observed `NativeSearchOutcome` to `record_search_outcome`. The source
   form document and the outcome document have separate IDs because the native
   flow may navigate. Both remain bound to the same application session, attempt
   and configuration.
4. A matching hold must have the requested seats/quantity, affirmative adjacency when required,
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

The initial outcome remains immutable through `record_search_outcome`. Later
observations use the explicit reconciliation API below, preserving the original
receipt in an append-only history. An empty cart, timeout, selection counter or
expired configuration never proves release.

Inspection JSON includes `workflow`; native bindings appear under
`binding.request`. Old quoted binding fields retain their previous JSON shape.
Both workflows validate caller-supplied claims only. The native workflow is now
connected to one browser submission through `cart_controller`; an authenticated
native receipt parser is still needed to confirm a hold.

Use application correlation IDs or digests in journal records. Do not supply
cookies, authentication tokens, checkout URLs or private cart identifiers.

Tests cover concurrent writers, duplicate dispatch, restart recovery, stale and
incomplete plans, mismatched receipts, uncertain outcomes and transaction
rollback. These are synthetic consistency tests, not a live Ticketmaster hold.

## Recover an interrupted native attempt

`CartJournal::reconcile_search` accepts a `NativeSearchReconciliation` containing
an application-generated `reconciliation_id`, the inspected `native_outcome_hash`
as `expected_outcome_hash`, and a new native outcome. It does not submit another
ticket search. The evidence must remain bound to the original attempt, session,
source document, configuration and performance; the outcome document may have
changed as the native flow navigated.

The update checks the expected outcome hash and writes the new receipt, history
entry and state transition in one transaction. Only one concurrent recovery can
supersede a given outcome. Replaying the same reconciliation ID with identical
evidence returns the current record, even after later recovery steps; it never
rewinds history. Reusing an ID with different evidence is rejected.

Observations must be newer than the retained evidence/recording time and less
than five seconds old. An invalid clock on an earlier uncertain receipt cannot
prevent recovery forever: the persisted recording time supplies the minimum in
that case, while the invalid original receipt remains visible in history.

- An uncertain attempt with no observed hold may become terminal `NoHold` only
  with matching, fresh evidence that the request finished and no hold exists.
- Continuing `Uncertain` observations may still lack event/date claims. They
  must be fresh, newer and bound to the same attempt, session, source document
  and configuration. Concrete claims must match the request and cannot be
  erased or contradicted. Missing-evidence review reasons remain visible and
  the event stays locked; this exception does not relax any hold or release check.
- An observed hold cannot become a generic `NoHold` or `Uncertain` outcome.
  Recovery may fill missing evidence for the same hold, including its seating,
  fee or ranking evidence, but cannot change known seat identities, total, price,
  expiry or a concrete event/date claim. It cannot replace or renew a hold.
- Releasing an observed hold requires `NativeSearchOutcome::Released`, with its
  original hold correlation, a release correlation, explicit release and absence
  confirmation, and a terminal request. This retains the fact that a hold was
  observed instead of rewriting history to say no hold ever existed.

The unchanged-expiry requirement is conservative: a malformed or missing original
expiry cannot be replaced with a new timer through hold-to-hold recovery. An
explicit correlated release is required to resolve that case. Mismatched request
lineage also remains locked for review.

Inspect retained observations without creating or migrating a database:

```bash
./target/release/ticket-watch cart-journal --file data/cart-attempts.sqlite \
  --history-attempt APP_ATTEMPT_ID
```

Schema 4 adds `cart_search_outcomes`. Migration preserves the last stored native
receipt from schema 3 as its initial history entry; it does not invent earlier
observations. Read-only inspection of an older journal exposes that retained
receipt without writing a migration.

These APIs validate supplied evidence consistently. Native receipt extraction
and source authentication are still unfinished; a successful reconciliation does
not independently establish Ticketmaster stock, authorize a reservation or send
a Discord message.
