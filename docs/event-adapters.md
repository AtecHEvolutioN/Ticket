# Reusable event monitoring

The program accepts selected public Ticketmaster event URLs and artist/location subscriptions. Extraction is selected by page structure, not artist, event name, event ID, or generated CSS classes. One recognized layout therefore works across events that use it. A new layout needs one new adapter, not a parser for every concert. The [21-event live coverage check](coverage-2026-11-09.md) documents the latest country, artist, and venue results.

## Use it

From the repository or extracted bot package:

```bash
./target/release/ticket-watch inspect --url 'YOUR_PUBLIC_EVENT_URL'
./target/release/ticket-watch inspect --url 'YOUR_PUBLIC_EVENT_URL' --headed
./target/release/ticket-watch inspect --url 'YOUR_PUBLIC_EVENT_URL' --html captures/event.html
```

`inspect` does not need a configuration or API key and never sends alerts. `--headed` requires a graphical display. `--currency PLN` supplies currency context when the displayed symbol is ambiguous; incompatible symbols do not adopt the configured currency. Offline HTML reports `source: saved_html` and `observed_at: 0`, so it cannot be mistaken for a fresh live check.

For continuing monitoring, copy `config.example.toml` to `config.local.toml`, replace the disabled placeholder with the selected event and enable it. Add another `[[watches]]` block for another event. Use `mode = "browser"` and `[watches.parser] adapter = "ticketmaster"`. The existing `run`, `status`, price/quantity filters, and Discord outbox apply to each watch independently. The examples enable `notify_offer_changes` for newly observed or changed qualifying listings; set `notify_initial = true` if the first positive observation should alert too. Initial observations are otherwise quiet.

For automatic enrollment of selected artists, use `examples/hybrid.example.toml`: set each subscription's artist/attraction ID, optional country/city, and enable it. The reusable profile covers the allowed public Ticketmaster hosts; each event still needs a recognized accessible layout. Discovery supplies identity and catalog metadata, not live inventory. Currency and price filters should be chosen per subscription/profile when events span currencies.

## Current layout coverage

| Page shape | Behavior |
| --- | --- |
| Regional `[data-testid="ticketTypeInfo"]` categories with quantity steppers | Display type, exact price or range, currency when resolvable, and selectable capacity. No remaining-stock claim. |
| Regional resale selection rows with type, explicit available count, price, and labeled section/row | Observed resale offers with listing quantities and per-ticket price semantics. These may qualify for alerts. |
| UK quick-picks with regional ticket-type components | Display prices/types with section and row context; search-filter quantities remain unknown inventory. |
| Global `li[role="menuitem"][data-listing-id]` listings | Detected but unsupported: locked offers and price multipliers need separate validated interpretation. |
| Unknown/changed layout, missing fields, ambiguous duplicate rows | Unknown, with a diagnostic; never inferred sold out. |
| HTTP 401/403/429, queue, or browser challenge | Blocked; preserve the previous valid inventory state and obey cooldowns. |

Opening the best-available tab is automated for the validated Polish, English, Czech, Spanish, and Dutch display labels. Other languages can be read when recognized ticket components are already exposed; unrecognized tab labels are not guessed. The adapter only uses display controls and recognized cookie-rejection buttons. It does not select tickets, change quantities, submit searches that reserve seats, or enter checkout.

Observed per-ticket text in Czech, Dutch, German, Spanish, Swedish, and English is parsed explicitly; arbitrary fee or multi-ticket text is not stripped. Ambiguous symbols such as `kr` resolve only with compatible configuration or unambiguous same-event JSON-LD currency evidence. Metadata never supplies inventory or observed prices.

Numeric event URLs, alphanumeric IDs, benign encoded slugs, supported locale prefixes, and Italian `/biglietti/.../event/ID` routes are bound to the same host and event ID across redirects. Supporting a URL format does not establish support for its inventory layout.

## Data and alerts

`ticket_categories` contains displayed category prices/ranges, with optional section/row context for quick-picks. `offers` contains interpreted available resale listings. A selected quantity of zero and a control maximum of six are purchase settings, not stock. Listing counts are never summed into an event-wide total. Required service fees and optional order fees can differ, so unresolved fee scope stays unknown.

`status` includes `latest_observation`, including categories and offers. This snapshot can be unknown or blocked while the last valid inventory baseline is preserved separately for restock deduplication. Editing the watched event or its extraction/filter configuration clears the old snapshot. Primary category ranges cannot satisfy exact offer-price or stock-quantity alert filters.

Custom adapters remain available through `adapter = "selectors"` and verified CSS selectors. The built-in mode does not accept a mixture of manual selectors; choose one interpretation explicitly.

## Verification

The regional adapter was checked against the saved real event capture, which produced four category/range entries and two resale listings with quantities of two. Sanitized fixtures exercise unrelated artist/event IDs, translated labels, currency ambiguity, hidden/disabled rows, changed layouts, and conflicting listings. Browser tests exercise ordinary tab opening, event identity, cookie dialogs, and the no-reservation boundary. These tests do not establish worldwide live coverage.

A separate direct graphical attempt on Pitbull's ERGO ARENA event on 9 November 2026 returned HTTP 401 and stopped. Sustained access, detection latency, and live Discord delivery have not been validated. The Android test client performs foreground checks; it remains separate from the always-on Rust service.

The broader check reached ticket data for 12 events across 8 countries using ordinary graphical artist-to-event navigation. Five other pages showed sale schedules, one required a presale code, and three used layouts that are not yet interpreted. This test route is distinct from the background worker’s direct event navigation and does not establish sustainable unattended access. See the coverage report for every event and the precise limitations.
