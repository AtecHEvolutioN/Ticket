# Production roadmap

Status: **10 November 2026**. Finish the monitoring and cart workflow first,
validate Discord delivery when the user is home, then develop the MCP research
experiment. This roadmap does not enable purchases, resale listings or repricing.

## Implemented

- Rust artist/location subscriptions, Discovery catalog refresh, website adapters,
  persistent observations/outbox, deduplication, pacing and saved cooldowns.
  Displayed prices, listing quantities and unknown inventory remain distinct.
  The [coverage report](coverage-2026-11-10.md) records the tested markets and gaps.
- Czech section/seat inspection and displayed-price monitoring, service templates,
  graceful shutdown and freshness checks. Production installation and live Discord
  delivery have not been verified.
- [Discord setup tools](discord-setup.md): offline installation/configuration
  report, explicit standalone test message, and a Vercel HTTP command app with
  signed `/ticket ping` and `/ticket help`, server/operator restrictions and
  ephemeral replies. Command health is separate from watcher/delivery health;
  remote watch/settings controls and the persistent host remain to be connected.
- Configurable ticket preferences: two adjacent tickets and cheapest eligible
  group by default. Android saves and exports preferences. A separate revisioned
  server registry and [read-only trigger preview](monitor-trigger-preview.md)
  evaluate stored observations against saved settings; phone synchronization,
  durable triggers and automatic dispatch remain unfinished.
- One experimental Czech native cart attempt, durable dispatch/event locks,
  local operator resume/review and recovery inspection. A successful hold receipt
  and usable customer checkout handoff remain unverified.
- Netherlands overview maps and draft venue presets. Dutch section interaction,
  individual seat inspection and carting are not implemented. The Android APK
  remains 0.7.0-test and does not contain the newer server work.

## Remaining bot milestones

| Milestone | Work to finish | Acceptance evidence |
| --- | --- | --- |
| Discord setup at home | Configure the bot token privately, destination channel and explicit user/role mentions. Connect watch, preference, status and pause controls to persisted server settings. | A controlled notification reaches the intended channel and mention; record source observation, application message ID and Discord acknowledgement. Exercise restart, stale-message expiry, delivery errors and cooldown recovery. Report ambiguous delivery honestly: Discord nonces do not guarantee exactly-once delivery indefinitely. |
| Reliable availability and price alerts | Show displayed price changes separately from qualifying available offers. Carry currency, per-ticket/group basis, fees, quantity meaning and source age through every alert. Add timestamps for request, capture, parse, outbox, dispatch and acknowledgement. | Replays and accessible live samples distinguish restock, changed price, schedule, presale, partial listings, empty views and blocked access. Publish p50/p95/p99 pipeline latency and observation gaps. Measure stock-change-to-alert latency only where the change time is known; otherwise label the measured interval. A one-second target remains a measured objective, not a guarantee. |
| Verified Czech hold and manual handoff | Implement receipt extraction tied to the same event/performance, browser, document and attempt. Verify quantity, ticket type, exact seats, source-backed adjacency, complete total/fees/currency and provider hold expiry. Keep the original browser available for review and manual checkout. | One successful bounded hold matches the configured preferences and is reviewable before expiry; fixtures also cover mismatch, partial response, challenge, timeout and release. A selected counter, redirect or HTTP success alone never reports a confirmed cart. Discord status includes public event details and application IDs rather than checkout tokens. |
| Preferences → monitoring → cart | Resolve versioned global/per-event preferences for every trigger. Persist a durable trigger and stable attempt ID; have the cart worker acquire the existing event lock and record dispatch before acting. Recheck source freshness, selected quantity, filters and price immediately before submission. | An eligible transition creates at most one reservation attempt across duplicate observations, concurrent workers, restart and ambiguous responses. Changed preferences invalidate stale work. Default quantity remains two and is editable. Cheapest means the cheapest proven eligible group; configured category ranges cannot establish that claim or satisfy an unknown fee-inclusive budget. |
| Netherlands section and seat support | Extend the overview work with separately validated Dutch section-opening and seat-hover adapters, then evaluate scheduled seat-price scans. Bind every observation to the current event and layout fingerprint. | Multiple Dutch venues and layouts produce matching section/row/seat/price evidence or an explicit unsupported result. Tests preserve the no-selection boundary. Presets supply geometry references only; colors and section counts never become stock or adjacency evidence. Dutch carting needs its own validated native flow. |
| Deployment and recovery | Install one supervised instance on the chosen host, coordinate host pacing across inventory/map workers, protect secrets and state, and provide pause/stop controls. Validate backup/restore, upgrades, browser cleanup and operational alerts. | A sustained soak run records source freshness, blocks, delivery status and resource use; a 24-hour run is the initial check. Restart and restore exercises preserve journal locks, pending-message identity and cooldowns. Health reports distinguish a running process from fresh source data and successful delivery. Build and verify updated downloadable artifacts after the source workflow is accepted. |

The existing KoRn attempt is still `needs_reconciliation`: Ticketmaster reported
insufficient tickets for its selection, but neither a hold nor absence of a hold
was verified. Keep its event lock. Resolve it through correlated terminal/release
evidence accepted by the journal; elapsed time, a new browser or a different
journal are not substitutes. See [native attempts](native-cart-attempt.md) and
[operator recovery](cart-operator.md).

The alert/operations work can progress while the receipt contract is being
validated. Automatic monitor-triggered carting depends on that contract and the
existing duplicate-attempt protection. Android synchronization requires a
versioned server interface and roundtrip tests; local phone settings currently
do not change the running bot. Operational setup is described in
[running the monitor](running.md) and [map service setup](map-service.md).

## Future MCP research and trading experiment

**MCP research, purchasing, resale management and repricing are not implemented.**
Begin with read-only tools exposing selected events, source observations,
preferences and candidate quotes. Record each tool result's timestamp, source,
currency, fee scope and uncertainty. Keep account credentials and browser session
tokens outside tool results. Research can rank opportunities, but an asking price
or popularity signal is not evidence of a completed sale or profitable trade.

1. **Paper trading first.** Add a separate simulated ledger for candidate entries,
   hypothetical purchases, exits and unfilled/expired opportunities. Record fill
   assumptions and distinguish observed quotes from inferred prices. Replay
   stale, unavailable and fee-unknown cases without turning them into simulated
   guaranteed fills. Compare estimated margins with actual later observations;
   report sample size, unsold exposure and uncertainty.
2. **Define the financial limits.** Persist configurable per-order, daily and
   total budgets; per-event and overall ticket exposure; concurrent cart-hold
   limits; acquisition and resale fees; minimum net margin/price floor; maximum
   inventory holding period and sale deadline. Keep limits per currency unless
   a recorded exchange-rate policy is supplied. Uncertain reservations consume
   the applicable exposure allowance until reconciled. Provider hold expiry and
   the strategy's inventory holding period are separate concepts.
3. **Add supported transaction flows.** After paper-trading evidence and the bot
   receipt work, implement one supported purchase/resale market at a time. Use
   legitimate flows for owned, transferable tickets within the provider's limits.
   Verify purchase confirmation, ticket ownership, listing state, transfers and
   settlement. Persist an operation ID before each mutation; reconcile an
   ambiguous outcome before retrying. Preserve manual verification/checkout
   handoff wherever the supported flow requires it.
4. **Schedule daily repricing.** Use a configured timezone and current comparable
   listings, known fees, remaining exposure and the saved price floor. Produce
   proposed changes first. For executable changes, reread listing ownership,
   sale/transfer status and current price before applying one idempotent revision.
   Record the old/new price and reason; skip stale or unsupported listings and
   stop repricing sold or transferred tickets.
5. **Account for realized results.** Track acquisition cost including buying fees,
   selling fees, refunds, chargebacks, transfer/fulfillment costs and settlement.
   Realized net profit is settled proceeds after selling fees/refunds minus
   allocated acquisition cost and remaining costs/taxes not already included.
   Keep unsold inventory and estimated margins separate; a listing markup is not
   realized profit. Daily reports reconcile holdings, cash, exposure, repricing
   actions and completed sales without double-counting fees.

The MCP phase is ready for real execution only when these limits, receipts,
reconciliation and accounting checks work together. No profitability claim or
new live transaction is implied by adding the research interface.
