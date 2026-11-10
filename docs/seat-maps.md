# Map scraping and reusable layout references

`map-inspect` extracts an event-bound map reference, venue identity, configured
selection zones and supported rendered SVG section outlines. Its browser mode
keeps the native map display instead of switching to best available. It can
export a clean section SVG, download a referenced image and save a draft static
layout in the venue registry.
Opt-in [Czech section and seat inspection](seat-inspection.md) also reads rendered
seat identities and supported resale hover prices.
These source features are newer than the 0.6.0-test downloads and Android APK.

The [Netherlands mapping audit](netherlands-mapping.md) adds verified parsing of
observed Dutch section-name aliases and multiple same-text label placements.
It includes seven overview maps, clean arena SVG exports and draft venue presets;
individual-seat inspection remains Czech-only.

```bash
cargo build --release --locked

# Read the current page in ordinary Chromium on a graphical desktop.
./target/release/ticket-watch map-inspect --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --geometry-svg captures/maps/korn-sections.svg

# Inspect saved HTML, download its referenced image and save a draft layout.
./target/release/ticket-watch map-inspect \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --html captures/event.html --map-image captures/maps/korn.png --save-layout
```

Inspection needs no API key, Discord token or watch configuration. Saved HTML has
unverified transport and age. `--map-image` explicitly makes a network request,
even when combined with `--html`; without that flag, saved-HTML inspection is
offline. The destination must be a new file. Parent directories are created.
`--geometry-svg` exports only observed section geometry; it fails if none was
captured. Existing output files are never overwritten. If a later output or
layout save fails, earlier completed files remain available and the command
reports the partial result.

`--save-layout` uses the configured database, or `data/ticket.sqlite` when the
default configuration file is absent. As with `venues import`, it takes the
monitor's database lock; stop the monitor before saving presets. Without this
flag, inspection does not create a database or change saved watches.

## What the report establishes

The parser binds the public event URL, canonical URL, rendered event name,
embedded state and fulfilled page-query identities. Conflicting events, stale
in-flight state, duplicate JSON keys, access challenges and hidden map regions
are rejected. Missing map references and unloaded section configuration remain
explicitly absent. Recognized visible image references must agree with metadata.

The report separates the regional venue ID from its Discovery ID. Configured
section IDs/titles and price-to-section **ID mappings** are configuration only;
the report contains no ticket-price or remaining-stock claims. A zone might mean
an entire floor rather than an individual numbered seating block.

A blank configured zone name is reported as `title: null`. Its section ID and
price mappings remain available, but draft presets omit the unknown label.
Missing or malformed source fields still fail validation.

The layout fingerprint hashes the site, regional venue identity, country, map URL
and sorted configured sections. It is not an image-content hash. A different map
or section configuration at the same venue becomes a separate layout candidate;
reordering the same section records does not. The same map/configuration can be
reused across events without sharing their live inventory.

Draft presets store the public map URL, section reference labels and source event.
They leave `verified_at` unset. Repeated saves preserve an existing layout's name
and review timestamp, and retain unrelated manual layouts. A conflicting saved
layout requires review. Saving metadata does not create inventory observations,
cart actions or Discord deliveries. See [venue presets](venue-presets.md).

## Images and limits

Supported native SVG maps add `rendered_geometry` to the JSON report: section
IDs/names, typed outline commands, label anchors/rotation, a view box and a
geometry fingerprint. Coordinates are local to that SVG, not screen coordinates.
They represent SVG attributes; external stylesheet geometry overrides are not
resolved, so the export is not a pixel-perfect reproduction of every page style.
Matching main-map/minimap copies are deduplicated; conflicting copies, unbound
labels, unsupported transforms or malformed/unbounded paths fail inspection.
Colors and `data-active` flags are ignored. Missing geometry stays `null`.

The optional SVG export is rebuilt from validated coordinates and escaped labels.
It contains no copied scripts, events, links, embedded images or source styling.
It represents section outlines only: stage drawings and other decorative layers
are not copied. `--save-layout` continues to save the static reference and
configured zone labels; it does not store the rendered outline data in the venue
preset.

Some native maps hide their SVG from screen readers using `aria-hidden` while
still painting it on screen. New browser captures attach a dedicated marker only
after checking computed style, ancestors, viewport and clipping rectangles. That
marker permits section-geometry extraction only; ordinary inventory visibility
rules remain unchanged. Older HTML without this marker cannot supply geometry.
Open native modal dialogs and inert/background controls remain unusable page
evidence. The marker is not a pixel-level occlusion or image-decoding guarantee.

Image downloads use narrowly validated observed Ticketmaster CDN paths, verified
HTTPS, a 20-second deadline and an 8 MiB limit. Redirects and retries are disabled.
HTTP status, MIME, PNG/JPEG/GIF/WebP headers and applicable size fields are
checked; the command reports a SHA-256 hash of the received bytes. This is
transport/header validation, not a complete image decoder or a geometry check.
SVG and unfamiliar asset paths are unsupported.

The page extractor reads captured HTML and bound page metadata. It does not read
canvas pixels, cross-origin iframe contents or shadow-root seat data. Visible
canvas/iframe counts within the map region are diagnostics, not coverage proof.
Supported seat layers now provide explicit seat identities and local coordinates
with partial DOM coverage; see [seat inspection](seat-inspection.md). Available
seat quantities, physical aisle boundaries and adjacent groups remain unverified.

A drawing's colors and labels do not establish which tickets are on sale.
All map reports explicitly keep `stock_verified`, `geometry_verified` and
`adjacency_verified` false: observed section paths do not verify permanent venue
or individual-seat topology. The ordinary inventory parser and cart planner remain
separate. An image URL can change its bytes later, so a saved reference is not a
permanent seating-layout guarantee.

## Observed validation

On 2026-11-09, the actual headed-browser command reached the Czech KoRn event and
extracted its O2 arena map reference plus three configured zones: `1. POSCHODÍ`,
`4. POSCHODÍ`, and `Plocha`. The public PNG downloaded successfully (178,291 bytes).
Visual inspection showed numbered blocks, a standing floor and a stage; it did
not expose individual-seat inventory. KoRn and Andrea Bocelli reference different
map images at the same arena, validating the need for separate layout candidates.

Offline replay also passed on 14 saved regional events across nine markets
(Czech Republic, Poland, Australia, Belgium, United Kingdom, Sweden, Spain,
Germany and Netherlands). Twelve exposed a static image reference; two correctly
reported no reference. These are checks of saved captures, not new live checks of
all nine markets.
The [sanitized evidence](evidence/seat-maps-2026-11-09.json) lists the checked
events and records the live check and downloaded image hash.

Local tests cover cross-event/cache conflicts, duplicate metadata, missing maps,
section order, separate layout variants, hidden/challenged pages, unsupported
surfaces, bounded downloads, redirects, malformed image headers, restart
persistence, preservation of reviewed layouts and refusal to overwrite image files.
These checks establish the supported extraction paths, not universal market
coverage or continuous unattended access.

The next live Czech check opened the map display and exposed 52 SVG section paths
with 52 matching labels. The map-specific worker returned HTTP 200 and
`map_svg` in 7,170 ms in that check. The clean export rendered as 52 paths and 52
labels with zero script elements. No seat, quantity, search or reservation action
was taken. This is one successful observation, not a polling latency guarantee.

The optimized `map-inspect` command subsequently exported the same 52 sections
and labels as a 44,945-byte SVG. The [geometry evidence](evidence/map-geometry-2026-11-09.json)
records both observations, the geometry fingerprint and the export hash. Local
verification passed 231 Rust tests and 57 browser tests with none skipped, plus
formatting, Clippy and the release build. The Android APK is unchanged.
