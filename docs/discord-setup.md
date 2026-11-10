# Discord setup and hosting

The Rust watcher sends alerts directly to Discord. The optional Vercel app
handles `/ticket ping` and `/ticket help` over signed HTTP interactions. Neither
command polls Ticketmaster, changes preferences, selects seats or reserves tickets.
The command service and the availability watcher have separate health states.

## Where each part runs

| Component | Host | Persistent data |
| --- | --- | --- |
| Discord commands | Vercel, project root `apps/discord`, Node.js 24, Frankfurt | None; public verification key and access settings are environment variables |
| Availability watcher and direct Discord alerts | One always-on Linux machine | SQLite observations, outbox and cooldowns on local persistent disk |
| Browser/session and cart operator | Persistent host with the appropriate browser/display access | Private browser evidence and the existing attempt journal |

Vercel Functions, including container services, autoscale and can lose local
state. They are useful for short Discord requests; the current watcher depends
on one persistent process, SQLite locking and retained browser sessions. Do not
deploy the watcher loop as a function or repeatedly invoke it with cron.
See [Vercel container hosting](https://vercel.com/docs/functions/container-images).

For a first browser-backed watcher, start with a European Linux VPS with about
2 vCPU, 4 GB RAM and persistent SSD storage, then measure memory/CPU with the
selected watches. This is a starting size, not a capacity promise. A desktop
computer can run the first test while awake. No VPS purchase or installation is
included in this repository.

One initial sender should own each bot token. The ordinary monitor and separate
map monitor currently maintain independent rate-limit state; running both with
the same token does not coordinate Discord's global limits.

## Discord application and channel

1. Open the existing application in the
   [Discord Developer Portal](https://discord.com/developers/applications).
   Copy **Application ID** and **Public Key** from General Information.
2. In Discord, enable Settings → Advanced → Developer Mode. Copy the server ID,
   destination text-channel ID and your user ID. These IDs and the public key are
   not credentials. Keep the bot token private.
3. Install the application in that server with the `bot` and
   `applications.commands` scopes. The bot needs **View Channel**, **Send Messages**
   and **Embed Links** in the destination text channel. Administrator and
   privileged Gateway intents are unnecessary. Channel overrides can still deny
   access. Use a normal text channel for the first test.
   To let the bot create channels, also grant **Manage Channels**, using the
   `discord-setup --manage-channels` invite option below.
4. Configure the worker's `config.local.toml`:

   ```toml
   [discord]
   token_env = "DISCORD_BOT_TOKEN"
   channel_id = "YOUR_CHANNEL_ID"
   mention_user_ids = ["YOUR_USER_ID"]
   mention_role_ids = []
   ```

   Keep the configured event watches disabled until the destination test passes.
   Role pings additionally depend on role/server permissions and mentionability.
   Phone push notifications depend on your Discord notification settings.

Build the current source first (`cargo build --release --locked`). The published
0.7.0-test binary predates these setup commands. Generate the invite and review
the destination without network requests or credentials:

```bash
./target/release/ticket-watch --config config.local.toml discord-setup \
  --application-id YOUR_APPLICATION_ID --guild-id YOUR_SERVER_ID
./target/release/ticket-watch --config config.local.toml discord-test
```

For channel creation, add `--manage-channels` to `discord-setup` and authorize the
resulting invite for the same application/server. This requests permission bit
`16` in addition to the alert permissions (`19472` total), without Administrator.
The invite itself creates no channels.

Set `DISCORD_BOT_TOKEN` privately on the worker. On Bash, a hidden prompt avoids
putting the token in shell history or command arguments:

```bash
read -rsp 'Discord bot token: ' DISCORD_BOT_TOKEN
export DISCORD_BOT_TOKEN
./target/release/ticket-watch --config config.local.toml discord-test --send
unset DISCORD_BOT_TOKEN
```

`--send` sends one clearly marked setup message to the configured channel, with
only configured mentions. It does not query Ticketmaster or open a database.
It does not retry ambiguous delivery. Verify the message in Discord; a timeout
can happen after acceptance. The reported cooldown belongs to this standalone
test, so wait it out before starting another sender.

## Vercel command endpoint

Deploy the Git repository with **Root Directory `apps/discord`**. The app uses
Node.js 24 with no third-party runtime dependencies. Its build runs the signed
interaction and registration tests. It has no cron, bot token or database.

This application's `vercel.json` currently supplies the four **public** Discord
settings below through deployment configuration, so a Git deployment can configure
the endpoint without project API access. The application code still reads and
validates environment variables; no bot token is included. These IDs and the
public verification key are visible in the repository.

For another Discord application, replace those values or remove the `env` block
and set these Vercel **production** environment variables, then redeploy. Dashboard
configuration is preferred when project access is available; Vercel marks the
`env` configuration field as deprecated. Never add a bot token or any other
secret to `vercel.json`.

| Name | Value |
| --- | --- |
| `DISCORD_APPLICATION_ID` | Application ID from General Information |
| `DISCORD_PUBLIC_KEY` | 64-character hexadecimal public verification key |
| `DISCORD_GUILD_ID` | The one allowed server ID |
| `DISCORD_ALLOWED_USER_IDS` | Your user ID; comma-separated for multiple operators |

The stable production domain must allow Discord to POST without a Vercel login.
Use Deployment Protection for previews as appropriate; the production interaction
route authenticates the exact request bytes using Discord's Ed25519 signature.
Never add a deployment bypass secret to the Discord endpoint URL.

Set the Developer Portal's **Interactions Endpoint URL** to:

```text
https://YOUR_PRODUCTION_DOMAIN/api/interactions
```

This endpoint is application-wide. If the application already has a running bot
handling other commands, integrate this handler with that bot or create a
dedicated application first. Changing the endpoint redirects interactions for
the application, including command names registered by other code.

Discord verifies that URL with a signed PING. Configure the public key and
application ID before saving it. Invalid signatures or stale timestamps are
rejected. Commands also require the configured application, server and allowed
user; DMs are denied. All command replies are ephemeral and contain no pings.

The public `/api/health` returns `needs_setup` (HTTP 503) until settings are valid,
or `configured` (HTTP 200) after configuration. This checks settings only;
`watcher_status` remains `unverified`. It does not validate live monitoring,
channel permissions, registration or successful Discord delivery.

## Register the commands

### From a phone using GitHub Actions

Save the bot token directly in this repository's Actions secrets as
`DISCORD_SETUP_BOT_TOKEN`. It is a temporary setup credential; never paste it in
chat, a workflow input, a Git-tracked file or an issue.

Open Actions → **Register Discord commands** → Run workflow, select `main`, then
run it once. The [manual workflow](../.github/workflows/discord-setup.yml) checks
out the exact dispatch commit and uses pinned actions. It has no push or pull
request trigger, refuses other branches, and supplies the token only to the
registration step. It installs no project dependencies and uploads no artifacts.
The application and server IDs are explicit in the workflow; change both the
workflow and the Vercel settings when using another Discord application.

The registration step checks that the token belongs to the intended application,
then creates or updates only the server's `/ticket` command. It sends no channel
messages and makes no automatic retries. If the result is uncertain, inspect
Server Settings → Integrations before another run. A successful run prints only
the application, server and command identifiers. Keep the token privately for the
future watcher, then remove the temporary GitHub setup secret after verification.

### From a computer using the CLI

Create a private root-level `.env.discord.local` file (`umask 077` before creating
it; `chmod 600 .env.discord.local` for an existing file) with the application ID,
guild ID and bot token, or export those variables in the shell. The repository
ignores this file. Do not place the bot token in Vercel for these commands.

From the repository root, first preview the registration locally:

```bash
node --env-file=.env.discord.local apps/discord/scripts/register-commands.mjs
```

Then apply it once:

```bash
node --env-file=.env.discord.local apps/discord/scripts/register-commands.mjs --apply
```

The script first verifies that the token belongs to the intended application,
then creates or updates only the server's `/ticket` command. It preserves other
commands, uses no global registration and makes no automatic retries. A network
failure during registration may be ambiguous; inspect Server Settings →
Integrations before trying again.

Commands default to administrators in Discord. If an allowed operator is not an
administrator, grant that user access under Server Settings → Integrations →
the application. The endpoint's user allowlist still applies to administrators.
Try `/ticket ping` and `/ticket help` in the configured server. A successful ping
verifies the command endpoint only. Watch controls, remote status and preference
sync still require a worker connection and are not exposed yet.

## Let the bot create a channel

The bot can create normal text channels using the operator helper below. It runs
on the operator's computer or persistent host with the same private bot token;
the Vercel command endpoint does not need that token. The first alert channel can
remain the existing one. Creation does not automatically change its configured
destination.

```bash
# Preview: verifies the application and searches existing channels if a token is set.
node --env-file=.env.discord.local apps/discord/scripts/provision-channel.mjs \
  --name ticket-alerts
# Apply once after reviewing the plan:
node --env-file=.env.discord.local apps/discord/scripts/provision-channel.mjs \
  --name ticket-alerts --apply
```

Without a token, the preview describes an unverified intention only. With a token,
it checks that the token belongs to the configured application and lists the
configured server's channels before acting. `--parent CATEGORY_ID` places the
channel in an existing category. An exact existing name/type/parent match is
reused; duplicate matches are rejected for manual review. The helper leaves
permissions to the server/category defaults and does not delete, move or rename
anything. Review those defaults if the channel must be private.

An apply makes at most one channel-creation request and never retries an
uncertain response. Inspect the server before another invocation after an
ambiguous failure. Run one provisioner at a time: the helper has no distributed
lock and concurrent invocations can race. Copy the confirmed channel ID into the
worker's `[discord].channel_id` if you want future alerts sent there. Channel
creation is an operator tool; `/ticket` currently exposes only ping/help.

## Start real alerts

After a verified setup message, enable selected watches and validate the config.
Supply the token using the host's private service environment, then start:

```bash
./target/release/ticket-watch --config config.local.toml validate
./target/release/ticket-watch --config config.local.toml run --send-discord
```

For a Linux service, use [the existing unit](../deploy/ticket-watch.service),
restricted `/etc/ticket-watch.env`, and the
[sending override](../deploy/ticket-watch.send-discord.conf.example).
Keep SQLite and browser state on persistent storage and retain the existing cart
journal. See [running the monitor](running.md) for installation and shutdown.

Monitor measured source freshness, failures and delivery acknowledgements.
Second-level alerts are a performance goal; source access, effective polling
intervals and Discord delivery determine observed latency. A working command
endpoint does not establish that goal or verify automatic carting.
