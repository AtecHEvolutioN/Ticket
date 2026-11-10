'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { validateUrl, validateRequest, retryAfter, identificationRetryAllowed, proxyOptions, capture } = require('./worker.cjs');
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
