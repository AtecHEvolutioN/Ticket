'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { options, targets, metadata, blocked, runCoverage } = require('./coverage.cjs');
const target = (id, host = 'ticketmaster.cz') => ({ id: 'artist-' + id, artist: 'Artist ' + id, venue: null,
  event_url: `https://www.${host}/event/synthetic/${id}` });
const observation = { adapter: 'ticketmaster-regional', observed_at: 0, availability: 'unknown',
  offers: [], ticket_categories: [{ title: 'Standing', quantity: { kind: 'selectable_order_capacity', value: 8 } }] };
async function fixture(action) {
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'ticket-coverage-'));
  const config = options(['--input', path.join(dir, 'input.json'), '--output', path.join(dir, 'report.json'),
    '--capture-dir', path.join(dir, 'pages')]);
  try { await action(config, dir); } finally { await fs.rm(dir, { recursive: true, force: true }); }
}
function fakeDependencies(overrides = {}) {
  let now = 1000000;
  const calls = [], sleeps = [];
  return { calls, sleeps, clock: () => now, sleep: async ms => { sleeps.push(ms); now += ms; },
    createBrowser: async () => ({ browser: { close: async () => {} }, context: { close: async () => {} } }),
    capture: async (_, request) => {
      calls.push(request); now += 100;
      return { id: request.id, status: 200, final_url: request.url, html: '<html>fixture</html>',
        adapter_state: 'ready', adapter_reason: 'regional', elapsed_ms: 100 };
    }, parse: async () => structuredClone(observation), ...overrides };
}

test('coverage arguments bound scope and prevent unsafe or duplicate target paths', () => {
  assert.equal(options(['--help']).help, true);
  const base = ['--input', 'input.json', '--output', 'report.json', '--capture-dir', 'captures/run'];
  assert.equal(options(base).limit, 100);
  for (const args of [['--limit', '301'], ['--host-gap-ms', '1999'], ['--timeout-ms', '120001'],
    ['--resume', '--replay'], ['--limit', '2', '--limit', '3']]) assert.throws(() => options([...base, ...args]));
  const first = target('1234567890');
  assert.equal(targets([first, { ...first, id: 'duplicate-url', event_url: first.event_url.replace('synthetic', 'other') }]).length, 1);
  assert.equal(targets([first])[0].venue, undefined);
  assert.throws(() => targets([{ ...first, id: '../escape' }]));
  assert.throws(() => targets([{ ...first, event_url: 'https://secure.ticketmaster.cz/payment' }]));
  assert.throws(() => targets([first, { ...target('1234567891'), id: first.id }]));
});

test('coverage uses only normal display requests, host pacing and private captures without stock totals', async () => {
  await fixture(async config => {
    const deps = fakeDependencies();
    const report = await runCoverage([target('1234567890'), target('1234567891'), target('1234567892', 'ticketmaster.nl')], config, deps);
    assert.equal(deps.calls.length, 3);
    assert.deepEqual(deps.sleeps, [2000]);
    for (const request of deps.calls) {
      assert.equal(request.adapter, 'ticketmaster');
      assert.equal(request.display, undefined);
      assert.equal(request.cart_selection, undefined);
      assert.equal(request.map_section_id, undefined);
    }
    assert.equal(report.summary.displayed_category_rows, 3);
    assert.equal(report.summary.remaining_stock_total, null);
    assert.equal(report.events[0].observation.availability, 'unknown');
    assert.equal(report.events[0].observation.observed_at, 0);
    assert.equal(report.events[0].event_identity_matched, true);
    assert.equal((await fs.stat(path.join(config.captureDir, 'artist-1234567890.html'))).mode & 0o077, 0);
    assert.equal((await fs.stat(config.captureDir)).mode & 0o077, 0);
    assert.equal((await fs.stat(config.output)).mode & 0o077, 0);
    assert.equal(report.finished, true);
  });
});

test('protection and Retry-After halt that host and do not invent a sold-out state', async () => {
  await fixture(async config => {
    const deps = fakeDependencies();
    const normal = deps.capture;
    deps.capture = async (ctx, request) => {
      if (request.url.includes('ticketmaster.cz')) {
        deps.calls.push(request);
        return { status: 429, final_url: request.url, html: '', retry_after_secs: 3600, adapter_state: 'blocked' };
      }
      return normal(ctx, request);
    };
    const report = await runCoverage([target('1234567890'), target('1234567891'), target('1234567892', 'ticketmaster.nl')], config, deps);
    assert.equal(deps.calls.length, 2);
    assert.deepEqual(report.events.map(e => e.state), ['blocked', 'not_attempted', 'parsed_ticket_details']);
    assert.equal(report.events[1].reason, 'host_blocked_after_prior_probe');
    assert.equal(report.events[0].capture.retry_after_secs, 3600);
    assert.equal(report.events[0].observation, undefined);
  });
});

test('resume replays saved captures and retains host blocks before considering missing targets', async () => {
  await fixture(async config => {
    const first = target('1234567890'), second = target('1234567891');
    await runCoverage([first], config, fakeDependencies({ capture: async (_, req) => ({
      status: 403, final_url: req.url, html: '', adapter_state: 'blocked', manual_intervention: 'access_blocked' }) }));
    const deps = fakeDependencies();
    const report = await runCoverage([second, first], { ...config, resume: true }, deps);
    assert.equal(deps.calls.length, 0);
    assert.equal(report.events[0].reason, 'cached_host_block');
    assert.equal(report.events[1].state, 'blocked');
    assert.equal(report.events[1].source, 'saved_capture');
  });
});

test('offline replay reparses after fixes, never starts a browser, and verifies capture hashes', async () => {
  await fixture(async config => {
    const first = target('1234567890');
    const initial = await runCoverage([first], config, fakeDependencies({ parse: async () => { throw new Error('parser bug'); } }));
    assert.equal(initial.events[0].state, 'parser_error');
    const deps = fakeDependencies({ createBrowser: () => { throw new Error('must not launch'); } });
    const report = await runCoverage([first, target('1234567891')], { ...config, replay: true }, deps);
    assert.equal(report.events[0].state, 'parsed_ticket_details');
    assert.equal(report.events[1].reason, 'capture_missing');
    assert.equal(deps.calls.length, 0);
    await fs.appendFile(path.join(config.captureDir, first.id + '.html'), 'changed');
    await assert.rejects(runCoverage([first], { ...config, replay: true }, deps), /capture_hash_mismatch/);
  });
});

test('coverage enforces deadline and URL binding while separating parser and transport failures', async () => {
  await fixture(async config => {
    const deps = fakeDependencies({ capture: async (_, req) => ({ status: 200,
      final_url: req.url.replace('1234567890', '9876543210'), html: '<html></html>' }) });
    const report = await runCoverage([target('1234567890')], config, deps);
    assert.equal(report.events[0].state, 'transport_error');
    assert.equal(report.events[0].reason, 'invalid_capture_result');
    const stop = fakeDependencies();
    let now = 0;
    stop.clock = () => (now += 60000);
    const expired = await runCoverage([target('1234567891')], { ...config, resume: true, maxMinutes: 1 }, stop);
    assert.equal(expired.events[0].reason, 'run_deadline');
    assert.equal(stop.calls.length, 0);
  });
});

test('capture metadata contains only allowlisted public transport details', () => {
  const first = target('1234567890');
  const meta = metadata(first, { status: 200, html: '<html></html>', final_url: first.event_url,
    server: 'private', cookie: 'secret', adapter_state: 'ready', retry_after_secs: 0 }, 1000000);
  assert.equal(JSON.stringify(meta).includes('secret'), false);
  assert.equal(meta.server, undefined);
  assert.equal(blocked(meta), false);
  assert.equal(blocked({ ...meta, manual_intervention: 'verification' }), true);
  assert.equal(blocked({ ...meta, retry_after_secs: 30 }), true);
});

test('an interrupted orphan capture is preserved and skipped without another provider request', async () => {
  await fixture(async config => {
    await fs.mkdir(config.captureDir, { mode: 0o700 });
    const first = target('1234567890');
    await fs.writeFile(path.join(config.captureDir, first.id + '.html'), '<html>incomplete</html>', { mode: 0o600 });
    const deps = fakeDependencies();
    const report = await runCoverage([first, target('1234567891')], { ...config, resume: true }, deps);
    assert.equal(report.events[0].reason, 'existing_capture_requires_review');
    assert.equal(report.events[0].state, 'not_attempted');
    assert.equal(deps.calls.length, 1);
    assert.equal(await fs.readFile(path.join(config.captureDir, first.id + '.html'), 'utf8'), '<html>incomplete</html>');
  });
});

test('fixed worker errors remain diagnostic and repeated connection failures halt only their host', async () => {
  await fixture(async config => {
    const deps = fakeDependencies();
    const normal = deps.capture;
    deps.capture = async (ctx, request) => {
      if (request.url.includes('ticketmaster.cz')) { deps.calls.push(request); return { error: 'navigation_timeout' }; }
      return normal(ctx, request);
    };
    const report = await runCoverage([target('1234567890'), target('1234567891'), target('1234567892'),
      target('1234567893'), target('1234567894', 'ticketmaster.nl')], config, deps);
    assert.equal(deps.calls.length, 4);
    assert.equal(report.events[0].reason, 'navigation_timeout');
    assert.equal(report.events[2].state, 'transport_error');
    assert.equal(report.events[3].reason, 'repeated_host_transport_failures');
    assert.equal(report.events[4].state, 'parsed_ticket_details');
  });
});

test('browser launch failure stops later probes and never prints arbitrary exception text', async () => {
  await fixture(async config => {
    let launches = 0;
    const deps = fakeDependencies({ createBrowser: async () => { launches++; throw new Error('private_session_secret'); } });
    const report = await runCoverage([target('1234567890'), target('1234567891', 'ticketmaster.nl')], config, deps);
    assert.equal(launches, 1);
    assert.equal(report.events[0].reason, 'capture_failed');
    assert.equal(report.events[1].reason, 'browser_unavailable');
    assert.equal(JSON.stringify(report).includes('private_session_secret'), false);
  });
});

test('queue redirect is a persisted host block, never a transport retry or leaked private URL', async () => {
  await fixture(async config => {
    const first = target('1234567890', 'ticketmaster.it'), next = target('1234567891', 'ticketmaster.it');
    const deps = fakeDependencies();
    deps.capture = async (_, request) => { deps.calls.push(request); throw new Error('queue_redirect'); };
    const report = await runCoverage([first, next], config, deps);
    assert.equal(deps.calls.length, 1);
    assert.equal(report.events[0].state, 'blocked');
    assert.equal(report.events[0].reason, 'queue_redirect');
    assert.equal(report.events[0].capture, undefined);
    assert.equal(report.events[1].reason, 'host_blocked_after_prior_probe');
    const resume = fakeDependencies({ createBrowser: () => { throw new Error('must not launch'); } });
    const resumed = await runCoverage([next, first], { ...config, resume: true }, resume);
    assert.equal(resumed.events[0].reason, 'cached_host_block');
    assert.equal(resumed.events[1].reason, 'queue_redirect');
    assert.equal(resumed.events[1].source, 'saved_diagnostic');
    assert.equal(resume.calls.length, 0);
    assert.equal((await fs.stat(path.join(config.captureDir, first.id + '.error.json'))).mode & 0o077, 0);
    assert.equal(resumed.events[1].observation, undefined);
  });
});

test('parser-discovered protection blocks the host and is rechecked before resumed live requests', async () => {
  await fixture(async config => {
    const first = target('1234567890'), next = target('1234567891');
    const blockedObservation = { ...observation, availability: 'blocked', ticket_categories: [], offers: [] };
    const deps = fakeDependencies({ parse: async () => blockedObservation });
    const report = await runCoverage([first, next], config, deps);
    assert.equal(deps.calls.length, 1);
    assert.equal(report.events[0].capture.status, 200);
    assert.equal(report.events[0].capture.adapter_state, 'ready');
    assert.equal(report.events[0].state, 'blocked');
    assert.equal(report.events[0].reason, 'parsed_access_block');
    assert.equal(report.events[1].reason, 'host_blocked_after_prior_probe');
    const resumedDeps = fakeDependencies({ parse: async () => blockedObservation,
      createBrowser: () => { throw new Error('must not launch before saved protection is checked'); } });
    const resumed = await runCoverage([next, first], { ...config, resume: true }, resumedDeps);
    assert.equal(resumed.events[0].reason, 'cached_parser_access_block');
    assert.equal(resumed.events[1].state, 'blocked');
    assert.equal(resumed.events[1].reason, 'parsed_access_block');
    assert.equal(resumedDeps.calls.length, 0);
    assert.equal(resumed.summary.remaining_stock_total, null);
  });
});
