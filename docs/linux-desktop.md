# Open Ticket Watch on Linux

The Discord application, RE:ENTRY server channels and `/ticket` commands already
exist. The desktop launcher installs the official Discord x86_64 archive for
your Linux user when needed, then opens the server and the chart viewer.
It reuses an existing `discord` executable or Flatpak installation. It needs no
bot token, administrator permissions or changes to your system package sources.

From your checkout, run this in your Linux desktop terminal:

```sh
bash deploy/open-ticket-watch-linux.sh
```

For a new checkout:

```sh
git clone https://github.com/AtecHEvolutioN/Ticket.git
cd Ticket
bash deploy/open-ticket-watch-linux.sh
```

The archive installation needs `curl`, `tar` and `python3`; opening the browser
needs `xdg-utils`. Other architectures can use the browser option below. Discord
stores its usual sign-in session on your computer. Enter account credentials
only in Discord's own app or website.

- `--check`: inspect the local application/display and print the server/viewer links.
- `--install-only`: install Discord without opening windows.
- `--browser-only`: open Discord in your browser and the chart viewer without installing the app.

In Discord, open **RE:ENTRY → TICKETMASTER NL → bot-status**. The preview contains
two recorded primary/resale seat quotes with their observation times. It labels
stock, fees and adjacency as unknown. Try `/ticket ping`, `/ticket help` and
`/ticket chart`. In the chart viewer, **Try synthetic demo** shows the chart
controls using clearly labelled invented history.

[Open the confirmed preview message](https://discord.com/channels/1569717598709350491/1569753975182332027/1570830552636858460).

The launcher opens the interfaces. Continuous collection and real live charts
still need the persistent watcher/API host and private credentials described in
[the server guide](netherlands-server.md). Dutch carting remains disabled.

## Operator preview

The manually dispatched **Show Discord bot preview** GitHub workflow posts one
message to the existing `bot-status` channel using the repository's existing
`DISCORD_SETUP_BOT_TOKEN` secret. It verifies the token application and target
server/channel before sending. Its message suppresses mentions. Failed or
ambiguous submissions stop without automatic retries; inspect Discord before
rerunning. No token is returned in the workflow output.

The workflow does not start continuous collection. Its seat quotes come from
[the dated validation evidence](evidence/netherlands-seat-validation-2026-11-13.json).
The first delivery is recorded in [the preview evidence](evidence/discord-preview-2026-11-13.json).
