# Prepare an always-on Netherlands watcher

The Discord category and nine channels already exist; `/ticket chart` and the
Vercel viewer are deployed. **No persistent watcher server has been provisioned.**
These files prepare the host; they do not create a paid cloud resource or start
monitoring. Live alert routing and actual-host validation remain unfinished.
Country date partitioning and inventory priorities are implemented with explicit
coverage/freshness reports; they do not establish full or instant NL coverage.

## Concrete starting host

Use a **Debian 13 x86_64 VM with 2 vCPU, 4 GB RAM and persistent SSD storage**.
A current example is DigitalOcean Basic Regular in Amsterdam: 4 GiB memory,
2 vCPU and 80 GiB SSD, listed at **USD 24/month** (USD 0.03571/hour) on
[the provider's pricing page](https://www.digitalocean.com/pricing/droplets),
checked 10 November 2026. Taxes, optional provider backups and excess usage can
add cost. Confirm image/region availability and the checkout total before
creating it. No purchase, billing approval or cloud-account access is implied.

The installer is provider-neutral. An equivalent Debian VM at another provider
is also suitable for the first measured run. This is a starting resource size,
not a capacity or detection-latency guarantee. Scaling the CPU does not remove
provider pacing, catalog-pagination limits or blocked website access.

Vercel continues to host commands and the chart viewer. SQLite and Chromium run
on the VM. Initially there is no public worker HTTP service: allow SSH only from
the operator's trusted addresses; keep the database and browser state private.
The optional [authenticated history API](live-history-api.md) binds to loopback.
After source validation, expose that read-only service through an HTTPS reverse
proxy if remote charts are needed; the worker itself has no public control API.

## Install the reviewed application

Use a fresh host and a reviewed source revision or unpack the server bundle into
`/opt/ticket-watch`, owned by the deployment administrator. Application code must
not be writable by the service account. The Linux binary needs glibc 2.39 or
newer. Debian 13 supplies a compatible runtime and
[Node.js 20](https://packages.debian.org/trixie/nodejs).

The following commands belong on the chosen VM, not on a developer workstation:

```sh
sudo apt-get update
sudo apt-get install --no-install-recommends \
  ca-certificates nodejs npm chromium systemd passwd util-linux sqlite3

# In the reviewed, root-owned /opt/ticket-watch deployment:
cd /opt/ticket-watch
sudo env PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --omit=dev --ignore-scripts

# Plan and prerequisite checks perform no provider requests.
bash deploy/prepare-nl-server.sh
sudo bash deploy/prepare-nl-server.sh --check
sudo bash deploy/prepare-nl-server.sh --apply
```

The helper verifies Debian/architecture, runtime tools, code ownership,
dependencies, binary version and offline TOML validation. It creates only missing
items and refuses mismatched existing paths. It does not install packages,
download code, load secrets, fetch events, send messages, overwrite configuration,
or start/enable the service.

Prepared paths:

| Item | Path |
| --- | --- |
| Service | `ticket-watch-nl.service` |
| Account | `ticketwatch-nl` |
| Private configuration | `/etc/ticket-watch-nl/config.toml` |
| Private environment | `/etc/ticket-watch-nl/environment` |
| SQLite history/outbox/state | `/var/lib/ticket-watch-nl` |
| Chromium cache | `/var/cache/ticket-watch-nl` |
| Private backup directory | `/var/backups/ticket-watch-nl` |

Enter `TICKETMASTER_API_KEY` and `DISCORD_BOT_TOKEN` privately in the environment
file. Do not paste them into chat, command arguments, Git, or screenshots. The
existing GitHub setup secret remains a separate channel/command provisioning
credential; this preparation does not extract it.

## First activation

The prepared subscription is disabled and its destination is unset. It selects
all artists in NL when enabled, using resumable date windows across a 730-day
horizon and retaining up to 10,000 events within storage/request limits. Undated
events and saturated windows remain explicit gaps. Inventory priorities favor
sale windows and recent changes, with regular oldest-due checks. Host pacing and
provider cooldowns are unchanged; inspect [scheduler reports](scheduling.md).
The single existing sender cannot yet populate the nine semantic channels on
its own. Set a reviewed test destination and implement/validate routing before
enabling ongoing live deliveries. Do not run nine duplicate watchers to imitate
channel routing: that would duplicate requests, inventory state and rate budgets.

After supplying private configuration and validating ordinary provider access on
the actual host, start one preview instance explicitly:

```sh
sudo systemctl start ticket-watch-nl.service
sudo journalctl -u ticket-watch-nl.service -f
```

Preview mode still makes provider requests and records real observations once the
subscription is enabled. It does not send Discord messages. Keep queue/challenge
handoffs manual and retain source cooldowns. A running process is not proof of
fresh source data or successful delivery; inspect catalog coverage, stored watch
timestamps, blocks and pending outbox entries separately.

The [inactive send drop-in](../deploy/ticket-watch-nl.send-discord.conf.example)
shows the explicit delivery switch. Install it only for the reviewed route after
source validation; then reload systemd and restart that single service. Use
`systemctl enable ticket-watch-nl.service` only when the chosen mode should start
at boot. `systemctl stop ticket-watch-nl.service` stops polling and delivery.

## Preserve history and recover

Keep one supervised process per database. Back up SQLite consistently with the
SQLite backup API or `sqlite3`'s `.backup` command; copying only the main `.sqlite`
file while WAL writes are active can lose committed state. Use a new private
backup filename and verify it with `PRAGMA quick_check`. Keep an encrypted copy
off the VM as well. The preparation creates a backup directory but does not yet
schedule backups or configure paid provider backups.

Restore into an inactive deployment, check the restored database and preserve
outbox identity/cooldowns before restarting. Do not delete state to force a new
alert or bypass a cooldown. History grows until explicitly retained/archived, so
monitor free disk space and backup size. The unresolved Czech cart journal is
not migrated or unlocked by this Netherlands deployment.

Read [the channel layout](discord-netherlands.md),
[history semantics](price-history.md) and
[the production roadmap](production-roadmap.md) for the remaining functional
work. New VM preparation alone does not make Dutch per-seat extraction or
automatic carting available.
