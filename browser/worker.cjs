'use strict';

// A long-lived, ordinary Playwright browser. No stealth, CAPTCHA handling,
// checkout actions, login automation, or hidden-endpoint replay is performed.
const readline = require('node:readline');
const { captureRendered, sameEvent } = require('./rendered.cjs');
const { prepareTicketmaster } = require('./ticketmaster.cjs');
const ALLOWED_DOMAINS = new Set([
  'ticketmaster.pl', 'ticketmaster.cz', 'ticketmaster.com', 'ticketmaster.co.uk',
  'ticketmaster.de', 'ticketmaster.at', 'ticketmaster.ch', 'ticketmaster.be',
  'ticketmaster.nl', 'ticketmaster.ie', 'ticketmaster.es', 'ticketmaster.it',
  'ticketmaster.fr', 'ticketmaster.dk', 'ticketmaster.no', 'ticketmaster.se',
  'ticketmaster.fi', 'ticketmaster.ca', 'ticketmaster.com.au', 'ticketmaster.co.nz',
  'ticketmaster.com.mx', 'ticketmaster.co.za',
]);

function validateUrl(input) {
  const url = new URL(input);
  if (url.protocol !== 'https:' || !ALLOWED_DOMAINS.has(url.hostname.replace(/^www\./, '')) ||
      (url.port && url.port !== '443') || url.username || url.password ||
      !(/\/event\//.test(url.pathname) || /\/artist\//.test(url.pathname)) ||
      [...url.searchParams.keys()].some(key => !['language', 'locale', 'page'].includes(key))) {
    throw new Error('invalid_request');
  }
  url.hash = '';
  return url;
}

function retryAfter(value, now = Date.now()) {
  if (!value) return null;
  if (/^\d+$/.test(value.trim())) return Number(value.trim());
  const date = Date.parse(value);
  return Number.isFinite(date) ? Math.max(0, Math.ceil((date - now) / 1000)) : null;
}

function proxyOptions(env = process.env) {
  const raw = env.HTTPS_PROXY || env.https_proxy || env.HTTP_PROXY || env.http_proxy;
  if (!raw) return undefined;
  const url = new URL(raw);
  return { server: url.origin,
    ...(url.username ? { username: decodeURIComponent(url.username) } : {}),
    ...(url.password ? { password: decodeURIComponent(url.password) } : {}),
    ...((env.NO_PROXY || env.no_proxy) ? { bypass: env.NO_PROXY || env.no_proxy } : {}) };
}

function validateRequest(request) {
  if (!request || !Number.isSafeInteger(request.id) || request.id < 1 ||
      !Number.isSafeInteger(request.timeout_ms) || request.timeout_ms < 1 || request.timeout_ms > 120000 ||
      !Number.isSafeInteger(request.max_body_bytes) || request.max_body_bytes < 1024 || request.max_body_bytes > 16 * 1024 * 1024 ||
      !Number.isSafeInteger(request.render_wait_ms) || request.render_wait_ms < 0 || request.render_wait_ms > 30000) {
    throw new Error('invalid_request');
  }
  if (request.adapter !== undefined && !['ticketmaster', 'selectors'].includes(request.adapter)) throw new Error('invalid_request');
  if (request.adapter === 'ticketmaster' && !sameEvent(request.url, request.url)) throw new Error('invalid_request');
  return validateUrl(request.url);
}

async function capture(context, request) {
  const target = validateRequest(request);
  const started = performance.now();
  const page = await context.newPage();
  page.setDefaultTimeout?.(request.timeout_ms);
  let unsafeRedirect = false;
  try {
    await page.route('**/*', async route => {
      const outgoing = route.request();
      if (outgoing.isNavigationRequest() && outgoing.frame() === page.mainFrame()) {
        try {
          const redirect = validateUrl(outgoing.url());
          if (redirect.hostname !== target.hostname) throw new Error('unsafe_redirect');
        } catch {
          unsafeRedirect = true;
          return route.abort('blockedbyclient');
        }
      }
      return route.fallback();
    });
    const response = await page.goto(target.href, { waitUntil: 'domcontentloaded', timeout: request.timeout_ms });
    if (!response) throw new Error('navigation_failed');
    const status = response.status();
    const headers = response.headers();
    let html = '';
    let adapterState;
    let adapterReason;
    if (status >= 200 && status < 300) {
      const remaining = Math.max(0, request.timeout_ms - (performance.now() - started));
      const renderingWait = Math.min(request.render_wait_ms, remaining);
      if (renderingWait) await page.waitForTimeout(renderingWait);
      if (request.adapter === 'ticketmaster') {
        const prepared = await prepareTicketmaster(page, target.href, request.timeout_ms - (performance.now() - started));
        adapterReason = prepared.state;
        adapterState = ['regional', 'listings'].includes(prepared.state) ? 'ready'
          : prepared.state === 'blocked' ? 'blocked'
            : prepared.state === 'no_visible_offers' ? 'no_offers' : 'unsupported';
      }
      html = await captureRendered(page, request.max_body_bytes);
    }
    const finalUrl = validateUrl(page.url());
    if (unsafeRedirect || finalUrl.hostname !== target.hostname) throw new Error('unsafe_redirect');
    return { id: request.id, status, html, final_url: finalUrl.href,
      ...(adapterState ? { adapter_state: adapterState, adapter_reason: adapterReason } : {}),
      retry_after_secs: retryAfter(headers['retry-after']), elapsed_ms: Math.round(performance.now() - started),
      server: headers.server?.slice(0, 120), content_type: headers['content-type']?.slice(0, 120) };
  } catch (error) {
    let code = 'navigation_failed';
    if (unsafeRedirect || error.message === 'unsafe_redirect') code = 'unsafe_redirect';
    else if (error.name === 'TimeoutError') code = 'navigation_timeout';
    else if (error.message === 'body_too_large') code = 'body_too_large';
    throw new Error(code);
  } finally { await page.close().catch(() => {}); }
}

async function main() {
  let browserServer;
  let browser;
  let context;
  let stopping = false;
  const parentPid = process.ppid;
  const send = value => process.stdout.write(JSON.stringify(value) + '\n');
  const close = async () => {
    if (stopping) return;
    stopping = true;
    clearInterval(parentWatch);
    await context?.close().catch(() => {});
    await browser?.close().catch(() => {});
    await browserServer?.close().catch(() => {});
    process.exit(0);
  };
  const parentWatch = setInterval(() => { if (process.ppid !== parentPid) void close(); }, 1000);
  parentWatch.unref();
  process.once('SIGTERM', close);
  process.once('SIGINT', close);
  process.stdout.once('error', close);
  const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  let requests = Promise.resolve();
  lines.on('line', line => {
    requests = requests.then(async () => {
      if (stopping) return;
      let request;
      try {
        if (line.length > 8192) throw new Error('invalid_request');
        request = JSON.parse(line);
        validateRequest(request);
      } catch {
        send({ id: request?.id || null, error: 'invalid_request' });
        return;
      }
      if (!context) {
        try {
          const { chromium } = require('playwright');
          const browserEnv = { ...process.env };
          delete browserEnv.DISCORD_BOT_TOKEN;
          delete browserEnv.TICKETMASTER_API_KEY;
          browserServer = await chromium.launchServer({ headless: process.env.TICKET_BROWSER_HEADED !== '1', host: '127.0.0.1',
            env: browserEnv,
            ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}),
            ...(proxyOptions() ? { proxy: proxyOptions() } : {}) });
          // Expose only the process ID, never the private browser websocket URL.
          send({ browser_pid: browserServer.process().pid });
          browser = await chromium.connect(browserServer.wsEndpoint());
          context = await browser.newContext({ serviceWorkers: 'block' });
        } catch {
          send({ id: request.id, error: 'browser_launch_failed' });
          return;
        }
      }
      try { send(await capture(context, request)); }
      catch (error) { send({ id: request.id, error: error.message }); }
    }).catch(() => { send({ id: null, error: 'capture_failed' }); });
  });
  lines.once('close', () => { void requests.finally(close); });
}

module.exports = { validateUrl, validateRequest, retryAfter, proxyOptions, capture };
if (require.main === module) main().catch(() => { process.exitCode = 1; });
