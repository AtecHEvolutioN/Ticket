'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const http = require('node:http');

test('market viewer renders SVG charts and safely resets imported history on desktop and mobile',
  { skip: process.env.TICKET_BROWSER_TESTS !== '1', timeout: 45000 }, async () => {
    const { chromium } = require('playwright');
    const publicDirectory = path.resolve(__dirname, '../apps/discord/public');
    const [html, script, style, deployment] = await Promise.all([
      fs.readFile(path.join(publicDirectory, 'market.html')),
      fs.readFile(path.join(publicDirectory, 'market.js')),
      fs.readFile(path.join(publicDirectory, 'market.css')),
      fs.readFile(path.join(publicDirectory, '../vercel.json'), 'utf8'),
    ]);
    const csp = JSON.parse(deployment).headers.flatMap(rule => rule.headers)
      .find(header => header.key.toLowerCase() === 'content-security-policy').value;
    const assets = new Map([
      ['/market', [html, 'text/html; charset=utf-8']],
      ['/market.js', [script, 'text/javascript; charset=utf-8']],
      ['/market.css', [style, 'text/css; charset=utf-8']],
    ]);
    const server = http.createServer((request, response) => {
      const pathname = new URL(request.url, 'http://127.0.0.1').pathname;
      response.setHeader('Content-Security-Policy', csp);
      response.setHeader('X-Content-Type-Options', 'nosniff');
      if (pathname === '/favicon.ico') { response.writeHead(204); response.end(); return; }
      const asset = request.method === 'GET' && assets.get(pathname);
      if (!asset) { response.writeHead(404); response.end(); return; }
      response.writeHead(200, { 'Content-Type': asset[1] });
      response.end(asset[0]);
    });
    await new Promise((resolve, reject) => {
      server.once('error', reject);
      server.listen(0, '127.0.0.1', resolve);
    });
    const origin = `http://127.0.0.1:${server.address().port}`;
    let browser;
    try {
      browser = await chromium.launch({ headless: true,
        ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
      const context = await browser.newContext({ serviceWorkers: 'block',
        viewport: { width: 1280, height: 900 }, locale: 'en-US', timezoneId: 'UTC' });
      const externalRequests = [];
      const failedRequests = [];
      await context.route('**/*', route => {
        if (new URL(route.request().url()).origin !== origin) {
          externalRequests.push(route.request().url());
          return route.abort();
        }
        return route.continue();
      });
      context.on('requestfailed', request => failedRequests.push(request.url()));
      await context.addInitScript(() => {
        globalThis.__marketTestExecuted = false;
        globalThis.__marketPolicyViolations = [];
        document.addEventListener('securitypolicyviolation', event => {
          globalThis.__marketPolicyViolations.push(event.violatedDirective);
        });
      });
      const page = await context.newPage();
      page.setDefaultTimeout(5000);
      const pageErrors = [];
      page.on('pageerror', error => pageErrors.push(error.message));
      const response = await page.goto(`${origin}/market`, { waitUntil: 'load' });
      assert.equal(response.status(), 200);
      assert.equal(response.headers()['content-security-policy'], csp);

      const chart = page.locator('#price-chart');
      const assertVisibleChart = async () => {
        await chart.waitFor({ state: 'visible' });
        const box = await chart.evaluate(element => ({ hidden: element.hasAttribute('hidden'),
          display: getComputedStyle(element).display,
          width: element.getBoundingClientRect().width, height: element.getBoundingClientRect().height }));
        // Setting SVGElement.hidden alone leaves the actual [hidden] attribute in place.
        assert.equal(box.hidden, false);
        assert.notEqual(box.display, 'none');
        assert.ok(box.width > 100 && box.height > 100, JSON.stringify(box));
      };
      const importHistory = async (name, data) => {
        await page.locator('#history-file').setInputFiles({ name, mimeType: 'application/json',
          buffer: Buffer.from(JSON.stringify(data)) });
        await page.waitForFunction(file => document.getElementById('load-status').textContent.includes(file), name);
      };

      assert.equal(await chart.isVisible(), false);
      assert.equal(await chart.getAttribute('hidden'), '');
      assert.equal(await page.locator('#empty-state').isVisible(), true);
      assert.equal(await page.locator('#price-value').textContent(), '—');
      for (const id of ['event-select', 'source-select', 'series-select']) {
        assert.equal(await page.locator(`#${id}`).isDisabled(), true);
      }
      await page.locator('#demo-button').click();
      await assertVisibleChart();
      assert.match(await page.locator('#load-status').textContent(), /SYNTHETIC DEMO/);
      assert.ok(await chart.locator('.chart-line').count() > 0);
      assert.ok(await chart.locator('.chart-bar').count() > 0);
      assert.notEqual(await page.locator('#price-value').textContent(), '—');
      await page.locator('#chart-mode').selectOption('candles');
      assert.ok(await chart.locator('rect.chart-candle-up, rect.chart-candle-down').count() > 0);
      assert.equal(await chart.locator('.chart-line').count(), 0);

      await page.setViewportSize({ width: 390, height: 844 });
      await assertVisibleChart();
      assert.equal(await page.evaluate(() => Math.max(document.documentElement.scrollWidth,
        document.body.scrollWidth) <= window.innerWidth + 1), true, '390px layout must not overflow horizontally');

      await importHistory('empty-history.json', { schema_version: 1, generated_at: 1700000000, events: [] });
      assert.equal(await chart.isVisible(), false);
      assert.equal(await chart.getAttribute('hidden'), '');
      assert.equal(await chart.locator('*').count(), 0);
      assert.equal(await page.locator('#empty-state').isVisible(), true);
      for (const id of ['price-value', 'count-value', 'time-value', 'change-value']) {
        assert.equal(await page.locator(`#${id}`).textContent(), '—', `${id} must not retain demo data`);
      }
      for (const id of ['event-select', 'source-select', 'series-select']) {
        assert.equal(await page.locator(`#${id}`).isDisabled(), true);
        assert.equal(await page.locator(`#${id} option`).count(), 0);
      }
      assert.equal(await page.locator('#series-tags > *').count(), 0);
      assert.equal(await page.locator('#inspection').isVisible(), false);
      assert.equal(await page.locator('#event-link').isVisible(), false);
      assert.equal(await page.locator('#chart-mode').isDisabled(), true);
      assert.equal(await page.locator('#count-mode').isDisabled(), true);
      assert.equal(await page.locator('#bucket-caption').textContent(), 'No observations');

      const imported = await page.evaluate(() => globalThis.TicketHistory.demoExport(1700000000));
      const hostileName = '<img id="history-injected" src="https://fixture.invalid/pixel" onerror="globalThis.__marketTestExecuted=true">';
      imported.events[0].name = hostileName;
      imported.events[0].artist = '<script>globalThis.__marketTestExecuted=true</script>';
      imported.events[0].event_url = 'https://fixture.invalid/event/synthetic';
      await importHistory('literal-metadata.json', imported);
      await assertVisibleChart();
      assert.equal(await page.locator('#event-heading').textContent(), hostileName);
      assert.equal(await page.locator('#event-heading > *').count(), 0);
      assert.equal(await page.locator('#history-injected').count(), 0);
      assert.match(await page.locator('#event-context').textContent(), /<script>/);
      assert.equal(await page.locator('#event-context script').count(), 0);
      assert.equal(await page.evaluate(() => globalThis.__marketTestExecuted), false);

      await page.locator('#history-file').setInputFiles({ name: 'invalid.json', mimeType: 'application/json',
        buffer: Buffer.from('{not valid json') });
      await page.locator('#error-banner').waitFor({ state: 'visible' });
      assert.match(await page.locator('#error-banner').textContent(), /not valid JSON/);
      assert.equal(await page.locator('#event-heading').textContent(), hostileName,
        'invalid imports must not replace the last valid data');
      assert.deepEqual(await page.evaluate(() => globalThis.__marketPolicyViolations), []);
      assert.deepEqual(externalRequests, []);
      assert.deepEqual(failedRequests, []);
      assert.deepEqual(pageErrors, []);
      await context.close();
    } finally {
      await browser?.close();
      server.closeAllConnections?.();
      await new Promise(resolve => server.close(resolve));
    }
  });

test('live chart uses the authenticated worker API, preserves selections and labels stale or disconnected data',
  { skip: process.env.TICKET_BROWSER_TESTS !== '1', timeout: 45000 }, async t => {
    const { chromium } = require('playwright');
    const { createHistoryServer, ApiError } = require('../server/history-api.cjs');
    await import('../apps/discord/public/market.js');
    const { demoExport } = globalThis.TicketHistory;
    const directory = path.resolve(__dirname, '../apps/discord/public');
    const [html, script, style, deployment] = await Promise.all([
      fs.readFile(path.join(directory, 'market.html')), fs.readFile(path.join(directory, 'market.js')),
      fs.readFile(path.join(directory, 'market.css')), fs.readFile(path.join(directory, '../vercel.json'), 'utf8'),
    ]);
    const csp = JSON.parse(deployment).headers.flatMap(rule => rule.headers)
      .find(header => header.key.toLowerCase() === 'content-security-policy').value;
    const assets = new Map([['/market', [html, 'text/html']], ['/market.js', [script, 'text/javascript']], ['/market.css', [style, 'text/css']]]);
    const app = http.createServer((req, res) => {
      const asset = assets.get(new URL(req.url, 'http://127.0.0.1').pathname);
      res.setHeader('Content-Security-Policy', csp);
      if (!asset) { res.writeHead(204); res.end(); return; }
      res.writeHead(200, { 'Content-Type': asset[1] }); res.end(asset[0]);
    });
    t.after(async () => { app.closeAllConnections?.(); await new Promise(resolve => app.close(resolve)); });
    const listen = server => new Promise((resolve, reject) => { server.once('error', reject); server.listen(0, '127.0.0.1', resolve); });
    await listen(app);
    const origin = `http://127.0.0.1:${app.address().port}`;
    const token = 'synthetic-browser-read-token-123456789';
    let exports = 0; let fixture = demoExport(Math.floor(Date.now() / 1000)); let failure = false; let pending = null;
    fixture.events[0].name = 'Fixture concert';
    const second = structuredClone(fixture.events[0]); second.event_id = 'second-event'; second.name = 'Second fixture concert';
    second.sources.push({ ...structuredClone(second.sources[0]), source_id: 'second-source', watch_id: 'second-watch' });
    fixture.events.push(second);
    const api = createHistoryServer({ token, host: '127.0.0.1', port: 0, binary: '/fixture/unused',
      databases: ['/fixture/unused.sqlite'], allowedOrigins: [origin], timeoutMs: 30000, minIntervalMs: 0 }, {
      exporter: async () => {
        exports += 1;
        if (failure) throw new ApiError(503, 'history_unavailable', 'Fixture unavailable');
        if (pending) await new Promise(resolve => { pending.resolve = resolve; });
        return Buffer.from(JSON.stringify(fixture));
      },
    });
    t.after(async () => { api.closeAllConnections?.(); await new Promise(resolve => api.close(resolve)); });
    const apiRequests = [];
    api.on('request', req => apiRequests.push({ method: req.method, url: req.url, authorization: req.headers.authorization }));
    await listen(api);
    const endpoint = `http://127.0.0.1:${api.address().port}`;
    let browser;
    try {
      browser = await chromium.launch({ headless: true, ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
      const context = await browser.newContext({ serviceWorkers: 'block', viewport: { width: 1280, height: 1000 } });
      const external = []; const errors = [];
      await context.route('**/*', route => {
        if (![origin, endpoint].includes(new URL(route.request().url()).origin)) { external.push(route.request().url()); return route.abort(); }
        return route.continue();
      });
      const page = await context.newPage(); page.setDefaultTimeout(5000);
      page.on('pageerror', error => errors.push(error.message));
      await page.goto(`${origin}/market`);
      assert.equal(apiRequests.length, 0, 'loading viewer must not start any authenticated request');
      await page.locator('#demo-button').click();
      assert.equal(apiRequests.length, 0, 'synthetic demo stays offline');
      await page.locator('#live-panel summary').click();
      const connect = async value => {
        await page.locator('#live-endpoint').fill(endpoint);
        await page.locator('#live-token').fill(value);
        await page.locator('#live-connect').click();
      };
      await connect(token);
      await page.waitForFunction(() => document.getElementById('live-status').dataset.phase === 'live');
      assert.equal(exports, 1);
      assert.equal(await page.locator('#live-token').inputValue(), '');
      assert.match(await page.locator('#load-status').textContent(), /Worker history/);
      assert.doesNotMatch(await page.locator('#load-status').textContent(), /SYNTHETIC DEMO/);
      assert.match(await page.locator('#snapshot-badge').textContent(), /Connected · source recent/);
      await page.locator('#event-select').selectOption('second-event');
      await page.locator('#source-select').selectOption('second-source');
      await page.locator('#series-select').selectOption('synthetic-primary');
      await page.locator('#chart-mode').selectOption('candles');
      const beforePrice = await page.locator('#price-value').textContent();
      for (const event of fixture.events) for (const source of event.sources) for (const sample of source.samples) for (const series of sample.series) {
        if (series.series_id === 'synthetic-primary') for (const key of ['min_price_minor', 'max_price_minor', 'median_price_minor']) series[key] += 100;
      }
      await page.locator('#live-refresh').click();
      await page.waitForFunction(price => document.getElementById('price-value').textContent !== price, beforePrice);
      assert.equal(await page.locator('#event-select').inputValue(), 'second-event');
      assert.equal(await page.locator('#source-select').inputValue(), 'second-source');
      assert.equal(await page.locator('#series-select').inputValue(), 'synthetic-primary');
      assert.equal(await page.locator('#chart-mode').inputValue(), 'candles');
      const validPrice = await page.locator('#price-value').textContent();
      const priorGeneratedAt = fixture.generated_at;
      fixture.generated_at += 3600;
      await page.locator('#live-refresh').click();
      await page.waitForFunction(() => document.getElementById('snapshot-badge').textContent.includes('clock mismatch'));
      fixture.generated_at = priorGeneratedAt;
      const selectedSource = fixture.events[1].sources[1];
      const lastSample = selectedSource.samples.at(-1);
      const observedAt = lastSample.observed_at;
      lastSample.observed_at += 3600;
      await page.locator('#live-refresh').click();
      await page.waitForFunction(() => document.getElementById('live-status').dataset.phase === 'live');
      assert.match(await page.locator('#snapshot-badge').textContent(), /clock mismatch/);
      lastSample.observed_at = observedAt;
      const lastSeries = lastSample.series;
      lastSample.availability = 'blocked'; lastSample.evidence = 'none'; lastSample.series = [];
      await page.locator('#live-refresh').click();
      await page.waitForFunction(() => document.getElementById('snapshot-badge').textContent.includes('source blocked'));
      assert.equal(await page.locator('#price-value').textContent(), 'Not observed');
      lastSample.availability = 'available'; lastSample.evidence = 'offers'; lastSample.series = lastSeries;
      await page.locator('#live-refresh').click();
      await page.waitForFunction(price => document.getElementById('price-value').textContent === price, validPrice);
      failure = true;
      await page.locator('#live-refresh').click();
      await page.waitForFunction(() => document.getElementById('live-status').dataset.phase === 'error');
      assert.equal(await page.locator('#price-value').textContent(), validPrice);
      assert.match(await page.locator('#snapshot-badge').textContent(), /Refresh failed/);
      failure = false;
      for (const event of fixture.events) {
        event.first_observed_at -= 86400; event.last_observed_at -= 86400;
        for (const source of event.sources) for (const sample of source.samples) sample.observed_at -= 86400;
      }
      await page.locator('#live-refresh').click();
      await page.waitForFunction(() => document.getElementById('live-status').dataset.phase === 'live');
      assert.match(await page.locator('#snapshot-badge').textContent(), /source stale/);
      await page.locator('#live-pause').click();
      assert.match(await page.locator('#snapshot-badge').textContent(), /Paused snapshot/);
      assert.equal(await page.locator('#live-refresh').isDisabled(), true);
      await page.locator('#live-pause').click();
      await page.waitForFunction(() => document.getElementById('live-status').dataset.phase === 'live');
      await page.locator('#live-disconnect').click();
      assert.match(await page.locator('#snapshot-badge').textContent(), /Disconnected snapshot/);
      await connect('wrong-synthetic-token-1234567890123456789');
      await page.waitForFunction(() => document.getElementById('live-status').dataset.phase === 'unauthorized');
      assert.equal(await page.locator('#live-token').inputValue(), '');
      assert.equal(await page.locator('#live-connect').isDisabled(), false);
      assert.equal(await page.locator('#live-refresh').isDisabled(), true);
      assert.equal(await page.locator('#price-value').textContent(), validPrice);
      assert.deepEqual(await page.evaluate(() => ({ local: Object.keys(localStorage), session: Object.keys(sessionStorage) })), { local: [], session: [] });
      assert.equal((await context.cookies()).length, 0);
      assert.equal(page.url().includes(token), false);
      await page.setViewportSize({ width: 390, height: 844 });
      assert.equal(await page.evaluate(() => Math.max(document.documentElement.scrollWidth,
        document.body.scrollWidth) <= window.innerWidth + 1), true, 'live controls must fit a 390px mobile viewport');
      for (const request of apiRequests) {
        assert.equal(request.url.includes(token), false);
        if (request.method === 'OPTIONS') assert.equal(request.authorization, undefined);
        else assert.ok(request.authorization === `Bearer ${token}` || request.authorization.startsWith('Bearer wrong-synthetic'));
      }
      pending = {};
      await connect(token);
      await page.waitForFunction(() => document.getElementById('live-status').dataset.phase === 'loading');
      // Wait for the local API exporter, then switch back to a demo while its response is pending.
      for (let i = 0; i < 50 && !pending.resolve; i += 1) await new Promise(resolve => setTimeout(resolve, 10));
      assert.equal(typeof pending.resolve, 'function');
      await page.locator('#demo-button').click();
      pending.resolve(); pending = null;
      await page.waitForTimeout(50);
      assert.match(await page.locator('#snapshot-badge').textContent(), /SYNTHETIC DATA/);
      assert.match(await page.locator('#load-status').textContent(), /SYNTHETIC DEMO/);
      assert.deepEqual(external, []); assert.deepEqual(errors, []);
      await context.close();
    } finally {
      pending?.resolve?.();
      await browser?.close();
    }
  });
