'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { capture } = require('./worker.cjs');
const enabled = process.env.TICKET_BROWSER_TESTS === '1';
const url = 'https://www.ticketmaster.cz/event/synthetic-scan-identification/12345678';
const request = { id: 1, url, timeout_ms: 12000, max_body_bytes: 65536, render_wait_ms: 0,
  adapter: 'ticketmaster', display: 'map', map_section_id: 's_1', map_scan_limit: 20 };
const identification = '<!doctype html><script>setTimeout(() => location.replace(location.href), 6000)</script>';
const seats = '<g class="seats"><g data-component="svg__block" data-section-id="s_1" data-section-name="1P-101"><g data-row-name="1"><circle data-component="svg__seat" id="standard_1" data-seat-name="1" cx="100" cy="100" r="10"/></g></g></g><g class="polygons"></g>';
const event = `<!doctype html><meta charset="utf-8"><body data-section-clicks="0" data-ticket-clicks="0"><h1>Synthetic event</h1>
<aside aria-label="Mapa míst"><button aria-pressed="true">Mapa míst</button><div data-component="zoomer"><svg id="map" data-component="svg" aria-hidden="true" width="300" height="300" viewBox="0 0 200 200"><g class="seats"></g><g class="polygons"><path data-component="svg__section" data-section-id="s_1" data-section-name="101" data-active="true" d="M20 20L180 20L180 180L20 180Z"/></g></svg></div>
<button data-testid="findTicketsBtn" onclick="document.body.dataset.ticketClicks='1'">Najít vstupenky</button></aside>
<script>const map=document.getElementById('map');map.querySelector('path').onclick=()=>{document.body.dataset.sectionClicks='1';map.innerHTML=${JSON.stringify(seats)}};</script></body>`;

async function browserFixture(run) {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try { await run(browser); } finally { await browser.close(); }
}

test('ordinary checks can opt into a longer normal navigation wait while defaults keep their shorter window', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    await Promise.all(['scan', 'section', 'ordinary', 'ordinary-extended'].map(async mode => {
      const context = await browser.newContext({ serviceWorkers: 'block' });
      let documents = 0, gotos = 0;
      context.on('page', page => {
        const goto = page.goto.bind(page);
        page.goto = (...args) => { gotos++; return goto(...args); };
        page.reload = () => { throw new Error('worker must not reload an identification page'); };
      });
      await context.route('**/*', route => {
        if (!route.request().isNavigationRequest()) return route.fulfill({ status: 204 });
        return ++documents === 1
          ? route.fulfill({ status: 401, contentType: 'text/html', headers: { 'tm-bl': '1', 'retry-after': '0' }, body: identification })
          : route.fulfill({ status: 200, contentType: 'text/html', body: event });
      });
      const input = mode === 'scan' ? request : mode === 'section' ? { ...request, map_scan_limit: undefined }
        : { ...request, adapter: 'selectors', display: undefined,
        ...(mode === 'ordinary-extended' ? { identification_wait_ms: 15000 } : {}),
        map_section_id: undefined, map_scan_limit: undefined };
      const result = await capture(context, input);
      if (mode === 'ordinary-extended') {
        assert.equal(result.status, 200);
        assert.match(result.html, /data-section-clicks="0" data-ticket-clicks="0"/);
        assert.equal(result.map_scan, undefined);
        assert.equal(result.map_inspection, undefined);
        assert.equal(documents, 2);
      } else if (mode !== 'ordinary') {
        assert.equal(result.status, 200);
        assert.equal(result.adapter_reason, mode === 'scan' ? 'scan_ready' : 'section_ready');
        if (mode === 'scan') {
          assert.equal(result.map_scan.candidate_count, 0);
          assert.equal(result.map_scan.stock_verified, false);
        } else {
          assert.equal(result.map_inspection.section_id, 's_1');
          assert.equal(result.map_inspection.stock_verified, false);
          assert.equal(result.map_inspection.seat_id, null);
        }
        assert.match(result.html, /data-section-clicks="1" data-ticket-clicks="0"/);
        assert.equal(documents, 2);
      } else {
        assert.equal(result.status, 401);
        assert.equal(result.manual_intervention, 'identification_incomplete');
        assert.equal(result.html, '');
        assert.equal(result.map_scan, undefined);
        assert.equal(documents, 1);
      }
      assert.equal(gotos, 1);
      assert.equal(context.pages().length, 0);
      await context.close();
    }));
  });
});

test('a short request caps both configured ordinary and scan identification waits at its own deadline', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    for (const extra of [{}, { display: undefined, map_section_id: undefined, map_scan_limit: undefined, identification_wait_ms: 15000 }]) {
      const context = await browser.newContext({ serviceWorkers: 'block' });
      let documents = 0;
      await context.route('**/*', route => {
        if (route.request().isNavigationRequest()) documents++;
        return route.fulfill({ status: 401, contentType: 'text/html', headers: { 'tm-bl': '1' }, body: identification });
      });
      const started = performance.now();
      const result = await capture(context, { ...request, ...extra, timeout_ms: 650 });
      assert.equal(result.status, 401);
      assert.equal(result.manual_intervention, 'identification_incomplete');
      assert.equal(result.adapter_state, 'blocked');
      assert.equal(result.map_scan, undefined);
      assert.equal(documents, 1);
      assert.ok(performance.now() - started < 1600);
      assert.equal(context.pages().length, 0);
      await context.close();
    }
  });
});

test('scan identification never consumes the extended wait when Retry-After prohibits continuation', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    for (const cooldown of ['60', 'invalid', '0, 120']) {
      for (const extra of [{}, { display: undefined, map_section_id: undefined, map_scan_limit: undefined, identification_wait_ms: 15000 }]) {
        const context = await browser.newContext({ serviceWorkers: 'block' });
        let documents = 0;
        context.on('page', page => { page.evaluate = () => { throw new Error('must not inspect restricted response HTML'); }; });
        await context.route('**/*', route => {
          if (route.request().isNavigationRequest()) documents++;
          return route.fulfill({ status: 401, contentType: 'text/html',
            headers: { 'tm-bl': '1', 'retry-after': cooldown }, body: identification });
        });
        const started = performance.now();
        const result = await capture(context, { ...request, ...extra });
        assert.equal(result.status, 401);
        assert.equal(result.html, '');
        assert.equal(result.map_scan, undefined);
        assert.equal(documents, 1);
        assert.ok(performance.now() - started < 1600);
        assert.equal(context.pages().length, 0);
        await context.close();
      }
    }
  });
});
