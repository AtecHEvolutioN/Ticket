'use strict';

// Operator-assisted capture of a public event page. It does not automate ticket
// selection, verification, login, cart creation, or checkout.
const fs = require('node:fs/promises');
const path = require('node:path');
const readline = require('node:readline/promises');
const { validateUrl, proxyOptions } = require('./worker.cjs');
const { sameEvent, captureRendered } = require('./rendered.cjs');
const { prepareTicketmaster } = require('./ticketmaster.cjs');
const { inspectChallenge, waitForManualClearance } = require('./manual-intervention.cjs');

const MAX_BYTES = 8 * 1024 * 1024;
const HELP = `Capture a public Ticketmaster event for local scraper setup.

Usage:
  node browser/capture.cjs --url PUBLIC_EVENT --output captures/event.html
    [--executable-path PATH] [--timeout-secs 300] [--overwrite] [--adapter ticketmaster]

An ordinary browser opens in a fresh, isolated context. When the public ticket
offers are visible, return to this terminal and press Enter. The program does
not solve verification, sign in, select tickets, or create a cart.
The optional ticketmaster adapter opens recognized best-available display tabs.

Outputs: private HTML and <output>.json provenance, with profile_verified=false.
Captures can contain page/session data. Keep them private and review before sharing.
`;

function parseArgs(argv) {
  const result = { timeoutSecs: 300, overwrite: false, adapter: 'selectors' };
  const seen = new Set();
  for (let i = 0; i < argv.length; i++) {
    const flag = argv[i];
    if (flag === '--help' || flag === '-h') return { help: true };
    if (seen.has(flag)) throw new Error('invalid_arguments');
    seen.add(flag);
    if (flag === '--overwrite') { result.overwrite = true; continue; }
    const names = { '--url': 'url', '--output': 'output', '--executable-path': 'executablePath', '--timeout-secs': 'timeoutSecs', '--adapter': 'adapter' };
    if (!names[flag] || i + 1 >= argv.length || argv[i + 1].startsWith('--')) throw new Error('invalid_arguments');
    const value = argv[++i];
    if (!value || /[\x00-\x1f\x7f]/.test(value)) throw new Error('invalid_arguments');
    result[names[flag]] = value;
  }
  if (!result.url || !result.output) throw new Error('invalid_arguments');
  if (!['ticketmaster', 'selectors'].includes(result.adapter)) throw new Error('invalid_arguments');
  if (!/^\d+$/.test(String(result.timeoutSecs))) throw new Error('invalid_arguments');
  result.timeoutSecs = Number(result.timeoutSecs);
  if (!Number.isSafeInteger(result.timeoutSecs) || result.timeoutSecs < 30 || result.timeoutSecs > 1800) throw new Error('invalid_arguments');
  if (!sameEvent(result.url, result.url)) throw new Error('invalid_event_url');
  const target = validateUrl(result.url);
  result.url = target.href;
  result.output = path.resolve(result.output);
  if (result.executablePath) result.executablePath = path.resolve(result.executablePath);
  return result;
}

function publicUrl(input) {
  const url = validateUrl(input);
  url.search = '';
  url.hash = '';
  return url.href;
}

function provenance(input) {
  const original = publicUrl(input.original_url);
  const final = publicUrl(input.final_url);
  if (!sameEvent(original, final) || !Number.isInteger(input.status) || input.status < 200 || input.status >= 300
      || typeof input.captured_at !== 'string' || !/^\d{4}-\d\d-\d\dT\d\d:\d\d:\d\d\.\d{3}Z$/.test(input.captured_at)
      || !Number.isFinite(Date.parse(input.captured_at))) throw new Error('invalid_capture_metadata');
  // Never serialize arbitrary caller fields, headers, cookies, or account data.
  return { schema_version: 1, original_url: original, final_url: final, captured_at: input.captured_at,
    status: input.status, rendered: true, profile_verified: false };
}

async function checkDestination(file, overwrite) {
  try {
    const info = await fs.lstat(file);
    if (!info.isFile() || info.isSymbolicLink()) throw new Error('unsafe_output');
    if (!overwrite) throw new Error('output_exists');
  } catch (error) {
    if (error.code !== 'ENOENT') throw error;
  }
}

/** Publish private staged files. Without --overwrite, hard links refuse races. */
async function writeCapture(output, html, metadata, { overwrite = false } = {}) {
  if (typeof html !== 'string' || Buffer.byteLength(html, 'utf8') > MAX_BYTES) throw new Error('body_too_large');
  const record = provenance(metadata);
  const destination = path.resolve(output);
  const sidecar = `${destination}.json`;
  await checkDestination(destination, overwrite);
  await checkDestination(sidecar, overwrite);
  await fs.mkdir(path.dirname(destination), { recursive: true, mode: 0o700 });
  const stage = await fs.mkdtemp(path.join(path.dirname(destination), '.ticket-capture-'));
  const stagedHtml = path.join(stage, 'page.html');
  const stagedJson = path.join(stage, 'page.json');
  let publishedHtml = false;
  let stagedIdentity;
  try {
    await fs.writeFile(stagedHtml, html, { mode: 0o600, flag: 'wx' });
    await fs.writeFile(stagedJson, `${JSON.stringify(record, null, 2)}\n`, { mode: 0o600, flag: 'wx' });
    if (overwrite) {
      // Recheck immediately before publication; never follow an output symlink.
      await checkDestination(destination, true);
      await checkDestination(sidecar, true);
      await fs.rename(stagedHtml, destination);
      await fs.rename(stagedJson, sidecar);
    } else {
      stagedIdentity = await fs.stat(stagedHtml);
      await fs.link(stagedHtml, destination);
      publishedHtml = true;
      await fs.link(stagedJson, sidecar);
    }
    await fs.chmod(destination, 0o600);
    await fs.chmod(sidecar, 0o600);
    return { output: destination, sidecar };
  } catch (error) {
    if (publishedHtml) {
      const current = await fs.lstat(destination).catch(() => null);
      if (current && current.ino === stagedIdentity.ino && current.dev === stagedIdentity.dev) await fs.unlink(destination).catch(() => {});
    }
    if (error.code === 'EEXIST') throw new Error('output_exists');
    throw error;
  } finally { await fs.rm(stage, { recursive: true, force: true }); }
}

async function captureSession(options, dependencies = {}) {
  const input = dependencies.stdin || process.stdin;
  const output = dependencies.stdout || process.stdout;
  if (!input.isTTY) throw new Error('interactive_terminal_required');
  if (!sameEvent(options.url, options.url)) throw new Error('invalid_event_url');
  const target = validateUrl(options.url);
  await checkDestination(options.output, options.overwrite);
  await checkDestination(`${options.output}.json`, options.overwrite);
  const chromium = dependencies.chromium || require('playwright').chromium;
  const controller = new AbortController();
  let browser;
  let context;
  let terminal;
  let expired = false;
  let cancelled = false;
  const stop = () => { cancelled = true; controller.abort(); };
  const timeout = setTimeout(() => { expired = true; controller.abort(); }, options.timeoutSecs * 1000);
  const abort = new Promise((_, reject) => controller.signal.addEventListener('abort', () => {
    reject(new Error(expired ? 'capture_timeout' : 'capture_cancelled'));
  }, { once: true }));
  // Mark the rejection handled even if an abort occurs between awaited operations.
  abort.catch(() => {});
  const bounded = promise => Promise.race([promise, abort]);
  process.once('SIGINT', stop);
  process.once('SIGTERM', stop);
  try {
    const env = { ...process.env };
    delete env.DISCORD_BOT_TOKEN;
    delete env.TICKETMASTER_API_KEY;
    const executablePath = options.executablePath || process.env.CHROMIUM_PATH;
    const launching = chromium.launch({ headless: false, env,
      ...(executablePath ? { executablePath } : {}),
      ...(proxyOptions() ? { proxy: proxyOptions() } : {}) });
    launching.then(value => { if (controller.signal.aborted) void value.close().catch(() => {}); }, () => {});
    browser = await bounded(launching);
    context = await bounded(browser.newContext({ serviceWorkers: 'block' }));
    const page = await bounded(context.newPage());
    let blockedNavigation = false;
    let latestResponse = null;
    await bounded(context.route('**/*', async route => {
      const request = route.request();
      if (request.isNavigationRequest() && request.frame().parentFrame() === null) {
        try {
          const destination = validateUrl(request.url());
          if (!sameEvent(target.href, destination.href)) throw new Error('unsafe_redirect');
        } catch {
          blockedNavigation = true;
          return route.abort('blockedbyclient');
        }
      }
      return route.fallback();
    }));
    page.on('response', response => {
      if (response.request().isNavigationRequest() && response.frame() === page.mainFrame()) {
        latestResponse = { status: response.status(), url: response.url() };
      }
    });
    const initial = await bounded(page.goto(target.href, { waitUntil: 'domcontentloaded', timeout: 60000 }));
    if (!initial) throw new Error('page_access_refused');
    if (options.adapter === 'ticketmaster' && initial.status() >= 200 && initial.status() < 300) {
      await bounded(page.waitForTimeout(1500));
      const prepared = await bounded(prepareTicketmaster(page, target.href, 10000));
      output.write(`Ticket display adapter: ${prepared.state}.\n`);
    }
    output.write('Browser opened. Read the public offers, then return here and press Enter to capture. Ctrl+C cancels.\n');
    if (initial.status() < 200 || initial.status() >= 300) {
      output.write(`The initial page returned HTTP ${initial.status()}. A capture is accepted only after a successful response for this same event; no automatic retries are performed.\n`);
    }
    output.write('Do not enter payment details. Captures remain private and do not verify a scraper profile.\n');
    terminal = readline.createInterface({ input, output });
    await bounded(terminal.question('', { signal: controller.signal }));
    const validateCurrentPage = () => {
      if (blockedNavigation) throw new Error('unsafe_redirect');
      const final = validateUrl(page.url());
      if (!sameEvent(target.href, final.href) || !latestResponse || !sameEvent(final.href, latestResponse.url)) throw new Error('unsafe_redirect');
      if (latestResponse.status < 200 || latestResponse.status >= 300) throw new Error('page_access_refused');
      return final;
    };
    const inspect = async () => {
      validateCurrentPage();
      const kind = await inspectChallenge(page);
      validateCurrentPage();
      return kind;
    };
    const requestHuman = dependencies.requestHuman || (async ({ message, attempt, signal }) => {
      output.write(`Manual attention (${attempt}/3): ${message}\n`);
      await terminal.question('When the public offers are visible, press Enter to recheck. Ctrl+C cancels.\n', { signal });
    });
    const clearance = await bounded(waitForManualClearance({ inspect, requestHuman, signal: controller.signal,
      timeoutMs: options.timeoutSecs * 1000 }));
    if (clearance.state !== 'clear') throw new Error('page_access_refused');
    const html = await bounded((dependencies.captureRendered || captureRendered)(page, MAX_BYTES));
    if (await bounded(inspect())) throw new Error('page_access_refused');
    const final = validateCurrentPage();
    return await bounded(writeCapture(options.output, html, {
      original_url: target.href, final_url: final.href, captured_at: new Date().toISOString(), status: latestResponse.status,
    }, { overwrite: options.overwrite }));
  } catch (error) {
    if (expired) throw new Error('capture_timeout');
    if (cancelled || error.name === 'AbortError') throw new Error('capture_cancelled');
    const safe = new Set(['invalid_event_url', 'interactive_terminal_required', 'output_exists', 'unsafe_output',
      'body_too_large', 'invalid_capture_metadata', 'unsafe_redirect', 'page_access_refused']);
    throw new Error(safe.has(error.message) ? error.message : 'capture_failed');
  } finally {
    clearTimeout(timeout);
    process.removeListener('SIGINT', stop);
    process.removeListener('SIGTERM', stop);
    terminal?.close();
    await context?.close().catch(() => {});
    await browser?.close().catch(() => {});
  }
}

async function main(argv = process.argv.slice(2)) {
  let options;
  try { options = parseArgs(argv); }
  catch { process.stderr.write('Invalid capture arguments. Run with --help for usage.\n'); process.exitCode = 2; return; }
  if (options.help) { process.stdout.write(HELP); return; }
  try {
    const result = await captureSession(options);
    process.stdout.write(`Saved private capture: ${result.output}\nSaved unverified provenance: ${result.sidecar}\n`);
  } catch (error) {
    const messages = {
      interactive_terminal_required: 'Run capture in an interactive terminal with a graphical desktop.',
      output_exists: 'Output already exists. Choose another filename or explicitly use --overwrite.',
      unsafe_output: 'Output must be an ordinary file path, not a symlink or directory.',
      unsafe_redirect: 'Navigation left the selected public event. No capture was accepted.',
      page_access_refused: 'The public event is blocked, unavailable, or showing verification. No capture was accepted.',
      capture_timeout: 'Capture timed out. The browser was closed.',
      capture_cancelled: 'Capture cancelled. The browser was closed.',
      body_too_large: 'Rendered page exceeds the 8 MiB capture limit.',
    };
    process.stderr.write(`${messages[error.message] || 'Capture failed. Check browser installation and access to the public event.'}\n`);
    process.exitCode = 1;
  }
}

module.exports = { parseArgs, writeCapture, captureSession, provenance, main, MAX_BYTES };
if (require.main === module) main().catch(() => { process.stderr.write('Capture failed.\n'); process.exitCode = 1; });
