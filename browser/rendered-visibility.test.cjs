'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { captureRendered } = require('./rendered.cjs');

const enabled = process.env.TICKET_BROWSER_TESTS === '1';
const root = path.resolve(__dirname, '..');
const offer = '<main data-test="complete"><article data-test="offer" data-offer-id="synthetic">'
  + '<span class="type">Synthetic Standard</span><span class="section">A</span>'
  + '<span class="price">910,80 zł</span><span class="quantity">2</span>'
  + '<button class="buy">Select</button></article></main>';
const document = body => '<!doctype html><html><head><meta charset="utf-8">'
  + '<link rel="canonical" href="https://www.ticketmaster.pl/event/synthetic/496543017">'
  + '<script id="__NEXT_DATA__" type="application/json">{"synthetic":true}</script>'
  + '</head><body>' + body + '</body></html>';

async function withPage(run) {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'ticket-visibility-'));
  try {
    const context = await browser.newContext();
    // These fixtures never need network access, even if a regression adds a URL.
    let networkRequests = 0;
    await context.route('**/*', route => { networkRequests++; return route.abort(); });
    const page = await context.newPage();
    const inspect = async html => {
      const file = path.join(directory, 'rendered.html');
      await fs.writeFile(file, html);
      return JSON.parse(execFileSync(process.env.TICKET_WATCH_BIN || path.join(root, 'target/debug/ticket-watch'),
        ['--config', 'examples/offline-demo.toml', 'check', '--watch', 'demo', '--html', file],
        { cwd: root, encoding: 'utf8' }));
    };
    await run(page, inspect);
    assert.equal(networkRequests, 0);
  } finally {
    await browser.close();
    await fs.rm(directory, { recursive: true, force: true });
  }
}

test('inert ancestors cannot supply offers or coarse status and capture leaves live DOM unchanged',
  { skip: !enabled, timeout: 30000 }, async () => withPage(async (page, inspect) => {
    await page.setContent(document('<section inert><p data-test="available">Available</p>' + offer + '</section>'));
    const before = await page.evaluate(() => document.documentElement.outerHTML);
    const html = await captureRendered(page, 65536);
    assert.equal(await page.evaluate(() => document.documentElement.outerHTML), before);
    assert.match(html, /<section[^>]*inert[^>]*hidden/);
    const blocked = await inspect(html);
    assert.equal(blocked.availability, 'unknown');
    assert.deepEqual(blocked.offers, []);
    await page.locator('section').evaluate(element => { element.inert = false; });
    const usable = await inspect(await captureRendered(page, 65536));
    assert.equal(usable.availability, 'available');
    assert.equal(usable.offers.length, 1);
  }));

test('native modality masks background evidence, keeps metadata and modal diagnostics, and clears after close',
  { skip: !enabled, timeout: 30000 }, async () => withPage(async (page, inspect) => {
    await page.setContent(document('<section data-test="available">' + offer
      + '<dialog id="filter"><h2>Choose a section</h2><button>Close filters</button></dialog></section>'));
    await page.locator('#filter').evaluate(element => element.showModal());
    const before = await page.evaluate(() => document.documentElement.outerHTML);
    const html = await captureRendered(page, 65536);
    assert.equal(await page.evaluate(() => document.documentElement.outerHTML), before);
    assert.match(html, /<html[^>]*data-ticketwatch-native-modal="open"/);
    const snapshot = await page.evaluate(html => {
      const copy = new DOMParser().parseFromString(html, 'text/html');
      return { headHidden: copy.head.hasAttribute('hidden'), canonical: copy.querySelector('link[rel=canonical]').getAttribute('href'),
        metadata: copy.querySelector('#__NEXT_DATA__').textContent,
        modalHidden: copy.querySelector('#filter').hasAttribute('hidden'),
        backgroundHidden: copy.querySelector('[data-test=complete]').hasAttribute('hidden') };
    }, html);
    assert.equal(snapshot.headHidden, false);
    assert.match(snapshot.canonical, /496543017$/);
    assert.equal(snapshot.metadata, '{"synthetic":true}');
    assert.equal(snapshot.modalHidden, false);
    assert.equal(snapshot.backgroundHidden, true);
    const blocked = await inspect(html);
    assert.equal(blocked.availability, 'unknown');
    assert.deepEqual(blocked.offers, []);
    assert.match(blocked.detail, /native modal open/);
    await page.locator('#filter').evaluate(element => element.close());
    const clear = await captureRendered(page, 65536);
    assert.doesNotMatch(clear, /data-ticketwatch-native-modal/);
    assert.equal((await inspect(clear)).availability, 'available');
  }));

test('nonmodal dialogs and normal popovers do not manufacture native modality',
  { skip: !enabled, timeout: 30000 }, async () => withPage(async (page, inspect) => {
    await page.setContent(document(offer + '<dialog id="info">Information</dialog><div id="tip" popover>Price explanation</div>'));
    await page.locator('#info').evaluate(element => element.show());
    await page.locator('#tip').evaluate(element => element.showPopover());
    const html = await captureRendered(page, 65536);
    assert.doesNotMatch(html, /data-ticketwatch-native-modal/);
    assert.equal((await inspect(html)).availability, 'available');
  }));

test('painted map marker distinguishes SVG accessibility from actual paint and rejects copied markers',
  { skip: !enabled, timeout: 30000 }, async () => withPage(async (page) => {
    const svg = (id, attrs = '') => `<svg id="${id}" data-component="svg" width="100" height="50" ${attrs}><path d="M0 0H100V50H0Z"/></svg>`;
    await page.setContent(document(
      svg('painted', 'aria-hidden="true"')
      + svg('hidden', 'hidden data-ticketwatch-map-painted="true"')
      + '<section aria-hidden="true">' + svg('aria-parent') + '</section>'
      + '<section inert>' + svg('inert-parent') + '</section>'
      + '<section style="opacity:0">' + svg('transparent-parent') + '</section>'
      + svg('offscreen', 'style="position:absolute;left:-200px;top:0"')
      + '<div style="overflow:hidden;width:20px;height:20px;position:relative">'
      + svg('clipped', 'style="position:absolute;left:100px;top:0"') + '</div>'
      + '<svg id="unknown" width="100" height="50" data-ticketwatch-map-painted="true"><path d="M0 0H100V50H0Z"/></svg>'));
    const before = await page.evaluate(() => document.documentElement.outerHTML);
    const html = await captureRendered(page, 65536);
    assert.equal(await page.evaluate(() => document.documentElement.outerHTML), before);
    const attributes = await page.evaluate(html => {
      const copy = new DOMParser().parseFromString(html, 'text/html');
      return Object.fromEntries([...copy.querySelectorAll('svg')].map(svg => [svg.id, {
        marker: svg.getAttribute('data-ticketwatch-map-painted'), hidden: svg.hasAttribute('hidden'),
        ariaHidden: svg.getAttribute('aria-hidden'),
      }]));
    }, html);
    assert.deepEqual(attributes.painted, { marker: 'true', hidden: false, ariaHidden: 'true' });
    for (const id of ['hidden', 'aria-parent', 'inert-parent', 'transparent-parent', 'offscreen', 'clipped', 'unknown']) {
      assert.equal(attributes[id].marker, null, id);
    }
    await page.evaluate(() => {
      const dialog = document.createElement('dialog');
      dialog.innerHTML = '<p>Filters open</p><svg data-component="svg" width="100" height="50"><path d="M0 0H100V50H0Z"/></svg>';
      document.body.append(dialog);
      dialog.showModal();
    });
    assert.doesNotMatch(await captureRendered(page, 65536), /data-ticketwatch-map-painted/);
  }));

test('native modal escape from ancestor inertness stays readable but does not become inventory evidence',
  { skip: !enabled, timeout: 30000 }, async () => withPage(async (page, inspect) => {
    await page.setContent(document('<section inert>' + offer
      + '<dialog id="verify"><h2>Verify you are human</h2><button>Continue</button></dialog></section>'));
    await page.locator('#verify').evaluate(element => element.showModal());
    const html = await captureRendered(page, 65536);
    const blocked = await inspect(html);
    assert.equal(blocked.availability, 'blocked');
    assert.deepEqual(blocked.offers, []);
    const state = await page.evaluate(html => {
      const copy = new DOMParser().parseFromString(html, 'text/html');
      return { liveInert: document.querySelector('section').inert,
        shellInert: copy.querySelector('section').hasAttribute('inert'),
        modalHidden: copy.querySelector('#verify').hasAttribute('hidden') };
    }, html);
    assert.deepEqual(state, { liveInert: true, shellInert: false, modalHidden: false });
  }));
