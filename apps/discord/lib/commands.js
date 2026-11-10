// Guild-scoped registration uses POST for just this command; never bulk-overwrite
// other application commands. Server administrators can grant command access to
// configured operators. The runtime allowlist is always enforced independently.
export const ticketCommand = Object.freeze({
  name: 'ticket',
  description: 'Ticket Watch connection help',
  type: 1,
  default_member_permissions: '0',
  dm_permission: false,
  options: [
    {
      type: 1,
      name: 'ping',
      description: 'Check the Discord command endpoint connection',
    },
    {
      type: 1,
      name: 'help',
      description: 'Show available commands and ticket alert setup',
    },
  ],
});

export function commandResponse(interaction) {
  const data = interaction.data;
  if (data?.type !== 1 || data?.name !== 'ticket' || !Array.isArray(data.options)
      || data.options.length !== 1 || data.options[0]?.type !== 1
      || (data.options[0].options !== undefined
        && (!Array.isArray(data.options[0].options) || data.options[0].options.length !== 0))) {
    return ephemeral('This command is not supported. Use /ticket ping or /ticket help.');
  }
  switch (data.options[0].name) {
    case 'ping':
      return ephemeral('Discord commands are connected. This checks the command endpoint only; '
        + 'it does not confirm that the ticket watcher is running or that tickets are available.');
    case 'help':
      return ephemeral('**Ticket Watch**\n'
        + '• /ticket ping — check the Discord command endpoint.\n'
        + '• /ticket help — show this help.\n\n'
        + 'The Rust watcher sends ticket alerts directly to its configured Discord channel. '
        + 'It runs separately on a persistent host. Configure artists, events and alert filters '
        + 'in the watcher configuration. Watcher status, remote settings and cart commands '
        + 'are not connected to this endpoint yet.');
    default:
      return ephemeral('This command is not supported. Use /ticket ping or /ticket help.');
  }
}

export function ephemeral(content) {
  return { type: 4, data: { content, flags: 64, allowed_mentions: { parse: [] } } };
}
