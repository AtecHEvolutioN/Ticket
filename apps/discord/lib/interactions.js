import { createPublicKey, verify } from 'node:crypto';
import { readConfig, isSnowflake } from './config.js';
import { commandResponse, ephemeral } from './commands.js';

export const MAX_BODY_BYTES = 32 * 1024;
export const MAX_SIGNATURE_AGE_SECONDS = 300;
export const MAX_FUTURE_SKEW_SECONDS = 30;
const ED25519_SPKI_PREFIX = Buffer.from('302a300506032b6570032100', 'hex');

export function json(body, status = 200, extraHeaders = {}) {
  return Response.json(body, {
    status,
    headers: { 'Cache-Control': 'no-store', 'X-Content-Type-Options': 'nosniff', ...extraHeaders },
  });
}

async function readBody(request) {
  const length = request.headers.get('content-length');
  if (length !== null && (!/^[0-9]+$/.test(length) || Number(length) > MAX_BODY_BYTES)) {
    return null;
  }
  if (!request.body) return Buffer.alloc(0);
  const reader = request.body.getReader();
  const chunks = [];
  let size = 0;
  try {
    while (true) {
      const { done, value } = await reader.read();
      if (done) break;
      size += value.byteLength;
      if (size > MAX_BODY_BYTES) {
        await reader.cancel();
        return null;
      }
      chunks.push(Buffer.from(value));
    }
  } finally {
    reader.releaseLock();
  }
  return Buffer.concat(chunks, size);
}

// No state-changing commands are exposed. Timestamp freshness limits replay;
// retrying a signed request inside that window only returns the same static help.
// Future mutations require persistent interaction-ID deduplication before effects.
export async function handleInteraction(request, { env = process.env, nowMs = Date.now() } = {}) {
  if (request.method !== 'POST') return json({ error: 'method_not_allowed' }, 405, { Allow: 'POST' });
  const config = readConfig(env);
  if (!config.signatureReady) return json({ error: 'not_configured' }, 503);

  const signature = request.headers.get('x-signature-ed25519');
  const timestamp = request.headers.get('x-signature-timestamp');
  if (!signature || !/^[0-9a-fA-F]{128}$/.test(signature)
      || !timestamp || !/^[0-9]{1,12}$/.test(timestamp)) {
    return json({ error: 'invalid_signature' }, 401);
  }
  const seconds = Number(timestamp);
  const now = Math.floor(nowMs / 1000);
  if (!Number.isSafeInteger(now) || seconds < now - MAX_SIGNATURE_AGE_SECONDS
      || seconds > now + MAX_FUTURE_SKEW_SECONDS) {
    return json({ error: 'invalid_signature' }, 401);
  }
  if (!/^application\/json(?:\s*;|$)/i.test(request.headers.get('content-type') ?? '')) {
    return json({ error: 'unsupported_media_type' }, 415);
  }

  let raw;
  try {
    raw = await readBody(request);
  } catch {
    return json({ error: 'invalid_body' }, 400);
  }
  if (raw === null) return json({ error: 'body_too_large' }, 413);

  let valid = false;
  try {
    const key = createPublicKey({
      key: Buffer.concat([ED25519_SPKI_PREFIX, Buffer.from(config.publicKey, 'hex')]),
      format: 'der',
      type: 'spki',
    });
    valid = verify(null, Buffer.concat([Buffer.from(timestamp), raw]), key, Buffer.from(signature, 'hex'));
  } catch {
    // Never log request bodies, interaction tokens or configuration values.
    return json({ error: 'invalid_signature' }, 401);
  }
  if (!valid) return json({ error: 'invalid_signature' }, 401);

  let interaction;
  try {
    interaction = JSON.parse(new TextDecoder('utf-8', { fatal: true }).decode(raw));
  } catch {
    return json({ error: 'invalid_body' }, 400);
  }
  if (!interaction || typeof interaction !== 'object' || Array.isArray(interaction)
      || interaction.application_id !== config.applicationId) {
    return json({ error: 'invalid_interaction' }, 400);
  }
  // Discord verifies the endpoint with PING before it has a guild or member.
  if (interaction.type === 1) return json({ type: 1 });
  if (interaction.type !== 2) return json({ error: 'unsupported_interaction' }, 400);
  if (!config.commandsReady) return json(ephemeral('Ticket Watch commands are not configured yet.'));

  const memberId = interaction.member?.user?.id;
  if (interaction.guild_id !== config.guildId || !isSnowflake(memberId)
      || !config.allowedUserIds.includes(memberId) || interaction.member.user.bot === true
      || (interaction.context !== undefined && interaction.context !== 0)) {
    return json(ephemeral('You are not authorized to use this command here.'));
  }
  return json(commandResponse(interaction));
}
