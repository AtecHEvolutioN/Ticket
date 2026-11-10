# Android test-build verification

Version 0.7.0-test verified on 2026-11-10 using an isolated Android 9 / API 28 x86_64 emulator.
The APK supports API 26 and newer by manifest; API 26 and API 35 physical devices
have not been tested in this environment.

- The signed APK installs and its launcher activity opens successfully.
- The visible demo flow creates a saved watch, changes unavailable to available,
  and posts a labelled "Demo ticket alert" to Android's notification service.
  Watchlist and notification screenshots are included with the local build output.
- 364 on-device offline assertions pass: public-link validation, page identity and
  challenge classification, API schema handling, watch persistence, quiet initial
  observations, restock deduplication, unknown/blocked recovery, selector-change
  baselines, URL-free demo persistence, Android Keystore encryption/removal, bottom
  navigation, country-chip selection, search-draft persistence, blank credential
  inputs and screenshot protection, demo state buttons and local alert delivery,
  availability filters that exclude demo data, and an offline page inspector.
- 73 preference assertions cover the default of two tickets, global choices,
  event-specific overrides and reset, exact total-price parsing, Unicode filter
  normalization, supported event identity, corruption handling, and bounded JSON
  export. The real editor is exercised through invalid input, Save, reopen,
  inheritance and Save & export with a local document result. Pasted negative or
  oversized quantities and malformed amounts remain visible and are rejected;
  comma decimals retain their intended value. Preferences never modify saved
  inventory observations or authorize a reservation.
- Actual Android exports with Unicode ticket types, section/row/category filters,
  a CZK total and an event override pass the desktop planner's compatibility checks.
  The export stays within its 256 KiB limit near storage capacity without dropping
  configured entries. Preview images `android-ticket-preferences.png`,
  `android-event-preferences.png`, and `android-ticket-filters.png` use the app's
  actual views and clearly synthetic data.
- Ticket detail coverage includes advertised ranges, decimal/currency parsing,
  missing and ambiguous quantities, wrong-event and blocked-page rejection,
  current-snapshot persistence, profile-change invalidation, stale-offer removal,
  synthetic offer isolation, and Discord payload construction without sending.
- A local HTML fixture runs in Android WebView with network loads disabled. It
  verifies visible price/type extraction, duplicate suppression, hidden/disabled
  rows (including disabled fieldsets), and dropdown quantities that must be labelled
  selectable rather than remaining stock. This fixture is synthetic, not captured
  Ticketmaster production markup.
- Explore, Watchlist, Settings, and Page inspector previews are rendered from the
  actual app views on the emulator. The Settings preview contains blank inputs;
  its system screenshot protection remains enabled. Preview files are saved under
  `/sdcard/Android/data/app.ticketwatch.mobile/files/ui-previews/` and copied into
  the local `dist/` build output.
- `dist/android-tickets.png` previews the two synthetic priced offers in the
  inspector. `dist/android-generic-tickets.png` previews displayed categories and
  a separate resale listing for a different synthetic event. Demo quantities are
  not presented as a total remaining count.
- The built-in regional adapter is tested with unrelated concert and sports event
  identities, primary price ranges kept separate from resale inventory, explicit
  Polish listing counts, hidden/disabled rows, no quantity/cart/reservation clicks,
  unsupported layouts, conflicting currency markers, and category-only persistence
  with inventory still unknown. Encoded display slugs, localized event routes, and
  Italian routes are accepted while ambiguous IDs/routes are rejected.
- European fixtures preserve the observed Czech, Spanish, and Dutch tab/field
  labels, localized price units, Czech listing quantities, and Swedish `kr`
  disambiguation from matching Event metadata only. Different-event, conflicting,
  and malformed currency bindings cannot create numeric quotes; custom-profile
  currency contradictions also remain unknown.
- UK quickpick fixtures retain separate section/row contexts for identical ticket
  type names, suppress exact duplicates, and reject conflicting same-context prices.
  Toolbar quantities and result totals never become listing or remaining counts.
- Venue tests preserve the Discovery venue ID, isolate Ticketmaster sites and distinct
  provider IDs, reject conflicting locations, normalize exact fallback metadata,
  retain stable preset references during fallback-to-ID promotion, and reject
  ambiguous fallback matches and stale-key collisions after a venue rename.
- Presets persist across store instances and can be selected for another event at
  the same venue. Applying one preserves the target event's own observations and
  never copies prices, ticket types, inventory, or verified selectors. Metadata-only
  re-saves retain a valid selection; explicit clear and deletion invalidate it.
- The real venue form is exercised through Add → Save → Use with two section labels
  and notes. Its current selection is verified, then deletion and reopening are
  checked. `dist/android-venues.png` renders that actual app view with clearly
  labelled synthetic fixtures. Accessibility connects before opening the activity
  and waits for dialog readiness; no live requests are involved.
- AAPT2 packaging, D8 compilation, APK alignment, and v2/v3 signature verification
  pass. The APK contains no native ABI-specific libraries.
- The signing certificate matches the published 0.6.0-test APK. Installing the new
  APK over the emulator's existing 0.3.0-test app succeeded without uninstalling.
  Guest Wi-Fi and mobile data were disabled; the final run reported no active
  default network.
- Debugging, application backups, and cleartext network access are disabled.
  Only the launcher activity is exported.
- Tests use synthetic data and a fake temporary credential that is removed after
  the encryption test. They do not make live Ticketmaster requests or send Discord
  messages. The regional layout was observed in a real desktop capture; this Android
  verification uses offline fixtures and does not establish sustained phone access.

The internal test package is built separately and is not needed on your phone:

```bash
ANDROID_SDK_ROOT=/path/to/android-sdk ./build-apk.sh
ANDROID_SDK_ROOT=/path/to/android-sdk ./build-instrumentation.sh
adb install -r ../dist/ticket-watch-android.apk
adb install -r ../dist/ticket-watch-tests.apk
adb shell am instrument -w app.ticketwatch.mobile.test/app.ticketwatch.mobile.SmokeInstrumentation
```

Run instrumentation on a disposable emulator. The test credential is synthetic;
the suite preserves an existing saved API key if one is present, but development
tests should not use a phone holding personal credentials.
The UI flow leaves a clearly labelled synthetic demo watch and local notification
for inspection. On Android 13 or newer, grant the notification permission to
Ticket Watch before running the suite; the UI flow verifies notification delivery.
