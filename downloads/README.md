# Downloads and venue presets

The Linux worker is the **v0.8.0-server-test** package; Android remains at
**v0.7.0-test**. The venue catalog on `main` was updated on 10 November 2026.
Use each file's matching `.sha256` file to verify the download.

- [Android APK](ticket-watch-android.apk) — version 0.7.0-test, Android 8.0 or newer. Editable ticket preferences with a default quantity of two; retains the signing certificate from earlier local test builds.
- [Linux x86_64 bot archive](ticket-watch-bot-linux-x86_64.tar.gz) — current worker, broader discovery, sale-time priorities, live chart API and Proxmox installer. Requires glibc 2.39 or newer; browser mode needs Node.js and Chromium. [Installation guide](../docs/proxmox-deployment.md).
- [Venue starter SQLite database](venue-presets.sqlite) — 29 venues and six draft Dutch static layouts, with no watches, offers, or pending alerts.
- [Venue JSON export](venue-presets.json) — import this into an existing monitor using the [venue commands](../docs/venue-presets.md).

On GitHub's file page, use **Download raw file**. The Android app checks pages while open; continuous monitoring runs in the separately configured Rust service. No API credentials or private signing key are included. Automatic carting and phone/server synchronization remain inactive. Dutch seat inspection has limited live validation; unattended access and reservations still need validation.

The files are committed here because the release-asset upload endpoint rejected the available credentials. App/bot release links point to the version tag so those builds remain pinned. The latest venue-catalog links use `main`; tagged catalogs retain their earlier contents. Older builds remain available at their earlier Git tags.
