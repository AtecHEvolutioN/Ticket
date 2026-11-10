# Inspect Czech map sections and seat prices

The current source can open a numbered section in the observed Czech map layout
and optionally hover one explicit resale seat. [Section scans](section-scans.md)
can discover resale seat candidates automatically and compare displayed prices
between runs. This is a read-only inspection:
section clicks change the map view; seat clicks, quantity changes, ticket searches
and reservations are not performed. The Android APK is unchanged.

Build the source and use a graphical desktop:

```bash
cargo build --release --locked

# Get section IDs from map.rendered_geometry.sections.
./target/release/ticket-watch map-inspect --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657

# Open a section and inspect explicit seat IDs, row labels and coordinates.
./target/release/ticket-watch map-inspect --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --section-id s_128

# Hover an ID found in that section's rendered seat records.
./target/release/ticket-watch map-inspect --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --section-id s_128 --seat-id GFIC2MJRGA5DCMR2GM
```

The example IDs were observed on 2026-11-10; they are not hardcoded in the
implementation and may stop being inspectable. The feature requires the observed
Czech numbered-section layout and currently supports the observed resale hover
card. It is not a claim that every Czech event, standard seat or international
layout is supported. Standing sections are excluded from section opening.

`--section-id` and `--seat-id` require a live capture; they cannot be combined with
`--html`. Offline HTML inspection can extract seat labels and coordinates from a
new browser capture, but does not manufacture a hover interaction or quote.
The command needs no API key, Discord token or watch configuration. It creates no
database unless `--save-layout` is explicitly supplied. Saved venue presets still
contain static map references and configured zones, not seat inventory.

## What is returned

`map.rendered_seats` contains explicit section IDs/names, row names, seat IDs and
labels, and coordinates in the SVG's view box. Circle centers are read directly;
the observed resale icon's circle center is calculated from its bounded transform.
Unrecognized compositions and conflicting or duplicate seat identities fail
inspection. Colors do not establish availability. The explicit `type="resale"`
attribute is a display classification, not a purchase confirmation.

The map renders nearby blocks as it zooms. Its `rendered_seat_count` counts those
DOM records, including seats that may be outside the clipped viewport or
unavailable. It is **not the number of tickets available**, a complete venue
capacity, or proof of physical adjacency. Coverage is `partial_rendered_dom`;
`viewport_coverage_verified`, `stock_verified` and `adjacency_verified` stay false.
Zoomed seat layers are kept separate from the overview/minimap geometry.

`section_inspection` reports the requested section and its rendered seat count.
With a successfully validated seat hover, `displayed_quote` contains the explicit
zone, section, row, seat, ticket type, restriction and per-ticket price. Prices use
integer source-currency minor units: `243800` means 2,438 CZK. The observed card
does not establish complete checkout fees or a purchasable group, so
`fees_complete` is false and `available_quantity` is null. The quote is not sent to
Discord or passed to the cart planner automatically.

The browser binds actions to the original main-map nodes, checks their visibility,
and clears any earlier tooltip before hovering. It checks every consumed tooltip
field, binds the result to the seat's section/row/number, and revalidates after
capturing the page. Changed documents, replaced nodes, hidden fields, stale cards,
access challenges and unsupported layouts stop inspection. It never bypasses a
queue or verification challenge. A successful observation does not guarantee the
seat remains purchasable afterward.

## Observed development check

On 2026-11-10, a normal Czech KoRn map session exposed section 110. With networking
disabled before the section click, opening it rendered seven nearby blocks,
113 rows and 2,150 seat records. Section 110 contained 376 of those records.
Hovering row 12, seat 3 displayed verified resale, restricted view, and
2,438 CZK per ticket. This verified display behavior without sending a seat
selection or reservation. Initial access was intermittent: the first probe's
identification load timed out; a later ordinary session completed.

The optimized command subsequently reproduced the same seat identity and price
in a fresh live session. The [sanitized validation record](evidence/seat-inspection-2026-11-10.json)
contains that result and the seat-layout fingerprint. Local verification passed
245 Rust tests and 66 browser tests, with none skipped, plus formatting, Clippy
and the optimized build.

These are historical observations of one layout. Remaining work includes standard
seat hover support, broader event coverage, fresh availability and fee validation,
complete group search, explicit user quantity, site-confirmed adjacency, and the
durable reservation flow described in [cart readiness](czech-carting.md).
