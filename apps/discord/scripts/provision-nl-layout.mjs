import { pathToFileURL } from 'node:url';
import { isSnowflake } from '../lib/config.js';

const API = 'https://discord.com/api/v10';
const MAX_BODY_BYTES = 1024 * 1024;
const CATEGORY = 'TICKETMASTER NL';
const UNCERTAIN = 'Discord channel creation is uncertain. Stop and inspect the server before retrying; confirmed IDs were printed as progress. No automatic retry was made.';
const CHANNELS = [
  ['nl-upcoming-sales', 'Ticketmaster NL onsale schedules across artists, from event metadata. A scheduled sale does not confirm available tickets.'],
  ['nl-presales', 'Ticketmaster NL presale schedules and eligibility. Presale metadata does not confirm current stock or access.'],
  ['nl-on-sale', 'Public sale window has opened according to the published schedule; this does not prove current inventory. Primary/resale channels require offer evidence.'],
  ['nl-primary', 'Explicitly identified original (primary-sale) offers. Unknown sale type stays unknown. Standard, VIP and Official Platinum are ticket types; primary sale does not imply face value or the lowest price.'],
  ['nl-resale', 'Explicitly identified resale offers. Unknown sale type stays unknown. Displayed asking prices and listing quantities do not prove total event stock or a reservation.'],
  ['nl-price-changes', 'Changes in observed ticket asking prices across artists. These are observations, not completed trades, sales volume or guaranteed availability.'],
  ['nl-event-updates', 'Published cancellations, postponements, venue or date changes; include source and observation time. Never infer cancellation from an unreadable page.'],
  ['nl-charts', 'Exported asking-price history and observed listing counts, not trades or sales volume. This channel is not connected to live charts yet.'],
  ['bot-status', 'Monitor and publisher status only when observed. Creating these channels does not mean monitoring is running, messages are being delivered or tickets are available.'],
];

export function layoutIntent(env) {
  for (const field of ['DISCORD_APPLICATION_ID', 'DISCORD_GUILD_ID']) {
    if (!isSnowflake(env[field])) throw new Error(`${field} must be a positive Discord numeric ID.`);
  }
  return {
    application_id: env.DISCORD_APPLICATION_ID,
    guild_id: env.DISCORD_GUILD_ID,
    category: { name: CATEGORY, type: 4, parent_id: null },
    channels: CHANNELS.map(([name, topic]) => ({ name, type: 0, parent_category: CATEGORY, topic })),
    permissions: 'No permission overwrites are supplied. New channels inherit server/category access; existing channels are unchanged.',
    messages_sent: 0,
  };
}

async function cancel(response) {
  try { await response.body?.cancel(); } catch { /* Never expose provider content. */ }
}

async function readJson(response) {
  if (Number(response.headers.get('content-length')) > MAX_BODY_BYTES) {
    await cancel(response); throw new Error('response_limit');
  }
  if (!response.body) throw new Error('empty_response');
  const chunks = []; let size = 0;
  for await (const chunk of response.body) {
    size += chunk.byteLength;
    if (size > MAX_BODY_BYTES) throw new Error('response_limit');
    chunks.push(Buffer.from(chunk));
  }
  return JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(Buffer.concat(chunks, size)));
}

function validateSnapshot(channels, guild) {
  if (!Array.isArray(channels) || channels.length > 500 || channels.some(channel =>
    !channel || typeof channel !== 'object' || Array.isArray(channel)
    || !isSnowflake(channel.id) || !Number.isInteger(channel.type) || channel.type < 0 || channel.type > 255
    || typeof channel.name !== 'string' || !channel.name || channel.name.length > 200
    || /[\u0000-\u001f\u007f]/.test(channel.name)
    || (channel.guild_id !== undefined && channel.guild_id !== guild)
    || (channel.parent_id != null && !isSnowflake(channel.parent_id)))) {
    throw new Error('Discord channel listing could not be verified. No creation was attempted.');
  }
  const ids = new Map(channels.map(channel => [channel.id, channel]));
  if (ids.size !== channels.length || channels.some(channel => channel.parent_id != null
    && (channel.type === 4 || ids.get(channel.parent_id)?.type !== 4))) {
    throw new Error('Discord channel IDs or category parents are inconsistent. No creation was attempted.');
  }
}

function matching(channels, name, type, parent) {
  const candidates = channels.filter(channel => channel.name.toLowerCase() === name.toLowerCase());
  if (candidates.length > 1 || candidates.some(channel => channel.name !== name
    || channel.type !== type || (channel.parent_id ?? null) !== parent)) {
    throw new Error(`Conflicting or duplicate channel name: ${name}. Resolve it manually before setup; no creation was attempted.`);
  }
  return candidates[0] || null;
}

// One operator at a time: listing plus creation is not a distributed lock.
// Progress contains only verified public identifiers, never provider responses.
export async function provisionNlLayout(env, { apply = false, inspect = false } = {}, fetchImpl = fetch, progress = () => {}, wait = milliseconds => new Promise(resolve => setTimeout(resolve, milliseconds))) {
  const intent = layoutIntent(env);
  const token = env.DISCORD_BOT_TOKEN?.trim();
  if (!apply && !inspect) return { ...intent, preview: true, verified: false,
    operation: 'inspect_then_reuse_or_create', note: 'Offline intent only; use --inspect to check existing channels. Permissions have not been verified.' };
  if (!token || /[^\x21-\x7e]/.test(token)) throw new Error('Set DISCORD_BOT_TOKEN privately to inspect or apply the NL layout.');
  let cooldownUntil = 0;
  let cooldownWaitable = true;
  let cooldownHint = null;
  async function request(method, route, body) {
    if (!cooldownWaitable) {
      throw new Error(`Discord requested a rate-limit cooldown with an unknown or greater-than-30-second reset.${cooldownHint === null ? '' : ` Wait at least ${cooldownHint} seconds.`} Inspect confirmed progress before another setup run; no automatic retry was made.`);
    }
    if (cooldownUntil > performance.now()) {
      // Wait before a different request, never replay a prior creation POST.
      const delay = Math.ceil(cooldownUntil - performance.now());
      try { await wait(delay); }
      catch { throw new Error('Discord cooldown wait was interrupted. Inspect confirmed progress before continuing; no automatic retry was made.'); }
      cooldownUntil = 0;
    }
    let response;
    try {
      response = await fetchImpl(`${API}${route}`, { method,
        headers: { authorization: `Bot ${token}`, 'content-type': 'application/json',
          ...(method === 'POST' ? { 'x-audit-log-reason': 'Ticket%20Watch%20operator%20NL%20layout%20setup' } : {}) },
        body: body === undefined ? undefined : JSON.stringify(body),
        redirect: 'error', signal: AbortSignal.timeout(20_000) });
    } catch { throw new Error(method === 'POST' ? UNCERTAIN : 'Discord layout inspection failed. No creation was attempted.'); }
    const reset = Number(response.headers.get('x-ratelimit-reset-after'));
    if (response.headers.get('x-ratelimit-remaining') === '0') {
      cooldownWaitable = Number.isFinite(reset) && reset > 0 && reset <= 30;
      cooldownHint = Number.isFinite(reset) && reset > 0 && reset <= 31_536_000 ? Math.ceil(reset) : null;
      cooldownUntil = cooldownWaitable ? performance.now() + Math.ceil(reset * 1000) : 0;
    }
    if (!response.ok) {
      await cancel(response);
      if (method === 'POST' && (response.status >= 500 || response.status === 408)) throw new Error(UNCERTAIN);
      const reason = { 400: 'Discord rejected the layout request.', 401: 'Discord rejected the bot token.',
        403: 'The bot lacks access or Manage Channels permission.', 404: 'Discord could not find this application or server.',
        429: 'Discord rate limited setup. Wait before retrying; no automatic retry was made.' }[response.status]
        ?? 'Discord did not confirm the layout request.';
      const retryAfter = Number(response.headers.get('retry-after'));
      const retryHint = response.status === 429 && Number.isFinite(retryAfter) && retryAfter > 0 && retryAfter <= 31_536_000
        ? ` Wait at least ${Math.ceil(retryAfter)} seconds.` : '';
      throw new Error(`${reason} (HTTP ${response.status})${retryHint}`);
    }
    try { return await readJson(response); }
    catch { throw new Error(method === 'POST' ? UNCERTAIN : 'Discord inspection response could not be verified. No creation was attempted.'); }
  }

  const application = await request('GET', '/oauth2/applications/@me');
  if (application?.id !== intent.application_id) throw new Error('DISCORD_BOT_TOKEN belongs to a different application. No creation was attempted.');
  const route = `/guilds/${intent.guild_id}/channels`;
  const channels = await request('GET', route);
  validateSnapshot(channels, intent.guild_id);
  const category = matching(channels, CATEGORY, 4, null);
  // Preflight every target before creating even the category. Same names in
  // another category, wrong channel types and case-only matches are conflicts.
  const children = intent.channels.map(channel => ({ channel,
    existing: matching(channels, channel.name, 0, category?.id ?? null) }));
  if (!category && children.some(child => child.existing)) {
    throw new Error('NL channel names already exist without the target category. No creation was attempted.');
  }
  const missing = children.filter(child => !child.existing).length;
  if (channels.length + missing + (category ? 0 : 1) > 500
      || (category && channels.filter(channel => channel.parent_id === category.id).length + missing > 50)) {
    throw new Error('The requested layout would exceed Discord channel capacity. No creation was attempted.');
  }
  const summary = { ...intent, preview: !apply, verified: true,
    category: { ...intent.category, operation: category ? 'reuse' : 'create', created: false, channel_id: category?.id ?? null },
    channels: children.map(({ channel, existing }) => ({ ...channel, parent_id: category?.id ?? null,
      operation: existing ? 'reuse' : 'create', created: false, channel_id: existing?.id ?? null })) };
  if (!apply) return summary;
  const knownIds = new Set(channels.map(channel => channel.id));
  async function confirm(target, parent) {
    if (target.operation === 'create') {
      const body = { name: target.name, type: target.type, parent_id: parent,
        ...(target.type === 0 ? { topic: target.topic } : {}) };
      const created = await request('POST', route, body);
      if (!created || !isSnowflake(created.id) || knownIds.has(created.id)
        || created.guild_id !== intent.guild_id || created.name !== body.name || created.type !== body.type
        || created.parent_id !== parent || (body.topic !== undefined && created.topic !== body.topic)) throw new Error(UNCERTAIN);
      knownIds.add(created.id); target.channel_id = created.id; target.created = true;
    }
    target.parent_id = parent;
    try { await progress({ status: 'confirmed', application_id: intent.application_id, guild_id: intent.guild_id,
      channel_id: target.channel_id, name: target.name, type: target.type, parent_id: parent,
      operation: target.created ? 'created' : 'reused' }); }
    catch { throw new Error('Confirmed layout progress could not be written. Inspect the server before continuing; no automatic retry was made.'); }
  }
  await confirm(summary.category, null);
  for (const child of summary.channels) await confirm(child, summary.category.channel_id);
  return { ...summary, cooldown_secs: cooldownWaitable ? Math.max(0, Math.ceil((cooldownUntil - performance.now()) / 1000)) : cooldownHint,
    cooldown_unknown_or_over_30_seconds: !cooldownWaitable };
}

export function parseArgs(args) {
  if (args.length > 1 || (args.length === 1 && !['--apply', '--inspect'].includes(args[0]))) throw new Error(usage());
  return { apply: args[0] === '--apply', inspect: args[0] === '--inspect' };
}
function usage() {
  return 'Usage: node scripts/provision-nl-layout.mjs [--inspect | --apply]. Default: offline intent. --inspect verifies application and existing channels using GET only. --apply creates only missing NL channels; never edits permissions or posts messages. Run one setup process at a time.';
}
export async function main(args = process.argv.slice(2), env = process.env) {
  if (args.length === 1 && args[0] === '--help') { process.stdout.write(`${usage()}\n`); return; }
  const result = await provisionNlLayout(env, parseArgs(args), fetch, value => new Promise((resolve, reject) => {
    process.stdout.write(`${JSON.stringify(value)}\n`, error => error ? reject(error) : resolve());
  }));
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}
if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
