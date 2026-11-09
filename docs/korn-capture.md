# KoRn live capture — 9 November 2026

Ordinary graphical Chromium navigation from the [Korn artist page](https://www.ticketmaster.pl/artist/korn-bilety/3404) to [KoRn at TAURON Arena Kraków, 17 November 2026 at 18:00](https://www.ticketmaster.pl/event/korn-bilety/496543017), followed by “Wybierz najlepsze dostępne miejsca” (best available), exposed these offers. The capture timestamp is `2026-11-09T13:02:07.392871Z`. This is one observed snapshot; availability can change.

Artifacts: [ticket-type screenshot](../dist/korn-live-tickets.png), [resale screenshot](../dist/korn-live-resale.png), and [normalized capture data](../dist/korn-live-capture.json). Raw HTML and other session-related captures remain ignored/private.

## Displayed prices and quantities

| Primary ticket type | Displayed price per ticket, PLN |
| --- | ---: |
| GA — standing | 379.00 |
| Seated | 179.00–379.00 |
| Restricted view | 179.00–379.00 |
| Official Platinum | 343.00–932.50 |

The primary controls had selected quantity 0 and selectable maximum 6, matching the displayed event purchase limit. Neither value establishes remaining stock.

| Resale section / row | Displayed type | Listing quantity | Displayed price per ticket, PLN |
| --- | --- | ---: | ---: |
| C9 / 27 | Oficjalny bilet Platinum | 2 | 575.00 |
| A9 / 6 | Official Platinum Ticket | 2 | 910.80 |

The resale count is labeled “Dostępne 2” (2 available) for each listing. Do not sum these listings into total event stock. The public page component renders these prices through its per-ticket price label. The information panel confirmed that the mandatory service fee is included; delivery and additional products can add costs. Mobile delivery was listed at 0 PLN. The existing all-fees field should remain `unknown` because inclusion of the mandatory service fee does not establish the final cost of every order option.

## Selectors observed in this capture

These selectors matched the captured Polish layout. Generated class names can change; they are not a general Ticketmaster adapter.

| Field | Selector |
| --- | --- |
| Resale offer row and enabled selection control | `li.sc-8ba94237-1[role='button']` |
| Section, relative to resale row | `dl.sc-8ba94237-2 > div:nth-child(2) > dd` |
| Row, relative to resale row | `dl.sc-8ba94237-2 > div:nth-child(3) > dd` |
| Resale ticket description/type | `.sc-ec5170ca-5` |
| Inventory label (“Bilety Resale”) | `.sc-ec5170ca-3` |
| Displayed price | `.sc-ec5170ca-6` |
| Listing quantity label | `.sc-ec5170ca-4` |
| Primary type/price container | `[data-testid='ticketTypeInfo']` inside `li.sc-14f3239d-0` |
| Primary selectable quantity control | `[data-testid='quantityStepper'] [role='spinbutton']` |

For offline resale review, the row selector can serve as both `offers_selector` and `offer_available_selector`: the parser tests the row itself as well as descendants. Use currency `PLN` and decimal separator `,`. No stable listing ID, seat numbers, or complete-inventory marker was established. Positional section/row selectors must be rechecked if the page omits or reorders its level/section/row labels.

At capture time the custom-selector parser accepted numeric text/select options only, so these raw selectors left “Dostępne 2” unknown. The subsequent [built-in event adapter](event-adapters.md) now handles the label as a listing count and separates primary category ranges/selectable limits. It uses structural components rather than the generated class names listed above. Custom selector profiles retain their original strict numeric behavior.

An offline `profile-check` against the actual capture returned both resale candidates with matching prices, types, sections, and rows; quantity remained unknown as expected. Its private configuration and report are `captures/korn-profile.toml` and `captures/korn-profile-review.json`. This review did not enable monitoring or alerts.

## What this capture establishes

The visible page and screenshots validate the listed types, prices, section/row details, and resale listing counts for this snapshot. No seats were reserved, ticket search submitted, or purchase made. The best-available view and information panels were sufficient; clicking a resale row or “Wyszukaj bilety” can enter a reservation flow and was not needed.

The capture itself involved no runtime changes. The subsequent reusable adapter now opens recognized best-available tabs and supports optional graphical Chromium, but still navigates directly to the selected event. Sustained unattended access, live alerts, and worldwide layout coverage remain unverified; see [current adapter behavior](event-adapters.md).
