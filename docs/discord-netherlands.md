# Netherlands Discord layout

Created on 10 November 2026: **TICKETMASTER NL** and all nine text channels in
the configured server. The [setup run](https://github.com/AtecHEvolutioN/Ticket/actions/runs/42857120073)
confirmed every public ID; the [saved layout](evidence/discord-nl-layout-2026-11-10.json)
records them for subsequent routing configuration. No messages were posted.

The initial scope is **all artists on Ticketmaster Netherlands**, across Dutch
cities and venues. Discovery coverage and inventory-check coverage must be
reported separately: finding an event does not prove its ticket page is readable,
that it has inventory, or that every Dutch event has been discovered.

The server layout uses one **TICKETMASTER NL** category:

| Channel | Intended content |
| --- | --- |
| `nl-upcoming-sales` | Announced events and scheduled public sale dates. These are catalog facts, not stock alerts. |
| `nl-on-sale` | Events whose published public sale window has opened. Current inventory may still be unavailable or unverified. |
| `nl-presales` | Published presale windows and access requirements when provided. Being in a presale window does not prove eligibility or inventory. |
| `nl-primary` | Explicitly identified original-sale offers, with asking prices, price/fee basis and observed quantity evidence. |
| `nl-resale` | Explicitly identified resale listings and asking prices. Disappearing listings are not confirmed sales. |
| `nl-price-changes` | Changes within comparable ticket series: same event, type, sale channel, currency, price basis and fee scope. |
| `nl-charts` | Asking-price and observed-entry history, with separate source/type/channel series and supported seat quote histories. |
| `nl-event-updates` | Published cancellations, postponements, venue/date changes, with their source and observation time. |
| `bot-status` | Last successful checks, source freshness, partial coverage, blocked pages and delivery problems. |

Restocks belong in the appropriate primary/resale stream, labelled as an observed
return of eligible inventory. Unknown sale channels must not be guessed into one
of those streams. There is no sold-volume or cart-success channel yet: those
require confirmed transactions or a verified hold, respectively.

## Create or reuse the layout

The manual **Set up Netherlands Discord channels** GitHub Actions workflow runs
the reviewed helper with the existing `DISCORD_SETUP_BOT_TOKEN` repository secret.
The bot needs Manage Channels. It verifies the configured application before
inspecting the target guild, reuses exact matches, and creates only missing
channels. No existing channels, permission overwrites or topics are changed.
New channels inherit normal server/category access. No messages or pings are sent.

The helper also runs locally:

```sh
# Public identifiers only: inspect the proposed layout, no network or token needed.
node apps/discord/scripts/provision-nl-layout.mjs

# With DISCORD_BOT_TOKEN set privately, inspect existing channels without writes.
node apps/discord/scripts/provision-nl-layout.mjs --inspect

# Apply the reviewed layout with the privately configured token.
node apps/discord/scripts/provision-nl-layout.mjs --apply
```

Set `DISCORD_APPLICATION_ID` and `DISCORD_GUILD_ID` as documented in
[Discord setup](discord-setup.md). A conflicting/ambiguous channel name fails
before creation. Confirmed channel IDs are printed as each action completes.
If a create request has an uncertain result, inspect Discord before rerunning;
the helper never blindly repeats an ambiguous write. Run only one provisioner
for this layout at a time.

## What is ready and what still needs deployment

Channel provisioning does not start the watcher or configure alert routing.
The existing sender uses its configured destination channel; routing these
streams and a persistent worker are separate deployment tasks. The Vercel
command endpoint already works, while the continuous watcher is not installed.
The chart viewer supports exported history and an authenticated connection to the
[read-only history API](live-history-api.md). A production worker/API host still
needs deployment and private configuration before real continuous updates work.

The [Netherlands adapter audit](netherlands-mapping.md) has tested event metadata,
displayed ticket categories/prices, explicit resale listings where exposed, and
overview maps. Individual Dutch seat hovering and cart reservations have not been
validated. Per-seat history support can store validated observations from
supported adapters; it does not by itself add Dutch seat extraction.

Country discovery uses resumable date windows while respecting pagination,
quotas and configured capacity. Saturated windows split; partial/capped scans
remain incomplete and preserve unseen cached events. Inventory checks prioritize
imminent sales and recent changes with fair older checks. Host pacing and provider
cooldowns remain in force. No one-second country-wide detection guarantee follows.

The [all-artists NL configuration](../examples/netherlands-all.example.toml)
uses `all_artists = true` and `country = "NL"`, without an attraction ID. It
includes all returned event types and preserves actual artist labels and published
sale/presale information. The example scans a 730-day dated horizon with a
10,000-event capacity, while each API query keeps its 1,000-result ceiling.
Undated/TBA events and dates outside the horizon are not covered. It has not been
activated on a live host. The current conservative per-host pacing still makes
a one-second sweep of every event impossible; use the measured freshness report.
