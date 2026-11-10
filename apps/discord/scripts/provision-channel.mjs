import { pathToFileURL } from 'node:url';
import { isSnowflake } from '../lib/config.js';

const API = 'https://discord.com/api/v10';
const MAX_BODY_BYTES = 1024 * 1024;
const UNCERTAIN = 'Discord channel creation is uncertain. Inspect the server channels before retrying; no automatic retry was made.';

function requireId(value, label) {
  if (!isSnowflake(value)) throw new Error(`${label} must be a positive Discord numeric ID.`);
  return value;
}

export function channelIntent(env, { name, parent } = {}) {
  if (typeof name !== 'string' || !/^[a-z0-9][a-z0-9_-]{0,99}$/.test(name)) {
    throw new Error('Channel name must use 1–100 lowercase letters, digits, hyphens or underscores and start with a letter or digit.');
  }
  return {
    application_id: requireId(env.DISCORD_APPLICATION_ID, 'DISCORD_APPLICATION_ID'),
    guild_id: requireId(env.DISCORD_GUILD_ID, 'DISCORD_GUILD_ID'),
    channel: { name, type: 0, parent_id: parent === undefined ? null : requireId(parent, '--parent') },
    permissions: 'No permission overwrites are set; review inherited server/category access.',
  };
}

async function cancelBody(response) {
  try { await response.body?.cancel(); } catch { /* A provider error must never reach logs. */ }
}

async function readJson(response) {
  if (Number(response.headers.get('content-length')) > MAX_BODY_BYTES) {
    await cancelBody(response);
    throw new Error('response_limit');
  }
  if (!response.body) throw new Error('empty_response');
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body) {
    size += chunk.byteLength;
    if (size > MAX_BODY_BYTES) throw new Error('response_limit');
    chunks.push(Buffer.from(chunk));
  }
  return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks, size)));
}

function channelMatches(channel, intent) {
  return channel.name === intent.channel.name && channel.type === 0
    && (channel.parent_id ?? null) === intent.channel.parent_id;
}

// Listing then creating is safe for ordinary repeated operator use, but is not a
// distributed lock. Do not run concurrent provisioning processes for this target.
export async function provisionChannel(env, options, fetchImpl = fetch) {
  const intent = channelIntent(env, options);
  const apply = options?.apply === true;
  const token = env.DISCORD_BOT_TOKEN?.trim();
  if (!token || /[^\x21-\x7e]/.test(token)) {
    throw new Error('Set DISCORD_BOT_TOKEN privately to inspect or create a Discord channel.');
  }
  async function request(path, method, body) {
    let response;
    try {
      response = await fetchImpl(`${API}${path}`, {
        method,
        headers: {
          authorization: `Bot ${token}`,
          'content-type': 'application/json',
          ...(method === 'POST' ? { 'x-audit-log-reason': 'Ticket%20Watch%20operator%20channel%20setup' } : {}),
        },
        body: body === undefined ? undefined : JSON.stringify(body),
        redirect: 'error',
        signal: AbortSignal.timeout(20_000),
      });
    } catch {
      throw new Error(method === 'POST' ? UNCERTAIN : 'Discord channel setup could not be inspected. No channel creation was attempted.');
    }
    if (!response.ok) {
      await cancelBody(response);
      if (method === 'POST' && (response.status >= 500 || response.status === 408)) throw new Error(UNCERTAIN);
      const reason = {
        400: 'Discord rejected the channel setup request.',
        401: 'Discord rejected the bot token.',
        403: 'The bot lacks access or Manage Channels permission. Check its server installation and category permissions.',
        404: 'Discord could not find this application or server.',
        429: 'Discord rate limited setup. Wait before retrying; no automatic retry was made.',
      }[response.status] ?? 'Discord did not confirm this channel setup request.';
      throw new Error(`${reason} (HTTP ${response.status})`);
    }
    try {
      return await readJson(response);
    } catch {
      throw new Error(method === 'POST' ? UNCERTAIN : 'Discord setup response could not be verified. No channel creation was attempted.');
    }
  }

  const application = await request('/oauth2/applications/@me', 'GET');
  if (application?.id !== intent.application_id) {
    throw new Error('DISCORD_BOT_TOKEN belongs to a different application. No channel creation was attempted.');
  }
  const channels = await request(`/guilds/${intent.guild_id}/channels`, 'GET');
  if (!Array.isArray(channels) || channels.length > 500 || channels.some(channel =>
    !channel || typeof channel !== 'object' || Array.isArray(channel)
    || !isSnowflake(channel.id) || !Number.isInteger(channel.type)
    || typeof channel.name !== 'string'
    || (channel.guild_id !== undefined && channel.guild_id !== intent.guild_id)
    || (channel.parent_id != null && !isSnowflake(channel.parent_id)))) {
    throw new Error('Discord channel listing could not be verified. No channel creation was attempted.');
  }
  if (new Set(channels.map(channel => channel.id)).size !== channels.length) {
    throw new Error('Discord channel listing contains duplicate identifiers. No channel creation was attempted.');
  }
  if (intent.channel.parent_id !== null) {
    const parent = channels.find(channel => channel.id === intent.channel.parent_id);
    if (!parent || parent.type !== 4) {
      throw new Error('--parent must identify an existing category in this server. No channel creation was attempted.');
    }
  }
  const matches = channels.filter(channel => channelMatches(channel, intent));
  if (matches.length > 1) {
    throw new Error('Multiple text channels match this name and parent. Resolve the ambiguity before setup; no channel creation was attempted.');
  }
  const summary = {
    ...intent,
    preview: !apply,
    verified: true,
    operation: matches.length === 1 ? 'reuse' : 'create',
    created: false,
  };
  if (matches.length === 1) return { ...summary, channel_id: matches[0].id };
  if (!apply) return summary;

  const created = await request(`/guilds/${intent.guild_id}/channels`, 'POST', intent.channel);
  if (!created || !isSnowflake(created.id) || created.guild_id !== intent.guild_id
      || !channelMatches(created, intent) || created.parent_id !== intent.channel.parent_id
      || channels.some(channel => channel.id === created.id)) {
    throw new Error(UNCERTAIN);
  }
  return { ...summary, created: true, channel_id: created.id };
}

export function parseArgs(args) {
  const options = {};
  const seen = new Set();
  for (let i = 0; i < args.length; i += 1) {
    const arg = args[i];
    if (!['--name', '--parent', '--apply'].includes(arg) || seen.has(arg)) throw new Error(usage());
    seen.add(arg);
    if (arg === '--apply') options.apply = true;
    else {
      const value = args[++i];
      if (!value || value.startsWith('--')) throw new Error(usage());
      options[arg === '--name' ? 'name' : 'parent'] = value;
    }
  }
  if (!seen.has('--name')) throw new Error(usage());
  return options;
}

function usage() {
  return 'Usage: node scripts/provision-channel.mjs --name ticket-alerts [--parent CATEGORY_ID] [--apply]. Default: read-only inspection if a token is configured, otherwise an unverified offline intent. Do not run concurrent setup processes.';
}

export async function main(args = process.argv.slice(2), env = process.env) {
  if (args.length === 1 && args[0] === '--help') {
    process.stdout.write(`${usage()}\n`);
    return;
  }
  const options = parseArgs(args);
  const result = !options.apply && !env.DISCORD_BOT_TOKEN?.trim()
    ? { ...channelIntent(env, options), preview: true, verified: false, operation: 'inspect_then_reuse_or_create',
      note: 'No token configured: existing channels, category and bot permissions have not been checked.' }
    : await provisionChannel(env, options);
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
