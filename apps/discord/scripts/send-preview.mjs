import { createHash } from 'node:crypto';
import { readFileSync } from 'node:fs';
import { pathToFileURL } from 'node:url';
import { isSnowflake } from '../lib/config.js';

const API = 'https://discord.com/api/v10';
const EVIDENCE = new URL('../../../docs/evidence/netherlands-seat-validation-2026-11-13.json', import.meta.url);
const UNCERTAIN = 'Preview delivery is uncertain. Inspect #bot-status before retrying; no automatic retry was made.';

function id(value, label) {
  if (!isSnowflake(value)) throw new Error(`${label} must be a Discord numeric ID.`);
  return value;
}

export function previewPlan(env) {
  const application = id(env.DISCORD_APPLICATION_ID, 'DISCORD_APPLICATION_ID');
  const guild = id(env.DISCORD_GUILD_ID, 'DISCORD_GUILD_ID');
  const channel = id(env.DISCORD_PREVIEW_CHANNEL_ID, 'DISCORD_PREVIEW_CHANNEL_ID');
  const evidence = JSON.parse(readFileSync(EVIDENCE, 'utf8'));
  const fields = evidence.validated_hover_observations.map(item => {
    const inspection = item.validated_inspection;
    const quote = inspection.displayed_quote;
    if (!['primary', 'resale'].includes(inspection.displayed_inventory_kind)
      || quote.currency !== 'EUR' || quote.price_basis !== 'per_ticket'
      || !Number.isSafeInteger(quote.price_minor) || quote.price_minor < 0) {
      throw new Error('Recorded preview evidence could not be validated.');
    }
    const observed = new Date(item.observed_at);
    if (!Number.isFinite(observed.getTime())) throw new Error('Recorded preview timestamp is invalid.');
    return {
      name: `${inspection.displayed_inventory_kind === 'primary' ? 'Primary' : 'Resale'} · ${item.venue}`,
      value: `**€${(quote.price_minor / 100).toFixed(2)} per ticket**\nSection ${quote.section} · row ${quote.row} · seat ${quote.seat}\n${quote.ticket_type}\nObserved <t:${Math.floor(observed.getTime() / 1000)}:f>\n[Event page](${item.event_url})`,
      inline: true,
    };
  });
  if (fields.length !== 2) throw new Error('Expected two recorded seat observations.');
  return {
    application_id: application, guild_id: guild, channel_id: channel,
    message: {
      content: 'Ticket Watch preview is ready. Try `/ticket ping` or `/ticket chart`.',
      allowed_mentions: { parse: [], users: [], roles: [], replied_user: false },
      embeds: [{
        title: 'Ticket Watch · Netherlands preview',
        description: 'Recorded seat-price evidence from two Dutch venues. Stock, complete fees and adjacency are unknown.\n\n**Watcher:** awaiting always-on host. **Carting:** disabled. The chart viewer includes a labelled synthetic demonstration.',
        color: 0x71e6c0,
        fields,
        footer: { text: 'Recorded observations · current availability requires a fresh check' },
      }],
      components: [{ type: 1, components: [
        { type: 2, style: 5, label: 'Open price charts', url: 'https://ticket-discord.vercel.app/market.html' },
        { type: 2, style: 5, label: 'Open NL channels', url: `https://discord.com/channels/${guild}/${channel}` },
      ] }],
    },
  };
}

async function json(response) {
  const chunks = [];
  let size = 0;
  for await (const chunk of response.body ?? []) {
    size += chunk.byteLength;
    if (size > 1024 * 1024) throw new Error('response_limit');
    chunks.push(Buffer.from(chunk));
  }
  return JSON.parse(Buffer.concat(chunks).toString('utf8'));
}

export async function sendPreview(env, fetchImpl = fetch) {
  const plan = previewPlan(env);
  const token = env.DISCORD_BOT_TOKEN?.trim();
  if (!token || /[^\x21-\x7e]/.test(token)) throw new Error('Set DISCORD_BOT_TOKEN privately.');
  // Workflow reruns retain the run ID. Discord nonce deduplication is bounded;
  // ambiguous outcomes still stop for inspection rather than retrying.
  if (!/^[0-9]{1,20}$/.test(env.GITHUB_RUN_ID ?? '')) throw new Error('A valid GITHUB_RUN_ID is required for this preview.');
  const nonce = createHash('sha256').update(`ticket-preview:${plan.guild_id}:${env.GITHUB_RUN_ID}`).digest('hex').slice(0, 24);
  async function request(path, method = 'GET', body) {
    let response;
    try {
      response = await fetchImpl(API + path, {
        method, redirect: 'error', signal: AbortSignal.timeout(20_000),
        headers: { authorization: `Bot ${token}`, 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
      });
    } catch {
      throw new Error(method === 'POST' ? UNCERTAIN : 'Discord preview preflight failed. No message was sent.');
    }
    if (!response.ok) {
      try { await response.body?.cancel(); } catch { /* Never expose provider errors. */ }
      if (method === 'POST' && (response.status >= 500 || response.status === 408)) throw new Error(UNCERTAIN);
      throw new Error(`Discord preview request rejected (HTTP ${response.status}). No automatic retry was made.`);
    }
    try { return await json(response); }
    catch { throw new Error(method === 'POST' ? UNCERTAIN : 'Discord preview preflight response was invalid. No message was sent.'); }
  }
  const application = await request('/oauth2/applications/@me');
  if (application?.id !== plan.application_id) throw new Error('Token belongs to a different application. No message was sent.');
  const channel = await request(`/channels/${plan.channel_id}`);
  if (channel?.id !== plan.channel_id || channel.guild_id !== plan.guild_id || channel.type !== 0 || channel.name !== 'bot-status') {
    throw new Error('Expected #bot-status in the configured server. No message was sent.');
  }
  const message = await request(`/channels/${plan.channel_id}/messages`, 'POST', { ...plan.message, nonce, enforce_nonce: true });
  if (!isSnowflake(message?.id) || message.channel_id !== plan.channel_id || message.author?.id !== plan.application_id || message.author?.bot !== true) throw new Error(UNCERTAIN);
  return { delivered: true, channel_id: plan.channel_id, message_id: message.id, message_url: `https://discord.com/channels/${plan.guild_id}/${plan.channel_id}/${message.id}` };
}

export async function main(args = process.argv.slice(2), env = process.env) {
  if (args.length > 1 || (args.length === 1 && args[0] !== '--send')) throw new Error('Usage: node scripts/send-preview.mjs [--send]. Default: offline preview.');
  const result = args[0] === '--send' ? await sendPreview(env) : { preview: true, ...previewPlan(env) };
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => { process.stderr.write(`${error.message}\n`); process.exitCode = 1; });
}
