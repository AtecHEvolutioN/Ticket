'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { execFileSync } = require('node:child_process');
const { capture } = require('./worker.cjs');

test('arbitrary event pages open their display and reach the Rust built-in adapter',
  { skip: process.env.TICKET_BROWSER_TESTS !== '1', timeout: 30000 }, async () => {
    const { chromium } = require('playwright');
    const root = path.resolve(__dirname, '..');
    const binary = process.env.TICKET_WATCH_BIN || path.join(root, 'target/debug/ticket-watch');
    const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'ticket-generic-flow-'));
    const fixture = await fs.readFile(path.join(root, 'tests/fixtures/ticketmaster-regional.html'), 'utf8');
    const body = fixture.match(/<body>([\s\S]*)<\/body>/)[1];
    const html = `<!doctype html><meta charset="utf-8"><button onclick="document.getElementById('offers').hidden=false">See best available</button>
      <div id="offers" hidden>${body}</div><button id="reserve" onclick="document.body.textContent='UNEXPECTED RESERVATION'">Find tickets</button>`;
    const browser = await chromium.launch({ headless: true,
      ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
    try {
      const context = await browser.newContext({ serviceWorkers: 'block' });
      let navigations = 0;
      await context.route('**/*', route => {
        navigations++;
        return route.fulfill({ status: 200, contentType: 'text/html', body: html });
      });
      const urls = ['https://www.ticketmaster.pl/event/jazz-night/111111111',
        'https://www.ticketmaster.cz/event/a-different-show/222222222'];
      for (const [index, url] of urls.entries()) {
        const result = await capture(context, { id: index + 1, url, adapter: 'ticketmaster',
          timeout_ms: 5000, max_body_bytes: 65536, render_wait_ms: 0 });
        assert.equal(result.adapter_state, 'ready');
        assert.doesNotMatch(result.html, /<body[^>]*>UNEXPECTED RESERVATION/);
        const file = path.join(directory, `event-${index}.html`);
        await fs.writeFile(file, result.html);
        const report = JSON.parse(execFileSync(binary, ['inspect', '--url', url, '--html', file],
          { cwd: root, encoding: 'utf8' }));
        assert.equal(report.event_url, url);
        assert.equal(report.observation.availability, 'available');
        assert.equal(report.observation.ticket_categories.length, 4);
        assert.equal(report.observation.offers.length, 2);
        assert.ok(report.observation.offers.every(offer => offer.quantity === 2 && offer.quantity_kind === 'listing'));
        assert.deepEqual(report.observation.offers.map(offer => offer.price_minor).sort((a, b) => a - b), [57500, 91080]);
      }
      assert.equal(navigations, 2); // All traffic was fulfilled locally.
      await context.close();
    } finally {
      await browser.close();
      await fs.rm(directory, { recursive: true, force: true });
    }
  });
