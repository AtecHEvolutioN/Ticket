# Reusable venue presets

Events at the same venue can share a saved venue reference. The registry stores
the Ticketmaster site, official venue ID when available, venue name, city, country,
public source events, and separately named layout references. It lives in the
existing SQLite database, separate from observations and the notification outbox.

The starter catalog in [venue-presets.json](../examples/venue-presets.json) contains
29 venues from the coverage checks, including ten Dutch venues. Six Dutch entries
have draft static map references from the [Netherlands audit](netherlands-mapping.md);
all layout review timestamps remain unset. Other entries retain venue metadata
without invented layouts. The captures establish reference data, not permanent
seating geometry or inventory. City names retain their observed spelling; for
example, `Praha 9` is not silently treated as `Prague`.

## Use the registry

```bash
# No API key or network request is needed.
./target/release/ticket-watch venues import examples/venue-presets.json
./target/release/ticket-watch venues list
./target/release/ticket-watch venues match --site ticketmaster.cz \
  --name 'O2 arena' --city 'Praha 9' --country CZ
./target/release/ticket-watch venues show VENUE_ID_FROM_THE_RESULT
```

These commands use `config.toml` when present, otherwise `data/ticket.sqlite`.
Pass `--config config.local.toml` to use the monitor's configured database.
Other missing configuration paths are errors. Import validates the
entire array and commits it atomically; repeating the same import is idempotent.
Import takes the monitor's process lock, so stop a running monitor before editing
its presets through the CLI. Read commands can inspect the running database.

During a successful Discovery catalog refresh, selected events register their
venue metadata and receive `venue_preset_ids` in cached catalog metadata. A second
event at a known venue resolves to the same reference with no extra venue request.
The two event watches, prices, availability, and alerts remain independent. Invalid
or ambiguous venue metadata produces a catalog note while event monitoring
continues.

## Matching and layout changes

- Match the normalized Ticketmaster site and official venue ID first. Distinct
  official IDs remain distinct even when venue names match.
- When an ID is unavailable, use an unambiguous exact match of normalized name,
  city, country, and site. Letter case and repeated spaces are normalized; fuzzy
  names, accent substitutions, and city aliases are not guessed.
- Adding an official ID to an unambiguous earlier name-only entry preserves its
  local preset ID. A conflicting city/country or ambiguous identity is rejected.
- Keep configurations such as an end-stage concert, standing floor, and basketball
  in separate named layouts. Saved sections are reference labels and do not prove
  that those sections are open, sold, or physically configured for a new event.

To add a layout, export a venue with `venues show`, add a `layouts` entry, wrap the
venue object in a JSON array, and import the file:

```json
{
  "id": "concert-end-stage",
  "name": "Concert with end stage",
  "adapter": null,
  "sections": ["A", "B"],
  "source_event_urls": [],
  "verified_at": null
}
```

This is a synthetic example, not a verified arrangement at a real venue. A positive
`verified_at` timestamp requires a public source event and records an explicit
layout review only. Matching layout IDs replace that layout on import; other
layouts survive metadata refreshes. An adapter hint is descriptive and does not
activate extraction or bypass page-layout verification.

Presets reject inventory fields. Prices, remaining quantities, ticket categories,
available seats, and session/cart links never become shared venue data. Optional
`map_url` references must use a recognized public static-image path and have a
public source event. [Map inspection](seat-maps.md) can download these images and
save separate draft layouts for different map/section configurations at one venue.
Supported rendered section outlines can be inspected and exported separately;
they are not copied into these presets. [Seat inspection](seat-inspection.md)
reads explicit row/seat labels and supported hover prices, but those transient
records are not saved as venue inventory. Inventory detection still requires a
fresh check for each event.

## Android

The Android client keeps its own local venue registry and preserves official venue
IDs from Discovery. In **Watchlist**, open an event's **Venue presets**, choose
**Add preset**, and save a name, section references, and optional notes. On another
watch at that venue, open **Venue presets** and choose **Use for this event**. The
watchlist also provides a venue library for browsing saved references. Existing
watches register their venue metadata when that library opens. The app
does not transfer prices, quantities, or selector verification between watches.
The Android and server registries are independent; automatic synchronization is
not implemented.

## Verification

The Rust suite passes 164 tests, including shared venues across independent
events, same-name venue collisions, official-ID promotion and venue renaming,
layout persistence, atomic import rollback, source-history bounds, and rejection
of inventory fields. Clippy and release compilation pass. Verification uses local
fixtures and temporary databases; no Ticketmaster or Discord calls are needed.

Android 0.6.0-test passes 291 offline assertions on an Android 9 / API 28 emulator,
including the actual Add → Save → Use form flow and event-data isolation. The
[venue screen preview](../dist/android-venues.png) shows labelled synthetic data.
