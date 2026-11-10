import assert from 'node:assert/strict';
import test from 'node:test';
import { registrationPlan, registerCommands } from './register-commands.mjs';

const env = { DISCORD_APPLICATION_ID: '123', DISCORD_GUILD_ID: '456', DISCORD_BOT_TOKEN: 'private-test-token' };

test('offline registration plan requires no bot token and never includes it', () => {
  assert.equal(registrationPlan({ ...env, DISCORD_BOT_TOKEN: undefined }).command.name, 'ticket');
  assert.ok(!JSON.stringify(registrationPlan(env)).includes(env.DISCORD_BOT_TOKEN));
  for (const value of ['0', '-1', '01', '1.0', '18446744073709551616']) {
    assert.throws(() => registrationPlan({ ...env, DISCORD_GUILD_ID: value }));
  }
});

test('registers only /ticket after checking token application identity', async () => {
  const calls = [];
  const fetcher = async (url, options) => {
    calls.push({ url, options });
    assert.equal(options.redirect, 'error');
    assert.equal(options.headers.authorization, `Bot ${env.DISCORD_BOT_TOKEN}`);
    return Response.json(options.method === 'GET' ? { id: '123' } : { id: '789', name: 'ticket', application_id: '123', guild_id: '456' });
  };
  const result = await registerCommands(env, fetcher);
  assert.equal(result.registered, true);
  assert.equal(calls.length, 2);
  assert.equal(calls[0].options.method, 'GET');
  assert.equal(calls[1].options.method, 'POST');
  assert.match(calls[1].url, /\/applications\/123\/guilds\/456\/commands$/);
  assert.equal(JSON.parse(calls[1].options.body).name, 'ticket');
  assert.ok(!JSON.stringify(result).includes(env.DISCORD_BOT_TOKEN));
});

test('mismatched application or rejected credentials cannot register commands', async () => {
  let calls = 0;
  await assert.rejects(registerCommands(env, async () => {
    calls += 1;
    return Response.json({ id: '999', token: env.DISCORD_BOT_TOKEN });
  }), /different application/);
  assert.equal(calls, 1);
  await assert.rejects(registerCommands(env, async () => Response.json({ token: env.DISCORD_BOT_TOKEN }, { status: 401 })), error => !error.message.includes(env.DISCORD_BOT_TOKEN) && /401/.test(error.message));
});

test('rate limits and transport ambiguity do not cause retries or leak server bodies', async () => {
  for (const mode of ['429', 'transport']) {
    let calls = 0;
    await assert.rejects(registerCommands(env, async () => {
      calls += 1;
      if (calls === 1) return Response.json({ id: '123' });
      if (mode === 'transport') throw new Error(env.DISCORD_BOT_TOKEN);
      return Response.json({ private: env.DISCORD_BOT_TOKEN }, { status: 429 });
    }), error => !error.message.includes(env.DISCORD_BOT_TOKEN) && (mode === '429' ? /rate limited/.test(error.message) : /uncertain/.test(error.message)));
    assert.equal(calls, 2);
  }
});

test('bad successful responses do not claim registration success', async () => {
  for (const body of [{ id: '0', name: 'ticket', application_id: '123', guild_id: '456' }, { id: '789', name: 'other', application_id: '123', guild_id: '456' }]) {
    await assert.rejects(registerCommands(env, async (_url, options) => Response.json(options.method === 'GET' ? { id: '123' } : body)));
  }
});

test('response stream failures are redacted and an ambiguous POST is not retried', async () => {
  for (const failedMethod of ['GET', 'POST']) {
    let calls = 0;
    await assert.rejects(registerCommands(env, async (_url, options) => {
      calls += 1;
      if (options.method !== failedMethod) return Response.json({ id: '123' });
      return new Response(new ReadableStream({
        start(controller) { controller.error(new Error(env.DISCORD_BOT_TOKEN)); },
      }));
    }), error => !error.message.includes(env.DISCORD_BOT_TOKEN)
      && (failedMethod === 'POST' ? /uncertain.*before retrying/.test(error.message) : /No command was registered/.test(error.message)));
    assert.equal(calls, failedMethod === 'GET' ? 1 : 2);
  }
});
