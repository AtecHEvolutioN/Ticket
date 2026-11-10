# Tested downloads — 0.7.0-test

These are the builds verified locally for this test release. Use each file's matching `.sha256` file to verify the download.

- [Android APK](ticket-watch-android.apk) — version 0.7.0-test, Android 8.0 or newer. Editable ticket preferences with a default quantity of two; retains the signing certificate from earlier local test builds.
- [Linux x86_64 bot archive](ticket-watch-bot-linux-x86_64.tar.gz) — matching preference import/planning and monitoring code; requires glibc 2.39 or newer. Browser mode needs Node.js and Chromium.
- [Venue starter SQLite database](venue-presets.sqlite) — 21 venue metadata records, with no watches, offers, or pending alerts.
- [Venue JSON export](venue-presets.json) — import this into an existing monitor using the [venue commands](../docs/venue-presets.md).

On GitHub's file page, use **Download raw file**. The Android app checks pages while open; continuous monitoring runs in the Rust service. No API credentials or private signing key are included. Automatic carting and phone/server synchronization remain inactive.

The files are committed here because the release-asset upload endpoint rejected the available credentials. The release links point to the version tag so downloads remain pinned to this tested build. Older builds remain available at their earlier Git tags.
