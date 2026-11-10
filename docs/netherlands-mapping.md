# Ticketmaster Netherlands mapping

On 10 November 2026, ordinary headed Chromium opened **11 Dutch events across
10 venues**. Every map-page request returned HTTP200. After fixing two observed
label formats, the same captures yielded **seven SVG maps, 231 section outlines
and 232 label placements**. A separate ticket-display pass extracted 34 priced
category rows from ten events; Jamiroquai displayed a sale schedule.

These are overview maps and displayed prices. No sections or individual seats
were clicked, no tickets were selected, and no reservations were submitted.

| Event | Venue | Observed map result |
| --- | --- | --- |
| Monster Jam | GelreDome | 73 SVG sections |
| Gypsy Jazz Festival | Musis Sacrum | 4 SVG sections |
| Physical Graffiti | Poppodium Q-Factory | No map reference exposed |
| Thy Art Is Murder | Dynamo | No map reference exposed |
| 10cc | TivoliVredenburg | 11 SVG sections |
| New Wave | Ziggo Dome | 29 SVG sections |
| U2 | Johan Cruijff ArenA | 106 SVG sections, 107 label placements |
| The Last Internationale | Melkweg | Static map reference |
| Famke Louise | AFAS Live | 3 SVG sections |
| Papa Roach | RTM Stage — Rotterdam Ahoy | 5 SVG sections |
| Jamiroquai | Ziggo Dome | Sale schedule; interactive map unsupported in this view |

Eight venues exposed a rendered or static map. Metadata was bound for ten venues;
that larger count does not mean ten usable maps. See the
[machine-readable evidence](evidence/netherlands-mapping-2026-11-10.json) for
public URLs, capture timestamps, hashes, venue identities and map fingerprints.

## Fixed Dutch map formats

The source sometimes names a section `Vak127` while drawing `127`, or names
`SKYBOXEN 601` while drawing `601`. U2 also uses exact standing-room suffixes in
section names that are absent from the displayed label. These observed aliases
are accepted only after the event, host and market have been bound to
Ticketmaster Netherlands. Other markets retain exact label matching.

U2 draws `INNER CIRCLE` in two positions within the same bound section. The
geometry model now preserves bounded additional placements in `additional_labels`
instead of losing the second label. Conflicting text, duplicate placements and
inconsistent map copies still fail validation. Exports and fingerprints include
all retained labels; existing single-label fingerprints remain unchanged.

Melkweg's configured zone has a blank name. Reading the raw JSON script now
preserves that as `title: null`, instead of manufacturing an `&nbsp;` label during
HTML serialization. Its section ID and price mappings remain intact; the preset
omits the unknown name.

Clean exports contain only validated outlines and labels, with no copied scripts
or external resources. Decorative stages and other unparsed artwork are omitted.

- [GelreDome / Monster Jam SVG](maps/nl-gelredome-monster-jam.svg)
- [Johan Cruijff ArenA / U2 SVG](maps/nl-johan-cruijff-arena-u2.svg)

## Reusable venue presets

The [importable Dutch presets](evidence/nl-venue-presets-2026-11-10.json) contain
ten observed venues and six draft static layout references. Four entries contain
venue metadata only: no static image URL was invented from an SVG or an absent
map. All layout review timestamps remain unset. The export passed import and
roundtrip checks in isolated databases; the live monitor database was untouched.
The shipped [29-venue starter catalog](venue-presets.md) also includes these
entries and preserves the previously saved venue identities.

Import into your chosen monitor database using the existing venue command:

```bash
./target/release/ticket-watch --config config.local.toml venues import \
  docs/evidence/nl-venue-presets-2026-11-10.json
```

The command uses the database lock described in [venue presets](venue-presets.md).
A venue can have multiple event layouts; a stored reference never supplies live
ticket inventory for a different event.

## Current scope

Use the current Rust source for Netherlands overview inspection:

```bash
./target/release/ticket-watch map-inspect --headed \
  --url https://www.ticketmaster.nl/event/monster-jam-2027-tickets/381518243 \
  --geometry-svg captures/maps/gelredome-sections.svg
```

The output path must be new. Missing geometry remains explicit. Section opening,
seat hover, scheduled section scans and carting still require the separately
validated Czech implementation; Netherlands support here is for overview maps
and ticket-display extraction. Colors, section counts and repeated labels do not
establish ticket availability, quantities or adjacency. The Android APK is
unchanged. See the [artist coverage report](coverage-2026-11-10.md) for the full
audit and validation results.
