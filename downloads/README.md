# Downloads and venue presets

The app and bot builds remain the locally verified 0.7.0-test release. The venue
catalog on `main` was updated on 10 November 2026. Use each file's matching
`.sha256` file to verify the download.

- [Android APK](ticket-watch-android.apk) — version 0.7.0-test, Android 8.0 or newer. Editable ticket preferences with a default quantity of two; retains the signing certificate from earlier local test builds.
- [Linux x86_64 bot archive](ticket-watch-bot-linux-x86_64.tar.gz) — matching preference import/planning and monitoring code; requires glibc 2.39 or newer. Browser mode needs Node.js and Chromium.
- [Venue starter SQLite database](venue-presets.sqlite) — 29 venues and six draft Dutch static layouts, with no watches, offers, or pending alerts.
- [Venue JSON export](venue-presets.json) — import this into an existing monitor using the [venue commands](../docs/venue-presets.md).

On GitHub's file page, use **Download raw file**. The Android app checks pages while open; continuous monitoring runs in the Rust service. No API credentials or private signing key are included. The released builds keep automatic carting and phone/server synchronization inactive. Current source contains the newer coverage and Dutch map improvements described in the main README.

The files are committed here because the release-asset upload endpoint rejected the available credentials. App/bot release links point to the version tag so those builds remain pinned. The latest venue-catalog links use `main`; tagged catalogs retain their earlier contents. Older builds remain available at their earlier Git tags.
