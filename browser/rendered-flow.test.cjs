'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const path = require('node:path');
const os = require('node:os');
const { execFileSync } = require('node:child_process');
const { capture } = require('./worker.cjs');

test('rendered offers reach Rust filters and one durable alert preview',
  { skip: process.env.TICKET_BROWSER_TESTS !== '1', timeout: 30000 }, async () => {
    const { chromium } = require('playwright');
    const root = path.resolve(__dirname, '..');
    const binary = process.env.TICKET_WATCH_BIN || path.join(root, 'target/debug/ticket-watch');
    const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'ticket-rendered-flow-'));
    const browser = await chromium.launch({ headless: true,
      ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
    try {
      const context = await browser.newContext({ serviceWorkers: 'block' });
      const offer = (id, extra, quantity, purchase = '<button class="buy">Select</button>') =>
        `<article data-test="offer" data-offer-id="${id}" ${extra}>`
        + `<span class="type">Synthetic Standard</span><span class="section">${id}</span>`
        + `<span class="price">910,80 zł</span>${quantity}${purchase}</article>`;
      const html = '<!doctype html><html><head><meta charset="utf-8"><style>.ghost{display:none}</style></head><body><main data-test="complete">'
        + offer('remaining', '', '<span class="quantity">2</span>')
        + offer('capacity', '', '<select class="quantity"><option>1</option><option>4</option><option disabled>8</option></select>')
        + offer('hidden', 'class="ghost"', '<span class="quantity">99</span>')
        + '<fieldset disabled>' + offer('disabled', '', '<span class="quantity">99</span>') + '</fieldset>'
        + '</main></body></html>';
      let requests = 0;
      await context.route('**/*', route => { requests++; return route.fulfill({ status: 200, contentType: 'text/html', body: html }); });
      const page = await capture(context, { id: 1,
        url: 'https://www.ticketmaster.pl/event/korn-bilety/496543017', timeout_ms: 5000,
        max_body_bytes: 65536, render_wait_ms: 0 });
      assert.equal(page.status, 200);
      const file = path.join(directory, 'rendered.html');
      await fs.writeFile(file, page.html, { mode: 0o600 });
      const args = ['--config', 'examples/offline-demo.toml'];
      const observation = JSON.parse(execFileSync(binary, [...args, 'check', '--watch', 'demo', '--html', file], { cwd: root, encoding: 'utf8' }));
      assert.equal(observation.availability, 'available');
      assert.equal(observation.offers.length, 2);
      const remaining = observation.offers.find(value => value.key === 'id:remaining');
      const capacity = observation.offers.find(value => value.key === 'id:capacity');
      assert.equal(remaining.price_minor, 91080);
      assert.equal(remaining.quantity, 2);
      assert.equal(remaining.quantity_kind, 'remaining');
      assert.equal(remaining.price_scope, 'per_ticket');
      assert.equal(capacity.quantity, 4);
      assert.equal(capacity.quantity_kind, 'selectable');
      const replay = execFileSync(binary, [...args, 'replay', '--watch', 'demo',
        'tests/fixtures/synthetic-unavailable.html', file, file], { cwd: root, encoding: 'utf8' });
      assert.deepEqual([...replay.matchAll(/"new_alerts":\s*(\d+)/g)].map(value => Number(value[1])), [0, 1, 0]);
      assert.match(replay, /Reported remaining for this offer: 2/);
      assert.doesNotMatch(replay, /Selectable order quantity:/);
      assert.equal(requests, 1); // All browser traffic was fulfilled locally.
      await context.close();
    } finally {
      await browser.close();
      await fs.rm(directory, { recursive: true, force: true });
    }
  });
