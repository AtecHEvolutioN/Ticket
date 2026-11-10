# Ticket Watch for Android — test build

This is a standalone Android test client for the Ticket Watch project. It runs on
Android 8.0 or newer and uses the phone's own network connection. It does not need
a deployed server to try artist search, saved watches, page inspection, or demo
notifications. It does not contain your consumer key, consumer secret, or a Discord
token.

## Install and try it

1. Download [ticket-watch-android.apk](https://github.com/AtecHEvolutioN/Ticket/raw/refs/heads/main/downloads/ticket-watch-android.apk) and open it on your
   phone. If Android asks, allow installation from the browser or file manager
   you used to open the file.
2. Open **Ticket Watch → Explore → Try local demo**. Simulate unavailable, then available.
   Allow notifications when prompted. The notification is explicitly a demo;
   repeating available or recovering from blocked does not create another restock.
   Choose **Ticket details** to preview two synthetic offers with prices, types,
   listing quantities, and fee information.
3. In **Settings**, enter your Ticketmaster **consumer key**. Discovery does not
   require the consumer secret. Credentials are encrypted using Android Keystore
   and kept in private device storage; app backups and credential-screen capture
   are disabled.
4. In **Explore**, search for an artist, select the match, and optionally choose a
   country chip or enter a two-letter country code and city. Save an event to the Watchlist or open its
   official ticket link. A missing event in Discovery does not imply sold out.
5. For an event absent from Discovery, use **Add event URL** in the Watchlist.
   **Check page** opens the Ticketmaster page on your phone and reports its state.

## App screens

The app uses a compact dark interface with persistent bottom navigation:

- **Explore:** artist and location search, country shortcuts, direct event links,
  and an offline demo entry. Search drafts survive switching tabs.
- **Watchlist:** saved-event and availability counts, All / Available / Needs check
  filters, readable status cards, and page-check actions. Demo events are labelled
  and excluded from real-event counts and availability filters.
- **Venue presets:** a local library of saved venues, named configurations, section
  references, and notes that can be selected for another event at the same venue.
- **Settings:** separate search-key, phone-notification, and Discord setup cards.
  Saved credentials never appear in the input fields.
- **Ticket preferences:** global defaults and event overrides for ticket count,
  total budget, adjacency, ticket types, sections, rows, and category IDs.
- **Page inspector:** event details, status and check time, an embedded public page,
  and a collapsible advanced selector editor.

The refreshed build uses the same local signing certificate as earlier test builds, so it
can be installed as an update without removing saved settings.

## Choose your tickets

Version 0.7.0-test adds **Settings → Edit ticket preferences**. The starting defaults
are **2 tickets**, **adjacent seats**, **cheapest eligible group**, **CZK**, and no
total budget limit. Quantity is editable from 1 to 20; the event's own purchase
limit still applies. A budget is the complete group total including fees, entered
as a decimal amount. No currency conversion is performed. The current Czech
desktop planner accepts CZK; other currencies can be saved but cannot currently
be applied to Czech cart planning.

Open a saved event's **Ticket preferences** from its watch card or page inspector.
Clear **Use my default preferences**, change the fields, and tap **Save for this
event**. An override is a complete set of choices: later default changes do not
alter it. Re-enable **Use my default preferences** and save to remove the override.
Preferences are stored separately from watch observations, so subsequent page
checks and event metadata updates cannot overwrite them. Slug, locale, and `www`
aliases for the same site/event share the override; different sites remain separate.

Ticket-type, section, and row filters accept one exact label per line; capitalization
and repeated Unicode whitespace are ignored. Blank lists allow any value. When a
page check has supplied ticket-type labels, **Choose types from last page check**
offers those labels for selection. They are observations, not a promise of current
inventory. Advanced category IDs must match the desktop event catalog exactly.
Missing label evidence cannot satisfy a selected filter. Restricted-view and
accessibility suitability are not currently verified by the planner.

**Save & export preferences** opens Android's file picker to write
`ticket-preferences.json`. The file contains a versioned set of defaults and public
event overrides; it includes no API keys, bot tokens, account sessions, or payment
data. See [desktop preference usage](../docs/ticket-preferences.md) for applying it
to offline cart planning. A saved preference is not permission to place a live hold.

This screen saves and exports choices. **Automatic carting is not active in the
Android app**, existing availability alerts are not filtered by these preferences,
and the app does not synchronize them to the Rust service. Changing quantities
or filters here never clicks Find tickets or submits a reservation.

## Reuse a venue preset

Version 0.6 keeps a private venue library on your phone. Save an event from Explore,
then open **Watchlist → Venue presets** or the event's **Venue presets** action.
Choose **Add preset**, name the configuration, and enter section labels and notes.
On another saved event at the same venue, choose **Use for this event**. The selected
references appear in the watch card and page inspector. **Clear for this event**
removes just that selection; removing a preset invalidates its saved selections.
Venue records and presets remain after removing the original event watch.

Discovery's venue ID is retained and matched within the Ticketmaster site. Without
an ID, the full venue name, city, and country must match after Unicode, whitespace,
and letter-case normalization. There is no fuzzy matching. An unambiguous fallback
can acquire the first matching official ID while retaining its preset references.
Different known IDs or sites stay separate; conflicting locations and ambiguous
name-only matches cannot reuse presets.

Section references are manually named notes, not a seating map or verified layout.
Prices, ticket types, counts, available seats, sessions, and verified page selectors
are never copied between events. Every event still needs its own page observation.
Existing watches with complete venue metadata can populate the library when you
open it. This device library is learned from saved watches; it does not automatically
import or synchronize the Rust service's example venue registry.

## Prices, quantities, and ticket types

Version 0.6.0-test supports advertised Discovery price ranges in search results and
saved events. These optional ranges are catalog metadata; they do not identify
currently available tickets. Discovery's `standard` range category is not proof
of an available admission type, and its purchase limits are not remaining stock.

The inspector and Watchlist can display individual observed offers: ticket type,
price/currency, price basis, fee scope, quantity and its meaning, section, and
observation time. Local and Discord restock alerts include up to three current
offers. Missing fields stay **not reported**. Listing quantities, selectable order
quantities, and reported remaining quantities are kept distinct. Offers may overlap,
so their numbers are never summed into an event-wide remaining count.

The built-in regional Ticketmaster adapter works for any selected event using the
recognized ticket-selection layout. It opens the known best-available display tab,
reads displayed primary ticket categories and price ranges, and extracts resale
listing types, prices, sections, rows, and explicit listing quantities. It does not
use an artist name, event ID, or generated CSS class. Primary categories and their
selectable maximums are displayed separately and do not establish remaining stock.

Desktop captures on November 9, 2026 confirmed the regional layout across multiple
artists and venues. Version 0.5 adds observed Czech, Dutch, and Spanish tab and
listing labels, Czech section/row names, and exact per-ticket wording in Czech,
Dutch, Spanish, Swedish, German, and English. The observed UK quickpicks layout
is read as displayed categories with section and row context; its search quantity
is never a stock count. Identical ticket-type names at different locations stay
separate, while conflicting prices for the same location remain unknown.
Swedish `kr` prices use only an explicit currency from matching Event metadata;
wrong-event or conflicting metadata remains unknown. Metadata does not supply
stock, quoted amounts, or availability. The Android flow is tested on offline WebView pages
with different event identities. Other
Ticketmaster templates remain unsupported until adapters are validated; the app
reports unknown rather than interpreting arbitrary page prices as live offers.
Queues and browser restrictions can still prevent a check on your phone.

Verified custom rules in **Advanced selectors** take precedence over the built-in
adapter. They require both event-status rules and row-relative offer rules. Editing
a rule clears verification and resets the saved baseline. Missing or incomplete
results never mean sold out. Your Discovery key does not grant restricted inventory
API access.

## Availability and alerts

Discovery supplies catalog information, not proven remaining inventory. The page
inspector recognizes supported layouts automatically. An enabled resale listing
with an explicit positive quantity can establish available; displayed primary
categories alone remain unknown. Optional custom available/unavailable CSS selectors
can be saved and explicitly marked verified in the advanced section. The app checks visibility,
event identity, conflicting signals, errors, and verification pages before accepting
a result. Changing those selectors resets the baseline.

The first available observation is quiet. A verified unavailable-to-available
transition creates a local alert. Unknown and blocked observations preserve the
last definite state. The APK's page inspector can report event-level availability
and the offer fields described above. It does not infer adjacency, reserve seats,
or establish an event-wide remaining-ticket total from a partial page.

Discord is optional. Enter a bot token, numeric channel ID, and optional user ID
in Settings, then use the explicitly labelled connection-test button. Enable
Discord alerts if desired. Demo availability never sends to Discord. The Android
client reports delivery failures but does not implement the Rust service's durable
notification outbox.

Front Gate festival links can be opened in the browser; the embedded page inspector
currently supports Ticketmaster event pages. Events with unsupported links retain
their catalog details but cannot be added as page watches.

## Scope of this APK

Checks are manual and operate while the app is open. This build does not schedule
background polling, promise 24-hour Android monitoring, or connect to the Rust
daemon. The Rust service remains the continuous-monitoring implementation.

Searches are paced and bounded by a device-local 4,000-attempt rolling daily budget;
other apps using your key consume the account's separate shared quota. Website
access and selector validity still need testing on your actual phone. No real
Ticketmaster or Discord credentials are used by the emulator test suite.

Build and signing details: [BUILDING.md](BUILDING.md). Keep the local signing key
to install future test updates over this APK. Uninstalling removes the device-local
watchlist and credentials.
