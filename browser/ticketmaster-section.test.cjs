'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { inspectTicketmasterSection, assertSectionInspectionCurrent } = require('./ticketmaster-section.cjs');
const enabled = process.env.TICKET_BROWSER_TESTS === '1';
const target = 'https://www.ticketmaster.cz/event/synthetic-section/1234567890';
const request = { sectionId: 's_128', seatId: 'seat_3' };

function fixture({ name = '110', active = true, duplicate = false, extra = '',
  click = '', tooltipChange = '', stale = false, seatExtra = '', primary = false } = {}) {
  const outline = `<path data-component="svg__section" data-section-id="s_128" data-section-name="${name}" data-active="${active}" d="M20 20L180 20L180 180L20 180Z"/>`;
  const mark = primary ? `<circle data-component="svg__seat" id="seat_3" type="primary" data-seat-name="3" cx="100" cy="100" r="12" ${seatExtra}/>`
    : `<g data-component="svg__seat" id="seat_3" type="resale" data-seat-name="3" transform="translate(100,100)" ${seatExtra}><circle r="12"/></g>`;
  const layers = `<g class="seats"><g data-component="svg__block" data-section-id="s_128" data-section-name="1P-110"><g data-row-name="12">${mark}</g></g></g><g class="polygons"></g>`;
  const tip = '<div data-component="tooltip" aria-hidden="true" style="position:absolute;left:260px;top:40px;background:white"><dl><div><dt>Zóna</dt><dd>1. POSCHODÍ</dd></div><div><dt>Sekce</dt><dd>110</dd></div><div><dt>Řada</dt><dd>12</dd></div><div><dt>Místo</dt><dd>3</dd></div></dl><div data-testid="ticketTypeInfo"><span>Ověřené přeprodávané vstupenky</span><span>Omezený výhled</span><span>2 438,00 Kč za kus</span></div></div>';
  return `<!doctype html><meta charset="utf-8"><style>body{margin:0}aside{position:relative;width:700px;height:400px}dd{margin:0}span{display:block}#minimap{position:absolute;left:220px;top:320px;width:50px;height:50px}</style><body data-section-clicks="0" data-seat-clicks="0" data-reserve-clicks="0"><h1>Synthetic event</h1>
  <aside aria-label="Mapa míst"><button aria-pressed="true">Mapa míst</button><div data-component="zoomer"><svg id="main" data-component="svg" aria-hidden="true" viewBox="0 0 200 200" width="200" height="200"><g class="seats"></g><g class="polygons">${outline}${duplicate ? outline : ''}</g></svg></div>
  <svg id="minimap" data-component="svg" aria-hidden="true" viewBox="0 0 200 200"><g class="polygons">${outline}</g></svg>
  <button data-testid="findTicketsBtn" onclick="reserve()">Najít vstupenky</button><button data-testid="quantityStepper" onclick="reserve()">+</button></aside>${extra}
  <script>
  function reserve(){document.body.dataset.reserveClicks=String(Number(document.body.dataset.reserveClicks)+1)}
  const main=document.getElementById('main');const map=document.querySelector('aside');
  const makeTip=()=>{map.querySelector('[data-component=tooltip]')?.remove();map.insertAdjacentHTML('beforeend',${JSON.stringify(tip)});${tooltipChange}};
  main.querySelector('path').onclick=()=>{document.body.dataset.sectionClicks='1';main.innerHTML=${JSON.stringify(layers)};${click}
    const seat=main.querySelector('[data-component=svg__seat]');seat.onclick=()=>{document.body.dataset.seatClicks='1'};
    seat.onmouseenter=makeTip;seat.onmouseleave=()=>{${stale ? '' : "map.querySelector('[data-component=tooltip]')?.remove()"}};
    ${stale ? 'makeTip();' : ''}
  };
  </script></body>`;
}

async function browserTest(run) {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try { const context = await browser.newContext({ serviceWorkers: 'block' });
    try { await run(context); } finally { await context.close(); }
  } finally { await browser.close(); }
}

async function open(context, body, url = target) {
  const page = await context.newPage();
  await page.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html', body }));
  await page.goto(url);
  return page;
}

async function counters(page) {
  return page.locator('body').evaluate(node => ({ ...node.dataset }));
}

// Draft Netherlands contract replay. The localized strings and region labels
// come from reviewed public NL overview captures; this synthetic interaction
// does not establish that a live Dutch seat quote has been validated.
function dutchFixture(language = 'en', options = {}) {
  const dutch = language === 'nl';
  const event = { eventId: '1234567890', params: '1234567890', marketCode: 'NL',
    canonicalUrl: '/event/synthetic-section/1234567890', stage: 'success', inFlightParams: null };
  const next = { query: { eventId: event.eventId }, props: { pageProps: { initialReduxState: {
    eventId: event.eventId, config: { countryCode: 'NL', currencyCode: 'EUR' }, eventInfo: event,
    ticketSelection: { params: { eventId: event.eventId }, stage: 'success', inFlightParams: null,
      ticketTypes: [{id:'4',title:'Presale | Zitplaats'}] },
    translations: { 'edp.base.seatMap': dutch ? 'Stoelplattegrond' : 'Seat Map',
      'edp.resale.tooltipName': 'Verified Resale Tickets',
      'edp.reserve.seatInfoSec': dutch ? 'Vak' : 'Section',
      'edp.reserve.seatInfoRow': dutch ? 'Rij' : 'Row',
      'edp.reserve.seatInfoSeats': dutch ? 'Stoel' : 'Seat',
      'edp.reserve.seatInfoLevel': dutch ? 'Niveau' : 'Level',
      'edp.lowDemand.ticketsPriceRange': dutch ? '{priceRange} per stuk' : '{priceRange} each' },
  } } } };
  return fixture({ ...options, name: 'Vak110' })
    .replaceAll('Mapa míst', dutch ? 'Stoelplattegrond' : 'Seat Map')
    .replaceAll('Zóna', dutch ? 'Niveau' : 'Level').replaceAll('Sekce', dutch ? 'Vak' : 'Section')
    .replaceAll('Řada', dutch ? 'Rij' : 'Row').replaceAll('Místo', dutch ? 'Stoel' : 'Seat')
    .replaceAll('1. POSCHODÍ', '1e Ring')
    .replaceAll('Ověřené přeprodávané vstupenky', options.primary ? 'Presale | Zitplaats' : 'Verified Resale Tickets')
    .replaceAll('Omezený výhled', dutch ? 'Beperkt Zicht' : 'Obstructed View')
    .replaceAll('2 438,00 Kč za kus', dutch ? '€85,00 per stuk' : '€85.00 each')
    .replace('</body>', `<link rel="canonical" href="https://www.ticketmaster.nl${event.canonicalUrl}"><script id="__NEXT_DATA__" type="application/json">${JSON.stringify(next)}</script></body>`);
}

test('draft NL English and Dutch section/hover contracts keep identity and never select tickets', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const url = 'https://www.ticketmaster.nl/event/synthetic-section/1234567890';
    for (const language of ['en', 'nl']) {
      const page = await open(context, dutchFixture(language), url);
      const result = await inspectTicketmasterSection(page, url, request, 4000);
      assert.equal(result.state, 'section_ready');
      assert.equal(result.inspection.section_name, 'Vak110');
      assert.deepEqual(result.inspection.tooltip, { zone: '1e Ring', section: '110', row: '12', seat: '3',
        ticket_type: 'Verified Resale Tickets', restriction: language === 'en' ? 'Obstructed View' : 'Beperkt Zicht',
        price_text: language === 'en' ? '€85.00 each' : '€85,00 per stuk' });
      await assertSectionInspectionCurrent(page, url, result.inspection);
      assert.deepEqual(await counters(page), { sectionClicks: '1', seatClicks: '0', reserveClicks: '0' });
      await page.close();
    }
  });
});

test('draft NL hover rejects changed seat identity, incomplete price and primary type guesses', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const url = 'https://www.ticketmaster.nl/event/synthetic-section/1234567890';
    for (const change of [
      "map.querySelector('dl div:last-child dd').textContent='999'",
      "map.querySelector('[data-testid=ticketTypeInfo] span:last-child').textContent='85.00 each'",
      "map.querySelector('[data-testid=ticketTypeInfo] span:first-child').textContent='Standard'",
    ]) {
      const page = await open(context, dutchFixture('en', { tooltipChange: change }), url);
      assert.equal((await inspectTicketmasterSection(page, url, request, 800)).state, 'section_tooltip_unavailable');
      assert.deepEqual(await counters(page), { sectionClicks: '1', seatClicks: '0', reserveClicks: '0' });
      await page.close();
    }
    const page = await open(context, dutchFixture('en'), target);
    assert.notEqual((await inspectTicketmasterSection(page, target, request, 800)).state, 'section_ready');
    assert.equal((await counters(page)).sectionClicks, '0');
  });
});

test('NL primary hover requires the explicit primary circle and current configured ticket title', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const url = 'https://www.ticketmaster.nl/event/synthetic-section/1234567890';
    const page = await open(context, dutchFixture('nl', { primary: true,
      tooltipChange: "map.querySelector('[data-testid=ticketTypeInfo] span:nth-child(2)').remove()" }), url);
    const result = await inspectTicketmasterSection(page, url, request, 4000);
    assert.equal(result.state, 'section_ready');
    assert.equal(result.inspection.tooltip.ticket_type, 'Presale | Zitplaats');
    assert.equal(result.inspection.tooltip.restriction, null);
    assert.equal(result.inspection.tooltip.price_text, '€85,00 per stuk');
    await assertSectionInspectionCurrent(page, url, result.inspection);
    assert.deepEqual(await counters(page), { sectionClicks: '1', seatClicks: '0', reserveClicks: '0' });
    await page.close();
    for (const options of [
      { click: "main.querySelector('[data-component=svg__seat]').removeAttribute('type')" },
      { tooltipChange: "map.querySelector('[data-testid=ticketTypeInfo] span:first-child').textContent='Unconfigured ticket'" },
    ]) {
      const other = await open(context, dutchFixture('nl', { primary: true, ...options }), url);
      const result = await inspectTicketmasterSection(other, url, request, 900);
      assert.ok(['section_seat_unavailable', 'section_tooltip_unavailable'].includes(result.state));
      assert.deepEqual(await counters(other), { sectionClicks: '1', seatClicks: '0', reserveClicks: '0' });
      await other.close();
    }
  });
});

test('draft NL section actions require stable matching market, event, currency and locale configuration', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const url = 'https://www.ticketmaster.nl/event/synthetic-section/1234567890';
    for (const mutate of [
      next => { next.query.eventId = '9999999999'; },
      next => { next.props.pageProps.initialReduxState.config.currencyCode = 'CZK'; },
      next => { next.props.pageProps.initialReduxState.translations['edp.reserve.seatInfoSeats'] = 'Unknown'; },
      next => { next.props.pageProps.initialReduxState.translations = {
        'edp.base.seatMap': 'Stoelplattegrond', 'edp.resale.tooltipName': 'Verified Resale Tickets',
        'edp.reserve.seatInfoSec': 'Vak', 'edp.reserve.seatInfoRow': 'Rij',
        'edp.reserve.seatInfoSeats': 'Stoel', 'edp.lowDemand.ticketsPriceRange': '{priceRange} per stuk',
        'edp.reserve.seatInfoLevel': 'Niveau',
      }; },
      next => { next.props.pageProps.initialReduxState.eventInfo.marketCode = 'CZ'; },
    ]) {
      const page = await open(context, dutchFixture('en'), url);
      await page.evaluate(source => {
        const node = document.querySelector('#__NEXT_DATA__');
        const next = JSON.parse(node.textContent); (0,eval)(`(${source})`)(next); node.textContent = JSON.stringify(next);
      }, mutate.toString());
      assert.equal((await inspectTicketmasterSection(page, url, request, 1000)).state, 'section_unsupported');
      assert.deepEqual(await counters(page), { sectionClicks: '0', seatClicks: '0', reserveClicks: '0' });
      await page.close();
    }
    const page = await open(context, dutchFixture('en'), url);
    const result = await inspectTicketmasterSection(page, url, request, 4000);
    assert.equal(result.state, 'section_ready');
    await page.evaluate(() => document.querySelector('#__NEXT_DATA__').remove());
    await assert.rejects(assertSectionInspectionCurrent(page, url, result.inspection), /section_inspection_changed/);
    assert.equal((await counters(page)).seatClicks, '0');
  });
});

test('section inspection rejects unsupported markets, identifiers and invalid budgets without actions', async () => {
  for (const [url, request, budget] of [
    [target, { sectionId: 's_128"]' }, 100],
    [target, { sectionId: 's_128', seatId: {} }, 100],
    ['https://www.ticketmaster.pl/event/example/1234567890', { sectionId: 's_128' }, 100],
    ['invalid', { sectionId: 's_128' }, 100],
    [target, { sectionId: 's_128' }, 0],
  ]) assert.equal((await inspectTicketmasterSection({}, url, request, budget)).state, 'section_unsupported');
});

test('section open and explicit seat hover return a bounded matched quote without any seat or ticket click', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const page = await open(context, fixture({ click: "location.hash='section'" }));
    const guards = [];
    const result = await inspectTicketmasterSection(page, target, request, 4000, async state => guards.push(state));
    assert.equal(result.state, 'section_ready');
    assert.deepEqual(result.inspection, {
      section_id: 's_128', section_name: '110', seat_id: 'seat_3', coverage: 'section_only',
      tooltip: { zone: '1. POSCHODÍ', section: '110', row: '12', seat: '3',
        ticket_type: 'Ověřené přeprodávané vstupenky', restriction: 'Omezený výhled', price_text: '2 438,00 Kč za kus' },
      stock_verified: false, adjacency_verified: false,
    });
    assert.ok(guards.some(x => x.afterMapDisplayAction));
    await assertSectionInspectionCurrent(page, target, result.inspection);
    assert.deepEqual(await counters(page), { sectionClicks: '1', seatClicks: '0', reserveClicks: '0' });
  });
});

test('section-only inspection allows the bare Czech host and retains no quote or seat selection', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const url = target.replace('www.', '');
    const page = await open(context, fixture(), url);
    const result = await inspectTicketmasterSection(page, url, { sectionId: 's_128' }, 2000);
    assert.equal(result.state, 'section_ready');
    assert.equal(result.inspection.seat_id, null);
    assert.equal(result.inspection.tooltip, null);
    await assertSectionInspectionCurrent(page, url, result.inspection);
    assert.equal((await counters(page)).seatClicks, '0');
  });
});

test('late known consent is rejected before section actions without changing ticket controls', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const page = await open(context, fixture());
    let guards = 0;
    const result = await inspectTicketmasterSection(page, target, request, 3000, async () => {
      if (++guards === 3) await page.evaluate(() => {
        const button = document.createElement('button');
        button.id = 'ketch-banner-button-secondary'; button.textContent = 'Odmítnout vše';
        button.style = 'position:fixed;inset:0;z-index:100';
        button.onclick = () => { document.body.dataset.consentClicks = '1'; button.remove(); };
        document.body.append(button);
      });
    });
    assert.equal(result.state, 'section_ready');
    await assertSectionInspectionCurrent(page, target, result.inspection);
    assert.deepEqual(await counters(page), { sectionClicks: '1', seatClicks: '0', reserveClicks: '0', consentClicks: '1' });
  });
});

test('section action refuses standing, inactive, hidden, ambiguous and replaced map targets', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const options of [{ name: 'STÁNÍ' }, { active: false }, { duplicate: true },
      { extra: '<style>#main{display:none}</style>' },
      { extra: '<script>document.querySelector("[data-component=zoomer]").append(document.getElementById("main").cloneNode(true))</script>' },
      { extra: '<script>document.getElementById("main").setAttribute("inert","")</script>' }]) {
      const page = await open(context, fixture(options));
      const result = await inspectTicketmasterSection(page, target, request, 500);
      assert.notEqual(result.state, 'section_ready');
      assert.equal((await counters(page)).sectionClicks, '0');
      await page.close();
    }
    const page = await open(context, fixture());
    let count = 0;
    const result = await inspectTicketmasterSection(page, target, request, 1000, async () => {
      if (++count === 4) await page.locator('#main path').evaluate(node => node.replaceWith(node.cloneNode(true)));
    });
    assert.notEqual(result.state, 'section_ready');
    assert.equal((await counters(page)).sectionClicks, '0');
  });
});

test('hidden, inert, clipped, stale or mismatched tooltip fields cannot produce a quote', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const changes = [
      "map.querySelector('[data-testid=ticketTypeInfo] span:last-child').hidden=true",
      "map.querySelectorAll('dd')[2].setAttribute('inert','')",
      "map.querySelectorAll('dd')[3].textContent='4'",
      "map.querySelector('[data-testid=ticketTypeInfo]').style='height:0;overflow:hidden'",
      "map.querySelector('[data-testid=ticketTypeInfo] span:last-child').innerHTML='<span hidden>2 438,00 Kč za kus</span>'",
      "map.querySelector('[data-testid=ticketTypeInfo]').setAttribute('aria-disabled','true')",
    ];
    for (const tooltipChange of changes) {
      const page = await open(context, fixture({ tooltipChange }));
      const result = await inspectTicketmasterSection(page, target, request, 700);
      assert.equal(result.state, 'section_tooltip_unavailable');
      assert.equal((await counters(page)).seatClicks, '0');
      await page.close();
    }
    const page = await open(context, fixture({ stale: true }));
    assert.equal((await inspectTicketmasterSection(page, target, request, 1000)).state, 'section_tooltip_unavailable');
  });
});

test('post-capture assertion rejects replaced seats, changed quotes and native modal interference', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const change of [
      () => { const node = document.getElementById('seat_3'); node.replaceWith(node.cloneNode(true)); },
      () => { const node = document.getElementById('main'); const replacement = node.cloneNode(false);
        node.replaceWith(replacement); while (node.firstChild) replacement.append(node.firstChild); },
      () => { document.querySelector('[data-testid=ticketTypeInfo] span:last-child').textContent = '1 000,00 Kč za kus'; },
      () => { const modal = document.createElement('dialog'); document.body.append(modal); modal.showModal(); },
    ]) {
      const page = await open(context, fixture());
      const result = await inspectTicketmasterSection(page, target, request, 3000);
      assert.equal(result.state, 'section_ready');
      await page.evaluate(change);
      await assert.rejects(assertSectionInspectionCurrent(page, target, result.inspection), /section_inspection_changed/);
      assert.equal((await counters(page)).seatClicks, '0');
      await page.close();
    }
  });
});

test('section action honors document guards and rejects redirects, challenges and native modals', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const extra of ['<h2>Verify you are human</h2>',
      '<dialog id="modal">Pause</dialog><script>document.getElementById("modal").showModal()</script>']) {
      const page = await open(context, fixture({ extra }));
      const result = await inspectTicketmasterSection(page, target, request, 1000);
      assert.ok(['blocked', 'section_modal_blocked'].includes(result.state));
      assert.equal((await counters(page)).sectionClicks, '0');
      await page.close();
    }
    const page = await open(context, fixture());
    await assert.rejects(inspectTicketmasterSection(page, target, request, 2000, async ({ afterMapDisplayAction }) => {
      if (afterMapDisplayAction) throw new Error('navigation_failed');
    }), /navigation_failed/);
    assert.equal((await counters(page)).seatClicks, '0');
    await page.close();
    const redirected = await open(context, fixture({ click: "history.pushState({},'', '/event/other/9876543210')" }));
    await assert.rejects(inspectTicketmasterSection(redirected, target, request, 2000), /unsafe_redirect/);
  });
});
