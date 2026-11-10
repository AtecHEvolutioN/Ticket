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

## Required sequence for the future adapter

1. Call `begin` with an application-generated attempt ID and a fresh
   `CartPlanRequest`. The journal recomputes the planner decision. It requires an
   explicit quantity, site-confirmed adjacency, complete CZK fees and a complete
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

Receipts must match the attempt, event, session, document and quote. Hold receipts
must also match the exact seats, quantity, total, currency and complete fees, and
have a plausible future expiry. Exact retries are idempotent; conflicting outcome
receipts are rejected. State changes and their audit entries commit together.

All of this validates **caller-supplied evidence**, not its authenticity. Reports
keep `reservation_authorized=false`. The current planner has no expected event
date field, so reports also keep `date_verified=false`; an explicitly reported
date or event substitution is rejected. A native receipt parser and date binding
are still required before connecting this journal to real cart actions.

Use application correlation IDs or digests in journal records. Do not supply
cookies, authentication tokens, checkout URLs or private cart identifiers.

Tests cover concurrent writers, duplicate dispatch, restart recovery, stale and
incomplete plans, mismatched receipts, uncertain outcomes and transaction
rollback. These are synthetic consistency tests, not a live Ticketmaster hold.
