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
if ! command -v python3 >/dev/null 2>&1; then
    printf 'Packaging requires Python 3 for the public-file manifest and build metadata.\n' >&2
    exit 1
fi
stage_dir="$(mktemp -d /tmp/ticket-bot-package.XXXXXX)"
trap 'rm -rf "$stage_dir"' EXIT
bundle_dir="$stage_dir/ticket-watch-bot"
mkdir -p "$bundle_dir/target/release" "$output_dir"
cp "$binary_path" "$bundle_dir/target/release/ticket-watch"
chmod 755 "$bundle_dir/target/release/ticket-watch"
for entry in src browser server tests docs examples deploy; do
    cp -R "$repo_dir/$entry" "$bundle_dir/$entry"
done
for entry in Cargo.toml Cargo.lock package.json package-lock.json config.example.toml config.map-monitor.example.toml README.md HANDOFF.md; do
    cp "$repo_dir/$entry" "$bundle_dir/$entry"
done
# Rust embeds these exact public assets at compile time. The browser regression
# also reads the reviewed CSP in vercel.json; no app secrets or API code is copied.
mkdir -p "$bundle_dir/apps/discord/public"
for entry in market.html market.css market.js; do
    cp "$repo_dir/apps/discord/public/$entry" "$bundle_dir/apps/discord/public/$entry"
done
cp "$repo_dir/apps/discord/vercel.json" "$bundle_dir/apps/discord/vercel.json"
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
The included binary targets Linux x86_64. BUILD-INFO.json records its version,
SHA256 and detected minimum glibc version, plus the source revision and dirty flag
at packaging time. That source revision alone does not prove binary provenance.
Rust source is included to rebuild on other supported systems.
Browser scraping needs Node.js 20+ and Chromium (npm ci; npm run browser:install).
Check the extracted files with: sha256sum -c SHA256SUMS
The three public chart assets required to rebuild the Rust binary are included.

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

For the bounded all-artists Netherlands catalog, copy
examples/netherlands-all.example.toml to config.nl.local.toml at this folder's
root. Supply TICKETMASTER_API_KEY privately, then use:
   ./target/release/ticket-watch --config config.nl.local.toml validate
   ./target/release/ticket-watch --config config.nl.local.toml sync
   ./target/release/ticket-watch --config config.nl.local.toml catalog
Review the reported coverage before running the monitor. Resumable date windows
cover a configured 730-day horizon, retaining at most 10,000 events. Undated/TBA
events, saturated windows and storage/request limits remain explicit gaps.
Priority scheduling favors imminent sales and recent changes, with fair older
checks; the 60-second host interval still makes a 1,000-event sweep take over
16 hours. This is not a second-level polling promise. Choose the destination explicitly; the server's
Discord channel layout does not automatically route these watcher messages.

For an always-on Debian 13 server, first review the offline preparation plan:
   bash deploy/prepare-nl-server.sh --plan
The dedicated NL service starts with its subscription disabled and no channel.
Preparation does not start the service, load credentials or send Discord messages.

Read docs/running.md for capture, profile verification, quantity/price semantics,
Discovery integration, pacing, and service deployment. No credentials, local
database, private capture, or production deployment are included.

To populate the local venue registry, run:
   ./target/release/ticket-watch venues import examples/venue-presets.json
Read docs/venue-presets.md for matching and separate seating-layout references.
The 29 starter venue records contain no invented seating maps or live inventory.

Price and observed-entry history starts when this version records observations.
Export an existing database to a new standalone offline chart filename:
   ./target/release/ticket-watch history-export --database data/ticket-nl.sqlite --output nl-history.html
Use --event-url and --from/--to to narrow large exports. Keep the source database
path consistent with your configuration; no database is included in this package.
Supported Czech map quotes also have individual section/row/seat histories.
Manual Dutch inspection has validated primary/resale seat prices at two venues;
automatic Dutch section scans and seat-history collection remain disabled.
These are asking-price observations, not completed sales or total event stock.
Read docs/price-history.md for JSON exports, per-seat views and evidence gaps.

For authenticated live charts, server/history-api.cjs provides a loopback,
read-only API using the fixed configured binary and databases. Install a private
TICKET_HISTORY_TOKEN and use an HTTPS reverse proxy for remote access. The hosted
viewer connects only after you enter the endpoint and token; imported files and
standalone snapshots remain local. Read docs/live-history-api.md before setup.

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
# Refuse accidental local state under any copied public tree. The manifest
# hashes the actual packaged files, including uncommitted public source changes.
python3 - "$repo_dir" "$bundle_dir" <<'PY'
import datetime
import hashlib
import json
import pathlib
import re
import subprocess
import sys

repository, bundle = map(pathlib.Path, sys.argv[1:])
for path in bundle.rglob('*'):
    name = path.name.lower()
    forbidden = (
        path.is_symlink()
        or name in {'.git', 'node_modules', 'captures', 'config.local.toml', '.env'}
        or name.startswith('.env.')
        or name.endswith(('.local.toml', '.sqlite', '.sqlite3', '.sqlite-wal', '.sqlite-shm', '.db', '.pem', '.key', '.p12', '.pfx'))
    )
    if forbidden:
        raise SystemExit('Refusing to package local state, credential files or symbolic links')

# Only staged, reviewed public files are normalized. Source workspace modes
# remain unchanged; a dedicated service account must be able to read the worker.
bundle.chmod(0o755)
for path in bundle.rglob('*'):
    if path.is_dir():
        path.chmod(0o755)
    elif path.is_file():
        path.chmod(0o755 if path.suffix == '.sh' else 0o644)
binary = bundle / 'target/release/ticket-watch'
binary.chmod(0o755)
version = subprocess.check_output([str(binary), '--version'], text=True).strip()
source_version = re.search(r'^version\s*=\s*"([0-9A-Za-z.+-]+)"', (bundle / 'Cargo.toml').read_text(), re.MULTILINE)
if not source_version or version != 'ticket-watch ' + source_version.group(1):
    raise SystemExit('Binary package version does not match the bundled Cargo.toml')

revision, dirty = None, None
try:
    top = subprocess.check_output(['git', '-C', str(repository), 'rev-parse', '--show-toplevel'], text=True, stderr=subprocess.DEVNULL).strip()
    if pathlib.Path(top).resolve() == repository.resolve():
        revision = subprocess.check_output(['git', '-C', str(repository), 'rev-parse', 'HEAD'], text=True).strip()
        dirty = bool(subprocess.check_output(['git', '-C', str(repository), 'status', '--porcelain', '--untracked-files=normal'], text=True))
except (FileNotFoundError, subprocess.CalledProcessError):
    pass

minimum_glibc = None
try:
    versions = subprocess.check_output(['readelf', '--version-info', str(binary)], text=True, stderr=subprocess.DEVNULL)
    required = set(re.findall(r'\bGLIBC_([0-9]+(?:\.[0-9]+)+)\b', versions))
    if required:
        minimum_glibc = max(required, key=lambda value: tuple(map(int, value.split('.'))))
except (FileNotFoundError, subprocess.CalledProcessError):
    pass

def sha256(path):
    result = hashlib.sha256()
    with path.open('rb') as handle:
        for chunk in iter(lambda: handle.read(1024 * 1024), b''):
            result.update(chunk)
    return result.hexdigest()

metadata = {
    'schema_version': 1,
    'packaged_at': datetime.datetime.now(datetime.timezone.utc).isoformat(timespec='seconds'),
    'binary_target': 'linux-x86_64',
    'binary_version': version,
    'binary_path': 'target/release/ticket-watch',
    'binary_sha256': sha256(binary),
    'minimum_glibc': minimum_glibc,
    'source_revision_at_packaging': revision,
    'source_tree_dirty_at_packaging': dirty,
    'binary_source_revision_independently_verified': False,
}
(bundle / 'BUILD-INFO.json').write_text(json.dumps(metadata, indent=2, sort_keys=True) + '\n')
(bundle / 'BUILD-INFO.json').chmod(0o644)
files = sorted(path for path in bundle.rglob('*') if path.is_file())
with (bundle / 'SHA256SUMS').open('w') as manifest:
    for path in files:
        relative = path.relative_to(bundle).as_posix()
        if '\n' in relative or '\r' in relative or '\\' in relative:
            raise SystemExit('Unsupported filename in package manifest')
        manifest.write(f'{sha256(path)}  {relative}\n')
(bundle / 'SHA256SUMS').chmod(0o644)
PY
tar -czf "$output_dir/ticket-watch-bot-linux-x86_64.tar.gz" -C "$stage_dir" ticket-watch-bot
(cd "$output_dir" && sha256sum ticket-watch-bot-linux-x86_64.tar.gz > ticket-watch-bot-linux-x86_64.tar.gz.sha256)
chmod 644 "$output_dir/ticket-watch-bot-linux-x86_64.tar.gz" "$output_dir/ticket-watch-bot-linux-x86_64.tar.gz.sha256"
printf 'Bot package ready: %s\n' "$output_dir/ticket-watch-bot-linux-x86_64.tar.gz"
