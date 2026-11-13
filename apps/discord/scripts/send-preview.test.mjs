import assert from 'node:assert/strict';
import test from 'node:test';
import { previewPlan, sendPreview } from './send-preview.mjs';

const env = { DISCORD_APPLICATION_ID: '123', DISCORD_GUILD_ID: '456', DISCORD_PREVIEW_CHANNEL_ID: '789', DISCORD_BOT_TOKEN: 'private-test-only', GITHUB_RUN_ID: '1000' };
const application = { id: '123' };
const channel = { id: '789', guild_id: '456', type: 0, name: 'bot-status' };
const receipt = { id: '900', channel_id: '789', author: { id: '123', bot: true } };

test('preview labels recorded quotes, unknown stock and inactive watcher without mentions or credentials', () => {
  const plan = previewPlan(env);
  const raw = JSON.stringify(plan);
  assert.equal(plan.message.embeds[0].fields.length, 2);
  assert.match(raw, /€89.95/);
  assert.match(raw, /€73.90/);
  assert.match(raw, /Stock, complete fees and adjacency are unknown/);
  assert.match(raw, /awaiting always-on host/);
  assert.match(raw, /disabled/);
  assert.ok(!raw.includes(env.DISCORD_BOT_TOKEN));
  assert.deepEqual(plan.message.allowed_mentions.parse, []);
  assert.throws(() => previewPlan({ ...env, DISCORD_GUILD_ID: '0' }));
});

test('verifies application and target server/channel before sending one preview', async () => {
  const calls = [];
  async function run() {
    return sendPreview(env, async (url, options) => {
      calls.push({ url, options });
      assert.equal(options.redirect, 'error');
      return Response.json(url.endsWith('/@me') ? application : options.method === 'GET' ? channel : receipt);
    });
  }
  const result = await run();
  assert.equal(result.delivered, true);
  assert.equal(calls.filter(call => call.options.method === 'POST').length, 1);
  const body = JSON.parse(calls[2].options.body);
  assert.equal(body.enforce_nonce, true);
  assert.equal(body.nonce.length, 24);
  await run();
  assert.equal(JSON.parse(calls[5].options.body).nonce, body.nonce);
  assert.equal(result.message_url, 'https://discord.com/channels/456/789/900');
});

test('wrong application or wrong guild/channel prevents delivery', async () => {
  for (const [app, target] of [[{ id: '999' }, channel], [application, { ...channel, guild_id: '999' }], [application, { ...channel, name: 'general' }], [application, { ...channel, type: 4 }]]) {
    const methods = [];
    await assert.rejects(sendPreview(env, async (url, options) => {
      methods.push(options.method);
      return Response.json(url.endsWith('/@me') ? app : target);
    }), /No message was sent/);
    assert.ok(!methods.includes('POST'));
  }
});

test('ambiguous sends, rate limits and invalid receipts stop without retry or leaking bodies', async () => {
  for (const mode of ['transport', 'stream', '429', '500', 'receipt']) {
    let posts = 0;
    await assert.rejects(sendPreview(env, async (url, options) => {
      if (url.endsWith('/@me')) return Response.json(application);
      if (options.method === 'GET') return Response.json(channel);
      posts++;
      if (mode === 'transport') throw new Error(env.DISCORD_BOT_TOKEN);
      if (mode === 'stream') return new Response(new ReadableStream({ start(controller) { controller.error(new Error(env.DISCORD_BOT_TOKEN)); } }));
      if (mode === 'receipt') return Response.json({ ...receipt, channel_id: '999' });
      return Response.json({ private: env.DISCORD_BOT_TOKEN }, { status: Number(mode) });
    }), error => !error.message.includes(env.DISCORD_BOT_TOKEN) && /uncertain|429/.test(error.message));
    assert.equal(posts, 1);
  }
});
