'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const os = require('node:os');
const { PassThrough } = require('node:stream');
const { parseArgs, writeCapture, provenance, captureSession, MAX_BYTES } = require('./capture.cjs');

const url = 'https://www.ticketmaster.pl/event/synthetic/123456789';
const record = () => ({ original_url: `${url}?language=en`, final_url: url,
  captured_at: '2026-11-09T12:00:00.000Z', status: 200, headers: { authorization: 'do-not-copy' } });

test('capture arguments require one public event and bounded interactive timeout', () => {
  const args = parseArgs(['--url', url, '--output', 'captures/example.html']);
  assert.equal(args.timeoutSecs, 300);
  assert.equal(args.overwrite, false);
  assert.ok(path.isAbsolute(args.output));
  assert.deepEqual(parseArgs(['--help']), { help: true });
  for (const values of [
    ['--url', url],
    ['--url', url, '--output', 'x', '--timeout-secs', '0'],
    ['--url', url, '--output', 'x', '--timeout-secs', '1801'],
    ['--url', url, '--output', 'x', '--timeout-secs', '1e3'],
    ['--url', url, '--output', 'x', '--output', 'y'],
    ['--url', 'https://www.ticketmaster.pl/artist/12345', '--output', 'x'],
    ['--url', 'https://www.ticketmaster.pl/login/../event/123456789', '--output', 'x'],
    ['--url', 'https://secure.ticketmaster.pl/payment/?t=secret', '--output', 'x'],
  ]) assert.throws(() => parseArgs(values));
});

test('provenance strips queries and unrequested fields and cannot mark a profile verified', () => {
  const safe = provenance({ ...record(), rendered: false, profile_verified: true });
  assert.deepEqual(safe, { schema_version: 1, original_url: url, final_url: url, captured_at: '2026-11-09T12:00:00.000Z',
    status: 200, rendered: true, profile_verified: false });
  assert.throws(() => provenance({ ...record(), status: 403 }));
  assert.throws(() => provenance({ ...record(), final_url: url.replace('123456789', '999999999') }));
});

test('private capture refuses overwrite and publishes matching unverified sidecar', async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'ticket-capture-test-'));
  const output = path.join(directory, 'nested', 'event.html');
  try {
    await writeCapture(output, '<html>synthetic offer</html>', record());
    assert.equal(await fs.readFile(output, 'utf8'), '<html>synthetic offer</html>');
    const metadata = JSON.parse(await fs.readFile(`${output}.json`, 'utf8'));
    assert.equal(metadata.profile_verified, false);
    assert.equal(metadata.original_url, url);
    assert.ok(!JSON.stringify(metadata).includes('do-not-copy'));
    assert.equal((await fs.stat(output)).mode & 0o777, 0o600);
    assert.equal((await fs.stat(`${output}.json`)).mode & 0o777, 0o600);
    await assert.rejects(writeCapture(output, 'replaced', record()), /output_exists/);
    assert.equal(await fs.readFile(output, 'utf8'), '<html>synthetic offer</html>');
    await writeCapture(output, '<html>updated synthetic offer</html>', record(), { overwrite: true });
    assert.equal(await fs.readFile(output, 'utf8'), '<html>updated synthetic offer</html>');
  } finally { await fs.rm(directory, { recursive: true, force: true }); }
});

test('capture rejects symlink destinations and oversized pages before publication', async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'ticket-capture-test-'));
  const output = path.join(directory, 'event.html');
  const real = path.join(directory, 'real.html');
  try {
    await fs.writeFile(real, 'original');
    await fs.symlink(real, output);
    await assert.rejects(writeCapture(output, 'changed', record(), { overwrite: true }), /unsafe_output/);
    assert.equal(await fs.readFile(real, 'utf8'), 'original');
    await fs.unlink(output);
    await assert.rejects(writeCapture(output, 'a'.repeat(MAX_BYTES + 1), record()), /body_too_large/);
    await assert.rejects(fs.stat(output), { code: 'ENOENT' });
  } finally { await fs.rm(directory, { recursive: true, force: true }); }
});

test('noninteractive capture never launches a browser', async () => {
  let launched = false;
  await assert.rejects(captureSession({ url, output: 'unused.html', timeoutSecs: 30 }, {
    stdin: { isTTY: false }, chromium: { launch: () => { launched = true; } },
  }), /interactive_terminal_required/);
  assert.equal(launched, false);
});

function fakeSession(status = 200) {
  const input = new PassThrough();
  input.isTTY = true;
  const output = new PassThrough();
  const events = {};
  const frame = {};
  const state = { contextClosed: false, browserClosed: false, launchOptions: null, rendered: false };
  const response = { status: () => status, url: () => url, request: () => ({ isNavigationRequest: () => true }), frame: () => frame };
  const page = {
    on: (name, callback) => { events[name] = callback; },
    mainFrame: () => frame,
    goto: async () => { events.response(response); return response; },
    url: () => url,
    evaluate: async () => 'Public event with synthetic visible offers',
  };
  const context = { newPage: async () => page, route: async () => {}, close: async () => { state.contextClosed = true; } };
  const browser = { newContext: async () => context, close: async () => { state.browserClosed = true; } };
  const chromium = { launch: async options => { state.launchOptions = options; return browser; } };
  output.on('data', chunk => { if (String(chunk).includes('press Enter')) setImmediate(() => input.write('\n')); });
  return { state, dependencies: { stdin: input, stdout: output, chromium,
    captureRendered: async () => { state.rendered = true; return '<html>synthetic rendered offer</html>'; } },
  close: () => { input.destroy(); output.destroy(); } };
}

test('headed session captures only after operator input and closes isolated browser', async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'ticket-capture-test-'));
  const output = path.join(directory, 'event.html');
  const fake = fakeSession();
  try {
    await captureSession({ url, output, timeoutSecs: 30, overwrite: false }, fake.dependencies);
    assert.equal(fake.state.launchOptions.headless, false);
    assert.equal(fake.state.launchOptions.env.DISCORD_BOT_TOKEN, undefined);
    assert.equal(fake.state.launchOptions.env.TICKETMASTER_API_KEY, undefined);
    assert.equal(fake.state.rendered, true);
    assert.equal(fake.state.contextClosed, true);
    assert.equal(fake.state.browserClosed, true);
    const saved = JSON.parse(await fs.readFile(`${output}.json`, 'utf8'));
    assert.equal(saved.schema_version, 1);
    assert.equal(saved.original_url, url);
    assert.equal(saved.profile_verified, false);
  } finally { fake.close(); await fs.rm(directory, { recursive: true, force: true }); }
});

test('initial blocked page can be inspected manually but final blocked response cannot be captured', async () => {
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'ticket-capture-test-'));
  const output = path.join(directory, 'event.html');
  const fake = fakeSession(403);
  try {
    await assert.rejects(captureSession({ url, output, timeoutSecs: 30, overwrite: false }, fake.dependencies), /page_access_refused/);
    assert.equal(fake.state.rendered, false);
    assert.equal(fake.state.contextClosed, true);
    assert.equal(fake.state.browserClosed, true);
    await assert.rejects(fs.stat(output), { code: 'ENOENT' });
  } finally { fake.close(); await fs.rm(directory, { recursive: true, force: true }); }
});
