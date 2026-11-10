'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { validateUrl, validateRequest, retryAfter, identificationRetryAllowed, proxyOptions, capture, captureCartControls } = require('./worker.cjs');
const { captureRendered, sameEvent, challengeText } = require('./rendered.cjs');
const { EventEmitter } = require('node:events');

function errorOnlyContext(page) {
  const session = new EventEmitter();
  session.send = async () => ({ frameTree: { frame: { id: 'main' } } });
  session.detach = async () => {};
  return { newPage: async () => page, newCDPSession: async () => session };
}

test('watch targets exclude payment sessions and foreign hosts', () => {
  assert.equal(validateUrl('https://www.ticketmaster.pl/event/korn/496543017').hostname, 'www.ticketmaster.pl');
  for (const target of [
    'https://secure.ticketmaster.pl/payment/?t=secret',
    'https://www.ticketmaster.pl.evil.test/event/1',
    'https://www.ticketmaster.pl/event/1?token=secret',
    'https://user:secret@www.ticketmaster.pl/event/1',
    'http://www.ticketmaster.pl/event/1',
  ]) assert.throws(() => validateUrl(target));
});

test('Retry-After supports seconds and HTTP dates', () => {
  assert.equal(retryAfter('120', 0), 120);
  assert.equal(retryAfter('Thu, 01 Jan 1970 00:02:00 GMT', 30000), 90);
  assert.equal(retryAfter('bad'), null);
});

test('identification waiting requires an absent or strictly expired Retry-After value', () => {
  assert.equal(identificationRetryAllowed({}, 120000), true);
  for (const value of ['0', '00', 'Thu, 01 Jan 1970 00:02:00 GMT']) {
    assert.equal(identificationRetryAllowed({ 'retry-after': value }, 120000), true, value);
  }
  for (const value of ['', '1', '0, 1', '0, 12', '0, 120', '-1', '+1', '0.5', 'invalid',
    'Thu, 01 Jan 1970 00:02:01 GMT', 'Fri, 01 Jan 1970 00:02:00 GMT', 'Fri, 31 Feb 2023 00:00:00 GMT']) {
    assert.equal(identificationRetryAllowed({ 'retry-after': value }, 120000), false, value);
  }
});

test('proxy configuration preserves inherited server, credentials, and bypass', () => {
  assert.deepEqual(proxyOptions({HTTPS_PROXY: 'http://user:password@proxy.test:8080', NO_PROXY:'localhost'}),
    {server:'http://proxy.test:8080',username:'user',password:'password',bypass:'localhost'});
});

test('request protocol has bounded sizes and durations', () => {
  const request = {id:1,url:'https://www.ticketmaster.pl/event/1',timeout_ms:1000,max_body_bytes:1024,render_wait_ms:0};
  assert.doesNotThrow(() => validateRequest(request));
  assert.throws(() => validateRequest({...request,max_body_bytes:Infinity}));
  assert.throws(() => validateRequest({...request,timeout_ms:0}));
});

test('section inspection requires explicit bounded Czech map targets', () => {
  const request = { id: 1, url: 'https://www.ticketmaster.cz/event/synthetic/12345678',
    timeout_ms: 1000, max_body_bytes: 1024, render_wait_ms: 0,
    adapter: 'ticketmaster', display: 'map', map_section_id: 's_12', map_seat_id: 'seat-2' };
  assert.doesNotThrow(() => validateRequest(request));
  for (const change of [
    { display: undefined }, { adapter: 'selectors' }, { map_section_id: undefined },
    { url: 'https://www.ticketmaster.pl/event/synthetic/12345678' },
    { map_section_id: '' }, { map_section_id: 's_12"] *' },
    { map_section_id: 'x'.repeat(129) }, { map_seat_id: 'seat 2' },
    { map_seat_id: null },
  ]) assert.throws(() => validateRequest({ ...request, ...change }), /invalid_request/);
});

test('automatic seat scans are bounded and cannot combine with an explicit seat hover', () => {
  const request = { id: 1, url: 'https://www.ticketmaster.cz/event/synthetic/12345678',
    timeout_ms: 1000, max_body_bytes: 1024, render_wait_ms: 0,
    adapter: 'ticketmaster', display: 'map', map_section_id: 's_12', map_scan_limit: 20 };
  assert.doesNotThrow(() => validateRequest(request));
  for (const change of [{ map_scan_limit: 0 }, { map_scan_limit: 51 }, { map_scan_limit: 1.5 },
    { map_scan_limit: '20' }, { map_scan_limit: null }, { map_section_id: undefined },
    { map_seat_id: 'seat-1' }, { display: undefined }]) {
    assert.throws(() => validateRequest({ ...request, ...change }), /invalid_request/);
  }
});

test('cart control demonstration requires an exclusive bounded Czech target before browser creation', async () => {
  const request = { id:1,url:'https://www.ticketmaster.cz/event/synthetic/12345678',
    timeout_ms:45000,max_body_bytes:4096,render_wait_ms:0,adapter:'ticketmaster',
    display:'cart_controls',cart_category_id:'00000000-0000-0000-0000-000000000000',cart_demo_quantity:2 };
  assert.doesNotThrow(() => validateRequest(request));
  const browser = { newContext() { throw new Error('must reject before creating a context'); } };
  for (const change of [
    { display:undefined }, {display:'map'}, {adapter:'selectors'},
    {url:'https://www.ticketmaster.pl/event/synthetic/12345678'},
    {url:'https://www.ticketmaster.cz/artist/12345678'},
    {cart_category_id:undefined}, {cart_category_id:null}, {cart_category_id:''},
    {cart_category_id:'a'.repeat(129)}, {cart_category_id:'category] *'},
    {cart_demo_quantity:undefined}, {cart_demo_quantity:null}, {cart_demo_quantity:0},
    {cart_demo_quantity:21}, {cart_demo_quantity:1.5}, {cart_demo_quantity:'2'},
    {map_section_id:'s_1'}, {map_seat_id:'seat-2'}, {map_scan_limit:20},
  ]) {
    assert.throws(() => validateRequest({...request,...change}), /invalid_request/);
    await assert.rejects(captureCartControls(browser,{...request,...change}), /invalid_request/);
  }
  await assert.rejects(capture({}, request), /invalid_request/);
});

test('cart context is closed if websocket fencing is unsupported', async () => {
  let closed = false, created;
  const browser = { async newContext(options) { created=options;
    return {off(){},async close(){closed=true;},newPage(){throw new Error('must not navigate');}}; } };
  await assert.rejects(captureCartControls(browser, {id:1,url:'https://www.ticketmaster.cz/event/synthetic/12345678',
    timeout_ms:1000,max_body_bytes:1024,render_wait_ms:0,adapter:'ticketmaster',
    display:'cart_controls',cart_category_id:'category-1',cart_demo_quantity:1}), /navigation_failed/);
  assert.deepEqual(created,{serviceWorkers:'block'});
  assert.equal(closed,true);
});

test('cart preparation accepts only explicit bounded metadata IDs in the isolated cart flow', async () => {
  const request = { id:1,url:'https://www.ticketmaster.cz/event/synthetic/12345678',
    timeout_ms:45000,max_body_bytes:4096,render_wait_ms:0,adapter:'ticketmaster',
    display:'cart_controls',cart_category_id:'category-1',cart_demo_quantity:2,
    cart_selection:{price_id:'price-6',section_ids:['1P','4P']} };
  assert.doesNotThrow(() => validateRequest(request));
  assert.doesNotThrow(() => validateRequest({...request,cart_selection:{price_id:'price-6',section_ids:[]}}));
  const browser = { newContext() { throw new Error('must reject before creating a context'); } };
  for (const selection of [
    null, false, [], 'price-6', {}, {price_id:'price-6'}, {section_ids:[]},
    {price_id:'',section_ids:[]}, {price_id:'p'.repeat(129),section_ids:[]},
    {price_id:'price] *',section_ids:[]}, {price_id:6,section_ids:[]},
    {price_id:'price-6',section_ids:'1P'}, {price_id:'price-6',section_ids:[null]},
    {price_id:'price-6',section_ids:['1P','1P']}, {price_id:'price-6',section_ids:['section] *']},
    {price_id:'price-6',section_ids:['s'.repeat(129)]},
    {price_id:'price-6',section_ids:Array.from({length:129},(_,i)=>`section-${i}`)},
    {price_id:'price-6',section_ids:[],submit:true},
  ]) {
    assert.throws(() => validateRequest({...request,cart_selection:selection}), /invalid_request/);
    await assert.rejects(captureCartControls(browser,{...request,cart_selection:selection}), /invalid_request/);
  }
  for (const display of [undefined,'map']) {
    assert.throws(() => validateRequest({...request,display,cart_category_id:undefined,cart_demo_quantity:undefined}), /invalid_request/);
  }
  await assert.rejects(capture({},request), /invalid_request/);
});

test('event identity binds region and event ID rather than page slug', () => {
  const event = 'https://www.ticketmaster.pl/event/korn/496543017';
  assert.ok(sameEvent(event, 'https://ticketmaster.pl/event/new-slug/496543017?language=en'));
  assert.ok(sameEvent('https://www.ticketmaster.com/name/event/1A006330B9A02A3F',
    'https://www.ticketmaster.com/event/1a006330b9a02a3f'));
  for (const other of [
    'https://www.ticketmaster.pl/event/korn/99999999',
    'https://www.ticketmaster.cz/event/korn/496543017',
    'https://www.ticketmaster.pl/artist/496543017',
    'https://www.ticketmaster.pl/event/korn/496543017/checkout',
    'https://www.ticketmaster.pl/login/event/496543017',
    'https://www.ticketmaster.pl/event/%34%39%36%35%34%33%30%31%37',
    'https://www.ticketmaster.pl/foo/../event/korn/496543017',
    'https://www.ticketmaster.pl/event//496543017',
    'https://www.ticketmaster.pl/event/123456789012345678901',
  ]) assert.equal(sameEvent(event, other), false, other);
});

test('challenge detector distinguishes human-check requests from ordinary footer text', () => {
  assert.equal(challengeText('This site is protected by reCAPTCHA'), false);
  assert.equal(challengeText('Please complete the captcha to continue'), true);
  assert.equal(challengeText('You are in the queue'), true);
});

test('403 capture returns unknown transport status without extracting challenge HTML', async () => {
  let closed = false;
  const url = 'https://www.ticketmaster.pl/event/korn/496543017';
  const page = {
    route: async () => {},
    goto: async () => ({status:()=>403,headers:()=>({'retry-after':'90'}),url:()=>url}),
    url:()=>url,
    content:()=>{ throw new Error('must not read challenge body'); },
    close:async()=>{closed=true;},
  };
  const reply = await capture(errorOnlyContext(page), {id:1,url,timeout_ms:1000,max_body_bytes:1024,render_wait_ms:0});
  assert.equal(reply.status,403);
  assert.equal(reply.html,'');
  assert.equal(reply.retry_after_secs,90);
  assert.equal(reply.manual_intervention,undefined);
  assert.ok(closed);
});

test('Ticketmaster protection headers explain 403 without reading or rendering its body', async () => {
  const url = 'https://www.ticketmaster.cz/event/synthetic/12345678';
  const forbidden = () => { throw new Error('must not inspect an error body or prepare its display'); };
  for (const adapter of ['selectors', 'ticketmaster']) {
    let closed = false;
    const page = {
      route: async () => {},
      goto: async () => ({ status: () => 403,
        headers: () => ({ 'tm-bl': '1', 'retry-after': '90', 'ebid': 'private-session-value' }),
        url: () => url,
        body: forbidden, text: forbidden, json: forbidden }),
      url: () => url,
      evaluate: forbidden,
      content: forbidden,
      waitForTimeout: forbidden,
      close: async () => { closed = true; },
    };
    const reply = await capture(errorOnlyContext(page), {
      id: 1, url, timeout_ms: 1000, max_body_bytes: 1024, render_wait_ms: 10, adapter,
    });
    assert.equal(reply.status, 403);
    assert.equal(reply.html, '');
    assert.equal(reply.manual_intervention, 'ticketmaster_protection');
    assert.equal(reply.adapter_state, 'blocked');
    assert.equal(reply.adapter_reason, 'manual_intervention');
    assert.equal(reply.retry_after_secs, 90);
    assert.equal(JSON.stringify(reply).includes('private-session-value'), false);
    assert.equal(closed, true);
  }
});

test('ordinary Chromium renders an offline fixture and preserves blocked status',
  {skip: process.env.TICKET_BROWSER_TESTS !== '1'}, async () => {
    const { chromium } = require('playwright');
    const browser = await chromium.launch({headless:true,
      ...(process.env.CHROMIUM_PATH ? {executablePath:process.env.CHROMIUM_PATH} : {})});
    try {
      const context = await browser.newContext({serviceWorkers:'block'});
      let requested = 0;
      await context.route('**/*', async route => {
        requested += 1;
        // All traffic is fulfilled locally; this never contacts Ticketmaster.
        if (route.request().url().endsWith('/blocked')) {
          return route.fulfill({status:403,headers:{'retry-after':'60'},body:'challenge'});
        }
        return route.fulfill({status:200,contentType:'text/html',body:
          '<html><body><div id="offer">Loading</div><script>setTimeout(() => { document.getElementById("offer").textContent="Synthetic offer available" }, 20)</script></body></html>'});
      });
      const base = {id:1,timeout_ms:5000,max_body_bytes:4096,render_wait_ms:100};
      const rendered = await capture(context,{...base,url:'https://www.ticketmaster.pl/event/synthetic'});
      assert.equal(rendered.status,200);
      assert.match(rendered.html, /id="offer">Synthetic offer available/);
      const blocked = await capture(context,{...base,id:2,url:'https://www.ticketmaster.pl/event/blocked'});
      assert.equal(blocked.status,403);
      assert.equal(blocked.html,'');
      assert.equal(blocked.retry_after_secs,60);
      assert.equal(requested,2);
      await context.close();
    } finally { await browser.close(); }
  });

test('rendered capture reflects CSS visibility and disabled controls without mutating the page',
  {skip: process.env.TICKET_BROWSER_TESTS !== '1'}, async () => {
    const { chromium } = require('playwright');
    const browser = await chromium.launch({headless:true,
      ...(process.env.CHROMIUM_PATH ? {executablePath:process.env.CHROMIUM_PATH} : {})});
    try {
      const page = await browser.newPage();
      await page.route('**/*', route => route.abort());
      await page.setContent(`<html><head><style>
        .gone {display:none} .invisible {visibility:hidden} .transparent {opacity:0}
        .contents {display:contents} .zero {height:0;position:relative} .absolute {position:absolute}
      </style></head><body>
        <div class="gone" id="css-hidden"><button>Hidden</button></div>
        <div class="invisible" id="css-invisible">Invisible</div>
        <div class="transparent" id="css-transparent">Transparent</div>
        <div class="contents" id="contents"><button id="visible">Buy visible offer</button></div>
        <div class="zero" id="zero"><button class="absolute" id="positioned">Buy positioned offer</button></div>
        <fieldset disabled><button id="disabled-button">Unavailable</button></fieldset>
        <div aria-disabled="true"><a id="disabled-link">Unavailable</a></div>
        <select id="quantity"><option value="1">1</option><option value="2">2</option>
          <optgroup disabled label="No stock"><option value="8" id="disabled-option">8</option></optgroup>
        </select>
      </body></html>`);
      const html = await captureRendered(page, 16384);
      const normalized = await browser.newPage();
      await normalized.route('**/*', route => route.abort());
      await normalized.setContent(html);
      const flags = await normalized.evaluate(() => {
        const ids = ['css-hidden','css-invisible','css-transparent','contents','zero','visible','positioned',
          'disabled-button','disabled-link','disabled-option','quantity'];
        return Object.fromEntries(ids.map(id => { const item=document.getElementById(id);
          return [id,{hidden:item.hasAttribute('hidden'),disabled:item.hasAttribute('disabled')}]; }));
      });
      for (const id of ['css-hidden','css-invisible','css-transparent']) assert.ok(flags[id].hidden, id);
      for (const id of ['contents','zero','visible','positioned','quantity']) assert.equal(flags[id].hidden, false, id);
      for (const id of ['disabled-button','disabled-link','disabled-option']) assert.ok(flags[id].disabled, id);
      assert.equal(await normalized.locator('#quantity option[value="2"]').getAttribute('hidden'), null);
      assert.equal(await page.locator('#css-hidden').getAttribute('hidden'), null);
      assert.equal(await page.locator('#disabled-button').getAttribute('disabled'), null);
      await assert.rejects(captureRendered(page, 1024), /body_too_large/);
      await normalized.close();
      await page.close();
    } finally { await browser.close(); }
  });
