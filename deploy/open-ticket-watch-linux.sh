#!/usr/bin/env bash
# Install the official Discord Linux build for this user and open Ticket Watch.
set -euo pipefail
mode="${1:-open}"
if [[ $# -gt 1 || ! "$mode" =~ ^(open|--install-only|--browser-only|--check)$ ]]; then
    printf 'Usage: bash deploy/open-ticket-watch-linux.sh [--install-only|--browser-only|--check]\n' >&2
    exit 2
fi
if [[ "$(uname -s)" != Linux ]]; then
    printf 'This launcher requires Linux.\n' >&2
    exit 1
fi
app_dir="${XDG_DATA_HOME:-$HOME/.local/share}/ticket-watch/discord"
discord_bin="$(command -v discord || true)"
if [[ -z "$discord_bin" ]]; then
    for candidate in "$app_dir/Discord/discord" "$app_dir/Discord/Discord"; do
        if [[ -x "$candidate" ]]; then discord_bin="$candidate"; break; fi
    done
fi
if [[ "$mode" == --check ]]; then
    if [[ -n "$discord_bin" ]]; then printf 'Discord: installed (%s)\n' "$discord_bin";
    elif command -v flatpak >/dev/null && flatpak info com.discordapp.Discord >/dev/null 2>&1; then printf 'Discord: installed through Flatpak\n';
    else printf 'Discord: installation required\n'; fi
    if [[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then printf 'Desktop display: available\n';
    else printf 'Desktop display: unavailable in this terminal\n'; fi
    printf 'Bot channels: https://discord.com/channels/1569717598709350491/1569753975182332027\n'
    printf 'Charts: https://ticket-discord.vercel.app/market.html\n'
    printf 'Continuous collection requires the separately configured watcher host.\n'
    exit 0
fi
flatpak_discord=false
if [[ -z "$discord_bin" ]] && command -v flatpak >/dev/null && flatpak info com.discordapp.Discord >/dev/null 2>&1; then
    flatpak_discord=true
fi
if [[ "$mode" != --browser-only && -z "$discord_bin" && "$flatpak_discord" == false ]]; then
    if [[ "$(uname -m)" != x86_64 ]]; then
        printf 'The official Discord archive requires x86_64. Install Discord for your architecture or use --browser-only.\n' >&2
        exit 1
    fi
    for tool in curl tar python3; do
        command -v "$tool" >/dev/null || { printf 'Install %s with your distribution package manager first.\n' "$tool" >&2; exit 1; }
    done
    mkdir -p "$app_dir"
    staging="$(mktemp -d "$app_dir/.install.XXXXXX")"
    trap 'rm -rf "$staging"' EXIT
    printf 'Downloading the official Discord Linux build over HTTPS...\n'
    curl --fail --show-error --silent --location --proto '=https' --proto-redir '=https' \
        --connect-timeout 20 --max-time 300 \
        'https://discord.com/api/download?platform=linux&format=tar.gz' \
        --output "$staging/discord.tar.gz"
    # Inspect the archive before extracting. Reject links escaping its single
    # Discord tree and any device/FIFO entries; never preserve setuid ownership.
    python3 - "$staging/discord.tar.gz" <<'PY'
import pathlib, sys, tarfile
with tarfile.open(sys.argv[1]) as archive:
    for member in archive:
        name = pathlib.PurePosixPath(member.name)
        if name.is_absolute() or '..' in name.parts or not name.parts or name.parts[0] != 'Discord':
            raise SystemExit('Unexpected path in Discord download')
        if not (member.isfile() or member.isdir() or member.issym() or member.islnk()):
            raise SystemExit('Unexpected entry in Discord download')
        if member.issym() or member.islnk():
            target = pathlib.PurePosixPath(member.linkname)
            if target.is_absolute() or '..' in target.parts:
                raise SystemExit('Unexpected link in Discord download')
PY
    tar --extract --gzip --file "$staging/discord.tar.gz" --directory "$staging" --no-same-owner --no-same-permissions
    binary_name=''
    for candidate in discord Discord; do
        if [[ -x "$staging/Discord/$candidate" ]]; then binary_name="$candidate"; break; fi
    done
    [[ -n "$binary_name" ]] || { printf 'Discord download did not contain its executable.\n' >&2; exit 1; }
    [[ ! -e "$app_dir/Discord" ]] || { printf 'An existing local Discord directory needs inspection before replacement: %s\n' "$app_dir/Discord" >&2; exit 1; }
    mv "$staging/Discord" "$app_dir/Discord"
    discord_bin="$app_dir/Discord/$binary_name"
    printf 'Discord installed for this user: %s\n' "$discord_bin"
fi
if [[ "$mode" == --install-only ]]; then
    printf 'Run this launcher without arguments from your desktop terminal to open Discord and the charts.\n'
    exit 0
fi
if [[ -z "${DISPLAY:-}${WAYLAND_DISPLAY:-}" ]]; then
    printf 'Discord is ready, but this terminal has no graphical display. Run this launcher in your Linux desktop terminal.\n' >&2
    exit 1
fi
command -v xdg-open >/dev/null || { printf 'Install xdg-utils first so the launcher can open your browser.\n' >&2; exit 1; }
channel='https://discord.com/channels/1569717598709350491/1569753975182332027'
if [[ "$mode" == --browser-only ]]; then
    xdg-open "$channel" >/dev/null 2>&1 &
elif [[ "$flatpak_discord" == true ]]; then
    flatpak run com.discordapp.Discord 'discord://-/channels/1569717598709350491/1569753975182332027' >/dev/null 2>&1 &
else
    "$discord_bin" 'discord://-/channels/1569717598709350491/1569753975182332027' >/dev/null 2>&1 &
fi
xdg-open 'https://ticket-discord.vercel.app/market.html' >/dev/null 2>&1 &
printf 'Opening Discord and Ticket Watch charts. Sign into Discord, open RE:ENTRY → bot-status, then try /ticket ping or /ticket chart.\n'
printf 'Use “Try synthetic demo” in the chart viewer. Live updates require the separate watcher/API host.\n'
