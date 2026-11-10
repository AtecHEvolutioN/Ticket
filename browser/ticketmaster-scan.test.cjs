'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { scanTicketmasterSection, assertSectionScanCurrent } = require('./ticketmaster-section.cjs');
const enabled = process.env.TICKET_BROWSER_TESTS === '1';
const target = 'https://www.ticketmaster.cz/event/synthetic-scan/1234567890';

function fixture({ seats = [
  { id: 'seat_b', number: '2', x: 160, price: '2 000,00' },
  { id: 'seat_a', number: '1', x: 80, price: '1 800,00' },
  { id: 'seat_c', number: '3', x: 240, price: '2 100,00' },
], hover = '', leave = '', open = '', extra = '' } = {}) {
  const layers = `<g class="seats"><g data-component="svg__block" data-section-id="s_128" data-section-name="1P-110"><g data-row-name="12">${seats.map(s => `<g data-component="svg__seat" type="resale" id="${s.id}" data-seat-name="${s.number}" transform="translate(${s.x},100)" ${s.hidden ? 'hidden' : ''}><circle r="12" /></g>`).join('')}</g></g></g><g class="polygons"></g>`;
  return `<!doctype html><meta charset="utf-8"><style>body{margin:0}aside{position:relative;width:1100px;height:450px}dd{margin:0}span{display:block}[data-component=zoomer]{width:600px;height:300px;overflow:hidden}</style>
  <body data-section-clicks="0" data-seat-clicks="0" data-ticket-clicks="0"><h1>Synthetic event</h1><aside aria-label="Mapa míst"><button aria-pressed="true">Mapa míst</button>
  <div data-component="zoomer"><svg id="main" data-component="svg" aria-hidden="true" width="600" height="300" viewBox="0 0 600 300"><g class="seats"></g><g class="polygons"><path data-component="svg__section" data-section-id="s_128" data-section-name="110" data-active="true" d="M20 20L580 20L580 280L20 280Z"/></g></svg></div>
  <button data-testid="findTicketsBtn" onclick="document.body.dataset.ticketClicks='1'">Najít vstupenky</button><button onclick="document.body.dataset.ticketClicks='1'">+</button></aside>${extra}
  <script>
  const main=document.getElementById('main'),map=document.querySelector('aside');
  const seats=${JSON.stringify(seats)};
  const clear=()=>map.querySelector('[data-component=tooltip]')?.remove();
  function show(s){clear();if(s.noTooltip)return;map.insertAdjacentHTML('beforeend', '<div data-component="tooltip" aria-hidden="true" style="position:absolute;left:650px;top:40px;background:white"><dl><div><dt>Zóna</dt><dd>1. POSCHODÍ</dd></div><div><dt>Sekce</dt><dd>110</dd></div><div><dt>Řada</dt><dd>12</dd></div><div><dt>Místo</dt><dd>'+s.number+'</dd></div></dl><div data-testid="ticketTypeInfo"><span>Ověřené přeprodávané vstupenky</span><span>'+s.price+' Kč za kus</span></div></div>')}
  main.querySelector('path').onclick=()=>{document.body.dataset.sectionClicks=String(Number(document.body.dataset.sectionClicks)+1);main.innerHTML=${JSON.stringify(layers)};${open}
    for(const s of seats){const seat=document.getElementById(s.id);seat.onclick=()=>document.body.dataset.seatClicks='1';seat.onmouseenter=()=>{document.body.dataset.hoverSeat=s.id;show(s);${hover}};seat.onmouseleave=()=>{clear();${leave}};}
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

async function open(context, body) {
  const page = await context.newPage();
  await page.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html', body }));
  await page.goto(target);
  return page;
}

test('scan validates limits before interacting with a page', async () => {
  for (const limit of [0, 51, 1.5, NaN, '2', undefined]) {
    assert.equal((await scanTicketmasterSection({}, target, { sectionId: 's_128', limit }, 1000)).state, 'scan_unsupported');
  }
});

test('one section scan discovers sorted seats and keeps individually observed prices without selecting tickets', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const page = await open(context, fixture());
    const result = await scanTicketmasterSection(page, target, { sectionId: 's_128', limit: 20 }, 5000);
    assert.equal(result.state, 'scan_ready');
    const scan = result.scan;
    assert.equal(scan.candidate_count, 3);
    assert.equal(scan.visible_candidate_count, 3);
    assert.deepEqual(scan.results.map(r => [r.seat_id, r.tooltip.price_text]), [
      ['seat_a', '1 800,00 Kč za kus'], ['seat_b', '2 000,00 Kč za kus'], ['seat_c', '2 100,00 Kč za kus'],
    ]);
    assert.equal(scan.stop_reason, 'visible_candidates_exhausted');
    assert.equal(scan.coverage, 'partial_rendered_section');
    assert.equal(scan.stock_verified, false);
    assert.equal(scan.adjacency_verified, false);
    let time = scan.started_at_ms;
    for (const row of scan.results) { assert.ok(row.observed_at_ms >= time); time = row.observed_at_ms; }
    assert.ok(scan.finished_at_ms >= time);
    // Only the last card remains visible; earlier individually checked prices
    // remain historical observations rather than an atomic current snapshot.
    await assertSectionScanCurrent(page, target, scan);
    assert.deepEqual(await page.locator('body').evaluate(n => ({ ...n.dataset })),
      { sectionClicks: '1', seatClicks: '0', ticketClicks: '0', hoverSeat: 'seat_c' });
  });
});

test('scan limits and offscreen or hidden candidates retain explicit partial coverage', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const page = await open(context, fixture({ seats: [
      { id: 'seat_c', number: '3', x: 240, price: '2 100,00' },
      { id: 'seat_a', number: '1', x: 80, price: '1 800,00' },
      { id: 'seat_b', number: '2', x: 160, price: '2 000,00', hidden: true },
      { id: 'seat_z', number: '4', x: 2000, price: '2 200,00' },
    ] }));
    const result = await scanTicketmasterSection(page, target, { sectionId: 's_128', limit: 1 }, 3000);
    assert.equal(result.state, 'scan_ready');
    assert.equal(result.scan.candidate_count, 4);
    assert.equal(result.scan.visible_candidate_count, 2);
    assert.equal(result.scan.stop_reason, 'seat_limit');
    assert.deepEqual(result.scan.results.map(r => r.seat_id), ['seat_a']);
    await assertSectionScanCurrent(page, target, result.scan);
  });
});

test('a rendered section without supported resale candidates remains partial and unknown', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const page = await open(context, fixture({ open: "main.querySelectorAll('[type=resale]').forEach(n=>n.removeAttribute('type'))" }));
    const result = await scanTicketmasterSection(page, target, { sectionId: 's_128', limit: 20 }, 3000);
    assert.equal(result.state, 'scan_ready');
    assert.equal(result.scan.candidate_count, 0);
    assert.equal(result.scan.visible_candidate_count, 0);
    assert.equal(result.scan.stop_reason, 'visible_candidates_exhausted');
    assert.deepEqual(result.scan.results, []);
    assert.equal(result.scan.stock_verified, false);
    await assertSectionScanCurrent(page, target, result.scan);
  });
});

test('missing cards are unknown outcomes and a deadline leaves other candidates unattempted', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const [budget, expectedStop, expectedCount] of [[4000, 'visible_candidates_exhausted', 2], [650, 'time_budget', 1]]) {
      const page = await open(context, fixture({ seats: [
        { id: 'seat_a', number: '1', x: 80, price: '1 800,00', noTooltip: true },
        { id: 'seat_b', number: '2', x: 160, price: '2 000,00' },
      ] }));
      const result = await scanTicketmasterSection(page, target, { sectionId: 's_128', limit: 20 }, budget);
      assert.equal(result.state, 'scan_ready');
      assert.equal(result.scan.results.length, expectedCount);
      assert.equal(result.scan.stop_reason, expectedStop);
      assert.equal(result.scan.results[0].outcome, 'tooltip_unavailable');
      assert.equal(result.scan.results[0].tooltip, null);
      await assertSectionScanCurrent(page, target, result.scan);
      await page.close();
    }
  });
});

test('duplicate or malformed candidate identities cannot be scanned', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const change of [
      "document.getElementById('seat_b').id='seat_a'",
      "document.getElementById('seat_a').id='bad id'",
      "document.getElementById('seat_a').parentElement.removeAttribute('data-row-name')",
    ]) {
      const page = await open(context, fixture({ open: change }));
      const result = await scanTicketmasterSection(page, target, { sectionId: 's_128', limit: 20 }, 3000);
      assert.equal(result.state, 'scan_candidate_invalid');
      assert.equal(await page.locator('body').getAttribute('data-seat-clicks'), '0');
      await page.close();
    }
  });
});

test('changing a prior seat, candidate set or document during the scan rejects the whole batch', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const hover of [
      "if(s.id==='seat_b'){const old=document.getElementById('seat_a');old.replaceWith(old.cloneNode(true));}",
      "if(s.id==='seat_b')document.getElementById('seat_a').setAttribute('data-seat-name','8')",
      "if(s.id==='seat_b')document.getElementById('seat_a').remove()",
      "if(s.id==='seat_b'){const replacement=main.cloneNode(false);main.replaceWith(replacement);while(main.firstChild)replacement.append(main.firstChild);}",
      "if(s.id==='seat_b')history.pushState({},'', '/event/wrong/9876543210')",
      "if(s.id==='seat_b'){const d=document.createElement('dialog');document.body.append(d);d.showModal();}",
    ]) {
      const page = await open(context, fixture({ hover }));
      await assert.rejects(scanTicketmasterSection(page, target, { sectionId: 's_128', limit: 20 }, 5000), /scan_changed|unsafe_redirect/);
      assert.equal(await page.locator('body').getAttribute('data-seat-clicks'), '0');
      await page.close();
    }
  });
});

test('changed hover values across validation and replaced nodes after capture invalidate evidence', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const page = await open(context, fixture());
    const result = await scanTicketmasterSection(page, target, { sectionId: 's_128', limit: 20 }, 4000);
    assert.equal(result.state, 'scan_ready');
    await page.locator('#seat_a').evaluate(node => node.replaceWith(node.cloneNode(true)));
    await assert.rejects(assertSectionScanCurrent(page, target, result.scan), /scan_changed/);
    await page.close();
    const changed = await open(context, fixture());
    let hoverGuards = 0;
    await assert.rejects(scanTicketmasterSection(changed, target, { sectionId: 's_128', limit: 20 }, 4000, async () => {
      if (await changed.locator('body').getAttribute('data-hover-seat') === 'seat_a' && ++hoverGuards === 2) {
        await changed.locator('[data-testid=ticketTypeInfo] span:last-child').evaluate(n => n.textContent = '1 900,00 Kč za kus');
      }
    }), /scan_changed/);
  });
});

test('post-capture scan guard rejects a replaced SVG shell retaining the original seat nodes', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const page = await open(context, fixture());
    const result = await scanTicketmasterSection(page, target, { sectionId: 's_128', limit: 20 }, 4000);
    assert.equal(result.state, 'scan_ready');
    await page.locator('#main').evaluate(node => {
      const replacement = node.cloneNode(false); node.replaceWith(replacement);
      while (node.firstChild) replacement.append(node.firstChild);
    });
    await assert.rejects(assertSectionScanCurrent(page, target, result.scan), /scan_changed/);
  });
});

test('a later guard does not advance an already validated quote timestamp', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const page = await open(context, fixture({ seats: [{ id: 'seat_a', number: '1', x: 80, price: '1 800,00' }] }));
    let hoverGuards = 0;
    const result = await scanTicketmasterSection(page, target, { sectionId: 's_128', limit: 20 }, 4000, async () => {
      if (await page.locator('body').getAttribute('data-hover-seat') === 'seat_a' && ++hoverGuards === 3) {
        await page.waitForTimeout(120);
      }
    });
    assert.equal(result.state, 'scan_ready');
    assert.ok(result.scan.finished_at_ms - result.scan.results[0].observed_at_ms >= 100);
    await assertSectionScanCurrent(page, target, result.scan);
  });
});
