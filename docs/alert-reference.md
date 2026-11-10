# Discord alert reference from the user's screenshots

Recorded: 2026-11-09. Input: screenshots supplied in chat. This is a product/reference analysis, not independent validation of the pictured service. No links containing cart or payment-session identifiers were opened or copied into this repository.

Subsequent user clarification: the reference service scrapes websites. Adopt scraping as the implementation direction. The screenshots do not reveal the exact page requests, session requirements, or refresh cadence.

## What the reference shows

- Discord messages grouped into country/market channels, with an event name/date/location and event ID.
- KoRn alerts listing sections, ticket descriptions, prices/currencies, bracketed numbers, and links labeled ATC. The exact meaning of bracketed numbers must be validated before treating them as stock counts.
- A KoRn checkout page displaying two tickets, a row and adjacent seat numbers, a price summary, and a time limit. It is for the Krakow event inspected earlier.
- A separate message claiming tickets were carted for another event, including quantity and price.
- An interactive seat map for a Netherlands event with a standard-ticket color legend.

The screenshots provide a concrete output reference. They do not identify whether the service uses public page data, an authorized API, browser sessions, or another source. Message timestamps do not reveal when inventory first became available. Channel names do not prove inventory provenance, and an Official Platinum label alone does not establish resale inventory.

## First-version scope

Monitor chosen artists and locations and send Discord pings when matching inventory appears. Include detailed fields only when the source supports them. Cart creation, seat reservation, and checkout automation are separate features and are not added by treating these screenshots as an alert reference.

Country/market channels are a routing option; they do not mean all worldwide events are monitored. Watches remain artist/location selections. Multiple market adapters can share the Rust scheduler, state engine, and Discord dispatcher.

## Alert fields and evidence

| Field | Required behavior |
| --- | --- |
| Event | Name, date/time with timezone, venue, city/country, stable source event ID. |
| Inventory classification | Primary/resale/unknown, separately from pricing tier such as standard or platinum. |
| Location within venue | Section, row, and seats only when explicitly exposed by the source. |
| Quantity | Store both the number and its meaning: listing quantity, selectable quantity, reported remaining count, or unknown. A cart quantity is not total stock. |
| Price | Preserve source currency and price scope (per ticket or total). Record whether fees/taxes are included, excluded, or unknown; use decimal/minor-unit arithmetic. |
| Conversion | Optional display estimate with exchange-rate timestamp; never substitute it for the source-currency price. |
| Evidence time | Local observation timestamp, source timestamp if available, and signal scope. Do not label local receipt time as the inventory-release time. |
| Link | Canonical event URL by default. A documented stable offer link can be included if validated; private/session-bound checkout links are not reusable general links. |
| Ping | Only currently subscribed user/role mention targets; merge overlapping watches per destination. |

The Rust monitor now implements the sale-type field per offer and displayed
category; see [original tickets and resale](ticket-sale-types.md) for source
evidence, unknown cases and an offline Discord preview. Ticket descriptions such
as Official Platinum remain independent of that classification.

An alert should describe evidence, for example “matching ticket offer observed,” without implying that tickets have been reserved. If quantity/price/section filtering cannot be evaluated, show the unsupported filter instead of emitting a confirmed match.

## Offer-level change detection

An event-level available/unavailable flag would miss a new section or newly matching price while other tickets remain available. A detailed adapter therefore emits normalized offer observations as well as event health.

Prefer a stable upstream listing/offer identifier. If none exists, define a source-specific composite key using stable seat/section/type attributes, validate its behavior across snapshots, and document ambiguous cases. Price and quantity changes should not automatically invent a new offer identity. Do not add counts across rows that might represent the same seats or overlapping offers.

Apply the user's filters to each observation. A newly observed qualifying offer, a previously nonmatching offer becoming a match, or a valid unavailable-to-available transition can create a notification generation. Repeated identical observations do not. A fully rendered empty listing is only negative evidence if the adapter establishes that it is complete for the requested scope; pagination, partial loads, blocked requests, or missing selectors leave that scope unknown.

Persist meaningful offer transitions and outbox entries atomically. The occurrence key contains source event, inventory kind, stable offer key, and generation. Recompute active subscriptions before delivery; group simultaneous matches into one destination message where practical without an intentional delay on isolated urgent alerts.

## What must be established next

Use the reference to test an accessible source against the required fields: ordinary admission versus add-ons, primary versus resale, quantity semantics, fee scope, and event/offer identity. Validate repeated snapshots and real state changes before claiming restock detection. A screenshot of a checkout demonstrates a captured session state, not that a fresh monitor can reproduce access or sustain a given polling interval.

The immediate technical unknown is access to sufficiently detailed observations. Rust and the Discord message format are already settled design choices; neither explains or resolves that access requirement by itself.
