'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const { existsSync } = require('node:fs');
const http = require('node:http');
const path = require('node:path');
const os = require('node:os');
const crypto = require('node:crypto');
const { spawnSync } = require('node:child_process');
const { once } = require('node:events');
const { ApiError, configFromEnv, createHistoryServer, exportHistory, parseQuery } = require('./history-api.cjs');

const ROOT = path.resolve(__dirname, '..');
const TOKEN = 'synthetic-test-token-only-0123456789abcdef';
const ORIGIN = 'https://ticket-discord.vercel.app';
const EVENT = 'https://www.ticketmaster.nl/event/synthetic-show/777777';
const NOW = Date.parse('2026-11-13T12:00:00Z');
const empty = () => Buffer.from(JSON.stringify({ schema_version: 1, generated_at: Math.floor(NOW / 1000), events: [] }));
const config = extra => ({ ...configFromEnv({ TICKET_HISTORY_TOKEN: TOKEN }), port: 0, minIntervalMs: 0, ...extra });

async function serve(t, options = {}, overrides = {}) {
  const server = createHistoryServer(config(overrides), { clock: () => NOW, exporter: async () => empty(), ...options });
  server.listen(0, '127.0.0.1');
  await once(server, 'listening');
  t.after(async () => { server.closeAllConnections(); await new Promise(resolve => server.close(resolve)); });
  const base = `http://127.0.0.1:${server.address().port}`;
  const get = (route = '/api/history', opts = {}) => fetch(base + route, { headers: { Authorization: `Bearer ${TOKEN}`, Origin: ORIGIN }, ...opts });
  return { base, get, server };
}

test('configuration requires a private token, fixed paths, loopback binding and exact origins', () => {
  for (const env of [{}, { TICKET_HISTORY_TOKEN: 'short' }, { TICKET_HISTORY_TOKEN: TOKEN, TICKET_HISTORY_HOST: '0.0.0.0' }, { TICKET_HISTORY_TOKEN: TOKEN, TICKET_HISTORY_DATABASES: '["relative.sqlite"]' }, { TICKET_HISTORY_TOKEN: TOKEN, TICKET_HISTORY_ALLOWED_ORIGINS: '["*"]' }, { TICKET_HISTORY_TOKEN: TOKEN, TICKET_HISTORY_ALLOWED_ORIGINS: '["https://example.com/path"]' }]) {
    assert.throws(() => configFromEnv(env));
  }
  assert.equal(configFromEnv({ TICKET_HISTORY_TOKEN: TOKEN }).host, '127.0.0.1');
});

test('bounded query rejects paths, credentials, invalid dates and duplicate parameters', () => {
  const parse = query => parseQuery(new URL(`http://127.0.0.1/api/history${query}`), NOW);
  assert.deepEqual(parse(''), { from: '2026-11-11T12:00:00.000Z', to: '2026-11-13T12:00:00.000Z', eventUrl: null });
  assert.equal(parse(`?event_url=${encodeURIComponent(EVENT)}&from=2026-11-12T12:00:00.000Z`).eventUrl, EVENT);
  for (const query of ['?database=/etc/passwd', '?token=private', '?from=2026-11-01', '?from=2026-02-30T00:00:00Z', '?from=2026-11-12T24:00:00Z', '?from=2024-01-01T00:00:00Z', '?to=2027-01-01T00:00:00Z', '?from=2026-11-12T00:00:00Z&from=2026-11-13T00:00:00Z', '?event_url=http://127.0.0.1/event/777', '?event_url=https://ticketmaster.nl.evil.example/event/777', '?event_url=https://user:secret@ticketmaster.nl/event/777', '?event_url=https://ticketmaster.nl/event/777?token=private']) {
    assert.throws(() => parse(query), error => error.status === 400, query);
  }
});

test('authentication is required before export and origin failures cannot expose history', async t => {
  let calls = 0;
  const { get } = await serve(t, { exporter: async () => { calls++; return empty(); } });
  for (const headers of [{}, { Authorization: 'Bearer invalid' }, { Authorization: `Bearer ${TOKEN}`, Origin: 'https://evil.example' }, { Authorization: `Bearer ${TOKEN}`, Origin: 'null' }]) {
    const response = await get('/api/history', { headers });
    assert.ok([401, 403].includes(response.status));
    assert.equal(response.headers.get('cache-control'), 'no-store');
    assert.ok(!(await response.text()).includes(TOKEN));
  }
  assert.equal(calls, 0);
  const response = await get();
  assert.equal(response.status, 200);
  assert.equal(response.headers.get('access-control-allow-origin'), ORIGIN);
  assert.equal(response.headers.get('access-control-expose-headers'), 'Retry-After');
  assert.deepEqual(await response.json(), JSON.parse(empty()));
  assert.equal(calls, 1);
});

test('preflight allows only approved origin, GET and Authorization; mutation methods are refused', async t => {
  const { get } = await serve(t);
  const good = await get('/api/history', { method: 'OPTIONS', headers: { Origin: ORIGIN, 'Access-Control-Request-Method': 'GET', 'Access-Control-Request-Headers': 'authorization' } });
  assert.equal(good.status, 204);
  assert.equal(good.headers.get('access-control-allow-headers'), 'Authorization');
  const bad = await get('/api/history', { method: 'OPTIONS', headers: { Origin: ORIGIN, 'Access-Control-Request-Method': 'POST' } });
  assert.equal(bad.status, 403);
  assert.equal((await get('/api/history', { method: 'POST' })).status, 405);
  assert.equal((await get('/api/health?database=private')).status, 400);
});

test('duplicate authorization and body-bearing GET requests are refused', async t => {
  const { base } = await serve(t);
  async function raw(headers) {
    return await new Promise((resolve, reject) => {
      const request = http.request(base + '/api/history', { method: 'GET', headers }, response => { response.resume(); response.on('end', () => resolve(response.statusCode)); });
      request.on('error', reject);
      request.end();
    });
  }
  assert.equal(await raw(['Host', new URL(base).host, 'Authorization', `Bearer ${TOKEN}`, 'Authorization', `Bearer ${TOKEN}`]), 401);
  assert.equal(await raw({ Authorization: `Bearer ${TOKEN}`, 'Content-Length': '1' }), 400);
});

test('concurrent requests and excessive refreshes are bounded without cached cross-request responses', async t => {
  let finish;
  let started;
  const begun = new Promise(resolve => { started = resolve; });
  let nextStarted;
  const nextBegun = new Promise(resolve => { nextStarted = resolve; });
  let calls = 0;
  let now = NOW;
  const { get } = await serve(t, { clock: () => now, exporter: () => { calls++; return new Promise(resolve => { finish = resolve; (calls === 1 ? started : nextStarted)(); }); } }, { minIntervalMs: 5000 });
  const first = get();
  await begun;
  const busy = await get();
  assert.equal(busy.status, 429);
  assert.equal(busy.headers.get('retry-after'), '5');
  finish(empty());
  assert.equal((await first).status, 200);
  assert.equal((await get()).status, 429);
  now += 5001;
  const next = get();
  await nextBegun;
  finish(empty());
  assert.equal((await next).status, 200);
  assert.equal(calls, 2);
});

test('errors are redacted and health never equates an export with watcher readiness', async t => {
  let fail = false;
  const history = { schema_version: 1, generated_at: NOW / 1000, events: [{ sources: [{ samples: [{ observed_at: NOW / 1000 - 120 }] }] }] };
  const { get } = await serve(t, { exporter: async () => { if (fail) throw new Error('/private/database.sqlite token=secret'); return Buffer.from(JSON.stringify(history)); } });
  let health = await (await get('/api/health')).json();
  assert.equal(health.status, 'awaiting_first_export');
  assert.equal(health.latest_observation_age_secs, null);
  await get();
  health = await (await get('/api/health')).json();
  assert.equal(health.watcher_status, 'unverified');
  assert.equal(health.latest_observation_age_secs, 120);
  assert.equal(health.observation_scope, 'last_successful_filtered_history_request');
  assert.equal(health.observation_age_measures, 'any_observation_including_blocked_or_unknown');
  fail = true;
  const error = await get();
  assert.equal(error.status, 503);
  assert.ok(!(await error.text()).includes('private'));
  health = await (await get('/api/health')).json();
  assert.equal(health.status, 'export_error');
});

test('future observation timestamps remain unknown instead of falsely fresh', async t => {
  const history = { schema_version: 1, generated_at: NOW / 1000, events: [{ sources: [{ samples: [{ observed_at: NOW / 1000 + 60 }] }] }] };
  const { get } = await serve(t, { exporter: async () => Buffer.from(JSON.stringify(history)) });
  await get();
  const health = await (await get('/api/health')).json();
  assert.equal(health.latest_observation_age_secs, null);
  assert.equal(health.observation_clock_invalid, true);
});

async function temp(t) {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'history-api-test-'));
  t.after(() => fs.rm(directory, { recursive: true, force: true }));
  return directory;
}

test('missing configured databases never get created', async t => {
  const directory = await temp(t);
  const missing = path.join(directory, 'missing.sqlite');
  await assert.rejects(exportHistory(config({ binary: process.execPath, databases: [missing] }), parseQuery(new URL('http://localhost/api/history'), NOW)), error => error.status === 503);
  assert.equal(existsSync(missing), false);
});

test('child exporter receives fixed arguments, no token environment, and private temporary output', async t => {
  const directory = await temp(t);
  const binary = path.join(directory, 'exporter');
  const database = path.join(directory, 'fixed.sqlite');
  await fs.writeFile(database, 'synthetic');
  await fs.writeFile(binary, `#!${process.execPath}\nconst fs=require('node:fs'); const args=process.argv.slice(2); const output=args[args.indexOf('--output')+1]; fs.writeFileSync(output,JSON.stringify({schema_version:1,generated_at:1,events:[],args,private_mode:fs.statSync(process.cwd()).mode & 511,token_inherited:!!process.env.TICKET_HISTORY_TOKEN}));\n`, { mode: 0o700 });
  const query = parseQuery(new URL(`http://localhost/api/history?event_url=${encodeURIComponent(EVENT)}`), NOW);
  const result = JSON.parse(await exportHistory(config({ binary, databases: [database] }), query));
  assert.equal(result.token_inherited, false);
  assert.equal(result.private_mode, 0o700);
  assert.equal(result.args[0], 'history-export');
  assert.equal(result.args[result.args.indexOf('--database') + 1], database);
  assert.equal(result.args[result.args.indexOf('--event-url') + 1], EVENT);
  assert.equal(existsSync(path.dirname(result.args[result.args.indexOf('--output') + 1])), false);
});

test('timed out exporter is killed and stderr never leaves the service', async t => {
  const directory = await temp(t);
  const binary = path.join(directory, 'slow-exporter');
  const database = path.join(directory, 'fixed.sqlite');
  await fs.writeFile(database, 'synthetic');
  await fs.writeFile(binary, `#!${process.execPath}\nconsole.error('SECRET PRIVATE PATH');setInterval(()=>{},1000);\n`, { mode: 0o700 });
  await assert.rejects(exportHistory(config({ binary, databases: [database], timeoutMs: 100 }), parseQuery(new URL('http://localhost/api/history'), NOW)), error => error.status === 504 && !error.message.includes('SECRET'));
});

test('disconnect kills the active exporter before another export is admitted', async t => {
  const directory = await temp(t);
  const binary = path.join(directory, 'waiting-exporter');
  const database = path.join(directory, 'fixed.sqlite');
  const marker = path.join(directory, 'started.json');
  await fs.writeFile(database, 'synthetic');
  await fs.writeFile(binary, `#!${process.execPath}\nrequire('node:fs').writeFileSync(${JSON.stringify(marker)},JSON.stringify({pid:process.pid,cwd:process.cwd()}));setInterval(()=>{},1000);\n`, { mode: 0o700 });
  const { get } = await serve(t, { exporter: exportHistory }, { binary, databases: [database] });
  const controller = new AbortController();
  const request = get('/api/history', { signal: controller.signal }).catch(error => error);
  const deadline = Date.now() + 3000;
  while (!existsSync(marker) && Date.now() < deadline) await new Promise(resolve => setTimeout(resolve, 10));
  const started = JSON.parse(await fs.readFile(marker));
  controller.abort();
  assert.equal((await request).name, 'AbortError');
  let health;
  do {
    health = await (await get('/api/health')).json();
    if (health.last_error === 'client_closed') break;
    await new Promise(resolve => setTimeout(resolve, 10));
  } while (Date.now() < deadline);
  assert.equal(health.last_error, 'client_closed');
  assert.throws(() => process.kill(started.pid, 0), { code: 'ESRCH' });
  assert.equal(existsSync(started.cwd), false);
});

test('real SQLite history is read without mutation and a new observation appears on refresh', async t => {
  const binary = process.env.TICKET_HISTORY_TEST_BINARY || ['target/debug/ticket-watch', 'target/release/ticket-watch'].map(file => path.join(ROOT, file)).find(existsSync);
  if (!binary) return t.skip('Build ticket-watch or set TICKET_HISTORY_TEST_BINARY for SQLite export integration');
  const directory = await temp(t);
  const database = path.join(directory, 'history.sqlite');
  const at = Math.floor(NOW / 1000) - 120;
  const fixture = `import sqlite3,json,hashlib,sys\np=sys.argv[1];at=int(sys.argv[2]);price=int(sys.argv[3]);db=sqlite3.connect(p)\ndb.executescript("CREATE TABLE IF NOT EXISTS market_history_meta(id INTEGER PRIMARY KEY,schema_version INTEGER); CREATE TABLE IF NOT EXISTS market_events(event_id TEXT PRIMARY KEY,event_url TEXT,name TEXT,artist TEXT,country TEXT,city TEXT,first_observed_at INTEGER,last_observed_at INTEGER); CREATE TABLE IF NOT EXISTS market_sources(source_id TEXT PRIMARY KEY,event_id TEXT,watch_id TEXT,scope TEXT,interval_secs INTEGER); CREATE TABLE IF NOT EXISTS market_samples(id INTEGER PRIMARY KEY,source_id TEXT,observed_at INTEGER,payload_hash TEXT,sample_json TEXT); INSERT OR IGNORE INTO market_history_meta VALUES(1,1);")\nevent='tm:ticketmaster.nl:777777';source='a'*64\ndb.execute('INSERT OR IGNORE INTO market_events VALUES(?,?,?,?,?,?,?,?)',(event,'${EVENT.replace('www.', '')}','Synthetic show','Synthetic artist','NL','Amsterdam',at,at))\ndb.execute('UPDATE market_events SET last_observed_at=? WHERE event_id=?',(at,event))\ndb.execute('INSERT OR IGNORE INTO market_sources VALUES(?,?,?,?,?)',(source,event,'synthetic','event_page',60))\ndim=['Standard','primary','EUR','per_ticket',None,'included','listing','offer']\nseries={'series_id':hashlib.sha256(json.dumps(dim,separators=(',',':')).encode()).hexdigest(),'ticket_type':'Standard','inventory_kind':'primary','currency':'EUR','price_scope':'per_ticket','price_quantity':None,'fees':'included','quantity_kind':'listing','kind':'offer','seat':None,'min_price_minor':price,'max_price_minor':price,'median_price_minor':price,'observed_count':1,'reported_quantity':2}\nsample={'observed_at':at,'availability':'available','evidence':'offers','complete':False,'series':[series]}\ndb.execute('INSERT INTO market_samples(source_id,observed_at,payload_hash,sample_json) VALUES(?,?,?,?)',(source,at,str(at),json.dumps(sample)))\ndb.commit();db.close()\n`;
  function record(time, price) {
    const result = spawnSync('python3', ['-c', fixture, database, String(time), String(price)], { encoding: 'utf8' });
    assert.equal(result.status, 0, result.stderr || result.error?.message);
  }
  const hash = async () => crypto.createHash('sha256').update(await fs.readFile(database)).digest('hex');
  record(at, 5000);
  const { get } = await serve(t, { exporter: exportHistory }, { binary, databases: [database] });
  const before = await hash();
  const firstResponse = await get();
  assert.equal(firstResponse.status, 200, await firstResponse.clone().text());
  const first = await firstResponse.json();
  assert.equal(first.events[0].sources[0].samples[0].series[0].min_price_minor, 5000);
  assert.equal(await hash(), before);
  record(at + 60, 5100);
  const afterInsert = await hash();
  const second = await (await get()).json();
  assert.equal(second.events[0].sources[0].samples.length, 2);
  assert.equal(second.events[0].sources[0].samples[1].series[0].min_price_minor, 5100);
  assert.equal(await hash(), afterInsert);
  const selected = await get(`/api/history?event_url=${encodeURIComponent(EVENT)}`);
  assert.equal(selected.status, 200);
  assert.equal((await get('/api/history?event_url=https://www.ticketmaster.nl/event/absent/888888')).status, 404);
  assert.equal((await (await get('/api/history?from=2026-11-12T00:00:00Z&to=2026-11-12T01:00:00Z')).json()).events.length, 0);
});
