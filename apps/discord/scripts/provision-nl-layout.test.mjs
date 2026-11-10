import assert from 'node:assert/strict';
import test from 'node:test';
import { readFile } from 'node:fs/promises';
import { layoutIntent, parseArgs, provisionNlLayout } from './provision-nl-layout.mjs';

const env = { DISCORD_APPLICATION_ID: '123', DISCORD_GUILD_ID: '456', DISCORD_BOT_TOKEN: 'synthetic-private-token' };
const category = { id: '700', guild_id: '456', name: 'TICKETMASTER NL', type: 4, parent_id: null };
const existing = () => [category, ...layoutIntent(env).channels.map((channel, index) => ({
  id: String(701 + index), guild_id: '456', name: channel.name, type: 0, parent_id: '700', topic: 'Existing operator topic remains unchanged',
}))];

function mock({ application = { id: '123' }, channels = [], create } = {}) {
  const calls = []; const visible = structuredClone(channels); let posts = 0;
  return { calls, get posts() { return posts; }, fetch: async (url, request) => {
    calls.push({ url, request });
    assert.equal(request.redirect, 'error');
    assert.equal(request.headers.authorization, `Bot ${env.DISCORD_BOT_TOKEN}`);
    assert.ok(request.signal instanceof AbortSignal);
    if (url.endsWith('/oauth2/applications/@me')) { assert.equal(request.method, 'GET'); return Response.json(application); }
    assert.equal(url, 'https://discord.com/api/v10/guilds/456/channels');
    if (request.method === 'GET') return Response.json(visible);
    assert.equal(request.method, 'POST');
    const body = JSON.parse(request.body); posts += 1;
    const created = { id: String(1000 + posts), guild_id: '456', ...body };
    const response = create ? await create(body, posts, created) : Response.json(created, { status: 201 });
    if (response.ok) visible.push(created);
    return response;
  } };
}

test('offline default gives exactly nine bounded topics without requests, secrets or invented running status', async () => {
  const plan = await provisionNlLayout(env, {}, () => { throw new Error('must not fetch'); });
  assert.equal(plan.preview, true); assert.equal(plan.verified, false);
  assert.equal(plan.category.name, 'TICKETMASTER NL');
  assert.deepEqual(plan.channels.map(channel => channel.name), [
    'nl-upcoming-sales', 'nl-presales', 'nl-on-sale', 'nl-primary', 'nl-resale', 'nl-price-changes', 'nl-event-updates', 'nl-charts', 'bot-status',
  ]);
  assert.ok(plan.channels.every(channel => channel.type === 0 && channel.topic.length <= 1024));
  assert.match(plan.channels.find(channel => channel.name === 'nl-charts').topic, /not connected to live charts yet/);
  assert.match(plan.channels.find(channel => channel.name === 'bot-status').topic, /does not mean monitoring is running/);
  assert.equal(plan.messages_sent, 0);
  assert.ok(!JSON.stringify(plan).includes(env.DISCORD_BOT_TOKEN));
  assert.deepEqual(parseArgs([]), { apply: false, inspect: false });
  assert.deepEqual(parseArgs(['--inspect']), { apply: false, inspect: true });
  assert.deepEqual(parseArgs(['--apply']), { apply: true, inspect: false });
  for (const args of [['--delete'], ['--apply', '--apply'], ['--inspect', '--apply']]) assert.throws(() => parseArgs(args));
  for (const id of ['0', '01', '18446744073709551616', 'private-value']) {
    assert.throws(() => layoutIntent({ ...env, DISCORD_GUILD_ID: id }), error => !error.message.includes(id));
  }
});

test('missing token and wrong application stop before any channel mutation', async () => {
  for (const token of [undefined, '', ' ', 'synthetic-secret\ninside']) {
    let requests = 0;
    await assert.rejects(provisionNlLayout({ ...env, DISCORD_BOT_TOKEN: token }, { apply: true }, () => { requests++; }),
      error => /Set DISCORD_BOT_TOKEN privately/.test(error.message) && !error.message.includes('synthetic-secret'));
    assert.equal(requests, 0);
  }
  const fixture = mock({ application: { id: '999' } });
  await assert.rejects(provisionNlLayout(env, { apply: true }, fixture.fetch), /different application/);
  assert.equal(fixture.calls.length, 1);
});

test('inspection performs only two GETs and apply reuses exact existing channels without editing topics', async () => {
  const preview = mock();
  const plan = await provisionNlLayout(env, { inspect: true }, preview.fetch);
  assert.equal(plan.verified, true); assert.equal(plan.preview, true);
  assert.equal(plan.category.operation, 'create');
  assert.deepEqual(preview.calls.map(call => call.request.method), ['GET', 'GET']);
  const fixture = mock({ channels: existing() }); const progress = [];
  const result = await provisionNlLayout(env, { apply: true }, fixture.fetch, item => progress.push(item));
  assert.equal(fixture.posts, 0); assert.equal(progress.length, 10);
  assert.ok(progress.every(item => item.operation === 'reused'));
  assert.ok(result.channels.every(channel => channel.operation === 'reuse' && !channel.created));
  assert.equal(result.category.channel_id, '700');
});

test('apply creates only missing category and children, reports each ID before proceeding, and repeated apply reuses them', async () => {
  const progress = [];
  const fixture = mock({ create: (body, index, created) => {
    assert.equal(progress.length, index - 1, 'confirmed progress must precede the next create');
    assert.deepEqual(Object.keys(body).sort(), index === 1 ? ['name', 'parent_id', 'type'] : ['name', 'parent_id', 'topic', 'type']);
    assert.equal(body.parent_id, index === 1 ? null : '1001');
    return Response.json(created, { status: 201 });
  } });
  const result = await provisionNlLayout(env, { apply: true }, fixture.fetch, item => progress.push(item));
  assert.equal(fixture.posts, 10); assert.equal(progress.length, 10);
  assert.equal(result.category.created, true);
  assert.ok(result.channels.every(channel => channel.created && channel.parent_id === '1001'));
  assert.ok(!JSON.stringify(result).includes(env.DISCORD_BOT_TOKEN));
  await provisionNlLayout(env, { apply: true }, fixture.fetch);
  assert.equal(fixture.posts, 10, 'rerunning with a current snapshot must not duplicate channels');
  assert.ok(fixture.calls.every(call => !call.url.includes('/messages') && !['PATCH', 'DELETE', 'PUT'].includes(call.request.method)));

  const partial = mock({ channels: existing().slice(0, 4) });
  await provisionNlLayout(env, { apply: true }, partial.fetch);
  assert.equal(partial.posts, 6);
});

test('every duplicate, mismatched parent and wrong-type target is rejected before any creation', async () => {
  const child = existing()[1];
  for (const channels of [
    [category, { ...category, id: '799' }], [{ ...category, type: 0 }],
    [{ ...category, name: 'Ticketmaster NL' }], [category, { ...child, parent_id: null }],
    [category, child, { ...child, id: '799' }], [category, { ...child, type: 2 }],
    [{ ...child, parent_id: null }], [category, { ...child, parent_id: '999' }],
    [category, { ...child, guild_id: '999' }], [category, { ...child, id: category.id }],
  ]) {
    const fixture = mock({ channels });
    await assert.rejects(provisionNlLayout(env, { apply: true }, fixture.fetch), /No creation|no creation/);
    assert.equal(fixture.posts, 0); assert.equal(fixture.calls.length, 2);
  }
});

test('ambiguous POST outcomes stop immediately and preserve already confirmed public progress', async () => {
  for (const fail of [
    () => { throw new Error(env.DISCORD_BOT_TOKEN); },
    () => Response.json({ secret: env.DISCORD_BOT_TOKEN }, { status: 503 }),
    () => Response.json({}, { status: 408 }),
    () => new Response('not-json-' + env.DISCORD_BOT_TOKEN),
    () => new Response('oversized', { headers: { 'content-length': '1048577' } }),
    () => Response.json({ id: '999', guild_id: '456', name: 'wrong', type: 0, parent_id: '1001' }),
  ]) {
    const progress = [];
    const fixture = mock({ create: (body, index, created) => index === 1 ? Response.json(created) : fail() });
    await assert.rejects(provisionNlLayout(env, { apply: true }, fixture.fetch, item => progress.push(item)),
      error => /uncertain.*before retrying/.test(error.message) && !error.message.includes(env.DISCORD_BOT_TOKEN));
    assert.equal(fixture.posts, 2); assert.equal(progress.length, 1);
    assert.equal(progress[0].channel_id, '1001');
  }
});

test('known successful cooldown waits before a different POST without retrying any creation', async () => {
  const progress = []; const waits = [];
  const fixture = mock({ create: (body, index, created) => Response.json(created, { headers: index === 1
    ? { 'x-ratelimit-remaining': '0', 'x-ratelimit-reset-after': '5' } : {} }) });
  await provisionNlLayout(env, { apply: true }, fixture.fetch, item => progress.push(item), async milliseconds => {
    assert.equal(progress.length, 1); assert.ok(milliseconds > 0 && milliseconds <= 5000); waits.push(milliseconds);
  });
  assert.equal(waits.length, 1); assert.equal(fixture.posts, 10);
  assert.equal(new Set(fixture.calls.filter(call => call.request.method === 'POST').map(call => JSON.parse(call.request.body).name)).size, 10);
  for (const reset of [undefined, 'bad', '31']) {
    const headers = { 'x-ratelimit-remaining': '0', ...(reset === undefined ? {} : { 'x-ratelimit-reset-after': reset }) };
    const stopped = mock({ create: (body, index, created) => Response.json(created, { headers }) });
    await assert.rejects(provisionNlLayout(env, { apply: true }, stopped.fetch), /cooldown/);
    assert.equal(stopped.posts, 1);
  }
});

test('credential, permission and rate-limit errors remain redacted and are never retried', async () => {
  for (const status of [401, 403, 404, 429]) {
    const fixture = mock({ create: () => Response.json({ secret: env.DISCORD_BOT_TOKEN }, { status }) });
    await assert.rejects(provisionNlLayout(env, { apply: true }, fixture.fetch), error =>
      error.message.includes(String(status)) && !error.message.includes(env.DISCORD_BOT_TOKEN));
    assert.equal(fixture.posts, 1);
  }
});

test('manual workflow pins its actions and scopes the existing secret to the main-only creation step', async () => {
  const workflow = await readFile(new URL('../../../.github/workflows/discord-nl-setup.yml', import.meta.url), 'utf8');
  assert.match(workflow, /workflow_dispatch:/);
  assert.match(workflow, /if: github\.ref == 'refs\/heads\/main'/);
  assert.match(workflow, /contents: read/); assert.match(workflow, /timeout-minutes: 12/);
  assert.match(workflow, /cancel-in-progress: false/);
  assert.match(workflow, /actions\/checkout@[0-9a-f]{40}/);
  assert.match(workflow, /actions\/setup-node@[0-9a-f]{40}/);
  assert.match(workflow, /persist-credentials: false/);
  assert.equal((workflow.match(/secrets\.DISCORD_SETUP_BOT_TOKEN/g) || []).length, 1);
  assert.match(workflow, /DISCORD_APPLICATION_ID: '1569720679757127740'/);
  assert.match(workflow, /DISCORD_GUILD_ID: '1569717598709350491'/);
  assert.match(workflow, /run: node apps\/discord\/scripts\/provision-nl-layout\.mjs --apply/);
});
