import assert from 'node:assert/strict';
import test from 'node:test';
import { channelIntent, parseArgs, provisionChannel } from './provision-channel.mjs';

const env = { DISCORD_APPLICATION_ID: '123', DISCORD_GUILD_ID: '456', DISCORD_BOT_TOKEN: 'private-channel-token' };
const options = { name: 'ticket-alerts' };
const category = { id: '700', name: 'tickets', type: 4, guild_id: '456', parent_id: null };
const channel = { id: '789', name: 'ticket-alerts', type: 0, guild_id: '456', parent_id: null };

function mock({ application = { id: '123' }, channels = [], create = channel } = {}) {
  const calls = [];
  return {
    calls,
    fetch: async (url, request) => {
      calls.push({ url, request });
      assert.equal(request.redirect, 'error');
      assert.equal(request.headers.authorization, `Bot ${env.DISCORD_BOT_TOKEN}`);
      assert.ok(request.signal instanceof AbortSignal);
      if (url.endsWith('/oauth2/applications/@me')) return Response.json(application);
      assert.equal(url, 'https://discord.com/api/v10/guilds/456/channels');
      if (request.method === 'GET') return Response.json(channels);
      if (typeof create === 'function') return create();
      return Response.json(create);
    },
  };
}

test('offline intent validates public fields and never implies existing channel inspection', () => {
  const intent = channelIntent({ ...env, DISCORD_BOT_TOKEN: undefined }, options);
  assert.deepEqual(intent.channel, { type: 0, name: 'ticket-alerts', parent_id: null });
  assert.ok(!JSON.stringify(intent).includes(env.DISCORD_BOT_TOKEN));
  for (const name of ['', 'UPPER', 'has space', '@everyone', '-alerts', 'a'.repeat(101)]) {
    assert.throws(() => channelIntent(env, { name }));
  }
  for (const parent of ['0', '-1', '01', 'not-id', '18446744073709551616']) {
    assert.throws(() => channelIntent(env, { ...options, parent }));
  }
});

test('argument parser is bounded to one explicit target and rejects unknown/duplicate switches', () => {
  assert.deepEqual(parseArgs(['--name', 'ticket-alerts', '--parent', '700', '--apply']), {
    name: 'ticket-alerts', parent: '700', apply: true,
  });
  for (const args of [[], ['--apply'], ['--name'], ['--name', '--apply'],
    ['--name', 'ticket-alerts', '--delete'], ['--name', 'a', '--name', 'b'],
    ['--name', 'a', '--apply', '--apply']]) assert.throws(() => parseArgs(args));
});

test('missing or malformed token fails before network and errors are redacted', async () => {
  let called = false;
  for (const token of [undefined, '', ' ', '\nprivate-secret\nwith-newline']) {
    await assert.rejects(provisionChannel({ ...env, DISCORD_BOT_TOKEN: token }, options, async () => {
      called = true;
    }), error => /Set DISCORD_BOT_TOKEN privately/.test(error.message) && !error.message.includes('private-secret'));
  }
  assert.equal(called, false);
});

test('mismatched token application stops before listing or creation', async () => {
  const fixture = mock({ application: { id: '999' } });
  await assert.rejects(provisionChannel(env, { ...options, apply: true }, fixture.fetch), /different application/);
  assert.equal(fixture.calls.length, 1);
});

test('verified preview inspects application and channels using GET only', async () => {
  const fixture = mock();
  const result = await provisionChannel(env, options, fixture.fetch);
  assert.equal(result.preview, true);
  assert.equal(result.verified, true);
  assert.equal(result.operation, 'create');
  assert.equal(result.created, false);
  assert.equal(result.channel_id, undefined);
  assert.deepEqual(fixture.calls.map(call => call.request.method), ['GET', 'GET']);
});

test('an existing exact channel is reused in preview and apply with no POST', async () => {
  for (const apply of [false, true]) {
    const fixture = mock({ channels: [channel] });
    const result = await provisionChannel(env, { ...options, apply }, fixture.fetch);
    assert.equal(result.operation, 'reuse');
    assert.equal(result.created, false);
    assert.equal(result.channel_id, '789');
    assert.equal(fixture.calls.length, 2);
  }
});

test('ambiguous same-name/type/parent channels stop before POST', async () => {
  const fixture = mock({ channels: [channel, { ...channel, id: '790' }] });
  await assert.rejects(provisionChannel(env, { ...options, apply: true }, fixture.fetch), /Multiple text channels/);
  assert.equal(fixture.calls.length, 2);
});

test('parent must be an existing category in the selected server', async () => {
  for (const channels of [[], [{ ...category, type: 0 }], [{ ...category, guild_id: '999' }]]) {
    const fixture = mock({ channels });
    await assert.rejects(provisionChannel(env, { ...options, parent: '700', apply: true }, fixture.fetch));
    assert.equal(fixture.calls.length, 2);
  }
  const fixture = mock({ channels: [category, { ...channel, parent_id: '700' }] });
  const result = await provisionChannel(env, { ...options, parent: '700', apply: true }, fixture.fetch);
  assert.equal(result.operation, 'reuse');
  assert.equal(fixture.calls.length, 2);
});

test('apply creates exactly one normal text channel with no permission overwrites', async () => {
  const fixture = mock({ channels: [category, { ...channel, type: 2 }], create: { ...channel, id: '790', parent_id: '700' } });
  const result = await provisionChannel(env, { ...options, parent: '700', apply: true }, fixture.fetch);
  assert.equal(result.created, true);
  assert.equal(result.channel_id, '790');
  assert.equal(fixture.calls.length, 3);
  const post = fixture.calls[2].request;
  assert.equal(post.method, 'POST');
  assert.deepEqual(JSON.parse(post.body), { name: 'ticket-alerts', type: 0, parent_id: '700' });
  assert.match(post.headers['x-audit-log-reason'], /Ticket%20Watch/);
  assert.ok(!JSON.stringify(result).includes(env.DISCORD_BOT_TOKEN));
});

test('returned channel identity, guild, type, name and parent must match the request', async () => {
  for (const created of [null, { ...channel, id: '0' }, { ...channel, name: 'other' },
    { ...channel, guild_id: '999' }, { ...channel, type: 2 }, { ...channel, parent_id: '700' },
    { ...channel, parent_id: undefined }]) {
    const fixture = mock({ create: created });
    await assert.rejects(provisionChannel(env, { ...options, apply: true }, fixture.fetch), /uncertain.*before retrying/);
    assert.equal(fixture.calls.length, 3);
  }
});

test('POST transport failure, server failure and unreadable bodies are uncertain and never retried', async () => {
  for (const create of [
    () => { throw new Error(env.DISCORD_BOT_TOKEN); },
    () => Response.json({ secret: env.DISCORD_BOT_TOKEN }, { status: 500 }),
    () => Response.json({}, { status: 408 }),
    () => new Response('not-json-secret-' + env.DISCORD_BOT_TOKEN),
    () => new Response(new ReadableStream({ start(controller) { controller.error(new Error(env.DISCORD_BOT_TOKEN)); } })),
    () => new Response('secret', { headers: { 'content-length': '1048577' } }),
    () => new Response('s'.repeat(1048577)),
  ]) {
    const fixture = mock({ create });
    await assert.rejects(provisionChannel(env, { ...options, apply: true }, fixture.fetch), error =>
      /uncertain.*before retrying/.test(error.message) && !error.message.includes(env.DISCORD_BOT_TOKEN));
    assert.equal(fixture.calls.length, 3);
  }
});

test('credential/permission errors and rate limits are redacted without retries', async () => {
  for (const status of [401, 403, 404, 429]) {
    const fixture = mock({ create: () => Response.json({ private: env.DISCORD_BOT_TOKEN }, { status }) });
    await assert.rejects(provisionChannel(env, { ...options, apply: true }, fixture.fetch), error =>
      error.message.includes(String(status)) && !error.message.includes(env.DISCORD_BOT_TOKEN));
    assert.equal(fixture.calls.length, 3);
  }
});

test('unreadable GET responses and malformed listings cannot proceed to creation', async () => {
  for (const bad of [{}, [null], [{ ...channel, id: '0' }], [channel, channel],
    [{ ...channel, guild_id: '999' }], [{ ...channel, parent_id: '0' }]]) {
    const fixture = mock({ channels: bad });
    await assert.rejects(provisionChannel(env, { ...options, apply: true }, fixture.fetch), /No channel creation was attempted/);
    assert.equal(fixture.calls.length, 2);
  }
  let calls = 0;
  await assert.rejects(provisionChannel(env, options, async () => {
    calls += 1;
    return new Response(new ReadableStream({ start(controller) { controller.error(new Error(env.DISCORD_BOT_TOKEN)); } }));
  }), error => /No channel creation was attempted/.test(error.message) && !error.message.includes(env.DISCORD_BOT_TOKEN));
  assert.equal(calls, 1);
});

test('normal repeated apply reuses a now-visible channel without another POST', async () => {
  let visible = false;
  let posts = 0;
  const fetcher = async (url, request) => {
    if (url.endsWith('/oauth2/applications/@me')) return Response.json({ id: '123' });
    if (request.method === 'GET') return Response.json(visible ? [channel] : []);
    posts += 1;
    visible = true;
    return Response.json(channel);
  };
  assert.equal((await provisionChannel(env, { ...options, apply: true }, fetcher)).created, true);
  assert.equal((await provisionChannel(env, { ...options, apply: true }, fetcher)).operation, 'reuse');
  assert.equal(posts, 1);
});
