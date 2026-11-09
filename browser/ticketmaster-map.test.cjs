'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { capture, validateRequest } = require('./worker.cjs');
const { prepareTicketmasterMap } = require('./ticketmaster-map.cjs');
const enabled = process.env.TICKET_BROWSER_TESTS === '1';
const base = { id: 1, timeout_ms: 6000, render_wait_ms: 0, max_body_bytes: 65536,
  adapter: 'ticketmaster', display: 'map' };
const target = 'https://www.ticketmaster.cz/event/synthetic-map/1234567890';
const svg = '<svg data-component="svg" aria-hidden="true" viewBox="0 0 100 100" width="200" height="200"><g class="polygons"><path data-component="svg__section" data-section-id="s_1" data-section-name="101" d="M0 0L50 0L50 50Z"/></g><text>101</text></svg>';

function fixture({ label = 'Mapa míst', region = 'Mapa míst', selected = false,
  effect = "location.hash='map';document.getElementById('map').innerHTML=geometry", duplicate = false,
  initial = '', extra = '' } = {}) {
  return `<!doctype html><meta charset="utf-8"><body data-map-clicks="0" data-best-clicks="0" data-reserve-clicks="0">
    <h1>Synthetic map event</h1><aside aria-label="${region}">
    <button id="map-tab" aria-pressed="${selected}">${label}</button>
    ${duplicate ? `<button aria-pressed="false">${label}</button>` : ''}
    <button id="best-tab">Nejlepší dostupné</button><div id="map">${initial}</div>
    <button data-testid="findTicketsBtn" onclick="reserve()">Najít vstupenky</button>
    <button data-testid="quantityStepper" onclick="reserve()">+</button>
    <button aria-pressed="false" hidden onclick="reserve()">${label}</button>
    <button aria-pressed="false" disabled onclick="reserve()">${label}</button>
    </aside>${extra}<script>
    const geometry=${JSON.stringify(svg)};
    function reserve(){document.body.dataset.reserveClicks=String(Number(document.body.dataset.reserveClicks)+1)}
    document.getElementById('best-tab').onclick=()=>{document.body.dataset.bestClicks='1';document.getElementById('map').innerHTML='<div data-testid="ticketTypeInfo">Category</div>'};
    document.getElementById('map-tab').onclick=()=>{
      document.body.dataset.mapClicks=String(Number(document.body.dataset.mapClicks)+1);
      document.getElementById('map-tab').setAttribute('aria-pressed','true');${effect};
    };
    </script></body>`;
}

async function browserTest(run) {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    try { await run(context); } finally { await context.close(); }
  } finally { await browser.close(); }
}

test('map display opt-in rejects arbitrary actions and requires the Ticketmaster event adapter', () => {
  assert.doesNotThrow(() => validateRequest({ ...base, url: target }));
  for (const display of ['checkout', 'reserve', true, null, {}]) {
    assert.throws(() => validateRequest({ ...base, url: target, display }), /invalid_request/);
  }
  assert.throws(() => validateRequest({ ...base, url: target, adapter: 'selectors' }), /invalid_request/);
  assert.throws(() => validateRequest({ ...base, url: target, adapter: undefined }), /invalid_request/);
  assert.throws(() => validateRequest({ ...base, url: 'https://www.ticketmaster.cz/artist/1234567890' }), /invalid_request/);
});

test('known map displays render geometry and rebind only their fragment without clicking ticket controls', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const cases = [['Mapa míst', 'Mapa míst'], ['Zobacz miejsca na mapie', 'Mapa Obiektu'],
      ['View seat map', 'Seat Map'], ['Ver mapa de asientos', 'Mapa de Asientos'], ['Bekijk plattegrond', 'Stoelplattegrond']];
    let current;
    await context.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html', body: current }));
    for (const [index, [label, region]] of cases.entries()) {
      current = fixture({ label, region });
      const result = await capture(context, { ...base, id: index + 1, url: target });
      assert.equal(result.adapter_state, 'ready', result.adapter_reason);
      assert.equal(result.adapter_reason, 'map_svg');
      assert.match(result.html, /data-map-clicks="1" data-best-clicks="0" data-reserve-clicks="0"/);
      assert.equal(result.final_url, target);
      assert.equal(context.pages().length, 0);
    }
  });
});

test('an already rendered graphical SVG is preserved without switching to best available', { skip: !enabled }, async () => {
  await browserTest(async context => {
    await context.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html',
      body: fixture({ selected: true, initial: svg }) }));
    const result = await capture(context, { ...base, url: target });
    assert.equal(result.adapter_reason, 'map_svg');
    assert.match(result.html, /data-map-clicks="0" data-best-clicks="0" data-reserve-clicks="0"/);
  });
});

test('a same-label control in another landmark cannot replace the inspected map action', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const body = fixture({ extra: '<aside aria-label="Unrelated"><button aria-pressed="false" onclick="reserve()">Mapa míst</button></aside>' });
    await context.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html', body }));
    const result = await capture(context, { ...base, url: target });
    assert.equal(result.adapter_reason, 'map_svg');
    assert.match(result.html, /data-map-clicks="1" data-best-clicks="0" data-reserve-clicks="0"/);

    const page = await context.newPage();
    await page.goto(target);
    const evaluate = page.evaluate.bind(page);
    let replaced = false;
    page.evaluate = async (fn, args) => {
      const value = await evaluate(fn, args);
      if (!replaced && value?.mapLabel === 'Mapa míst' && value?.controlCount === 1) {
        replaced = true;
        await evaluate(() => document.querySelector('aside[aria-label="Mapa míst"]').remove());
      }
      return value;
    };
    const prepared = await prepareTicketmasterMap(page, target, 500);
    assert.equal(prepared.state, 'view_control_blocked');
    assert.equal(await page.locator('body').getAttribute('data-reserve-clicks'), '0');
  });
});

test('map mode rejects ambiguous controls, challenges, native modals and unexpected redirects', { skip: !enabled }, async () => {
  await browserTest(async context => {
    let current;
    await context.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html', body: current }));
    for (const [options, expectedState, reason] of [
      [{ duplicate: true }, 'unsupported', 'map_ambiguous'],
      [{ extra: '<h2>Your Browsing Activity Has Been Paused</h2>' }, 'blocked', 'manual_intervention'],
      [{ extra: '<dialog id="modal">Review selection</dialog><script>document.getElementById("modal").showModal()</script>' }, 'blocked', 'map_modal_blocked'],
    ]) {
      current = fixture(options);
      const result = await capture(context, { ...base, url: target });
      assert.equal(result.adapter_state, expectedState);
      assert.equal(result.adapter_reason, reason);
      assert.match(result.html, /data-map-clicks="0" data-best-clicks="0" data-reserve-clicks="0"/);
    }
    current = fixture({ effect: "location.href='https://www.ticketmaster.cz/event/different/9876543210'" });
    await assert.rejects(capture(context, { ...base, url: target }), /unsafe_redirect/);
    assert.equal(context.pages().length, 0);
  });
});

test('map display cannot rebind a new document or a changed query, and default capture keeps its strict guard', { skip: !enabled }, async () => {
  await browserTest(async context => {
    let current;
    let documents = 0;
    await context.route('**/*', route => {
      documents++;
      return route.fulfill({ status: 200, contentType: 'text/html', body: documents === 1 ? current : fixture({ selected: true, initial: svg }) });
    });
    current = fixture({ effect: 'location.reload()' });
    await assert.rejects(capture(context, { ...base, url: target }), /navigation_failed/);
    documents = 0;
    current = fixture({ effect: "history.pushState({},'', '?language=en');document.getElementById('map').innerHTML=geometry" });
    await assert.rejects(capture(context, { ...base, url: target }), /unsafe_redirect/);
    documents = 0;
    current = fixture().replace("document.body.dataset.bestClicks='1';", "document.body.dataset.bestClicks='1';location.hash='map';");
    await assert.rejects(capture(context, { ...base, display: undefined, url: target }), /navigation_failed/);
    assert.equal(context.pages().length, 0);
  });
});

test('bounded map surface diagnostics never assert seat availability and readiness changes are guarded', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const page = await context.newPage();
    await context.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html', body: fixture({ selected: true }) }));
    await page.goto(target);
    let calls = 0;
    assert.deepEqual(await prepareTicketmasterMap(page, target, 100, async ({ afterMapDisplayAction }) => {
      calls++; assert.equal(afterMapDisplayAction, false);
    }), { state: 'map_unavailable', opened: false });
    assert.ok(calls >= 2);
    for (const [surface, expected] of [
      ['<canvas width="100" height="100"></canvas>', 'map_canvas'],
      ['<iframe title="Map" style="width:100px;height:100px"></iframe>', 'map_iframe'],
    ]) {
      await page.locator('#map').evaluate((node, surface) => { node.innerHTML = surface; }, surface);
      assert.deepEqual(await prepareTicketmasterMap(page, target, 100), { state: expected, opened: false });
    }
  });
});
