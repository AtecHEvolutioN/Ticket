# Continue directly inside the Debian VM

**Current direction:** validate on the [Arch desktop first](arch-local-pilot.md).
This document records the earlier VM connection route; keep its watcher paused
until the desktop pilot has been validated.

The user's PC has only remote access. The chosen route is therefore to run Codex
inside the existing Debian VM, using an SSH terminal from the Proxmox node Shell.
This cloud chat has no verified LAN/VPN route or terminal-control tool for that
server. A screenshot or broad permission grant does not attach its terminal.

`deploy/connect-vm-codex.py` provides a one-time operator setup. Its default is a
read-only plan. `--apply VMID PRIVATE_IPV4` must run interactively as the Proxmox
administrator. It checks the guest's existing SSH policy, creates/reuses one
dedicated root-only private key on Proxmox, and appends its public key to the VM.
Existing keys/passwords are preserved; the SSH daemon's authentication policy is
not changed. The guest must already permit root public-key authentication.

The VM's public SSH host key is obtained through the authenticated guest agent.
The helper pins it in a separate root-only known-hosts file and uses strict SSH
verification. A changed pin or failed login stops setup. The private client key
stays on Proxmox. No ports, SSH tunnels, agent forwarding or VPN are configured.

After SSH succeeds, the helper installs the official Codex standalone CLI inside
the VM and creates/reuses `/root/ticket-codex`. It then opens an SSH terminal and
uses the documented `codex login --device-auth` flow if sign-in is needed. Enter
the device code only on the official OpenAI page shown in that terminal. Do not
paste device codes, account credentials, private keys or bot tokens into chat.

The resulting Codex session runs locally on the Debian VM. It can execute bot
commands directly there. It is a separate session: it reads `HANDOFF.md` and this
file to continue. Existing applications and private files outside this project
are outside the task. Use the verified worker binary; this shared VM does not
need a heavy Rust rebuild for the current JavaScript-only navigation change.

Private credential entry still requires the operator. The existing Proxmox
credential helper runs on the Proxmox node, not inside the VM. The VM session can
reuse its validated storage functions for a local private terminal prompt, or the
operator can run the node helper in another Proxmox Shell tab. The agent must
never ask the operator to paste keys into chat or print the environment file.

## Current deployment state for the local session

- The v0.8.0-server-test bundle is installed at `/opt/ticket-watch`. Its worker
  remains stopped/disabled. Credentials have not been confirmed installed.
- Headless Chromium returned HTTP403. Headed Chromium under Xvfb returned an
  initial identification HTTP 401 after about 7.4 seconds. The user's normal PC
  browser shows ticket types/prices. These observations do not identify the
  protection rule or verify automated inventory access.
- The installed ordinary worker uses a five-second passive identification wait.
  New source permits an explicit `TICKET_BROWSER_IDENTIFICATION_WAIT_MS=15000`
  within the original request deadline, preserving Retry-After/error handling.
  Tests verify a website's own delayed navigation, no worker reload, bounded
  timeout and retained blocks. This option has **not** been tested on the VM.
- The installed worker file from v0.8.0 has SHA256
  `81f577775a2b58ac9d74ed78fda94f13e396aacb014bbbe3b9760f57e0510766`.
  Before a controlled update, confirm that baseline, stop the worker, retain a
  backup and record the new source/hash as a local override. The Rust binary
  protocol remains compatible; archive checksums intentionally describe the
  original package rather than a later local patch.
- The last headed blocked observation was Unix timestamp 1794589178. Honor the
  configured 900-second host cooldown before another live check. Do not repeat
  blocked requests to defeat backoff or automate human verification.
- After source access is verified, configure API/bot credentials privately,
  validate Discovery and one pilot Discord destination, then supervise a measured
  run. Multi-channel routing and a production chart API connection remain
  unfinished. Dutch carting stays disabled.

Official installation/authentication references:

- https://github.com/openai/codex#installing-and-running-codex-cli
- https://developers.openai.com/codex/auth#login-on-headless-devices
