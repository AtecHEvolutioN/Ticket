import assert from 'node:assert/strict';
import { generateKeyPairSync, sign } from 'node:crypto';
import { afterEach, test } from 'node:test';
import { POST } from '../api/interactions.js';
import { GET } from '../api/health.js';
import { readConfig } from '../lib/config.js';
import { ticketCommand } from '../lib/commands.js';
import { handleInteraction, MAX_BODY_BYTES } from '../lib/interactions.js';

const { publicKey, privateKey } = generateKeyPairSync('ed25519');
const exported = publicKey.export({ format: 'der', type: 'spki' });
const applicationId = '123456789012345678';
const guildId = '234567890123456789';
const userId = '345678901234567890';
const nowMs = 1794300000000;
const env = {
  DISCORD_PUBLIC_KEY: exported.subarray(exported.length - 32).toString('hex'),
  DISCORD_APPLICATION_ID: applicationId,
  DISCORD_GUILD_ID: guildId,
  DISCORD_ALLOWED_USER_IDS: userId,
};
const originalEnv = Object.fromEntries(Object.keys(env).map(key => [key, process.env[key]]));
afterEach(() => {
  for (const [key, value] of Object.entries(originalEnv)) {
    if (value === undefined) delete process.env[key];
    else process.env[key] = value;
  }
});

function command(name = 'ping', changes = {}) {
  return {
    application_id: applicationId,
    type: 2,
    guild_id: guildId,
    context: 0,
    member: { user: { id: userId } },
    data: { type: 1, name: 'ticket', options: [{ type: 1, name }] },
    ...changes,
  };
}

function request(payload, { timestamp = String(Math.floor(nowMs / 1000)), headers = {}, raw } = {}) {
  const body = raw ?? Buffer.from(JSON.stringify(payload));
  const signature = sign(null, Buffer.concat([Buffer.from(timestamp), body]), privateKey).toString('hex');
  return new Request('https://commands.example/api/interactions', {
    method: 'POST',
    headers: {
      'content-type': 'application/json',
      'x-signature-ed25519': signature,
      'x-signature-timestamp': timestamp,
      ...headers,
    },
    body,
  });
}

function invoke(payload, requestOptions, options = {}) {
  return handleInteraction(request(payload, requestOptions), { env, nowMs, ...options });
}

test('actual POST route validates the signed raw body and emits a PONG without guild/member', async () => {
  Object.assign(process.env, env);
  delete process.env.DISCORD_GUILD_ID;
  delete process.env.DISCORD_ALLOWED_USER_IDS;
  const response = await POST(request({ application_id: applicationId, type: 1 }, {
    timestamp: String(Math.floor(Date.now() / 1000)),
    // Extra whitespace is signed as-is, and must never be reparsed before verify.
    raw: Buffer.from(`{ "type": 1, "application_id": "${applicationId}" }\n`),
  }));
  assert.equal(response.status, 200);
  assert.deepEqual(await response.json(), { type: 1 });
  assert.equal(response.headers.get('cache-control'), 'no-store');
});

test('actual exported route returns ephemeral ping and explains its limited scope', async () => {
  Object.assign(process.env, env);
  const response = await POST(request(command(), { timestamp: String(Math.floor(Date.now() / 1000)) }));
  assert.equal(response.status, 200);
  const result = await response.json();
  assert.equal(result.type, 4);
  assert.equal(result.data.flags, 64);
  assert.deepEqual(result.data.allowed_mentions, { parse: [] });
  assert.match(result.data.content, /command endpoint only/);
  assert.match(result.data.content, /does not confirm/);
});

test('help is static, does not echo supplied interaction fields, and mentions no users', async () => {
  const payload = command('help', { token: 'private-interaction-token', channel_id: '@everyone' });
  const response = await invoke(payload);
  const result = await response.json();
  assert.match(result.data.content, /Rust watcher sends ticket alerts directly/);
  assert.doesNotMatch(JSON.stringify(result), /private-interaction-token|@everyone/);
  assert.deepEqual(result.data.allowed_mentions, { parse: [] });
});

test('chart command opens a local-import viewer without claiming live sales data', async () => {
  const response = await invoke(command('chart'));
  const result = await response.json();
  assert.equal(result.data.flags, 64);
  assert.match(result.data.content, /https:\/\/ticket-discord\.vercel\.app\/market\.html/);
  assert.match(result.data.content, /Files stay on your device/);
  assert.match(result.data.content, /not connected to a live watcher/);
  assert.match(result.data.content, /not confirmed sales/);
  assert.deepEqual(result.data.allowed_mentions, { parse: [] });
});

test('missing, malformed, wrong-key and modified-body signatures return 401', async () => {
  for (const signature of ['', 'a'.repeat(127), 'z'.repeat(128), '00'.repeat(64)]) {
    const response = await invoke(command(), { headers: { 'x-signature-ed25519': signature } });
    assert.equal(response.status, 401);
    assert.deepEqual(await response.json(), { error: 'invalid_signature' });
  }
  const signed = request(command());
  const tampered = new Request(signed.url, {
    method: 'POST', headers: signed.headers, body: JSON.stringify(command('help')),
  });
  assert.equal((await handleInteraction(tampered, { env, nowMs })).status, 401);

  const other = generateKeyPairSync('ed25519').publicKey.export({ format: 'der', type: 'spki' });
  assert.equal((await invoke(command(), undefined, {
    env: { ...env, DISCORD_PUBLIC_KEY: other.subarray(other.length - 32).toString('hex') },
  })).status, 401);
});

test('expired or future signatures and malformed timestamps are rejected', async () => {
  const current = Math.floor(nowMs / 1000);
  for (const timestamp of [String(current - 301), String(current + 31), '', '-1', '1.5', '1e9', ' 1', '0'.repeat(13)]) {
    assert.equal((await invoke(command(), { timestamp })).status, 401, timestamp);
  }
  assert.equal((await invoke(command(), { timestamp: String(current - 300) })).status, 200);
  assert.equal((await invoke(command(), { timestamp: String(current + 30) })).status, 200);
});

test('missing key or application ID fail closed without exposing configuration', async () => {
  for (const bad of [
    {},
    { ...env, DISCORD_PUBLIC_KEY: 'invalid-secret-value' },
    { ...env, DISCORD_APPLICATION_ID: '18446744073709551616' },
  ]) {
    const response = await invoke(command(), undefined, { env: bad });
    assert.equal(response.status, 503);
    assert.deepEqual(await response.json(), { error: 'not_configured' });
  }
});

test('signed payload must belong to this application, including a PING', async () => {
  for (const payload of [null, [], 'text', 2, {},
    { type: 1 }, { type: 1, application_id: '999999999999999999' },
    command('ping', { application_id: '999999999999999999' })]) {
    const response = await invoke(payload);
    assert.equal(response.status, 400);
    assert.deepEqual(await response.json(), { error: 'invalid_interaction' });
  }
});

test('commands require exact guild, explicit allowlisted member, and guild context', async () => {
  for (const changes of [
    { guild_id: '999999999999999999' },
    { guild_id: undefined, member: undefined, user: { id: userId } },
    { member: { user: { id: '999999999999999999' } } },
    { member: { user: { id: userId, bot: true } } },
    { member: undefined },
    { member: { user: {} } },
    { context: 1 },
    { context: 2 },
    { context: '0' },
  ]) {
    const response = await invoke(command('ping', changes));
    const body = await response.json();
    assert.equal(response.status, 200);
    assert.equal(body.data.flags, 64);
    assert.deepEqual(body.data.allowed_mentions, { parse: [] });
    assert.match(body.data.content, /not authorized/);
  }
  // context is optional for older Discord payloads; guild and member remain required.
  assert.match((await (await invoke(command('ping', { context: undefined }))).json()).data.content,
    /commands are connected/);
});

test('invalid or missing operator configuration disables commands, not PING handshake', async () => {
  for (const changes of [
    { DISCORD_ALLOWED_USER_IDS: '' },
    { DISCORD_ALLOWED_USER_IDS: undefined },
    { DISCORD_ALLOWED_USER_IDS: `${userId},` },
    { DISCORD_ALLOWED_USER_IDS: `${userId},0` },
    { DISCORD_GUILD_ID: undefined },
    { DISCORD_GUILD_ID: '000000000000000000' },
  ]) {
    const configEnv = { ...env, ...changes };
    assert.match((await (await invoke(command(), undefined, { env: configEnv })).json()).data.content,
      /not configured/);
    assert.deepEqual(await (await invoke({ type: 1, application_id: applicationId }, undefined,
      { env: configEnv })).json(), { type: 1 });
  }
  const configEnv = { ...env, DISCORD_ALLOWED_USER_IDS: `999999999999999999, ${userId}` };
  assert.match((await (await invoke(command(), undefined, { env: configEnv })).json()).data.content,
    /commands are connected/);
});

test('unknown command shapes and extra options have no effects and receive static help', async () => {
  for (const data of [
    null,
    {},
    { name: 'ticket', type: 2 },
    { name: 'buy', type: 1, options: [{ name: 'ping', type: 1 }] },
    { name: 'ticket', type: 1, options: [{ name: 'buy', type: 1 }] },
    { name: 'ticket', type: 1, options: [] },
    { name: 'ticket', type: 1, options: [{ name: 'ping', type: 2 }] },
    { name: 'ticket', type: 1, options: [{ name: 'ping', type: 1 }, { name: 'help', type: 1 }] },
    { name: 'ticket', type: 1, options: [{ name: 'ping', type: 1, options: [{ type: 3, name: 'url', value: 'https://example.com' }] }] },
  ]) {
    const response = await invoke(command('ping', { data }));
    assert.match((await response.json()).data.content, /not supported/);
  }
});

test('only PING and application commands are handled', async () => {
  for (const type of [0, 3, 4, 5, 99, '2']) {
    assert.equal((await invoke(command('ping', { type }))).status, 400);
  }
});

test('malformed signed JSON and invalid UTF-8 are rejected', async () => {
  for (const raw of [Buffer.from('{'), Buffer.from(''), Buffer.from([0xff, 0xfe])]) {
    const response = await invoke(null, { raw });
    assert.equal(response.status, 400);
    assert.deepEqual(await response.json(), { error: 'invalid_body' });
  }
});

test('body limit covers Content-Length and streamed bytes independently', async () => {
  const response = await invoke(command(), { headers: { 'content-length': String(MAX_BODY_BYTES + 1) } });
  assert.equal(response.status, 413);
  const tooLarge = Buffer.alloc(MAX_BODY_BYTES + 1, 32);
  assert.equal((await invoke(command(), { raw: tooLarge })).status, 413);
  assert.equal((await invoke(command(), { raw: tooLarge, headers: { 'content-length': '1' } })).status, 413);
  // A matching signature over invalid JSON at exactly the limit passes the size guard.
  assert.equal((await invoke(command(), { raw: Buffer.alloc(MAX_BODY_BYTES, 32) })).status, 400);
});

test('method and content type are restricted', async () => {
  const get = new Request('https://commands.example/api/interactions');
  const response = await handleInteraction(get, { env, nowMs });
  assert.equal(response.status, 405);
  assert.equal(response.headers.get('allow'), 'POST');
  assert.equal((await invoke(command(), { headers: { 'content-type': 'text/plain' } })).status, 415);
  assert.equal((await invoke(command(), { headers: { 'content-type': 'application/jsonp' } })).status, 415);
  assert.equal((await invoke(command(), { headers: { 'content-type': 'application/json; charset=utf-8' } })).status, 200);
});

test('health endpoint reveals no identifiers, secrets, or claimed worker health', async () => {
  for (const key of Object.keys(env)) delete process.env[key];
  let response = GET();
  assert.equal(response.status, 503);
  assert.deepEqual(await response.json(), {
    service: 'ticket-discord-commands', status: 'needs_setup', watcher_status: 'unverified',
  });
  Object.assign(process.env, env);
  response = GET();
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('cache-control'), 'no-store');
  assert.deepEqual(await response.json(), {
    service: 'ticket-discord-commands', status: 'configured', watcher_status: 'unverified',
  });
});

test('registered command defaults to disabled and does not request DM access or purchase actions', () => {
  assert.equal(ticketCommand.default_member_permissions, '0');
  assert.equal(ticketCommand.dm_permission, false);
  assert.deepEqual(ticketCommand.options.map(value => value.name), ['ping', 'help', 'chart']);
});

test('configuration bounds prohibit invalid snowflakes and oversized allowlists', () => {
  for (const ids of ['0', '18446744073709551616', '0'.repeat(18), Array(101).fill(userId).join(',')]) {
    assert.equal(readConfig({ ...env, DISCORD_ALLOWED_USER_IDS: ids }).commandsReady, false);
  }
  assert.equal(readConfig({ ...env, DISCORD_ALLOWED_USER_IDS: ` ${userId} ` }).commandsReady, true);
});
