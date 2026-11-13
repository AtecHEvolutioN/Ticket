# Netherlands seat validation — 13 November 2026

**Two live Dutch seat quotes were validated**, using ordinary headed Chromium
followed by the Rust event/seat validator:

| Event / venue | Displayed provenance | Seat | Observed price |
| --- | --- | --- | --- |
| Monster Jam / GelreDome | Explicit primary circle; Presale · Zitplaats | Section 101, row 11, seat 213 | EUR89.95 per ticket |
| New Wave / Ziggo Dome | Explicit resale icon; Verified Resale Tickets | Section 209, row 8, seat 292 | EUR73.90 per ticket |

These are asking-price observations, with unknown stock and complete checkout
fees. The presale label does not establish eligibility. No seats were selected
and no reservation was submitted. See the
[sanitized evidence](evidence/netherlands-seat-validation-2026-11-13.json) for
timestamps, source hashes and normalized results.

Three earlier **headless** reads returned HTTP403 (Monster Jam, 10cc and Gypsy
Jazz Festival). Ordinary headed access subsequently worked at GelreDome and
Ziggo Dome. An intermediate identification response also needed its normal
navigation to finish. These results do not prove stable unattended access or
explain every403; no fingerprint changes or protection bypass was used.

## What the source now implements

The read-only section inspector now supports the **observed NL layout**, alongside
the previously observed Czech layout. English/Dutch region and translation
strings, NL/EUR metadata and numbered-section aliases bind every action to the
current event. The source-grounded live checks above supplement synthetic
regressions; they do not establish compatibility with every Dutch event.

For a supported accessible page, `map-inspect --section-id ID --seat-id ID` can
open one numbered section and hover an explicit primary or resale seat without selecting
it. It requires matching event/canonical identity, NL/EUR configuration, expected
locale strings, a unique original map node, numbered section identity and exact
row/seat labels. The result is rechecked after capture. The output includes
`adapter_validation: "limited_live_validation"` for Dutch inspection results.

Resale requires the explicit resale icon and **Verified Resale Tickets** label.
Primary requires an explicit `type="primary"` circle and a displayed title that
matches the current ticket configuration. Standard/VIP labels and colors alone
cannot establish the seller. The quote must show an explicit euro amount per
ticket (`each` in English, `per stuk` in Dutch). Displayed quantity and fee
completeness remain unknown. A `Vak110`
section may match displayed section `110`; this narrow Dutch alias does not
apply to Czech pages or standing sections. The observed tooltip heading is
`Level` in English and `Niveau` in Dutch. Unsupported or changed structures
fail closed, including hidden/stale tooltips, currency changes and replaced nodes.

Some Dutch skyboxes render distinct named blocks with the same native section
ID. The parser preserves those separate blocks, still requires unique seat IDs,
and rejects a requested section if its ID is ambiguous. Czech strictness and
existing unambiguous geometry fingerprints are preserved.

This does not enable Dutch automatic section scans, stock claims,
adjacent-group selection or carting. Venue presets still supply
references rather than current availability or adjacency.

## Reservation verification

Existing journal checks already require the same attempt/event/performance and
browser document, unique seats, exact quantity, complete total/fees/currency,
source-backed adjacency and hold expiry. They also retain the event lock after
ambiguous outcomes. The native browser currently emits uncertain receipts;
there is no reviewed successful Dutch hold contract to extract.

The passive request diagnostic now receives the **effective adjacency preference
from the persisted dispatch record**. It can report a split-seat request that
conflicts with `require_adjacent = true`; false remains a valid user preference,
and legacy callers without the field remain unknown. This does not establish
that tickets were actually reserved together, change selection actions or
permit a retry. Rust and browser fixtures exercise both values and overrides.

Before a real Dutch cart can be validated, an accessible event and bounded ticket
request are needed, followed by a reviewed native flow and receipt showing the
actual seats, complete price and expiry. Existing Czech unresolved attempt locks
must remain intact. A selected counter, redirect, empty basket or elapsed timer
is insufficient evidence of a confirmed hold or release.
