#!/usr/bin/env bash
# Connect a local Arch desktop Codex session; it performs and verifies the pilot.
set -euo pipefail
if [[ $# -gt 1 ]]; then
    printf 'Usage: bash start-arch-local-pilot.sh [--plan | --start]\n' >&2
    exit 2
fi
case "${1:---plan}" in
    --plan)
        printf '%s\n' 'Target: your regular Arch desktop user with a graphical session.' \
            'Reuse/install official Codex for that user, sign in privately and open ~/ticket-local-pilot.' \
            'The local session clones/reviews Ticket, configures the pilot and validates live prices, Discord and history.' \
            'This plan makes no changes. No VM or Proxmox connection is used.'
        exit 0 ;;
    --start) ;;
    *) printf 'Usage: bash start-arch-local-pilot.sh [--plan | --start]\n' >&2; exit 2 ;;
esac
[[ "$(uname -s)" == Linux && $EUID -ne 0 ]] || { printf 'Run this in the Arch desktop terminal as your regular user.\n' >&2; exit 1; }
source /etc/os-release
[[ ${ID:-} == arch || " ${ID_LIKE:-} " == *' arch '* ]] || { printf 'This launcher targets the Arch desktop, not the Debian VM or Proxmox node.\n' >&2; exit 1; }
[[ -n "${DISPLAY:-}${WAYLAND_DISPLAY:-}" && -t 0 ]] || { printf 'Open a terminal inside the Arch graphical desktop first.\n' >&2; exit 1; }
codex_binary="$(command -v codex || true)"
if [[ -z "$codex_binary" && -x "$HOME/.local/bin/codex" ]]; then
    codex_binary="$HOME/.local/bin/codex"
fi
if [[ -z "$codex_binary" ]]; then
    command -v curl >/dev/null || { printf 'curl is required to download the official Codex installer.\n' >&2; exit 1; }
    staging="$(mktemp -d /tmp/ticket-local-codex.XXXXXX)"
    trap 'rm -rf "$staging"' EXIT
    curl --fail --show-error --silent --location --proto '=https' --proto-redir '=https' --max-time 60 \
        https://chatgpt.com/codex/install.sh -o "$staging/install-codex.sh"
    CODEX_NON_INTERACTIVE=1 sh "$staging/install-codex.sh"
    codex_binary="$HOME/.local/bin/codex"
    [[ -x "$codex_binary" ]] || { printf 'Check the official installer result before continuing.\n' >&2; exit 1; }
    rm -rf "$staging"
    trap - EXIT
fi
workspace="$HOME/ticket-local-pilot"
[[ ! -L "$workspace" ]] || { printf 'The pilot directory is a link; inspect its destination first.\n' >&2; exit 1; }
mkdir -p "$workspace"
cd "$workspace"
"$codex_binary" login status >/dev/null 2>&1 || "$codex_binary" login --device-auth
mission='Set up and fully validate Ticket Watch on THIS Arch Linux desktop first. Work in this local folder. If it is empty, clone https://github.com/AtecHEvolutioN/Ticket.git here; inspect/preserve existing files otherwise. Read docs/arch-local-pilot.md and HANDOFF.md from the current repository, fetching the public guide if needed. Install/configure the required local runtime, use a visible ordinary browser with manual verification when required, diagnose unsupported pages, and validate real ticket prices/types, one Discord test message and automatic local history/chart updates. Ask for credentials only through a private local terminal workflow, never chat. Keep the always-on Debian VM paused. Do not declare success from HTTP200, a running process or synthetic data alone. Continue useful independent work when a credential or human step is pending; report any actual blocker precisely. Preserve other applications and keep automatic carting disabled.'
exec "$codex_binary" "$mission"
