# Rehearse a native Czech selection

`cart-prepare` connects saved ticket preferences to the Czech quantity and native
price/area controls. It loads the event, disables networking, applies the requested
quantity and exact filters, verifies the selected controls, and restores the form.
It never activates **Find tickets** or creates a reservation. This is a test of
the native form, not an availability search or a live cart command.

Build current Rust source with `cargo build --release --locked` first. These new
commands are newer than the 0.7.0-test downloads. The Android APK is unchanged.

First inspect current configured choices with your Android export:

```bash
./target/release/ticket-watch cart-catalog --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --preferences ticket-preferences.json
```

The added `preparation_candidates` report orders configured prices within your
category/type preferences and excludes configured subtotals already above your
total budget. A configured subtotal is the displayed per-ticket rollup multiplied
by the requested quantity. The separately reported handling fee does not establish
its charge scope or a complete checkout total.

Configured choices do not establish remaining stock, seat locations, adjacency
or the cheapest available group. For example, the inspected KoRn configuration
includes an `Omezený výhled` (restricted-view) category below the lowest `Sezení`
price. The report retains that cheaper category when your type filters permit it.
It does not silently equate “cheapest” with the cheapest standard seating category.

Using category, price and native area IDs from the current catalog:

```bash
./target/release/ticket-watch cart-prepare --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --preferences ticket-preferences.json \
  --category-id 2 --price-id 6 --native-section-id 4P
```

Those IDs are an example from the inspected configuration, not stable universal
Ticketmaster IDs. The command revalidates them against the loaded event. Without
`--preferences`, it uses the default document: two tickets, adjacent seats,
cheapest eligible group, CZK, no total cap and no type/seat filters. Export your
phone's choices to use edited defaults or an event override.

`--native-section-id` may be repeated. Omit it to select all configured areas
compatible with the selected price. The report distinguishes requested IDs from
`effective_section_ids`, since choosing a price can also change native area
selection. Native IDs such as `1P` and `4P` refer to floor/area filters. They are
not seat-section labels such as `110`, and no conversion between them is assumed.
Your actual section/row and adjacency requirements remain listed as pending until
exact seat-group evidence exists.

## Mapping and restoration

The rehearsal currently supports the validated `STANDARD` ticket-type mechanism.
Platinum and Collector use different native filtering/pricing contracts. Their
configured prices remain visible in candidate ranking, with native category
preparation marked unsupported; the command does not infer their semantics from
a similar-looking option.

The mapper requires a unique match between every observed price option and its
configured category title, price-level title and exact CZK amount. It does not use
option position or amount alone: the inspected page orders two equal-price levels
differently from its embedded metadata. Native area options require unique exact
configured labels. Missing or ambiguous mappings stop the rehearsal.

The browser verifies the combined selected price and area controls before
restoring their original state and zero quantity. It uses the same isolated,
network-fenced context as [cart-controls](cart-controls.md). A challenge, changed
document/configuration, unexpected modal or failed restoration prevents a
successful report. A failed rehearsal closes its isolated context.

The result includes current configured candidates, the exact observed control
mapping and explicit unverified group requirements. `reservation_authorized`,
`reservation_attempted`, stock, complete fees, adjacency and hold verification
remain false. The context is closed; the command cannot be resumed online to
submit an order. Native submission, receipt parsing and automatic monitoring-to-cart
dispatch remain separate unfinished work.

## Validated native results

The [sanitized validation report](evidence/native-preparation-2026-11-10.json)
records two successful live-page rehearsals on 2026-11-10:

| Event | Quantity | Configured price per ticket | Native area | Load and rehearsal |
| --- | ---: | ---: | --- | ---: |
| KoRn | 2 | 1,889 CZK, Sezení 5. kategorie | 4. POSCHODÍ | 21.516 s |
| Andrea Bocelli | 2 | 1,490 CZK, Základní 6. kategorie | 4. POSCHODÍ | 22.664 s |

Both runs verified the combined price/area selections, restored the native
filters and returned the quantity to zero. Networking was disabled before all
quantity and checkbox changes. No ticket search, reservation or Discord message
was submitted. These timings measure browser loading and form rehearsal, not
availability detection or carting latency.

Saved configuration checks also covered KoRn, Andrea Bocelli and Ross Noble.
They found 22, 37 and 2 configured candidates respectively for the default
quantity of two; those counts are price choices, not available tickets. The
checks are saved-capture parsing and do not establish current availability or
live form support for the third event.
