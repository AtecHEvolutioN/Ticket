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
