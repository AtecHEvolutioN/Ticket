# Original tickets and resale

The Rust monitor now records `inventory_kind` separately from the provider's
ticket description. Discord labels every observed offer with its own **Sale type**:

| Sale type | Meaning |
| --- | --- |
| Original (primary sale) | The reviewed source explicitly identifies a primary sale. This does not mean face value or the lowest price. |
| Resale | The source explicitly identifies a resale listing. Its original ticket description may still say Standard, VIP or Official Platinum. |
| Unknown | The observed source does not establish the sale channel, or its sale-channel signals conflict. |

An event can contain several sale types in the same alert. Classification never
comes from the event name, price, seat color, `onsale` metadata or a ticket
description alone. **Official Platinum is a ticket/pricing description, not
proof of either primary sale or resale.** The original source wording remains
available alongside the normalized classification.

## Evidence used

- Recognized regional resale-listing markers identify resale independently of
  the listing's ticket description.
- Reviewed global Ticketmaster price cards have explicit primary/resale
  component markers. Their classifications apply to displayed categories;
  these cards still do not establish purchasable stock or quantity.
- Validated Czech resale seat tooltips identify resale displayed-price
  observations. Availability and complete fees remain unverified.
- Custom verified selector profiles can classify complete, explicit labels
  extracted by `inventory_selector`. Other labels remain unknown.
- Ordinary regional categories and the reviewed Italian price layout remain
  unknown where no reliable sale-channel evidence is available.

`inventory_kind` is exported as `primary`, `resale` or `unknown` on offers and
displayed categories. Read-only trigger previews also retain this field without
turning it into reservation authorization. This change does not add a sale-type
purchase filter or modify ticket selection.

Older saved observations and pending alerts without the field load as unknown.
They are not guessed from old free text, and no database migration is required. New observations can carry
fresh classification without generating an extra restock ping merely because
unknown became known. A confirmed primary-to-resale or resale-to-primary change
is an offer change when `notify_offer_changes` is enabled, and conflicting old
classifications are removed from pending alerts.

## Offline example

After building the current source, this local replay shows one original offer,
one resale offer described as Official Platinum, and one unclassified VIP offer:

```bash
./target/release/ticket-watch --config examples/offline-demo.toml replay --watch demo \
  tests/fixtures/synthetic-unavailable.html tests/fixtures/synthetic-mixed-sales.html
```

The prices and tickets in this example are synthetic. Replay sends no Discord
messages and uses an in-memory database. The published Android APK and 0.7.0-test
Linux download predate this server change; no new app binary is included here.

Verification also replayed 21 previously saved event captures without contacting
Ticketmaster. Availability and row counts stayed unchanged. Two observed offers
were explicitly resale; 96 displayed categories included 38 resale, 3 primary
and 55 unknown. Those categorized price cards still produced no available
offers. These are historical parser checks, not current ticket availability.
