# Archived downloads — 0.6.0-test

For the current APK with editable ticket preferences and the matching Rust planner,
use the [0.7.0-test release](https://github.com/AtecHEvolutioN/Ticket/releases/tag/v0.7.0-test).
The files in this directory remain the older 0.6.0-test builds.

These are the builds verified locally for this test release. Use each file's matching `.sha256` file to verify the download.

- [Android APK](ticket-watch-android.apk) — version 0.6.0-test, Android 8.0 or newer. Retains the signing certificate from earlier local test builds.
- [Linux x86_64 bot archive](ticket-watch-bot-linux-x86_64.tar.gz) — requires glibc 2.39 or newer. Browser mode needs Node.js and Chromium.
- [Venue starter SQLite database](venue-presets.sqlite) — 21 venue metadata records, with no watches, offers, or pending alerts.
- [Venue JSON export](venue-presets.json) — import this into an existing monitor using the [venue commands](../docs/venue-presets.md).

On GitHub's file page, use **Download raw file**. The Android app checks pages while open; continuous monitoring runs in the Rust service. No API credentials or private signing key are included.
