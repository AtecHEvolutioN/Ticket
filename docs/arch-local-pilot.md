# Current task: validate on the Arch desktop first

**This is the latest user direction.** Set up a working pilot on the Arch Linux
PC where an ordinary browser shows Ticketmaster ticket types/prices. Move the
verified configuration to the always-on Debian VM afterwards. Keep the VM
watcher paused during the local pilot.

The user can control the Arch PC's full desktop. This cloud chat cannot operate
that desktop or its terminal. `deploy/start-arch-local-pilot.sh --start` starts a
local Codex session as the ordinary desktop user, with this task loaded. Its
default `--plan` makes no changes. The launcher refuses root and other Linux
distributions, so it cannot accidentally install on Proxmox or the Debian VM.
It reuses/installs official Codex, opens `~/ticket-local-pilot` and uses private
ChatGPT device sign-in if needed. Runtime installation and validation happen
through that local session; launching Codex alone is not a validated bot.

## Work directly on this PC

1. Confirm the local OS/user/display, existing processes, dependencies and the
   repository state. Clone/review the public Ticket repository if needed. Inspect
   existing Node, Chromium, libraries and the verified Linux worker before
   installing anything. Reuse compatible software. Keep this a local pilot;
   don't run Proxmox helpers or change the server's applications.
   If using the prebuilt server archive, extract its binary into separate staging
   and retain the current checkout's browser scripts. Unpacking the older full
   source bundle over this checkout would discard the new navigation option.
2. Configure one selected Dutch event initially. Discovery can later expand to
   all returned NL artists/events using the implemented dated partitions. Keep
   provider pacing, host cooldowns and one concurrent browser check. Use the
   existing UI/browser session when diagnosing what the user actually sees;
   preserve cookies/session material privately.
3. Open a normal headed browser on this desktop. Follow only ordinary page
   navigation; let the operator handle verification when necessary. A successful
   user check in another browser does not prove the fresh monitor context works.
   Classify actual event content, loading, access checks and unsupported layouts.
   If the page is unsupported, inspect the private capture and fix the adapter
   with sanitized regression fixtures instead of repeatedly fetching the site.
4. Collect the Ticketmaster Discovery key and Discord bot token through a
   private local workflow. Do not ask for secret values in the Codex/chat prompt,
   print them, put them in command arguments or commit them. Use user-owned mode
   0600 files and ignored local paths. The root/Proxmox credential helper is not
   the desktop entry point, although its tested validation/storage functions can
   be reused with the local UID/GID. Never overwrite existing credentials blindly.
5. Start the local watcher after source validation and prove it records current
   prices/types with explicit provenance and fee/quantity semantics. Connect one
   reviewed Discord pilot destination and the local authenticated history API.
   Verify a real update reaches the chart automatically and survives restart
   without duplicate alerts. The current sender uses one destination; nine
   Discord channels do not imply that semantic routing is implemented.

## Evidence required before saying it works

- At least one current real event yields supported live ticket type/price data.
  HTTP200, an event title, a process PID or a synthetic fixture is insufficient.
  A category price is not proof of stock, a purchasable adjacent pair or full fees.
- Discord accepts one clearly labelled connectivity test and returns its message
  ID. Availability alerts require the parser's supported availability evidence;
  do not manufacture a stock transition for the demonstration.
- A real observation is stored in the local SQLite history, and a subsequent
  recorded observation appears automatically through the authenticated API in
  the chart. Label known source age, missing stock/fees and blocked gaps honestly.
- Demonstrate restart/deduplication and summarize actual polling/processing/
  delivery timing. Do not claim second-level detection without measurement.

If a human/credential step is pending, continue independent diagnosis and setup.
Stop dependent actions at the actual blocker and state what is missing. Keep
session captures private. Automated verification solving, checkout and Dutch
carting remain disabled.

## Existing state to preserve

The Debian VM has v0.8.0-server-test installed. Its local Codex session reported
the longer passive-navigation wait applied and a headed HTTP200 after about 24
seconds, but the page remained unsupported with no offers/categories. Its worker
was still stopped/disabled and both credentials were absent in the last report.
Those results do not validate source access or a running feed. They were reported
by the operator's screenshot; this cloud session did not inspect that filesystem.

The main source supports `TICKET_BROWSER_IDENTIFICATION_WAIT_MS=15000`, bounded
by the original deadline and Retry-After rules. The ordinary default is still
five seconds. This changes passive waiting only and is not proof that an access
check completes. No Rust change is required for this JavaScript option.

Discord application/server/channel IDs and the hosted chart viewer are documented
in `HANDOFF.md`. The user prefers two adjacent cheapest tickets by default with
editable preferences; this pilot does not activate reservations or purchases.
