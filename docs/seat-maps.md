# Map scraping and reusable layout references

`map-inspect` extracts an event-bound static map reference, venue identity and
configured selection zones from the recognized regional Ticketmaster page. It
can download the referenced image and save a draft layout in the venue registry.
These source features are newer than the 0.6.0-test downloads and Android APK.

```bash
cargo build --release --locked

# Read the current page in ordinary Chromium on a graphical desktop.
./target/release/ticket-watch map-inspect --headed \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657

# Inspect saved HTML, download its referenced image and save a draft layout.
./target/release/ticket-watch map-inspect \
  --url https://www.ticketmaster.cz/event/korn-vstupenky/1472650657 \
  --html captures/event.html --map-image captures/maps/korn.png --save-layout
```

Inspection needs no API key, Discord token or watch configuration. Saved HTML has
unverified transport and age. `--map-image` explicitly makes a network request,
even when combined with `--html`; without that flag, saved-HTML inspection is
offline. The destination must be a new file. Parent directories are created.
If a later layout save fails, the successfully downloaded image remains available
and the command reports that partial result in its error.

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

Image downloads use narrowly validated observed Ticketmaster CDN paths, verified
HTTPS, a 20-second deadline and an 8 MiB limit. Redirects and retries are disabled.
HTTP status, MIME, PNG/JPEG/GIF/WebP headers and applicable size fields are
checked; the command reports a SHA-256 hash of the received bytes. This is
transport/header validation, not a complete image decoder or a geometry check.
SVG and unfamiliar asset paths are unsupported.

The page extractor reads captured HTML and bound page metadata. It does not read
canvas pixels, cross-origin iframe contents or shadow-root seat data. Visible
canvas/iframe counts within the map region are diagnostics, not coverage proof.
Individual seats, aisle boundaries and adjacent groups remain unverified.

A static drawing's colors and labels do not establish which tickets are on sale.
All map reports explicitly keep `stock_verified`, `geometry_verified` and
`adjacency_verified` false. The ordinary inventory parser and cart planner remain
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

The completed source change passed 221 Rust tests, Clippy and formatting checks.
