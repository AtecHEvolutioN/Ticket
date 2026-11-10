'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const { reviewedQueueRedirect, blockReviewedQueue } = require('./queue-boundary.cjs');
const { capture } = require('./worker.cjs');
const enabled = process.env.TICKET_BROWSER_TESTS === '1';
const italian = 'https://www.ticketmaster.it/biglietti/synthetic/event/segp4wi4du5m?language=en-us';
const queue = 'https://wait.ticketmaster.it/?c=private-marker&e=event&t=private-return';

test('queue classification is limited to the reviewed Italian top-document boundary', () => {
  assert.equal(reviewedQueueRedirect(italian, queue), true);
  for (const url of ['https://wait.ticketmaster.it.evil.test/?t=private', 'http://wait.ticketmaster.it/',
    'https://wait.ticketmaster.it:444/', 'https://user:password@wait.ticketmaster.it/',
    'https://queue.ticketmaster.it/', 'https://wait.ticketmaster.com/']) assert.equal(reviewedQueueRedirect(italian, url), false);
  assert.equal(reviewedQueueRedirect('https://www.ticketmaster.cz/event/12345678', queue), false);
});

test('queue request interception fails the paused main request without continuing or copying its URL', async () => {
  const session = new EventEmitter(), calls = [];
  session.send = async (method, params) => { calls.push({ method, params }); return { frameTree: { frame: { id: 'main' } } }; };
  session.detach = async () => {};
  const guard = await blockReviewedQueue({ newCDPSession: async () => session }, { close: async () => {} }, italian);
  session.emit('Fetch.requestPaused', { requestId: 'synthetic-request', frameId: 'main', request: { url: queue } });
  assert.equal(guard.blocked, true);
  assert.deepEqual(calls.at(-1), { method: 'Fetch.failRequest', params: { requestId: 'synthetic-request', errorReason: 'BlockedByClient' } });
  assert.equal(JSON.stringify(calls).includes('private-marker'), false);
  assert.equal(calls.some(c => c.method === 'Fetch.continueRequest'), false);
  await guard.close();
  assert.equal(session.listenerCount('Fetch.requestPaused'), 0);
});

test('a committed same-event401 survives an initial goto failure without body reads or retries', async () => {
  const url = 'https://www.ticketmaster.com/synthetic/event/0100650A23A974B2';
  for (const protection of [true, false]) {
    const session = new EventEmitter();
    session.send = async () => ({ frameTree: { frame: { id: 'main' } } });
    session.detach = async () => {};
    let gotos = 0, closed = false;
    const page = { route: async () => {}, url: () => url, close: async () => { closed = true; },
      evaluate: () => { throw new Error('must not read restricted body'); },
      goto: async () => {
        gotos++;
        session.emit('Network.requestWillBeSent', { type: 'Document', frameId: 'main', loaderId: 'doc1', requestId: 'req1', request: { url } });
        session.emit('Network.responseReceived', { type: 'Document', frameId: 'main', loaderId: 'doc1',
          response: { status: 401, url, headers: { 'content-type': 'text/html', 'retry-after': '120',
            ...(protection ? { 'tm-bl': '1' } : {}), 'private-header': 'private-session-value' } } });
        session.emit('Page.frameNavigated', { frame: { id: 'main', loaderId: 'doc1', url } });
        throw new Error('initial navigation failed');
      } };
    const result = await capture({ newPage: async () => page, newCDPSession: async () => session }, {
      id: 1, url, timeout_ms: 2000, max_body_bytes: 4096, render_wait_ms: 0, adapter: 'ticketmaster' });
    assert.equal(result.status, 401);
    assert.equal(result.html, '');
    assert.equal(result.adapter_state, 'blocked');
    assert.equal(result.retry_after_secs, 120);
    assert.equal(result.manual_intervention, protection ? 'identification_incomplete' : undefined);
    assert.equal(JSON.stringify(result).includes('private-session-value'), false);
    assert.equal(gotos, 1);
    assert.equal(closed, true);
  }
});

test('ordinary Chromium blocks the reviewed queue redirect before its response is followed', { skip: !enabled }, async () => {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    let queueResponses = 0;
    context.on('response', response => { if (response.url().startsWith('https://wait.ticketmaster.it/')) queueResponses++; });
    await context.route('**/*', route => route.request().url() === italian
      ? route.fulfill({ status: 307, headers: { location: queue }, body: '' })
      : route.fulfill({ status: 200, contentType: 'text/html', body: 'private queue response must not be read' }));
    const caught = await capture(context, { id: 1, url: italian, timeout_ms: 5000,
      max_body_bytes: 4096, render_wait_ms: 0, adapter: 'ticketmaster' }).then(() => null, e => e);
    assert.equal(caught?.message, 'queue_redirect');
    assert.equal(queueResponses, 0);
    assert.equal(String(caught).includes('private-marker'), false);
    assert.equal(context.pages().length, 0);
    await context.close();
  } finally { await browser.close(); }
});

test('unrelated main-document destinations remain unsafe_redirect with the queue guard installed', { skip: !enabled }, async () => {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    await context.route('**/*', route => route.request().url() === italian
      ? route.fulfill({ status: 200, contentType: 'text/html', body: '<script>location.replace("https://unrelated.invalid/event/12345678")</script>' })
      : route.abort('blockedbyclient'));
    await assert.rejects(capture(context, { id: 1, url: italian, timeout_ms: 5000,
      max_body_bytes: 4096, render_wait_ms: 100, adapter: 'ticketmaster' }), /unsafe_redirect/);
    assert.equal(context.pages().length, 0);
    await context.close();
  } finally { await browser.close(); }
});
