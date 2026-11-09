'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { challengeKind, responseIntervention, inspectChallenge, waitForManualClearance, MESSAGES } = require('./manual-intervention.cjs');
const { capture } = require('./worker.cjs');

test('challenge reasons recognize English and Czech prompts without treating attribution as a challenge', () => {
  for (const [text, kind] of [
    ['Please verify you are human', 'captcha'],
    ['Ověřte, že nejste robot', 'captcha'],
    ['You are in the queue', 'queue'],
    ['Jste ve frontě', 'queue'],
    ['Vaše relace vypršela', 'session_expired'],
    ['Sign in to continue', 'authentication_required'],
    ['Your Browsing Activity Has Been Paused', 'access_blocked'],
    ['Checking your browser', 'verification'],
    ['This site is protected by reCAPTCHA', null],
    ['Standard tickets: 1200 CZK', null],
  ]) assert.equal(challengeKind(text), kind);
});

test('without an operator handler a challenge returns immediately and an ordinary page is clear', async () => {
  assert.deepEqual(await waitForManualClearance({ inspect: async () => 'queue' }), { state: 'needs_human', kind: 'queue' });
  assert.deepEqual(await waitForManualClearance({ inspect: async () => null }), { state: 'clear' });
});

test('header diagnosis requires the observed 403 marker and never forwards arbitrary values', () => {
  for (const value of ['1', ' 1 ']) {
    const kind = responseIntervention(403, { 'tm-bl': value });
    assert.equal(kind, 'ticketmaster_protection');
    assert.equal(MESSAGES[kind], 'Ticketmaster access-protection block; exact trigger undisclosed');
  }
  for (const value of [undefined, '', '0', 'unknown-private-value', '1, 1', 1]) {
    assert.equal(responseIntervention(403, { 'tm-bl': value }), null);
  }
  assert.equal(responseIntervention(403, undefined), null);
  for (const status of [200, 401, 429, 500]) {
    assert.equal(responseIntervention(status, { 'tm-bl': '1' }), null);
  }
});

test('operator acknowledgement cannot override a challenge and repeated prompts are bounded', async () => {
  let inspections = 0;
  const requests = [];
  const result = await waitForManualClearance({
    inspect: async () => { inspections++; return 'captcha'; },
    requestHuman: async request => { requests.push(request); return { solved: true, state: 'clear' }; },
    maxPrompts: 2,
  });
  assert.deepEqual(result, { state: 'needs_human', kind: 'captcha' });
  assert.equal(inspections, 3);
  assert.deepEqual(requests.map(request => request.attempt), [1, 2]);
  for (const request of requests) {
    assert.equal(Object.isFrozen(request), true);
    assert.deepEqual(Object.keys(request).sort(), ['attempt', 'kind', 'message', 'signal']);
  }
});

test('clearance requires a fresh inspection after operator input; inspection failures do not resume', async () => {
  let kind = 'captcha';
  let inspections = 0;
  assert.deepEqual(await waitForManualClearance({
    inspect: async () => { inspections++; return kind; },
    requestHuman: async () => { kind = null; },
  }), { state: 'clear' });
  assert.equal(inspections, 2);
  let acknowledged = false;
  await assert.rejects(waitForManualClearance({
    inspect: async () => { if (acknowledged) throw new Error('unsafe_redirect'); return 'captcha'; },
    requestHuman: async () => { acknowledged = true; },
  }), /unsafe_redirect/);
  await assert.rejects(waitForManualClearance({ inspect: async () => undefined }), /invalid_manual_intervention/);
});

test('cancellation and timeout interrupt a hanging operator handler without another inspection', async () => {
  const controller = new AbortController();
  let inspections = 0;
  let notified;
  const started = new Promise(resolve => { notified = resolve; });
  let handlerSignal;
  const pending = waitForManualClearance({
    inspect: async () => { inspections++; return 'captcha'; },
    requestHuman: async ({ signal }) => { handlerSignal = signal; notified(); await new Promise(() => {}); },
    signal: controller.signal,
  });
  await started;
  controller.abort();
  await assert.rejects(pending, /manual_intervention_cancelled/);
  assert.equal(handlerSignal.aborted, true);
  assert.equal(inspections, 1);
  await assert.rejects(waitForManualClearance({
    inspect: async () => 'captcha', requestHuman: async () => new Promise(() => {}), timeoutMs: 10,
  }), /manual_intervention_timeout/);
  await assert.rejects(waitForManualClearance({
    inspect: async () => { throw new Error('must not run'); }, signal: controller.signal,
  }), /manual_intervention_cancelled/);
});

test('Chromium worker reports manual attention for both adapters, keeps cooldowns, and closes each page',
  { skip: process.env.TICKET_BROWSER_TESTS !== '1' }, async () => {
    const { chromium } = require('playwright');
    const browser = await chromium.launch({ headless: true,
      ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
    try {
      const context = await browser.newContext({ serviceWorkers: 'block' });
      let requests = 0;
      await context.route('**/*', route => {
        requests++;
        return route.fulfill({ status: 200, contentType: 'text/html; charset=utf-8', headers: { 'retry-after': '90' },
          body: '<body data-clicks="0"><h1>Ověřte, že nejste robot</h1><button onclick="document.body.dataset.clicks=1">See best available</button><div data-testid="ticketTypeInfo">Standard 1200 CZK</div></body>' });
      });
      for (const adapter of ['selectors', 'ticketmaster']) {
        const result = await capture(context, { id: 1, url: 'https://www.ticketmaster.cz/event/synthetic/12345678',
          timeout_ms: 5000, max_body_bytes: 8192, render_wait_ms: 0, adapter });
        assert.equal(result.adapter_state, 'blocked');
        assert.equal(result.manual_intervention, 'captcha');
        assert.equal(result.retry_after_secs, 90);
        assert.match(result.html, /data-clicks="0"/);
        assert.equal(context.pages().length, 0);
      }
      assert.equal(requests, 2); // All browser requests were fulfilled locally.
      const page = await context.newPage();
      await page.setContent('<div hidden>Verify you are human</div><footer>This site is protected by reCAPTCHA</footer>');
      assert.equal(await inspectChallenge(page), null);
      await context.close();
    } finally { await browser.close(); }
  });
