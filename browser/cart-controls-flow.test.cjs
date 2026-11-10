'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const http = require('node:http');
const crypto = require('node:crypto');
const { captureCartControls } = require('./worker.cjs');
const enabled = process.env.TICKET_BROWSER_TESTS === '1';
// A self-contained native-form fixture shared in shape with the module tests;
// every Ticketmaster request is fulfilled locally or rejected below.
const target = 'https://www.ticketmaster.cz/event/synthetic-controls/1234567890';
const eventId = '1234567890';
const prices = [
  '2 361,00 Kč • Sezení 1. kategorie',
  '2 243,00 Kč • Sezení 2. kategorie',
  '2 125,00 Kč • Sezení 4. kategorie',
  '2 125,00 Kč • Sezení 3. kategorie',
  '1 889,00 Kč • Sezení 5. kategorie',
];
const sections = ['1. POSCHODÍ', '4. POSCHODÍ'];

function fixture({ merged = false, initialQuantity = 0, otherQuantity = 0, wrongEvent = false,
  unexpectedStepper = false, duplicatePanel = false, invalidLabel = false,
  disabledSection = false, extra = '', mutate = '' } = {}) {
  const category = (id, title, count) => ({ id, title, quantities: [0, 1, 2, 3, 4, 5, 6],
    selectedQuantity: count, locked: false, membershipLocked: false });
  const state = {
    eventId,
    config: { countryCode: 'CZ', currencyCode: 'CZK' },
    eventInfo: { eventId: wrongEvent ? '9999999999' : eventId, params: eventId,
      stage: 'success', inFlightParams: null, name: 'Synthetic concert',
      canonicalUrl: `/event/synthetic-controls/${eventId}`, marketCode: 'CZ', currencyCode: 'CZK' },
    ticketSelection: { params: { eventId }, stage: 'success', inFlightParams: null,
      ticketTypes: [category('2', 'Sezení', initialQuantity), category('1', 'Stání', otherQuantity),
        ...(merged ? [category('3', 'Sezení', 0)] : [])], eventTicketLimit: 6 },
  };
  const stepper = (id, title, count) => `<li id="row-${id}">
    <div data-testid="ticketTypeInfo"><span>${title}</span> <span>1 889,00 Kč–2 361,00 Kč za kus</span></div>
    <div data-testid="quantityStepper">
      <button type="button" aria-hidden="true" data-direction="minus" ${count === 0 ? 'disabled' : ''}><svg aria-hidden="true"><path d="M0 0h10"/></svg></button>
      <span role="spinbutton" tabindex="0" aria-label="${title} 1 889,00 Kč, aktuální hodnota ${count}" aria-valuemin="0" aria-valuemax="6" aria-valuenow="${count}">${count}</span>
      <button type="button" aria-hidden="true" data-direction="plus"><svg aria-hidden="true"><path d="M0 0h10M5 -5v10"/></svg></button>
      ${unexpectedStepper && id === '2' ? '<button type="button" onclick="reserve()">Unexpected action</button>' : ''}
    </div>
  </li>`;
  return `<!doctype html><html><head><meta charset="utf-8"><title>Synthetic concert</title>
    <link rel="canonical" href="${target}">
    <script id="__NEXT_DATA__" type="application/json">${JSON.stringify({ query: { eventId },
      props: { pageProps: { initialReduxState: state } } })}</script>
    <style>body{margin:20px;font-family:sans-serif}[hidden]{display:none!important}button{min-width:34px;min-height:28px}li{margin:12px}span{padding:4px}fieldset label{display:block;padding:8px}#native-form{max-width:650px}</style>
    </head><body data-step-clicks="0" data-increments="0" data-decrements="0" data-reserve-clicks="0" data-seat-clicks="0" data-resale-clicks="0" data-checkbox-changes="0" data-back-clicks="0" data-panel-opens="0" data-escapes="0" data-submit-calls="0">
    <div id="native-form"><h1>Synthetic concert</h1><form>
      <button type="button" id="price-filter" disabled>Všechny ceny</button>
      <button type="button" id="section-filter" disabled>Všechny sekce</button>
      <ul>${stepper('1', 'Stání', otherQuantity)}${stepper('2', 'Sezení', initialQuantity)}</ul>
      <button type="button" data-testid="findTicketsBtn" onclick="reserve()">Najít vstupenky</button>
      <button type="button" data-component="svg__seat" onclick="bump('seatClicks');reserve()">Seat</button>
      <ul><li role="button" tabindex="0" onclick="bump('resaleClicks');reserve()"><div data-testid="ticketTypeInfo">Ověřená přeprodávaná vstupenka Sezení 2 438,00 Kč za kus</div></li></ul>
    </form></div>
    <div id="panel-container" role="dialog" hidden><button type="button" data-testid="panelBack">Zpět</button>
      <div data-testid="panelPriceSection"></div>
      <button type="button" data-testid="panelClose" hidden>Hotovo</button>
    </div>${extra}
    <script>
    function bump(name){document.body.dataset[name]=String(Number(document.body.dataset[name])+1)}
    function reserve(){bump('reserveClicks');fetch('/event/${eventId}/api/reserve',{method:'POST'}).catch(()=>{})}
    document.querySelector('form').onsubmit=e=>{e.preventDefault();bump('submitCalls');reserve()};
    document.addEventListener('keydown',e=>{if(e.key==='Escape')bump('escapes')});
    const form=document.getElementById('native-form'),container=document.getElementById('panel-container');
    const priceButton=document.getElementById('price-filter'),sectionButton=document.getElementById('section-filter');
    function refresh(){const total=[...document.querySelectorAll('[role=spinbutton]')].reduce((n,s)=>n+Number(s.getAttribute('aria-valuenow')),0);
      priceButton.disabled=total===0;sectionButton.disabled=total===0||${disabledSection};
      for(const spin of document.querySelectorAll('[role=spinbutton]')){const count=Number(spin.getAttribute('aria-valuenow'));
        spin.previousElementSibling.disabled=count===0;spin.nextElementSibling.disabled=total>=6}}
    for(const button of document.querySelectorAll('[data-direction]'))button.onclick=()=>{
      const spin=button.parentElement.querySelector('[role=spinbutton]');
      const count=Number(spin.getAttribute('aria-valuenow'))+(button.dataset.direction==='plus'?1:-1);
      spin.setAttribute('aria-valuenow',String(count));spin.textContent=String(count);
      spin.setAttribute('aria-label',spin.closest('li').id==='row-2'?'Sezení '+count:'Stání '+count);
      bump('stepClicks');bump(button.dataset.direction==='plus'?'increments':'decrements');refresh();${mutate}
    };
    function openPanel(kind){bump('panelOpens');form.hidden=true;form.setAttribute('aria-hidden','true');container.hidden=false;
      const labels=kind==='price'?${JSON.stringify(invalidLabel ? ['x'.repeat(2000)] : prices)}:${JSON.stringify(sections)};
      const panel=container.querySelector('[data-testid=panelPriceSection]');
      panel.innerHTML='<h1>'+(kind==='price'?'Cena':'Sekce')+'</h1><fieldset>'+labels.map((text,index)=>{
        const id='_r_'+kind+'_'+index+'-input';return '<div><label for="'+id+'"><div><input type="checkbox" id="'+id+'"></div><span><div>'+text+'</div></span></label></div>'
      }).join('')+'</fieldset>';
      panel.querySelectorAll('input').forEach(input=>input.onchange=()=>bump('checkboxChanges'));
      ${duplicatePanel ? "container.append(panel.cloneNode(true));" : ''}
    }
    priceButton.onclick=()=>openPanel('price');sectionButton.onclick=()=>openPanel('section');
    function closePanel(){bump('backClicks');container.hidden=true;form.hidden=false;form.removeAttribute('aria-hidden')}
    container.querySelector('[data-testid=panelBack]').onclick=closePanel;
    container.querySelector('[data-testid=panelClose]').onclick=closePanel;
    refresh();
    </script></body></html>`;
}

const request = { id: 1, url: target, timeout_ms: 8000, max_body_bytes: 65536,
  render_wait_ms: 0, adapter: 'ticketmaster', display: 'cart_controls',
  cart_category_id: '2', cart_demo_quantity: 2 };

async function withBrowser(run) {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try { await run(browser); } finally { await browser.close(); }
}

function localFixtureBrowser(browser, body, setup = async () => {}) {
  const state = { context: null, fixtureRequests: [], rejectedAttempts: [], webSockets: 0, stepClicks: 0 };
  const wrapper = { async newContext(options) {
    assert.equal(options.serviceWorkers, 'block');
    const context = await browser.newContext(options);
    state.context = context;
    await context.exposeBinding('reportStep', (_, count) => { state.stepClicks = count; });
    const routeWebSocket = context.routeWebSocket.bind(context);
    context.routeWebSocket = (pattern, handler) => routeWebSocket(pattern, socket => {
      state.webSockets += 1;
      return handler(socket);
    });
    context.on('request', outgoing => {
      if (outgoing.url().includes('/after-change')) state.rejectedAttempts.push(outgoing.url());
    });
    await context.route('**/*', async route => {
      const outgoing = route.request();
      state.fixtureRequests.push(outgoing.url());
      if (outgoing.url() === target && outgoing.isNavigationRequest())
        return route.fulfill({ status: 200, contentType: 'text/html', body });
      // No real provider endpoint can be contacted, even if a regression bypasses
      // the worker's fence. Reaching this fallback is itself a test failure.
      return route.fulfill({ status: 204 });
    });
    await setup(context, state);
    return context;
  } };
  return { wrapper, state };
}

function assertNoPurchases(result) {
  assert.equal(result.cart_controls.reservation_attempted, false);
  assert.equal(result.cart_controls.hold_verified, false);
  assert.equal(result.cart_controls.network_disabled_before_changes, true);
  assert.equal(result.cart_controls.form_restored, true);
  for (const name of ['reserve-clicks', 'seat-clicks', 'resale-clicks', 'checkbox-changes', 'submit-calls'])
    assert.match(result.html, new RegExp(`data-${name}="0"`));
}

test('dedicated controls fence rejects attempted HTTP mutations and leaves shared context online',
  { skip: !enabled, timeout: 20000 }, async () => {
    await withBrowser(async browser => {
      const shared = await browser.newContext();
      await shared.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html', body: '<h1>Shared online</h1>' }));
      const sharedPage = await shared.newPage();
      await sharedPage.goto('https://www.ticketmaster.cz/event/shared/9999999999');
      const { wrapper, state } = localFixtureBrowser(browser, fixture({ mutate: `
        reportStep(Number(document.body.dataset.stepClicks));
        fetch('/after-change-fetch',{method:'POST',body:'demo'}).catch(()=>{});
        const xhr=new XMLHttpRequest();xhr.open('POST','/after-change-xhr');xhr.send('demo');
        navigator.sendBeacon('/after-change-beacon','demo');
        const image=new Image();image.src='/after-change-image';
      ` }));
      const result = await captureCartControls(wrapper, request);
      assert.equal(result.adapter_reason, 'cart_controls_ready');
      assertNoPurchases(result);
      assert.equal(state.stepClicks, 4, 'two increments and two restoration decrements');
      assert.ok(state.rejectedAttempts.length >= 4, 'fixture actually attempted HTTP after controls changed');
      assert.deepEqual(state.fixtureRequests, [target], 'no attempted mutation reached the lower network route');
      assert.equal(state.context.pages().length, 0);
      assert.deepEqual(browser.contexts(), [shared], 'dedicated context is destroyed, shared context retained');
      const online = await sharedPage.evaluate(() => navigator.onLine);
      assert.equal(online, true);
      const resultText = await sharedPage.evaluate(() => fetch('/still-online').then(r => r.text()));
      assert.match(resultText, /Shared online/);
      await shared.close();
    });
  });

test('WebSocket connections are intercepted before handshake and dedicated contexts cannot leak',
  { skip: !enabled, timeout: 20000 }, async () => {
    const sockets = new Set();
    let handshakes = 0;
    const server = http.createServer((_, response) => response.end('local test'));
    server.on('connection', socket => { sockets.add(socket); socket.on('close', () => sockets.delete(socket)); });
    server.on('upgrade', (incoming, socket) => {
      handshakes += 1;
      const accept = crypto.createHash('sha1').update(incoming.headers['sec-websocket-key'] + '258EAFA5-E914-47DA-95CA-C5AB0DC85B11').digest('base64');
      socket.write(`HTTP/1.1 101 Switching Protocols\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Accept: ${accept}\r\n\r\n`);
    });
    await new Promise(resolve => server.listen(0, '127.0.0.1', resolve));
    const origin = `http://127.0.0.1:${server.address().port}`;
    const endpoint = `${origin.replace('http:', 'ws:')}/socket`;
    try {
      await withBrowser(async browser => {
        // Positive control proves the local server accepts a normal handshake.
        const shared = await browser.newContext();
        const page = await shared.newPage();
        await page.goto(origin);
        await page.evaluate(endpoint => new Promise((resolve, reject) => {
          const socket = new WebSocket(endpoint);
          socket.onopen = () => { window.testSocket = socket; resolve(); };
          socket.onerror = () => reject(new Error('baseline socket failed'));
          setTimeout(() => reject(new Error('baseline socket timeout')), 3000);
        }), endpoint);
        assert.equal(handshakes, 1);
        const { wrapper, state } = localFixtureBrowser(browser, fixture({ extra: `<script>window.earlySocket=new WebSocket(${JSON.stringify(endpoint)});</script>` }));
        const result = await captureCartControls(wrapper, request);
        assertNoPurchases(result);
        assert.equal(state.webSockets, 1, 'worker intercepted the startup WebSocket attempt');
        assert.equal(handshakes, 1, 'the dedicated capture sent no server handshake');
        assert.deepEqual(browser.contexts(), [shared]);
        await shared.close();
      });
    } finally {
      for (const socket of sockets) socket.destroy();
      await new Promise(resolve => server.close(resolve));
    }
  });

test('a transient popup caused by a demo control fails closed and destroys the context',
  { skip: !enabled, timeout: 15000 }, async () => {
    await withBrowser(async browser => {
      const { wrapper, state } = localFixtureBrowser(browser, fixture({ mutate: `
        reportStep(Number(document.body.dataset.stepClicks));
        const child=window.open('about:blank');if(child)child.close();
      ` }));
      await assert.rejects(captureCartControls(wrapper, request), /navigation_failed|navigation_timeout/);
      assert.ok(state.stepClicks <= 1, 'no further controls may change after a popup');
      assert.deepEqual(state.fixtureRequests, [target]);
      assert.equal(state.context.pages().length, 0);
      assert.equal(browser.contexts().length, 0);
    });
  });

test('an initial stalled request is cancelled before any demo control changes',
  { skip: !enabled, timeout: 15000 }, async () => {
    await withBrowser(async browser => {
      let release;
      const stalled = new Promise(resolve => { release = resolve; });
      let attempted = false, settled = false, changedBeforeSettlement = false;
      const { wrapper, state } = localFixtureBrowser(browser,
        fixture({ extra: `<script>fetch('/never-finishes').catch(()=>{});</script>`,
          mutate: 'checkSettlement();reportStep(Number(document.body.dataset.stepClicks));' }),
        async context => {
          await context.exposeBinding('checkSettlement', () => { if (!settled) changedBeforeSettlement = true; });
          context.on('requestfailed', outgoing => { if (outgoing.url().endsWith('/never-finishes')) settled = true; });
          await context.route('**/never-finishes', async route => {
            attempted = true;
            await stalled;
            await route.abort().catch(() => {});
          });
        });
      try {
        const result = await captureCartControls(wrapper, request);
        assertNoPurchases(result);
        assert.equal(attempted, true);
        assert.equal(settled, true);
        assert.equal(changedBeforeSettlement, false);
        assert.equal(state.stepClicks, 4);
        assert.equal(state.context.pages().length, 0);
        assert.equal(browser.contexts().length, 0);
      } finally { release(); }
    });
  });

test('a missing initial-request completion fails closed before controls change',
  { skip: !enabled, timeout: 15000 }, async () => {
    await withBrowser(async browser => {
      let release;
      const stalled = new Promise(resolve => { release = resolve; });
      const { wrapper, state } = localFixtureBrowser(browser,
        fixture({ extra: `<script>fetch('/lost-completion').catch(()=>{});</script>`,
          mutate: 'reportStep(Number(document.body.dataset.stepClicks));' }),
        async context => {
          await context.route('**/lost-completion', async route => {
            await stalled;
            await route.abort().catch(() => {});
          });
          // Model a completion notification that never arrives. The worker must
          // reject an undrainable tracker even though this test browser can stop
          // its underlying request; it must not infer that the fence settled.
          const on = context.on.bind(context);
          context.on = (name, listener) => on(name,
            ['requestfailed', 'requestfinished'].includes(name)
              ? outgoing => { if (!outgoing.url().endsWith('/lost-completion')) listener(outgoing); }
              : listener);
        });
      try {
        await assert.rejects(captureCartControls(wrapper, request), /navigation_failed|navigation_timeout/);
        assert.equal(state.stepClicks, 0);
        assert.equal(state.context.pages().length, 0);
        assert.equal(browser.contexts().length, 0);
      } finally { release(); }
    });
  });

test('known best-available and form actions can rebind only their own fragment changes',
  { skip: !enabled, timeout: 15000 }, async () => {
    await withBrowser(async browser => {
      const body = fixture({ mutate: "location.hash='quantity-'+count;" })
        .replace('<div id="native-form">', `<button type="button" id="show-form" onclick="document.getElementById('native-form').hidden=false;location.hash='display-open';this.remove()">See best available</button><div id="native-form" hidden>`)
        .replace("function openPanel(kind){bump('panelOpens');", "function openPanel(kind){location.hash=kind+'-panel';bump('panelOpens');")
        .replace("function closePanel(){bump('backClicks');", "function closePanel(){location.hash='panel-closed';bump('backClicks');");
      const { wrapper } = localFixtureBrowser(browser, body);
      const result = await captureCartControls(wrapper, request);
      assert.equal(result.adapter_reason, 'cart_controls_ready');
      assertNoPurchases(result);
      assert.match(result.html, /data-panel-opens="2"/);
    });
  });

test('an unrelated fragment change after inspection invalidates the final captured evidence',
  { skip: !enabled, timeout: 15000 }, async () => {
    await withBrowser(async browser => {
      let changed = false;
      const { wrapper, state } = localFixtureBrowser(browser, fixture(), async context => {
        context.on('page', page => {
          const evaluate = page.evaluate.bind(page);
          page.evaluate = async (fn, ...args) => {
            const result = await evaluate(fn, ...args);
            if (String(fn).includes('const copy = root.cloneNode(true)')) {
              await evaluate(() => { location.hash = 'unrelated-late-change'; });
              changed = true;
            }
            return result;
          };
        });
      });
      await assert.rejects(captureCartControls(wrapper, request), /navigation_failed|navigation_timeout/);
      assert.equal(changed, true);
      assert.equal(state.context.pages().length, 0);
      assert.equal(browser.contexts().length, 0);
    });
  });
