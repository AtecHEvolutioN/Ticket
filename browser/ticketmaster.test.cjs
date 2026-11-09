'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { prepareTicketmaster } = require('./ticketmaster.cjs');
const { capture, validateRequest } = require('./worker.cjs');
const { parseArgs } = require('./capture.cjs');

const base = { id: 1, timeout_ms: 5000, render_wait_ms: 30, max_body_bytes: 32768, adapter: 'ticketmaster' };
const enabled = process.env.TICKET_BROWSER_TESTS === '1';
function fixture(label, consent = false) {
  return `<!doctype html><html><head><meta charset="utf-8"></head><body data-display-clicks="0" data-reserve-clicks="0">
    <h1>Synthetic selected event</h1><button id="display">${label}</button>
    <button hidden onclick="reserve()">${label}</button>
    <button disabled onclick="reserve()">${label}</button>
    <button data-testid="findTicketsBtn" onclick="reserve()">Find tickets</button>
    <button data-testid="quantityStepper" onclick="reserve()">+</button>
    <li role="button" onclick="reserve()">Resale listing</li>
    <div id="offers"></div>
    ${consent ? `<div id="consent" style="position:fixed;inset:0;z-index:999;background:white"><button id="ketch-banner-button-secondary" onclick="document.getElementById('consent').remove()">${typeof consent === 'string' ? consent : 'Odrzuć wszystko'}</button></div>` : ''}
    <script>
      function reserve() { document.body.dataset.reserveClicks = String(Number(document.body.dataset.reserveClicks)+1); }
      document.getElementById('display').onclick = () => {
        document.body.dataset.displayClicks = String(Number(document.body.dataset.displayClicks)+1);
        setTimeout(() => document.getElementById('offers').innerHTML = '<div data-testid="ticketTypeInfo">Synthetic type <span>29,50 zł</span></div>', 25);
      };
    </script></body></html>`;
}

test('adapter opt-in rejects arbitrary actions and non-event requests', () => {
  const url = 'https://www.ticketmaster.pl/event/12345678';
  assert.doesNotThrow(() => validateRequest({ ...base, url }));
  assert.throws(() => validateRequest({ ...base, url, adapter: 'checkout' }));
  assert.throws(() => validateRequest({ ...base, url: 'https://www.ticketmaster.pl/artist/12345678' }));
  assert.equal(parseArgs(['--url', url, '--output', '/tmp/event.html', '--adapter', 'ticketmaster']).adapter, 'ticketmaster');
  assert.throws(() => parseArgs(['--url', url, '--output', '/tmp/event.html', '--adapter', 'checkout']));
});

test('captured Czech, Spanish and Dutch labels open only their display controls', { skip: !enabled }, async () => {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    const cases = [
      ['cz', 'Nejlepší dostupné', 'Odmítnout vše'],
      ['es', 'Ver lo mejor disponible', false],
      ['nl', 'Best beschikbaar', 'Alles weigeren'],
    ];
    await context.route('**/*', route => {
      const [country, label, consent] = cases.find(([country]) => new URL(route.request().url()).hostname.endsWith(`.${country}`));
      return route.fulfill({ status: 200, contentType: 'text/html', body: fixture(label, consent) });
    });
    for (const [index, [country]] of cases.entries()) {
      const result = await capture(context, { ...base, id: index + 1,
        url: `https://www.ticketmaster.${country}/event/another-venue/${12345670 + index}` });
      assert.equal(result.adapter_state, 'ready', result.adapter_reason);
      assert.match(result.html, /<body[^>]*data-display-clicks="1"[^>]*data-reserve-clicks="0"/);
      assert.doesNotMatch(result.html, /<div id="consent"/);
    }
    await context.close();
  } finally { await browser.close(); }
});

test('display preparation waits briefly for delayed hydration and stops on a late challenge', { skip: !enabled }, async () => {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    const documents = {
      12345671: fixture('Ver lo mejor disponible').replace('id="display"', 'id="display" hidden')
        .replace('</body>', '<script>setTimeout(() => document.getElementById("display").hidden=false, 250)</script></body>'),
      12345672: '<body data-clicks="0"><script>setTimeout(() => document.body.insertAdjacentHTML("beforeend", "<h1>Your Browsing Activity Has Been Paused</h1><button onclick=\'document.body.dataset.clicks=1\'>Best beschikbaar</button>"), 250)</script></body>',
    };
    await context.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html',
      body: documents[route.request().url().split('/').at(-1)] }));
    const ready = await capture(context, { ...base, url: 'https://www.ticketmaster.es/event/12345671' });
    assert.equal(ready.adapter_state, 'ready', ready.adapter_reason);
    assert.match(ready.html, /<body[^>]*data-display-clicks="1"[^>]*data-reserve-clicks="0"/);
    const blocked = await capture(context, { ...base, url: 'https://www.ticketmaster.nl/event/12345672' });
    assert.equal(blocked.adapter_state, 'blocked');
    assert.match(blocked.html, /<body[^>]*data-clicks="0"/);
    await context.close();
  } finally { await browser.close(); }
});

test('same adapter opens display tabs for unrelated events and languages without ticket selection', { skip: !enabled }, async () => {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    await context.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html',
      body: route.request().url().includes('ticketmaster.pl')
        ? fixture('Wybierz najlepsze dostępne miejsca', true) : fixture('See best available') }));
    for (const [id, url] of [
      [1, 'https://www.ticketmaster.pl/event/arbitrary-jazz-concert/12345678'],
      [2, 'https://www.ticketmaster.cz/event/arbitrary-sport/99999999?language=en'],
    ]) {
      const result = await capture(context, { ...base, id, url });
      assert.equal(result.adapter_state, 'ready', result.adapter_reason);
      assert.equal(result.adapter_reason, 'regional');
      assert.match(result.html, /<body[^>]*data-display-clicks="1"[^>]*data-reserve-clicks="0"/);
      assert.match(result.html, /data-testid="ticketTypeInfo">Synthetic type/);
      assert.doesNotMatch(result.html, /<div id="consent"/);
    }
    await context.close();
  } finally { await browser.close(); }
});

test('unknown, ambiguous and blocked pages never click a purchase or display control', { skip: !enabled }, async () => {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    const documents = {
      12345671: '<h1>New layout</h1><button onclick="document.body.dataset.clicked=1">Buy now</button>',
      12345672: '<h1>Your Browsing Activity Has Been Paused</h1><button onclick="document.body.dataset.clicked=1">See best available</button>',
      12345673: '<button onclick="document.body.dataset.clicked=1">See best available</button><button>See best available</button>',
    };
    await context.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html',
      body: '<body data-clicked="0">' + documents[route.request().url().split('/').at(-1)] + '</body>' }));
    for (const [event, expected] of [[12345671, 'unsupported'], [12345672, 'blocked'], [12345673, 'unsupported']]) {
      const result = await capture(context, { ...base, url: `https://www.ticketmaster.com/event/${event}` });
      assert.equal(result.adapter_state, expected);
      assert.match(result.html, /<body[^>]*data-clicked="0"/);
    }
    await context.close();
  } finally { await browser.close(); }
});

test('existing listings need no clicks and an empty view is not sold-out evidence', { skip: !enabled }, async () => {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    const url = 'https://www.ticketmaster.nl/event/12345678';
    await context.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html',
      body: '<li data-listing-id="synthetic" onclick="document.body.dataset.reserved=1">Displayed offer</li>' }));
    const page = await context.newPage();
    await page.goto(url);
    assert.deepEqual(await prepareTicketmaster(page, url, 1000), { state: 'listings', opened: false });
    await page.setContent('<body data-clicks="0"><button onclick="document.body.dataset.clicks=1">See best available</button></body>');
    assert.deepEqual(await prepareTicketmaster(page, url, 250), { state: 'no_visible_offers', opened: true });
    assert.equal(await page.locator('body').getAttribute('data-clicks'), '1');
    await assert.rejects(prepareTicketmaster(page, url.replace('12345678', '99999999'), 1000), /unsafe_redirect/);
    await context.close();
  } finally { await browser.close(); }
});
