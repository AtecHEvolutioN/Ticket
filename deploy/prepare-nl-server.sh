#!/usr/bin/env bash
# Prepare a reviewed, already installed checkout. No remote calls or service start.
set -euo pipefail
umask 077

readonly APP_DIR=/opt/ticket-watch
readonly ACCOUNT=ticketwatch-nl
readonly CONFIG_DIR=/etc/ticket-watch-nl
readonly STATE_DIR=/var/lib/ticket-watch-nl
readonly CACHE_DIR=/var/cache/ticket-watch-nl
readonly BACKUP_DIR=/var/backups/ticket-watch-nl
readonly UNIT_PATH=/etc/systemd/system/ticket-watch-nl.service
readonly BINARY="$APP_DIR/target/release/ticket-watch"

die() { printf '%s\n' "$*" >&2; exit 1; }
usage() {
    cat <<'TEXT'
Usage: deploy/prepare-nl-server.sh [--plan | --check | --apply | --help]
  --plan   Print the preparation plan (default); change nothing.
  --check  Check an installed /opt/ticket-watch checkout and prerequisites offline.
  --apply  As root, prepare the local account, private paths, and new config/unit files.
No mode installs software, copies application code, loads secrets, sends messages,
fetches tickets, starts/enables services, or overwrites existing files/databases.
TEXT
}
plan() {
    cat <<'TEXT'
Target: Debian 13, Linux x86_64, systemd; /opt/ticket-watch is operator-managed.
Prepare these prerequisites separately (network installation is NOT run here):
  sudo apt-get update
  sudo apt-get install --no-install-recommends ca-certificates nodejs npm chromium systemd passwd util-linux sqlite3
Place the reviewed checkout/release binary at /opt/ticket-watch, owned by root,
with code and dependencies not writable by the service account. On that checkout:
  cd /opt/ticket-watch
  sudo env PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --omit=dev --ignore-scripts
Node must be version 20+; the current bundled binary needs glibc 2.39+.
The installer checks binary --version, local Playwright resolution, and TOML validate.

Explicit local preparation:
  sudo bash /opt/ticket-watch/deploy/prepare-nl-server.sh --check
  sudo bash /opt/ticket-watch/deploy/prepare-nl-server.sh --apply
Creates the system account ticketwatch-nl and only missing paths/files:
  /etc/ticket-watch-nl/config.toml          root:ticketwatch-nl 0640
  /etc/ticket-watch-nl/environment          root:root 0600 (comments only)
  /etc/ticket-watch-nl/send-discord.conf.example (inactive example)
  /var/lib/ticket-watch-nl                  ticketwatch-nl, 0700
  /var/cache/ticket-watch-nl                ticketwatch-nl, 0700
  /var/backups/ticket-watch-nl              root, 0700
  /etc/systemd/system/ticket-watch-nl.service
Existing configuration, state, secrets and unit files are retained unchanged.
The subscription starts disabled, with no destination channel and no map watches.
Only systemctl daemon-reload is run; no enable/start/restart or Discord delivery.
After preparation, privately edit environment/config, review the bounded NL scope,
validate access and delivery, and explicitly enable/start the service when ready.
The 1000-event cap and 60-second host interval cannot provide second-level freshness.
TEXT
}

[[ $# -le 1 ]] || { usage >&2; exit 2; }
mode=${1:---plan}
case "$mode" in
    --plan) plan; exit 0 ;;
    --help|-h) usage; exit 0 ;;
    --check|--apply) ;;
    *) usage >&2; exit 2 ;;
esac

# Root operations use only standard Debian executable locations.
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
for tool in uname stat find realpath getent id install useradd groupadd runuser \
    systemctl systemd-analyze mktemp ln rm mkdir chown chmod env dirname; do
    command -v "$tool" >/dev/null || die "Missing required command: $tool"
done
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || die 'Requires Linux x86_64.'
[[ -r /etc/os-release ]] || die 'Cannot identify the target operating system.'
# /etc/os-release is the trusted operating-system file, never a supplied argument.
source /etc/os-release
[[ ${ID:-} == debian && ${VERSION_ID:-} == 13 ]] || die 'This template targets Debian 13.'
[[ "$mode" != --apply || $EUID -eq 0 ]] || die '--apply requires local root privileges.'
[[ -d /run/systemd/system ]] || die 'The target must be booted with systemd.'
[[ -x /usr/bin/node && -x /usr/bin/chromium ]] || die 'Install Debian nodejs and chromium first.'
node_major=$(/usr/bin/node -p 'process.versions.node.split(".")[0]')
[[ $node_major =~ ^[0-9]+$ && $node_major -ge 20 ]] || die 'Node.js 20 or newer is required.'
/usr/bin/chromium --version

check_root_path() {
    local path=$1 permission
    [[ ! -L "$path" && -e "$path" ]] || die "Missing or symbolic-link path: $path"
    [[ $(stat -c %u -- "$path") == 0 ]] || die "Expected root ownership: $path"
    permission=$(stat -c %a -- "$path")
    (( (8#$permission & 022) == 0 )) || die "Group/other writable root path: $path"
}
for path in / /opt "$APP_DIR" /etc /var /var/lib /var/cache /var/backups \
    /etc/systemd /etc/systemd/system; do
    check_root_path "$path"
done
[[ -x "$BINARY" ]] || die 'Install the reviewed release binary at /opt/ticket-watch/target/release/ticket-watch.'
for path in target target/release target/release/ticket-watch browser browser/worker.cjs \
    node_modules deploy deploy/nl-service.example.toml deploy/ticket-watch-nl.service \
    deploy/ticket-watch-nl.send-discord.conf.example; do
    check_root_path "$APP_DIR/$path"
done
unsafe=$(find "$APP_DIR" -xdev \( -type f -o -type d \) \( ! -user root -o -perm /022 \) -print -quit)
[[ -z "$unsafe" ]] || die "Application tree must be root-owned and not group/other writable: $unsafe"
while IFS= read -r -d '' path; do
    resolved=$(realpath -e -- "$path") || die 'Broken application symlink.'
    [[ "$resolved" == "$APP_DIR/"* ]] || die "Application symlink leaves the reviewed tree: $path"
done < <(find "$APP_DIR" -xdev -type l -print0)

# No environment file is sourced, and no credential is inherited by validation.
offline() {
    if [[ $EUID -eq 0 ]]; then
        runuser -u nobody -- env -i PATH=/usr/bin:/bin HOME=/nonexistent "$@"
    else
        env -i PATH=/usr/bin:/bin HOME=/nonexistent "$@"
    fi
}
offline "$BINARY" --version
offline /usr/bin/node -e 'require("node:fs").readFileSync("/opt/ticket-watch/browser/worker.cjs"); console.log("Browser worker readable by unprivileged account")'
offline /usr/bin/node -e 'require(require.resolve("playwright", {paths:["/opt/ticket-watch"]})); console.log("Playwright dependency resolved offline")'
offline "$BINARY" --config "$APP_DIR/deploy/nl-service.example.toml" validate
systemd-analyze verify "$APP_DIR/deploy/ticket-watch-nl.service"

if getent passwd "$ACCOUNT" >/dev/null; then
    IFS=: read -r _ _ account_uid account_gid _ account_home account_shell < <(getent passwd "$ACCOUNT")
    [[ $account_uid -ne 0 && "$account_home" == "$STATE_DIR" && "$account_shell" == /usr/sbin/nologin ]] \
        || die 'Existing ticketwatch-nl account does not match the dedicated service account.'
    [[ $(id -gn "$ACCOUNT") == "$ACCOUNT" && $(id -G "$ACCOUNT") == "$account_gid" ]] \
        || die 'Existing service account must use only its dedicated primary group.'
elif getent group "$ACCOUNT" >/dev/null; then
    die 'A group named ticketwatch-nl already exists without the service account; review it manually.'
fi

check_existing() {
    local path=$1 owner=$2 group=$3 mode=$4 type=$5
    [[ -e "$path" || -L "$path" ]] || return 0
    [[ ! -L "$path" ]] || die "Refusing existing symbolic link: $path"
    if [[ "$type" == directory ]]; then
        [[ -d "$path" ]] || die "Expected directory: $path"
    else
        [[ -f "$path" ]] || die "Expected regular file: $path"
    fi
    [[ $(stat -c %U -- "$path") == "$owner" && $(stat -c %G -- "$path") == "$group" \
        && $(stat -c %a -- "$path") == "$mode" ]] || die "Review existing ownership/mode; will not change: $path"
}
check_existing "$CONFIG_DIR" root "$ACCOUNT" 750 directory
check_existing "$STATE_DIR" "$ACCOUNT" "$ACCOUNT" 700 directory
check_existing "$CACHE_DIR" "$ACCOUNT" "$ACCOUNT" 700 directory
check_existing "$BACKUP_DIR" root root 700 directory
check_existing "$CONFIG_DIR/config.toml" root "$ACCOUNT" 640 file
check_existing "$CONFIG_DIR/environment" root root 600 file
check_existing "$CONFIG_DIR/send-discord.conf.example" root "$ACCOUNT" 640 file
check_existing "$UNIT_PATH" root root 644 file
if [[ -f "$CONFIG_DIR/config.toml" ]]; then
    if [[ $EUID -eq 0 ]]; then
        runuser -u "$ACCOUNT" -- env -i PATH=/usr/bin:/bin HOME="$STATE_DIR" \
            "$BINARY" --config "$CONFIG_DIR/config.toml" validate
    else
        env -i PATH=/usr/bin:/bin HOME="$STATE_DIR" "$BINARY" --config "$CONFIG_DIR/config.toml" validate
    fi
fi
[[ "$mode" == --apply ]] || { printf 'Offline preflight passed. Nothing changed.\n'; exit 0; }

if ! getent passwd "$ACCOUNT" >/dev/null; then
    useradd --system --user-group --home-dir "$STATE_DIR" --no-create-home --shell /usr/sbin/nologin "$ACCOUNT"
fi
new_directory() {
    local path=$1 owner=$2 group=$3 mode=$4
    [[ ! -e "$path" ]] || return 0
    mkdir -m "$mode" -- "$path"
    chown "$owner:$group" -- "$path"
}
new_directory "$CONFIG_DIR" root "$ACCOUNT" 750
new_directory "$STATE_DIR" "$ACCOUNT" "$ACCOUNT" 700
new_directory "$CACHE_DIR" "$ACCOUNT" "$ACCOUNT" 700
new_directory "$BACKUP_DIR" root root 700

temporary=''
trap '[[ -z "$temporary" ]] || rm -f -- "$temporary"' EXIT
publish_new() {
    local source=$1 destination=$2 group=$3 mode=$4
    if [[ -e "$destination" || -L "$destination" ]]; then
        printf 'Retained existing file: %s\n' "$destination"
        return
    fi
    temporary=$(mktemp "$(dirname "$destination")/.ticket-watch-nl.XXXXXX")
    if [[ "$source" == environment-template ]]; then
        printf '%s\n' '# Edit privately; systemd environment syntax, not shell commands.' \
            '# TICKETMASTER_API_KEY=replace_privately' '# DISCORD_BOT_TOKEN=replace_privately' > "$temporary"
        chown "root:$group" -- "$temporary"
        chmod "$mode" -- "$temporary"
    else
        install -o root -g "$group" -m "$mode" -- "$source" "$temporary"
    fi
    # Atomic no-clobber publication: another file appearing now is never replaced.
    ln -T -- "$temporary" "$destination"
    rm -f -- "$temporary"
    temporary=''
}
publish_new "$APP_DIR/deploy/nl-service.example.toml" "$CONFIG_DIR/config.toml" "$ACCOUNT" 640
publish_new environment-template "$CONFIG_DIR/environment" root 600
publish_new "$APP_DIR/deploy/ticket-watch-nl.send-discord.conf.example" "$CONFIG_DIR/send-discord.conf.example" "$ACCOUNT" 640
publish_new "$APP_DIR/deploy/ticket-watch-nl.service" "$UNIT_PATH" root 644
runuser -u "$ACCOUNT" -- env -i PATH=/usr/bin:/bin HOME="$STATE_DIR" \
    "$BINARY" --config "$CONFIG_DIR/config.toml" validate
systemctl daemon-reload
printf '%s\n' 'Prepared locally. Service was not started or enabled; no secrets loaded or messages sent.' \
    'Review /etc/ticket-watch-nl/config.toml and privately edit /etc/ticket-watch-nl/environment.' \
    'Existing files and databases were not overwritten. Backups must use SQLite backup or a stopped service.'
