#!/usr/bin/env bash
# Package explicit public source paths and the built Linux binary; never local configs or captures.
set -euo pipefail
repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
binary_path="$repo_dir/target/release/ticket-watch"
output_dir="$repo_dir/dist"
if [[ "$(uname -s)" != Linux || "$(uname -m)" != x86_64 ]]; then
    printf 'This packaging target requires a Linux x86_64 build.\n' >&2
    exit 1
fi
if [[ ! -x "$binary_path" ]]; then
    printf 'Build first: cargo build --release --locked\n' >&2
    exit 1
fi
stage_dir="$(mktemp -d /tmp/ticket-bot-package.XXXXXX)"
trap 'rm -rf "$stage_dir"' EXIT
bundle_dir="$stage_dir/ticket-watch-bot"
mkdir -p "$bundle_dir/target/release" "$output_dir"
cp "$binary_path" "$bundle_dir/target/release/ticket-watch"
chmod 755 "$bundle_dir/target/release/ticket-watch"
for entry in src browser tests docs examples deploy; do
    cp -R "$repo_dir/$entry" "$bundle_dir/$entry"
done
for entry in Cargo.toml Cargo.lock package.json package-lock.json config.example.toml README.md; do
    cp "$repo_dir/$entry" "$bundle_dir/$entry"
done
# These reviewed public screenshots/data support the capture and coverage reports. Never copy raw HTML.
for entry in korn-live-tickets.png korn-live-resale.png korn-live-capture.json ticketmaster-coverage.json coverage-czech.png coverage-dutch.png coverage-spanish.png android-venues.png android-ticket-preferences.png android-event-preferences.png android-rust-preferences-contract.json; do
    if [[ -f "$repo_dir/dist/$entry" ]]; then
        mkdir -p "$bundle_dir/dist"
        cp "$repo_dir/dist/$entry" "$bundle_dir/dist/$entry"
    fi
done
mkdir -p "$bundle_dir/android"
for entry in README.md BUILDING.md TESTING.md; do
    cp "$repo_dir/android/$entry" "$bundle_dir/android/$entry"
done
cat > "$bundle_dir/START-HERE.txt" <<'TEXT'
TICKET WATCH SCRAPING BOT

This is the server/computer backend, not an Android application.
The included binary targets Linux x86_64 and requires glibc 2.39 or newer.
Rust source is included to rebuild on other supported systems.
Browser scraping needs Node.js 20+ and Chromium (npm ci; npm run browser:install).

1. From this extracted folder, try the fully offline demo:
   ./target/release/ticket-watch --config examples/offline-demo.toml replay --watch demo tests/fixtures/synthetic-unavailable.html tests/fixtures/synthetic-offers.html tests/fixtures/synthetic-offers.html

2. Copy config.example.toml to config.local.toml and configure selected events.
   The example uses the built-in event-independent Ticketmaster layout adapter.
   Replace the disabled placeholder URL and labels, then enable the selected watch.
   Or inspect any event with: ./target/release/ticket-watch inspect --url PUBLIC_EVENT_URL
   Use npm run capture -- --adapter ticketmaster --url PUBLIC_EVENT_URL --output captures/event.html
   on a graphical desktop where the public event is accessible.
   Review a saved capture without event-specific selectors:
   ./target/release/ticket-watch inspect --url PUBLIC_EVENT_URL --html captures/event.html

3. After validating the source and profile, run the monitor in preview mode:
   ./target/release/ticket-watch --config config.local.toml run
   Configure DISCORD_BOT_TOKEN privately and the destination in TOML before
   explicitly adding --send-discord for real message delivery.

Read docs/running.md for capture, profile verification, quantity/price semantics,
Discovery integration, pacing, and service deployment. No credentials, local
database, private capture, or production deployment are included.

To populate the local venue registry, run:
   ./target/release/ticket-watch venues import examples/venue-presets.json
Read docs/venue-presets.md for matching and separate seating-layout references.
The 21 starter venue records contain no invented seating maps or live inventory.

Ticket preferences default to two adjacent tickets and the cheapest eligible group.
Export defaults and per-event choices from Android Settings > Ticket preferences,
then validate the saved file with:
   ./target/release/ticket-watch cart-preferences --file ticket-preferences.json
Apply it to supplied quotes with:
   ./target/release/ticket-watch cart-plan quotes.json --preferences ticket-preferences.json
Read docs/ticket-preferences.md for quantity, budget, category and seat filters.
This remains offline planning: phone/server syncing and live carting are not active.

The reusable regional adapter opens known best-available tabs and extracts
category ranges plus explicit resale listing quantities for any event using that
layout. Unknown layouts remain unsupported. Sustained live access and delivery
are unverified. Read docs/event-adapters.md for supported fields and operation.
TEXT
tar -czf "$output_dir/ticket-watch-bot-linux-x86_64.tar.gz" -C "$stage_dir" ticket-watch-bot
(cd "$output_dir" && sha256sum ticket-watch-bot-linux-x86_64.tar.gz > ticket-watch-bot-linux-x86_64.tar.gz.sha256)
chmod 644 "$output_dir/ticket-watch-bot-linux-x86_64.tar.gz" "$output_dir/ticket-watch-bot-linux-x86_64.tar.gz.sha256"
printf 'Bot package ready: %s\n' "$output_dir/ticket-watch-bot-linux-x86_64.tar.gz"
