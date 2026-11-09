'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { capture } = require('./worker.cjs');

const enabled = process.env.TICKET_BROWSER_TESTS === '1';
const url = 'https://www.ticketmaster.cz/event/synthetic/12345678';
const request = { id: 1, url, timeout_ms: 4000, max_body_bytes: 16384, render_wait_ms: 0, adapter: 'selectors' };
const identification = '<!doctype html><script>setTimeout(() => location.replace(location.href), 40)</script>';
async function browserFixture(callback) {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try { await callback(browser); } finally { await browser.close(); }
}

test('a website identification navigation commits its own 200 before offers are captured', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    let documents = 0;
    let gotoCalls = 0;
    context.on('page', page => {
      const original = page.goto.bind(page);
      page.goto = (...args) => { gotoCalls++; return original(...args); };
      page.reload = () => { throw new Error('worker must not reload'); };
    });
    await context.route('**/*', route => {
      if (!route.request().isNavigationRequest()) return route.fulfill({ status: 204 });
      documents++;
      return documents === 1
        ? route.fulfill({ status: 401, contentType: 'text/html; charset=utf-8', headers: { 'tm-bl': '1', server: 'identify', 'retry-after': '0' }, body: identification })
        : route.fulfill({ status: 200, contentType: 'text/html', headers: { server: 'inventory', 'retry-after': '17' },
          body: '<html><title>Synthetic event</title><body><button>Current offers</button></body></html>' });
    });
    const result = await capture(context, request);
    assert.equal(result.status, 200);
    assert.match(result.html, /Current offers/);
    assert.equal(result.final_url, url);
    assert.equal(result.server, 'inventory');
    assert.equal(result.retry_after_secs, 17);
    assert.equal(documents, 2);
    assert.equal(gotoCalls, 1);
    assert.equal(context.pages().length, 0);
    await context.close();
  });
});

test('identification follow-through returns final 403 or 429 without inspecting error DOM', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    for (const status of [403, 429]) {
      const context = await browser.newContext({ serviceWorkers: 'block' });
      let documents = 0;
      let release;
      const heldScript = new Promise(resolve => { release = resolve; });
      context.on('page', page => { page.evaluate = () => { throw new Error('must not inspect an error DOM'); }; });
      await context.route('**/*', route => {
        if (route.request().url().endsWith('/held-script.js')) {
          return heldScript.then(() => route.abort()).catch(() => {});
        }
        if (!route.request().isNavigationRequest()) return route.fulfill({ status: 204 });
        documents++;
        return documents === 1
          ? route.fulfill({ status: 401, contentType: 'text/html', headers: { 'tm-bl': '1' }, body: identification })
          : route.fulfill({ status, contentType: 'text/html', headers: { 'tm-bl': '1', 'retry-after': '120' },
            body: '<script src="/held-script.js"></script><button>Misleading tickets</button>' });
      });
      const result = await capture(context, request).finally(release);
      assert.equal(result.status, status);
      assert.equal(result.html, '');
      assert.equal(result.retry_after_secs, 120);
      assert.equal(result.manual_intervention, status === 403 ? 'ticketmaster_protection' : undefined);
      assert.equal(documents, 2);
      assert.equal(context.pages().length, 0);
      await context.close();
    }
  });
});

test('generic authentication, blocks, cooldowns and non-HTML responses never enable passive waiting', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    for (const [status, headers, contentType] of [
      [401, {}, 'text/html'], [403, { 'tm-bl': '1' }, 'text/html'],
      [429, { 'tm-bl': '1', 'retry-after': '60' }, 'text/html'],
      [401, { 'tm-bl': '1', 'retry-after': '60' }, 'text/html'],
      [401, { 'tm-bl': '1', 'retry-after': 'invalid' }, 'text/html'],
      [401, { 'tm-bl': '1', 'retry-after': '0, 120' }, 'text/html'],
      [401, { 'tm-bl': '1', 'retry-after': '0, 1' }, 'text/html'],
      [401, { 'tm-bl': '1' }, 'application/json'],
    ]) {
      const context = await browser.newContext({ serviceWorkers: 'block' });
      let documents = 0;
      context.on('page', page => { page.evaluate = () => { throw new Error('must not inspect an error DOM'); }; });
      await context.route('**/*', route => {
        if (!route.request().isNavigationRequest()) return route.fulfill({ status: 204 });
        documents++;
        return route.fulfill({ status, contentType, headers, body: '<script>setTimeout(() => location.reload(), 1000)</script>' });
      });
      const started = performance.now();
      const result = await capture(context, { ...request, timeout_ms: 2500 });
      assert.equal(result.status, status);
      assert.equal(result.html, '');
      assert.equal(documents, 1);
      assert.ok(performance.now() - started < 1500, 'must not consume the passive waiting window');
      await context.close();
    }
  });
});

test('an identification page with no navigation retains blocked health within the original deadline', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    let documents = 0;
    await context.route('**/*', route => {
      if (route.request().isNavigationRequest()) documents++;
      return route.fulfill({ status: 401, contentType: 'text/html', headers: { 'tm-bl': '1' },
        body: '<!doctype html><html><title>Identification</title><body>Waiting</body></html>' });
    });
    const started = performance.now();
    const result = await capture(context, { ...request, timeout_ms: 700 });
    assert.equal(result.status, 401);
    assert.equal(result.html, '');
    assert.equal(result.manual_intervention, 'identification_incomplete');
    assert.equal(result.adapter_state, 'blocked');
    assert.equal(result.final_url, url);
    assert.equal(documents, 1);
    assert.ok(performance.now() - started < 1500);
    assert.equal(context.pages().length, 0);
    await context.close();
  });
});

test('wrong-event, non-event and aborted identification navigations cannot produce inventory', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    for (const scenario of ['wrong_event', 'about_blank', 'aborted', 'uncommitted']) {
      const context = await browser.newContext({ serviceWorkers: 'block' });
      let documents = 0;
      await context.route('**/*', route => {
        if (!route.request().isNavigationRequest()) return route.fulfill({ status: 204 });
        documents++;
        if (documents === 1) return route.fulfill({ status: 401, contentType: 'text/html', headers: { 'tm-bl': '1' },
          body: scenario === 'wrong_event' ? '<script>setTimeout(() => location.replace("/event/99999999"), 40)</script>'
            : scenario === 'about_blank' ? '<script>setTimeout(() => location.replace("about:blank#done"), 40)</script>' : identification });
        if (scenario === 'aborted') return route.abort('failed');
        return route.fulfill({ status: 204 }); // A response with no committed replacement document.
      });
      if (scenario === 'wrong_event' || scenario === 'about_blank') {
        await assert.rejects(capture(context, { ...request, timeout_ms: 900 }), /unsafe_redirect/);
      } else {
        const result = await capture(context, { ...request, timeout_ms: 900 });
        assert.equal(result.status, 401);
        assert.equal(result.html, '');
        assert.equal(result.manual_intervention, 'identification_incomplete');
        assert.equal(result.adapter_state, 'blocked');
        assert.equal(result.final_url, url);
      }
      assert.equal(context.pages().length, 0);
      await context.close();
    }
  });
});

test('a committed late block preserves its cooldown after a readable identification follow-through', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    for (const status of [403, 429]) {
      const context = await browser.newContext({ serviceWorkers: 'block' });
      let documents = 0;
      await context.route('**/*', route => {
        if (!route.request().isNavigationRequest()) return route.fulfill({ status: 204 });
        documents++;
        if (documents === 1) return route.fulfill({ status: 401, contentType: 'text/html', headers: { 'tm-bl': '1' }, body: identification });
        if (documents === 2) return route.fulfill({ status: 200, contentType: 'text/html',
          body: '<body>Temporary offers<script>setTimeout(() => location.reload(), 40)</script></body>' });
        return route.fulfill({ status, contentType: 'text/html', headers: { 'tm-bl': '1', 'retry-after': '120' },
          body: '<body>Access interrupted</body>' });
      });
      const result = await capture(context, { ...request, render_wait_ms: 200 });
      assert.equal(result.status, status);
      assert.equal(result.html, '');
      assert.equal(result.retry_after_secs, 120);
      assert.equal(result.final_url, url);
      assert.equal(result.adapter_state, 'blocked');
      assert.equal(documents, 3);
      assert.equal(context.pages().length, 0);
      await context.close();
    }
  });
});

test('a new document navigation during extraction invalidates the captured old offers', { skip: !enabled }, async () => {
  await browserFixture(async browser => {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    let documents = 0;
    let triggered = false;
    context.on('page', page => {
      const evaluate = page.evaluate.bind(page);
      page.evaluate = async (operation, argument) => {
        const result = await evaluate(operation, argument);
        if (!triggered && String(operation).includes('root.cloneNode(true)')) {
          triggered = true;
          const navigation = page.waitForEvent('request', outgoing => outgoing.isNavigationRequest());
          await evaluate(() => window.dispatchEvent(new Event('fixture-navigate')));
          await navigation;
        }
        return result;
      };
    });
    await context.route('**/*', route => {
      if (!route.request().isNavigationRequest()) return route.fulfill({ status: 204 });
      documents++;
      return route.fulfill({ status: 200, contentType: 'text/html', body:
        '<body>Old offers<script>addEventListener("fixture-navigate", () => location.reload())</script></body>' });
    });
    await assert.rejects(capture(context, request), /navigation_failed/);
    assert.equal(triggered, true);
    assert.ok(documents >= 1);
    assert.equal(context.pages().length, 0);
    await context.close();
  });
});
