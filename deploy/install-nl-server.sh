#!/usr/bin/env bash
# New, isolated Debian installation. Does not start services or read credentials.
set -euo pipefail
usage() {
    cat <<'TEXT'
Usage: bash install-nl-server.sh [--plan | --install | --verify-bundle DIRECTORY]
Default: print the plan without changing the host or making network requests.
--install requires root on Debian 13 x86_64 with systemd, at least 2 GiB
available RAM and 5 GiB free on /opt. It installs dependencies and the verified
v0.8.0-server-test bundle under /opt/ticket-watch, then prepares private state.
The watcher is limited to one concurrent check, one CPU and 1536 MiB of memory.
Existing Ticket Watch paths cause a stop for inspection. No services are started
or enabled, no credentials are loaded, and no Docker apps are managed.
TEXT
}
verify_bundle() {
    python3 - "$1" <<'PY'
import hashlib, pathlib, re, sys, tarfile
root = pathlib.Path(sys.argv[1])
name = 'ticket-watch-bot-linux-x86_64.tar.gz'
if (root / name).stat().st_size > 32 * 1024 * 1024 or (root / (name + '.sha256')).stat().st_size > 256:
    raise SystemExit('Bundle download exceeds its bound')
manifest = (root / (name + '.sha256')).read_text()
match = re.fullmatch(r'([0-9a-f]{64})  ' + re.escape(name) + r'\n?', manifest)
if not match or hashlib.sha256((root / name).read_bytes()).hexdigest() != match[1]:
    raise SystemExit('Bundle SHA256 verification failed')
with tarfile.open(root / name) as archive:
    total = 0
    count = 0
    seen = set()
    for entry in archive:
        path = pathlib.PurePosixPath(entry.name)
        total += entry.size
        count += 1
        if (path.is_absolute() or '..' in path.parts or not path.parts or path.parts[0] != 'ticket-watch-bot'
            or not (entry.isdir() or entry.isfile()) or entry.mode & 0o7022 or path in seen
            or total > 250 * 1024 * 1024 or count > 10000):
            raise SystemExit('Bundle structure or permissions failed verification')
        seen.add(path)
print('Bundle SHA256 and archive structure verified')
PY
}
[[ $# -le 2 ]] || { usage >&2; exit 2; }
case "${1:---plan}" in
    --plan) [[ $# -le 1 ]] || { usage >&2; exit 2; }; usage; exit 0 ;;
    --verify-bundle) [[ $# == 2 ]] || { usage >&2; exit 2; }; verify_bundle "$2"; exit 0 ;;
    --install) [[ $# == 1 ]] || { usage >&2; exit 2; } ;;
    *) usage >&2; exit 2 ;;
esac
die() { printf '%s\n' "$*" >&2; exit 1; }
export PATH=/usr/sbin:/usr/bin:/sbin:/bin
[[ $EUID -eq 0 ]] || die '--install requires root inside the Debian VM.'
[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || die 'Requires Linux x86_64.'
source /etc/os-release
[[ ${ID:-} == debian && ${VERSION_ID:-} == 13 ]] || die 'Requires Debian 13.'
[[ -d /run/systemd/system ]] || die 'Requires a VM booted with systemd.'
for path in /opt /etc /etc/systemd /etc/systemd/system /run; do
    [[ -d "$path" && ! -L "$path" && $(stat -c %u "$path") == 0 ]] || die "Inspect system path: $path"
    mode=$(stat -c %a "$path")
    (( (8#$mode & 022) == 0 )) || die "System path is writable by other users: $path"
done
for path in /opt/ticket-watch /etc/ticket-watch-nl /var/lib/ticket-watch-nl \
    /var/cache/ticket-watch-nl /var/backups/ticket-watch-nl \
    /etc/systemd/system/ticket-watch-nl.service /etc/systemd/system/ticket-watch-nl.service.d; do
    [[ ! -e "$path" && ! -L "$path" ]] || die "Existing installation requires inspection: $path"
done
if getent passwd ticketwatch-nl >/dev/null || getent group ticketwatch-nl >/dev/null; then
    die 'Existing ticketwatch-nl account/group requires inspection.'
fi
available_kib=$(awk '/^MemAvailable:/{print $2}' /proc/meminfo)
[[ "$available_kib" =~ ^[0-9]+$ && "$available_kib" -ge 2097152 ]] || die 'At least 2 GiB available memory is required for this initial installation.'
available_disk_kib=$(df -Pk /opt | awk 'NR==2{print $4}')
[[ "$available_disk_kib" =~ ^[0-9]+$ && "$available_disk_kib" -ge 5242880 ]] || die 'At least 5 GiB free on /opt is required.'
umask 022
mkdir -m 700 /run/ticket-watch-install.lock || die 'Another installation is active; inspect /run/ticket-watch-install.lock.'
staging=''
cleanup() {
    [[ -z "$staging" ]] || rm -rf -- "$staging"
    rmdir /run/ticket-watch-install.lock
}
trap cleanup EXIT
staging=$(mktemp -d /opt/.ticket-watch-install.XXXXXX)
printf 'Installing Debian dependencies; existing Docker apps are not managed by this installer.\n'
apt-get update
DEBIAN_FRONTEND=noninteractive NEEDRESTART_MODE=l apt-get install -y --no-install-recommends \
    ca-certificates curl python3 tar nodejs npm chromium sqlite3 passwd util-linux
base='https://github.com/AtecHEvolutioN/Ticket/releases/download/v0.8.0-server-test'
archive='ticket-watch-bot-linux-x86_64.tar.gz'
for file in "$archive" "$archive.sha256"; do
    curl --fail --show-error --silent --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 20 --max-time 300 "$base/$file" -o "$staging/$file"
done
verify_bundle "$staging"
tar -xzf "$staging/$archive" -C "$staging" --no-same-owner --same-permissions
(cd "$staging/ticket-watch-bot" && sha256sum --check --status SHA256SUMS)
mv "$staging/ticket-watch-bot" /opt/ticket-watch
cd /opt/ticket-watch
PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --omit=dev --ignore-scripts
bash deploy/prepare-nl-server.sh --apply
# This is the newly created configuration; all existing paths were refused above.
python3 - <<'PY'
from pathlib import Path
p = Path('/etc/ticket-watch-nl/config.toml')
data = p.read_text()
if data.count('max_in_flight = 2\n') != 1:
    raise SystemExit('Prepared configuration needs inspection')
p.write_text(data.replace('max_in_flight = 2\n', 'max_in_flight = 1\n'))
PY
install -d -o root -g root -m 755 /etc/systemd/system/ticket-watch-nl.service.d
install -o root -g root -m 644 deploy/ticket-watch-nl.resources-shared-vm.conf.example \
    /etc/systemd/system/ticket-watch-nl.service.d/10-resources.conf
systemctl daemon-reload
printf '%s\n' 'INSTALLATION PREPARED. Watcher remains stopped and disabled.' \
    'Next: privately configure credentials, validate source access, then activate the reviewed pilot.' \
    'No API keys or bot tokens were requested or printed.'
