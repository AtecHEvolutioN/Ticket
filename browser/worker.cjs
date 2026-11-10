'use strict';

// A long-lived, ordinary Playwright browser. No stealth, CAPTCHA solving,
// checkout actions, login automation, or hidden-endpoint replay is performed.
const readline = require('node:readline');
const { captureRendered, sameEvent } = require('./rendered.cjs');
const { prepareTicketmaster } = require('./ticketmaster.cjs');
const { prepareTicketmasterMap } = require('./ticketmaster-map.cjs');
const { inspectTicketmasterSection, assertSectionInspectionCurrent,
  scanTicketmasterSection, assertSectionScanCurrent } = require('./ticketmaster-section.cjs');
const { inspectTicketmasterCartControls, assertCartControlsCurrent,
  releaseCartControlsInspection } = require('./ticketmaster-cart-controls.cjs');
const { inspectChallenge, responseIntervention } = require('./manual-intervention.cjs');
const { observeDocuments } = require('./document-lifecycle.cjs');
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

function identificationRetryAllowed(headers, now = Date.now()) {
  if (!Object.hasOwn(headers, 'retry-after')) return true;
  if (typeof headers['retry-after'] !== 'string') return false;
  const value = headers['retry-after'].trim();
  if (/^\d+$/.test(value)) return Number(value) === 0;
  // Date.parse accepts unrelated strings such as "0, 1" and "-1". Only a
  // canonical HTTP-date may authorize a passive wait after a dated cooldown.
  if (!/^(Mon|Tue|Wed|Thu|Fri|Sat|Sun), \d{2} (Jan|Feb|Mar|Apr|May|Jun|Jul|Aug|Sep|Oct|Nov|Dec) \d{4} \d{2}:\d{2}:\d{2} GMT$/i.test(value)) return false;
  const date = Date.parse(value);
  return Number.isFinite(date) && new Date(date).toUTCString().toLowerCase() === value.toLowerCase() && date <= now;
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
  if (request.display !== undefined && (!['map', 'cart_controls'].includes(request.display)
      || request.adapter !== 'ticketmaster')) throw new Error('invalid_request');
  if (request.adapter === 'ticketmaster' && !sameEvent(request.url, request.url)) throw new Error('invalid_request');
  const url = validateUrl(request.url);
  const identifier = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,128}$/.test(value);
  if (request.map_section_id !== undefined && (request.display !== 'map'
      || url.hostname.replace(/^www\./, '') !== 'ticketmaster.cz' || !identifier(request.map_section_id))) throw new Error('invalid_request');
  if (request.map_seat_id !== undefined && (request.map_section_id === undefined || !identifier(request.map_seat_id))) throw new Error('invalid_request');
  if (request.map_scan_limit !== undefined && (request.map_section_id === undefined
      || request.map_seat_id !== undefined || !Number.isInteger(request.map_scan_limit)
      || request.map_scan_limit < 1 || request.map_scan_limit > 50)) throw new Error('invalid_request');
  if (request.display === 'cart_controls') {
    if (url.hostname.replace(/^www\./, '') !== 'ticketmaster.cz'
        || !identifier(request.cart_category_id) || !Number.isInteger(request.cart_demo_quantity)
        || request.cart_demo_quantity < 1 || request.cart_demo_quantity > 20
        || request.map_section_id !== undefined || request.map_seat_id !== undefined
        || request.map_scan_limit !== undefined) throw new Error('invalid_request');
  } else if (request.cart_category_id !== undefined || request.cart_demo_quantity !== undefined) {
    throw new Error('invalid_request');
  }
  return url;
}

// Quantity/filter demonstrations never share cookies, pages or in-flight traffic
// with the ordinary monitor. WebSocket server connections are blocked before any
// document exists; HTTP routes and offline emulation are fenced before mutation.
async function captureCartControls(browser, request) {
  validateRequest(request);
  if (request.display !== 'cart_controls') throw new Error('invalid_request');
  const context = await browser.newContext({ serviceWorkers: 'block' });
  let primaryPage;
  let unsafePage = false;
  let fenced = false;
  let drained = false;
  let closed = false;
  const active = new Set();
  const began = outgoing => { if (!fenced) active.add(outgoing); };
  const ended = outgoing => active.delete(outgoing);
  const onPage = page => {
    if (!primaryPage) primaryPage = page;
    else { unsafePage = true; void page.close().catch(() => {}); }
  };
  try {
    if (typeof context.routeWebSocket !== 'function') throw new Error('navigation_failed');
    await context.routeWebSocket('**/*', socket => socket.close());
    context.on('page', onPage);
    context.on('request', began);
    context.on('requestfinished', ended);
    context.on('requestfailed', ended);
    await context.route('**/*', route => {
      const outgoing = route.request();
      if (outgoing.isNavigationRequest()) {
        let owner;
        try { owner = outgoing.frame().page(); } catch { /* Unbound navigation is unsafe. */ }
        if (owner !== primaryPage) {
          unsafePage = true;
          return route.abort('blockedbyclient');
        }
      }
      return route.fallback();
    });
    const isolation = {
      assertCurrent() { if (unsafePage || closed) throw new Error('navigation_failed'); },
      assertNetworkDisabled: async () => fenced && drained && !unsafePage && !closed,
      async fence(page, budgetMs) {
        this.assertCurrent();
        if (!(budgetMs > 0) || page !== primaryPage) throw new Error('navigation_failed');
        await context.route('**/*', route => route.abort('blockedbyclient'));
        await context.setOffline(true);
        fenced = true;
        // Terminate remaining initial-load traffic. New attempted requests are
        // already rejected and must not be confused with pre-fence connections.
        await page.evaluate(() => window.stop());
        const until = performance.now() + Math.min(2000, budgetMs);
        while (active.size && performance.now() < until) {
          await new Promise(resolve => setTimeout(resolve, 10));
          this.assertCurrent();
        }
        if (active.size) throw new Error('navigation_failed');
        drained = true;
        this.assertCurrent();
      },
    };
    return await capture(context, request, isolation);
  } finally {
    closed = true;
    context.off('page', onPage);
    context.off('request', began);
    context.off('requestfinished', ended);
    context.off('requestfailed', ended);
    await context.close().catch(() => {});
  }
}

async function capture(context, request, cartIsolation) {
  const target = validateRequest(request);
  if (request.display === 'cart_controls' && !cartIsolation) throw new Error('invalid_request');
  const started = performance.now();
  const page = await context.newPage();
  page.setDefaultTimeout?.(request.timeout_ms);
  let unsafeRedirect = false;
  let documents;
  let deadlineTimer;
  let stopped = false;
  let initialIdentification;
  let identificationPending = false;
  const deadline = started + request.timeout_ms;
  const remaining = () => Math.max(0, deadline - performance.now());
  const eventTarget = sameEvent(target.href, target.href);
  const validateDocument = document => {
    let url;
    try { url = validateUrl(document.url); } catch { throw new Error('unsafe_redirect'); }
    if (unsafeRedirect || url.hostname !== target.hostname || (eventTarget && !sameEvent(target.href, url.href))) {
      throw new Error('unsafe_redirect');
    }
  };
  const identification = document => document.status === 401 && document.headers['tm-bl']?.trim() === '1'
    && /^text\/html(?:\s*;|\s*$)/i.test(document.headers['content-type'] || '')
    && identificationRetryAllowed(document.headers) && eventTarget;
  const errorReply = (document, reason) => {
    validateDocument(document);
    const responseUrl = document.response_url || document.url;
    validateDocument({ url: responseUrl });
    return { id: request.id, status: document.status, html: '', final_url: validateUrl(responseUrl).href,
      adapter_state: 'blocked', adapter_reason: reason || 'blocked',
      ...(reason ? { manual_intervention: reason } : {}),
      retry_after_secs: retryAfter(document.headers['retry-after']), elapsed_ms: Math.round(performance.now() - started),
      server: document.headers.server?.slice(0, 120), content_type: document.headers['content-type']?.slice(0, 120) };
  };
  const incompleteIdentification = () => {
    if (unsafeRedirect) throw new Error('unsafe_redirect');
    const blocked = documents?.lastCommittedResponse();
    if (blocked && [403, 429].includes(blocked.status)) {
      return errorReply(blocked, responseIntervention(blocked.status, blocked.headers));
    }
    // Preserve the last confirmed error response and its host cooldown. This URL
    // identifies the reported error, not an unconfirmed replacement document.
    return errorReply(initialIdentification, 'identification_incomplete');
  };
  const work = async () => {
    documents = await observeDocuments(context, page, document => {
      try { validateDocument(document); }
      catch (error) { unsafeRedirect = true; throw error; }
    });
    if (stopped) { await documents.close(); throw new Error('navigation_timeout'); }
    await page.route('**/*', async route => {
      const outgoing = route.request();
      if (outgoing.isNavigationRequest() && outgoing.frame() === page.mainFrame()) {
        try { validateDocument({ url: outgoing.url() }); }
        catch {
          unsafeRedirect = true;
          return route.abort('blockedbyclient');
        }
      }
      return route.fallback();
    });
    if (!remaining()) throw new Error('navigation_timeout');
    const response = await page.goto(target.href, { waitUntil: 'domcontentloaded', timeout: remaining() });
    if (!response) throw new Error('navigation_failed');
    let document = { status: response.status(), headers: response.headers(), url: response.url() };
    let snapshot;
    if (identification(document)) {
      validateDocument(document);
      initialIdentification = document;
      identificationPending = true;
      await documents.synchronize();
      // Section scans already allow a longer initial page load. A successful
      // normal identification navigation may take more than five seconds; this
      // passive grace remains inside the original request deadline and cooldown.
      const identificationGrace = request.map_scan_limit !== undefined || request.display === 'cart_controls' ? 15000 : 5000;
      snapshot = await documents.waitForIdentification(Math.min(identificationGrace, Math.max(0, remaining() - 5)), identification, validateDocument);
      identificationPending = false;
      if (identification(snapshot)) return incompleteIdentification();
      document = snapshot;
    } else if (document.status >= 200 && document.status < 300) {
      await documents.synchronize();
      snapshot = documents.current();
      if (!snapshot) throw new Error('navigation_failed');
      document = snapshot;
    }
    const { status, headers } = document;
    validateDocument(document);
    let html = '';
    let adapterState;
    let adapterReason;
    let mapInspection;
    let mapScan;
    let cartControls;
    let manualIntervention = responseIntervention(status, headers);
    const guard = async () => {
      cartIsolation?.assertCurrent();
      await documents.synchronize();
      documents.assertCurrent(snapshot);
      validateDocument({ url: page.url() });
    };
    if (status >= 200 && status < 300) {
      const renderingWait = Math.min(request.render_wait_ms, remaining());
      if (renderingWait) await page.waitForTimeout(renderingWait);
      await guard();
      manualIntervention = await inspectChallenge(page);
      await guard();
      if (!manualIntervention && request.adapter === 'ticketmaster') {
        const mapGuard = async ({ afterMapDisplayAction }) => {
          if (!afterMapDisplayAction) return guard();
          await documents.synchronize();
          const current = documents.current();
          if (!current || current.loader !== snapshot.loader || current.status !== snapshot.status) {
            throw new Error('navigation_failed');
          }
          // current() already rejects query/path/history changes and unfinished
          // navigation. Only a known map click may rebind its fragment generation.
          validateDocument(current);
          validateDocument({ url: page.url() });
          snapshot = current;
        };
        const cartGuard = async ({ afterCartControlsAction = false, afterDisplayAction = false } = {}) => {
          if (!afterCartControlsAction && !afterDisplayAction) return guard();
          cartIsolation.assertCurrent();
          await documents.synchronize();
          const current = documents.current();
          if (!current || current.loader !== snapshot.loader || current.status !== snapshot.status) {
            throw new Error('navigation_failed');
          }
          // Rebind only around an inspected native display/quantity/panel action.
          // The lifecycle observer forbids path/query/history and loader changes.
          validateDocument(current);
          validateDocument({ url: page.url() });
          snapshot = current;
        };
        let prepared;
        if (request.display === 'cart_controls') {
          prepared = await prepareTicketmaster(page, target.href, remaining(), cartGuard);
          await guard();
          if (prepared.state === 'regional') {
            await cartIsolation.fence(page, remaining());
            await guard();
            prepared = await inspectTicketmasterCartControls(page, target.href, {
              categoryId: request.cart_category_id, quantity: request.cart_demo_quantity,
              networkDisabled: true, assertNetworkDisabled: cartIsolation.assertNetworkDisabled,
            }, remaining(), cartGuard);
          }
          // A recognizable listing without the requested controls is unsupported.
          if (!prepared.state.startsWith('cart_controls_') && !['blocked', 'consent_blocked'].includes(prepared.state)) {
            prepared = { ...prepared, state: 'cart_controls_unsupported' };
          }
        } else prepared = request.map_scan_limit !== undefined
          ? await scanTicketmasterSection(page, target.href,
            { sectionId: request.map_section_id, limit: request.map_scan_limit }, remaining(), mapGuard)
          : request.map_section_id !== undefined
            ? await inspectTicketmasterSection(page, target.href,
              { sectionId: request.map_section_id, seatId: request.map_seat_id }, remaining(), mapGuard)
            : request.display === 'map'
              ? await prepareTicketmasterMap(page, target.href, remaining(), mapGuard)
              : await prepareTicketmaster(page, target.href, remaining());
        await guard();
        if (prepared.state === 'section_ready') mapInspection = prepared.inspection;
        if (prepared.state === 'scan_ready') mapScan = prepared.scan;
        if (prepared.state === 'cart_controls_ready') cartControls = prepared.inspection;
        adapterReason = prepared.state;
        adapterState = ['regional', 'listings', 'map_svg', 'map_static', 'map_canvas', 'map_iframe', 'section_ready', 'scan_ready', 'cart_controls_ready'].includes(prepared.state) ? 'ready'
          : ['blocked', 'map_modal_blocked', 'cart_controls_modal_blocked'].includes(prepared.state) ? 'blocked'
            : prepared.state === 'no_visible_offers' ? 'no_offers' : 'unsupported';
      }
      if (cartControls) {
        await assertCartControlsCurrent(page, target.href, cartControls);
        await guard();
      }
      html = await captureRendered(page, request.max_body_bytes);
      await guard();
      if (mapInspection) {
        await assertSectionInspectionCurrent(page, target.href, mapInspection);
        await guard();
      }
      if (mapScan) {
        await assertSectionScanCurrent(page, target.href, mapScan);
        await guard();
      }
      if (cartControls) {
        await assertCartControlsCurrent(page, target.href, cartControls);
        await guard();
      }
      manualIntervention ||= await inspectChallenge(page);
      await guard();
    }
    if (manualIntervention) {
      adapterState = 'blocked';
      adapterReason = 'manual_intervention';
      mapInspection = undefined;
      mapScan = undefined;
      cartControls = undefined;
    }
    const finalUrl = validateUrl(page.url());
    validateDocument({ url: finalUrl.href });
    return { id: request.id, status, html, final_url: finalUrl.href,
      ...(adapterState ? { adapter_state: adapterState, adapter_reason: adapterReason } : {}),
      ...(mapInspection ? { map_inspection: mapInspection } : {}),
      ...(mapScan ? { map_scan: mapScan } : {}),
      ...(cartControls ? { cart_controls: cartControls } : {}),
      ...(manualIntervention ? { manual_intervention: manualIntervention } : {}),
      retry_after_secs: retryAfter(headers['retry-after']), elapsed_ms: Math.round(performance.now() - started),
      server: headers.server?.slice(0, 120), content_type: headers['content-type']?.slice(0, 120) };
  };
  try {
    return await Promise.race([work(), new Promise((_, reject) => {
      deadlineTimer = setTimeout(() => reject(new Error('navigation_timeout')), remaining());
    })]);
  } catch (error) {
    let code = 'navigation_failed';
    const blocked = documents?.lastCommittedResponse();
    if (unsafeRedirect || error.message === 'unsafe_redirect') code = 'unsafe_redirect';
    else if (blocked && [403, 429].includes(blocked.status)) {
      return errorReply(blocked, responseIntervention(blocked.status, blocked.headers));
    }
    else if (identificationPending) return incompleteIdentification();
    else if (error.name === 'TimeoutError' || error.message === 'navigation_timeout') code = 'navigation_timeout';
    else if (error.message === 'body_too_large') code = 'body_too_large';
    throw new Error(code);
  } finally {
    stopped = true;
    clearTimeout(deadlineTimer);
    if (request.display === 'cart_controls') await releaseCartControlsInspection(page).catch(() => {});
    await documents?.close().catch(() => {});
    await page.close().catch(() => {});
  }
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
      try { send(await (request.display === 'cart_controls'
        ? captureCartControls(browser, request) : capture(context, request))); }
      catch (error) { send({ id: request.id, error: error.message }); }
    }).catch(() => { send({ id: null, error: 'capture_failed' }); });
  });
  lines.once('close', () => { void requests.finally(close); });
}

module.exports = { validateUrl, validateRequest, retryAfter, identificationRetryAllowed, proxyOptions, capture, captureCartControls };
if (require.main === module) main().catch(() => { process.exitCode = 1; });
