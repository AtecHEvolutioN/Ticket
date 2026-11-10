'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const fs = require('node:fs/promises');
const os = require('node:os');
const path = require('node:path');
const { NativeCartSession, validateRequest, checkoutNavigationAllowed, eventNavigationAllowed, launchEnvironment } = require('./native-cart-session.cjs');
const enabled = process.env.TICKET_BROWSER_TESTS === '1';
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

function fixture({ merged = false, mergedOther = false, initialQuantity = 0, otherQuantity = 0, wrongEvent = false,
  unexpectedStepper = false, duplicatePanel = false, invalidLabel = false,
  disabledSection = false, descriptionLabelMismatch = false, extra = '', mutate = '' } = {}) {
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
        ...(merged ? [category('3', 'Sezení', 0)] : []),
        ...(mergedOther ? [category('4', 'Stání', 0)] : [])], eventTicketLimit: 6 },
  };
  const stepper = (id, title, count) => `<li id="row-${id}">
    <div data-testid="ticketTypeInfo"><span>${title}${mergedOther && id === '1'
      ? `<button aria-expanded="false" onclick="bump('descriptionClicks')"><span>Otevřít detailní popis pro: ${descriptionLabelMismatch ? 'Jiný typ' : title}</span></button>` : ''}</span> <span>1 889,00 Kč–2 361,00 Kč za kus</span></div>
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
    </head><body data-step-clicks="0" data-increments="0" data-decrements="0" data-reserve-clicks="0" data-seat-clicks="0" data-resale-clicks="0" data-checkbox-changes="0" data-back-clicks="0" data-panel-opens="0" data-escapes="0" data-submit-calls="0" data-description-clicks="0">
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

// Synthetic implementation of the observed native temporary/apply filter model.
// It deliberately remounts checkbox IDs and toolbar buttons between opens.
function preparationFixture({ preselected = false, missingApply = false, brokenApply = false,
  lostPriceAfterSection = false, multiplePriceSections = false, onCheckbox = '', discountType = 'STANDARD', reorderAfterIncrement = false, pauseAfterIncrement = false } = {}) {
  const mutations = [];
  if (reorderAfterIncrement) mutations.push(`if(count===1 && spin.closest('li').id==='row-2'){document.getElementById('row-1')?.remove();spin.closest('ul').prepend(spin.closest('li'))}`);
  if (pauseAfterIncrement) mutations.push(`if(count===1){const warning=document.createElement('div');warning.id='manual-challenge';warning.textContent='Verify you are human';document.body.append(warning)}`);
  let html = fixture({ mutate: mutations.join(';') });
  const match = /<script id="__NEXT_DATA__" type="application\/json">([^<]+)<\/script>/.exec(html);
  const metadata = JSON.parse(match[1]);
  const state = metadata.props.pageProps.initialReduxState;
  state.ticketSelection.ticketTypes[0].discountType = discountType;
  metadata.props.pageProps.globalTranslations = { global: { filters: { apply: 'Použít filtry', reset: 'Resetovat' } } };
  state.translations = { 'edp.tselection.sections': 'Sekce' };
  const amounts = [[2, 1950, 411], [3, 1850, 393], [4, 1750, 375], [5, 1750, 375], [6, 1550, 339]];
  state.ticketSelection.ticketTypes[0].prices = amounts.map(([id, faceValue, serviceFeeChargesValue]) => ({
    id: String(id), faceValue, serviceFeeChargesValue, upsellFeeChargesValue: 0,
    priceBreakdown: [{ name: 'Cena', value: faceValue * 100 }],
  }));
  state.ticketSelection.priceLevels = amounts.map(([id]) => ({ id: String(id), title: `${id - 1}. kategorie` }));
  state.ticketSelection.sections = sections.map((title, index) => ({ id: index ? '4P' : '1P', title, rawId: index ? '4P' : '1P' }));
  state.ticketSelection.priceToSections = amounts.map(([id]) => ({ priceIdentifier: String(id), sectionIdentifier: [5, 6].includes(id) ? '4P' : '1P' }));
  if (multiplePriceSections) state.ticketSelection.priceToSections.push({ priceIdentifier: '6', sectionIdentifier: '1P' });
  Object.assign(state.ticketSelection, { feeRollup: 'ROLLUP', priceDisplays: { primary: 'ROLLUP' }, handlingFeeValue: 0 });
  html = html.replace(match[0], `<script id="__NEXT_DATA__" type="application/json">${JSON.stringify(metadata)}</script>`);
  return html.replace('</body>', `<script>
    const priceIds=['2','3','5','4','6'], sectionIds=['1P','4P'];
    const priceLabels=${JSON.stringify(prices)}, sectionLabels=${JSON.stringify(sections)};
    const compatible={'2':['1P'],'3':['1P'],'4':['1P'],'5':['4P'],'6':${multiplePriceSections ? "['1P','4P']" : "['4P']"}};
    let committedPrices=[],committedSections=[],temporaryPrices=[],temporarySections=[],panelKind='price';
    function newToolbarButton(id,label){const old=document.getElementById(id),button=document.createElement('button');
      button.type='button';button.id=id;button.textContent=label;old.replaceWith(button);
      button.onclick=()=>openPanel(id==='price-filter'?'price':'section')}
    refresh=()=>{const total=[...document.querySelectorAll('[role=spinbutton]')].reduce((n,s)=>n+Number(s.getAttribute('aria-valuenow')),0);
      document.getElementById('price-filter').disabled=total===0;document.getElementById('section-filter').disabled=total===0;
      for(const spin of document.querySelectorAll('[role=spinbutton]')){const count=Number(spin.getAttribute('aria-valuenow'));
        spin.previousElementSibling.disabled=count===0;spin.nextElementSibling.disabled=total>=6}};
    function toolbar(){newToolbarButton('price-filter',committedPrices.length===1?priceLabels[priceIds.indexOf(committedPrices[0])].split(' • ')[0]:'Všechny ceny');
      newToolbarButton('section-filter',committedSections.length===1?sectionLabels[sectionIds.indexOf(committedSections[0])]
        :committedSections.length?committedSections.length+' Sekce':'Všechny sekce');refresh()}
    function renderPreparationPanel(){const panel=container.querySelector('[data-testid=panelPriceSection]');
      const ids=panelKind==='price'?priceIds:sectionIds,labels=panelKind==='price'?priceLabels:sectionLabels;
      const choices=panelKind==='price'?temporaryPrices:temporarySections;
      const possible=new Set(temporaryPrices.flatMap(id=>compatible[id]));
      panel.innerHTML='<h1>'+(panelKind==='price'?'Cena':'Sekce')+'</h1><fieldset>'+ids.map((id,index)=>{
        const domId='_prep_'+panelKind+'_'+document.body.dataset.panelOpens+'_'+index+'-input';
        const disabled=panelKind==='section'&&temporaryPrices.length>0&&!possible.has(id);
        return '<label for="'+domId+'"><input type="checkbox" id="'+domId+'" data-config-id="'+id+'" '+(choices.includes(id)?'checked':'')+' '+(disabled?'disabled':'')+'><span>'+labels[index]+'</span></label>'
      }).join('')+'</fieldset>' + (${missingApply}?'':'<button type="button" id="apply-preparation" '+(JSON.stringify(temporaryPrices)===JSON.stringify(committedPrices)&&JSON.stringify(temporarySections)===JSON.stringify(committedSections)?'disabled':'')+'>Použít filtry</button>');
      panel.querySelectorAll('input').forEach(input=>input.onchange=()=>{bump('checkboxChanges');const id=input.dataset.configId;
        const selected=panelKind==='price'?temporaryPrices:temporarySections;
        if(input.checked)selected.push(id);else selected.splice(selected.indexOf(id),1);selected.sort();
        if(panelKind==='price')temporarySections=[...new Set(temporaryPrices.flatMap(id=>compatible[id]))].sort();
        else if(${lostPriceAfterSection})temporaryPrices=['2'];
        ${onCheckbox}
        renderPreparationPanel()});
      const apply=panel.querySelector('#apply-preparation');if(apply)apply.onclick=()=>{
        if(!${brokenApply}){committedPrices=temporaryPrices.length&&temporarySections.length?[...temporaryPrices]:[];
          committedSections=committedPrices.length?[...temporarySections]:[]}
        closePanel();toolbar()}
    }
    openPanel=kind=>{bump('panelOpens');panelKind=kind;form.hidden=true;form.setAttribute('aria-hidden','true');container.hidden=false;
      temporaryPrices=[...committedPrices];temporarySections=[...committedSections];
      if(${preselected}&&kind==='price'&&!committedPrices.length)temporaryPrices=['2'];
      renderPreparationPanel()};
    toolbar();
  </script></body>`);
}


const eventStart = '2026-11-16T18:30:00Z';
const request = () => ({ event_url: target, category_id: '2', price_id: '6', section_ids: ['4P'],
  quantity: 2, expected_event_start: eventStart, configured_unit_price_minor: 188900 });
function liveFixture(options = {}) {
  return preparationFixture(options).replace('</body>', `<script>
  ${options.initialChallenge ? `const initialChallenge=document.createElement('div');initialChallenge.id='manual-challenge';initialChallenge.textContent='Verify you are human';document.body.append(initialChallenge);` : ''}
  ${options.delayedConsent ? `setTimeout(()=>{
    const banner=document.createElement('div');banner.id='ketch-consent-banner';banner.setAttribute('role','dialog');
    const button=document.createElement('button');button.id='ketch-banner-button-secondary';button.type='button';
    button.textContent=${JSON.stringify(options.unknownConsent ? 'Unknown consent action' : 'Odmítnout vše')};
    button.onclick=()=>{document.body.dataset.consentAtQuantity=[...document.querySelectorAll('[role=spinbutton]')]
      .reduce((total,node)=>total+Number(node.getAttribute('aria-valuenow')),0);banner.remove()};
    banner.append(button);document.body.append(banner);
  },400);` : ''}
  {const script=document.getElementById('__NEXT_DATA__'),value=JSON.parse(script.textContent);
  value.props.pageProps.initialReduxState.eventInfo.dates={dateStatus:'singleDate',dateDisplay:'showDateTime',eventDate:'${eventStart}',startDate:'${eventStart}',endDate:'${eventStart}'};
  script.textContent=JSON.stringify(value);}
  </script></body>`);
}
async function withSession(run, options = {}) {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'native-session-test-'));
  const session = new NativeCartSession({ browser, evidenceDir: dir, request: request(), checkoutNavigationPolicy: () => false });
  let reserveCalls = 0, navigationCalls = 0;
  // Synthetic Ticketmaster origin is fulfilled locally; no provider request.
  const original = browser.newContext.bind(browser);
  browser.newContext = async args => {
    const context = await original(args);
    await context.route('**/*', route => {
      if (route.request().isNavigationRequest()) navigationCalls++;
      if (route.request().url().endsWith('/api/reserve')) {
        reserveCalls++;
        return route.fulfill({ status: 200, contentType: 'application/json', body: '{"statusCode":"PENDING","private":"test-secret"}' });
      }
      return route.fulfill({ status: 200, contentType: 'text/html', body: liveFixture(options) });
    });
    return context;
  };
  try { await run(session, () => reserveCalls, dir, () => navigationCalls); }
  finally { await session.close(); await browser.close(); await fs.rm(dir, { recursive: true, force: true }); }
}

test('live session rejects invalid intent and sanitizes browser environment', () => {
  for (const mutation of [{ quantity: 0 }, { event_url: target + '?token=secret' }, { event_url: target + '#private' },
    { category_id: 'bad"id' }, { expected_event_start: 'tomorrow' }, { section_ids: ['4P', '4P'] }, { unexpected: 1 }])
    assert.throws(() => validateRequest({ ...request(), ...mutation }), /invalid_request/);
  assert.deepEqual(launchEnvironment({ PATH: '/bin', DISPLAY: ':1', TICKETMASTER_API_KEY: 'secret', DISCORD_BOT_TOKEN: 'secret', AWS_SECRET_ACCESS_KEY: 'secret' }),
    { PATH: '/bin', DISPLAY: ':1' });
});

test('checkout navigation is limited to reviewed SDK paths and nested event identity', () => {
  const checkout = 'https://checkout.ticketmaster.cz/request_123?edp=' + encodeURIComponent(target);
  assert.equal(checkoutNavigationAllowed(target, checkout), true);
  for (const url of [checkout + '&edp=' + encodeURIComponent(target), checkout.replace('ticketmaster.cz/', 'ticketmaster.com/'),
    checkout.replace('request_123', 'nested/path'), checkout.replace('request_123', '%2Fsecret'), checkout + '&token=secret',
    'https://www.ticketmaster.cz/checkout/controller/addTickets.php?v=secret', 'https://identity.ticketmaster.cz/sign-in'])
    assert.equal(checkoutNavigationAllowed(target, url), false, url);
  const login = new URL('https://identity.ticketmaster.cz/sign-in');
  login.search = new URLSearchParams({ integratorId: 'prd1908.CCPCheckout', placementId: 'checkout.v2', showHeader: 'true',
    headerTitle: 'Checkout', hideLeftPanel: 'true', redirectUri: checkout }).toString();
  assert.equal(checkoutNavigationAllowed(target, login.href), true);
  login.searchParams.set('redirectUri', 'https://unrelated.example/secret');
  assert.equal(checkoutNavigationAllowed(target, login.href), false);
  assert.equal(eventNavigationAllowed(target, target + '?token=private'), false);
});

test('prepare selects native controls online without Find and one durable-token commit clicks at most once', { skip: !enabled }, async () => {
  await withSession(async (session, calls, dir) => {
    const prepared = await session.prepare();
    assert.equal(prepared.state, 'prepared');
    assert.equal(calls(), 0);
    assert.deepEqual(prepared.preflight.section_ids, ['4P']);
    assert.equal(await session.page.locator('#row-2 [role="spinbutton"]').getAttribute('aria-valuenow'), '2');
    const refreshed = await session.refresh();
    assert.equal(refreshed.preflight.config_id, prepared.preflight.config_id);
    assert.notEqual(refreshed.preflight.preflight_id, prepared.preflight.preflight_id);
    await assert.rejects(session.commit({ attempt_id: 'attempt1', dispatch_token: 'wrong', preflight_id: refreshed.preflight.preflight_id }), /invalid_commit/);
    const command = { attempt_id: 'attempt1', dispatch_token: 'attempt1', preflight_id: refreshed.preflight.preflight_id };
    const results = await Promise.allSettled([session.commit(command), session.commit(command)]);
    assert.equal(results[0].status, 'fulfilled');
    assert.equal(results[0].value.outcome.kind, 'uncertain');
    assert.equal(results[1].status, 'rejected');
    assert.equal(calls(), 1);
    assert.equal(JSON.stringify(results[0]).includes('test-secret'), false);
    assert.equal(session.findClickAttempted, true);
    const result = await session.observe({ attempt_id: 'attempt1' });
    assert.equal(result.outcome.receipt.context.session_id, prepared.preflight.session_id);
    assert.equal(result.outcome.receipt.context.event_url, null);
    const sessionDir = path.join(dir, prepared.preflight.session_id);
    const filenames = await fs.readdir(sessionDir);
    assert.ok(filenames.some(name => name.endsWith('-response.json')));
    const actions = await Promise.all(filenames.filter(name => name.endsWith('-action.json'))
      .map(async name => JSON.parse(await fs.readFile(path.join(sessionDir, name), 'utf8'))));
    assert.deepEqual(actions.map(action => action.phase), ['find_click_attempted', 'find_click_returned']);
    const ledger = await Promise.all(filenames.filter(name => name.endsWith('-request-ledger.json'))
      .map(async name => JSON.parse(await fs.readFile(path.join(sessionDir, name), 'utf8'))));
    assert.ok(ledger.length, JSON.stringify(session.requestLedger.snapshot()));
    assert.deepEqual(new Set(ledger.map(record => record.phase)), new Set(['request_started', 'response_received', 'request_finished', 'response_body']));
    const requestId = ledger.find(record => record.phase === 'request_started').request_id;
    for (const record of ledger) {
      assert.equal(record.request_id, requestId);
      assert.equal(record.source_document_id, prepared.preflight.document_id);
      assert.equal(record.attempt_id, 'attempt1');
      assert.equal(record.evidence_scope, 'diagnostic_only');
      assert.ok(record.observed_at_ms >= actions[0].observed_at_ms);
    }
    const responses = await Promise.all(filenames.filter(name => name.endsWith('-response.json'))
      .map(async name => JSON.parse(await fs.readFile(path.join(sessionDir, name), 'utf8'))));
    assert.equal(responses[0].request_correlation_id, requestId);
    assert.equal(JSON.stringify(ledger).includes('test-secret'), false);
    for (const action of actions) {
      assert.deepEqual(Object.keys(action).sort(), ['attempt_id', 'monotonic_ms', 'observed_at_ms', 'phase']);
      assert.equal(action.attempt_id, 'attempt1');
      assert.ok(Number.isSafeInteger(action.observed_at_ms));
      assert.ok(Number.isFinite(action.monotonic_ms));
    }
    for (const name of filenames) assert.equal((await fs.stat(path.join(sessionDir, name))).mode & 0o077, 0);
    assert.equal((await fs.stat(sessionDir)).mode & 0o077, 0);
  });
});

test('stale or changed form never clicks Find, and consumed failed dispatch cannot retry', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    const { preflight } = await session.prepare();
    session.preflight.config_expires_at_ms = Date.now();
    await assert.rejects(session.commit({ attempt_id: 'stale', dispatch_token: 'stale', preflight_id: preflight.preflight_id }), /preflight_expired/);
    assert.equal(calls(), 0);
    const refreshed = await session.refresh();
    await session.page.locator('#row-2 [role="spinbutton"]').evaluate(node => node.setAttribute('aria-valuenow', '1'));
    const command = { attempt_id: 'changed', dispatch_token: 'changed', preflight_id: refreshed.preflight.preflight_id };
    const result = await session.commit(command);
    assert.equal(result.outcome.kind, 'uncertain');
    assert.equal(calls(), 0);
    await assert.rejects(session.commit(command), /invalid_commit/);
  });
});

test('controller disconnect invalidates prepared permission and never dispatches', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    const { preflight } = await session.prepare();
    session.disconnect();
    await assert.rejects(session.commit({ attempt_id: 'disconnected', dispatch_token: 'disconnected', preflight_id: preflight.preflight_id }), /invalid_commit/);
    assert.equal(calls(), 0);
  });
});

test('wrong event date and visible challenge pause before native Find', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    session.request = Object.freeze({ ...request(), expected_event_start: '2026-11-17T18:30:00Z' });
    await assert.rejects(session.prepare(), /event_date_changed/);
    assert.equal(calls(), 0);
  });
  await withSession(async (session, calls) => {
    const prepared = await session.prepare();
    await session.page.locator('body').evaluate(node => node.append('Verify you are human'));
    const result = await session.commit({ attempt_id: 'challenge', dispatch_token: 'challenge', preflight_id: prepared.preflight.preflight_id });
    assert.equal(result.manual_intervention, 'captcha');
    assert.equal(calls(), 0);
  });
});

test('JSON-lines close acknowledgement is followed by process termination without launching a browser', async () => {
  const { spawn } = require('node:child_process');
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'native-protocol-test-'));
  const child = spawn(process.execPath, [path.join(__dirname, 'native-cart-session.cjs'), '--evidence-dir', dir],
    { stdio: ['pipe', 'pipe', 'pipe'] });
  let stdout = '', stderr = '';
  child.stdout.on('data', data => { stdout += data; });
  child.stderr.on('data', data => { stderr += data; });
  const exit = new Promise((resolve, reject) => { child.once('exit', code => resolve(code)); child.once('error', reject); });
  const timer = setTimeout(() => child.kill('SIGKILL'), 5000);
  try {
    child.stdin.write(JSON.stringify({ id: 1, op: 'close' }) + '\n');
    assert.equal(await exit, 0);
    assert.deepEqual(JSON.parse(stdout.trim()), { id: 1, ok: true, state: 'closed' });
    assert.equal(stderr, '');
  } finally { clearTimeout(timer); child.kill('SIGKILL'); await fs.rm(dir, { recursive: true, force: true }); }
});

test('worker exits after explicit close of a spent session instead of starting disconnect grace', { skip: !enabled }, async () => {
  const { spawn } = require('node:child_process');
  const readline = require('node:readline');
  const dir = await fs.mkdtemp(path.join(os.tmpdir(), 'native-protocol-spent-'));
  const preload = path.join(dir, 'fixture-preload.cjs');
  const source = `const { createRequire }=require('node:module');
  const playwright=createRequire(${JSON.stringify(__filename)})('playwright');
  const launch=playwright.chromium.launch.bind(playwright.chromium);
  playwright.chromium.launch=async options=>{const browser=await launch(options);const create=browser.newContext.bind(browser);
    browser.newContext=async options=>{const context=await create(options);await context.route('**/*',route=>route.fulfill(
      route.request().url().endsWith('/api/reserve')?{status:200,contentType:'application/json',body:'{"statusCode":"PENDING"}'}:
      {status:200,contentType:'text/html',body:${JSON.stringify(liveFixture())}}));return context};return browser};`;
  await fs.writeFile(preload, source, { mode: 0o600 });
  const child = spawn(process.execPath, ['--require', preload, path.join(__dirname, 'native-cart-session.cjs'),
    '--evidence-dir', dir, ...(process.env.CHROMIUM_PATH ? ['--executable-path', process.env.CHROMIUM_PATH] : [])],
  { stdio: ['pipe', 'pipe', 'pipe'] });
  const pending = new Map();
  const replies = readline.createInterface({ input: child.stdout });
  replies.on('line', line => { const result = JSON.parse(line); pending.get(result.id)?.(result); pending.delete(result.id); });
  let stderr = '';
  child.stderr.on('data', value => { stderr += value; });
  const done = new Promise((resolve, reject) => { child.once('close', code => resolve(code)); child.once('error', reject); });
  const timer = setTimeout(() => child.kill('SIGKILL'), 20000);
  const command = value => new Promise(resolve => { pending.set(value.id, resolve); child.stdin.write(JSON.stringify(value) + '\n'); });
  try {
    const prepared = await command({ id: 1, op: 'prepare', request: request() });
    assert.equal(prepared.state, 'prepared');
    const resumed = await command({ id: 2, op: 'resume_preparation' });
    assert.equal(resumed.state, 'prepared');
    assert.equal(resumed.preflight.document_id, prepared.preflight.document_id);
    const committed = await command({ id: 3, op: 'commit', attempt_id: 'protocol_spent', dispatch_token: 'protocol_spent', preflight_id: resumed.preflight.preflight_id });
    assert.equal(committed.outcome.kind, 'uncertain');
    const closed = await command({ id: 4, op: 'close' });
    assert.equal(closed.state, 'closed');
    assert.equal(await done, 0);
    assert.equal(stderr, '');
  } finally { clearTimeout(timer); child.kill('SIGKILL'); replies.close(); await fs.rm(dir, { recursive: true, force: true }); }
});


test('delayed known consent is rejected before quantity changes, while an unknown dialog remains blocked', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    const result = await session.prepare();
    assert.equal(result.state, 'prepared');
    assert.equal(await session.page.locator('body').getAttribute('data-consent-at-quantity'), '0');
    assert.equal(await session.page.locator('#ketch-consent-banner').count(), 0);
    assert.equal(calls(), 0);
  }, { delayedConsent: true });
  await withSession(async (session, calls) => {
    await assert.rejects(session.prepare(), /native_modal_blocked/);
    assert.equal(await session.page.locator('body').getAttribute('data-consent-at-quantity'), null);
    assert.equal(await session.page.locator('#row-2 [role="spinbutton"]').getAttribute('aria-valuenow'), '0');
    assert.equal(calls(), 0);
  }, { delayedConsent: true, unknownConsent: true });
});

test('live quantity selection rebinds its category after another row disappears and positions change', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    const result = await session.prepare();
    assert.equal(result.state, 'prepared');
    assert.equal(await session.page.locator('#row-1').count(), 0);
    assert.equal(await session.page.locator('#row-2 [role="spinbutton"]').getAttribute('aria-valuenow'), '2');
    assert.equal(await session.page.locator('body').getAttribute('data-increments'), '2');
    assert.equal(calls(), 0);
  }, { reorderAfterIncrement: true });
});

// Sanitized captured shape: reserveView > aria-live > reserveError containing
// the two exact visible spans. No private provider IDs, bodies or tokens.
const insufficientText = 'Pro provedení vašeho požadavku není k dispozici dostatek vstupenek. Pro zobrazení dostupných vstupenek upravte množství nebo typ vstupenek.';
async function showInsufficientMessage(page) {
  await page.evaluate(message => {
    const view = document.createElement('div');
    view.dataset.testid = 'reserveView';
    const live = document.createElement('div');
    live.setAttribute('aria-live', 'assertive');
    const error = document.createElement('div');
    error.dataset.testid = 'reserveError';
    const spans = message.split('. ');
    for (const [index, value] of spans.entries()) {
      const span = document.createElement('span');
      span.textContent = value + (index === 0 ? '. ' : '');
      error.append(span);
    }
    live.append(error); view.append(live); document.body.append(view);
  }, insufficientText);
}

test('native insufficient-ticket diagnostic requires current visible exact same-event evidence and never becomes a no-hold claim', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    const prepared = await session.prepare();
    await showInsufficientMessage(session.page);
    assert.equal(await session.nativeSignal(), null, 'A pre-dispatch message is not an action diagnostic');
    await session.page.locator('[data-testid="reserveView"]').evaluate(node => node.remove());
    const initial = await session.commit({ attempt_id: 'signal_test', dispatch_token: 'signal_test', preflight_id: prepared.preflight.preflight_id });
    assert.equal(initial.native_signal, undefined);
    assert.equal(calls(), 1);
    await showInsufficientMessage(session.page);
    const observed = await session.observe({ attempt_id: 'signal_test' });
    assert.equal(observed.native_signal, 'insufficient_tickets_reported');
    assert.equal(observed.outcome.kind, 'uncertain');
    assert.equal(observed.outcome.receipt.context.event_url, null);
    const error = session.page.locator('[data-testid="reserveError"]');
    await error.evaluate(node => { node.hidden = true; });
    assert.equal((await session.observe({ attempt_id: 'signal_test' })).native_signal, undefined, 'Hidden current evidence clears the signal');
    await error.evaluate(node => { node.hidden = false; node.textContent = 'Unknown ticket warning'; });
    assert.equal((await session.observe({ attempt_id: 'signal_test' })).native_signal, undefined);
    await error.evaluate((node, message) => { node.textContent = message; }, insufficientText);
    await session.page.locator('link[rel="canonical"]').evaluate(node => { node.href = 'https://www.ticketmaster.cz/event/different/9876543210'; });
    assert.equal((await session.observe({ attempt_id: 'signal_test' })).native_signal, undefined, 'Wrong canonical event is rejected');
    await session.page.locator('link[rel="canonical"]').evaluate((node, url) => { node.href = url; }, target);
    await session.page.locator('#__NEXT_DATA__').evaluate(node => { const state = JSON.parse(node.textContent); state.query.eventId = '9876543210'; node.textContent = JSON.stringify(state); });
    assert.equal((await session.observe({ attempt_id: 'signal_test' })).native_signal, undefined, 'Wrong embedded event is rejected');
    assert.equal(calls(), 1, 'Diagnostic observations never submit another search');
  });
});


test('manual clearance resumes the same initial browser without navigation or trusting acknowledgement', { skip: !enabled }, async () => {
  await withSession(async (session, calls, _dir, navigations) => {
    await assert.rejects(session.prepare(), /needs_human/);
    assert.equal(navigations(), 1);
    await assert.rejects(session.resumePreparation(), /needs_human/);
    assert.equal(await session.page.locator('#row-2 [role="spinbutton"]').getAttribute('aria-valuenow'), '0');
    await session.page.locator('#manual-challenge').evaluate(node => node.remove());
    const resumed = await session.resumePreparation();
    assert.equal(resumed.state, 'prepared');
    assert.equal(resumed.preflight.session_id, session.sessionId);
    assert.equal(navigations(), 1, 'Resume must not navigate or reload');
    assert.equal(await session.page.locator('body').getAttribute('data-increments'), '2');
    assert.equal(calls(), 0);
  }, { initialChallenge: true });
});

test('initial manual document replacement can rebind only before any preflight or quantity action', { skip: !enabled }, async () => {
  await withSession(async (session, calls, _dir, navigations) => {
    await assert.rejects(session.prepare(), /needs_human/);
    const oldDocument = session.documentId;
    let laterNavigations = 0;
    session.context.on('request', request => { if (request.isNavigationRequest()) laterNavigations++; });
    await session.context.route(target, route => route.fulfill({ status: 200, contentType: 'text/html', body: liveFixture() }));
    await session.page.reload({ waitUntil: 'domcontentloaded' }); // Simulated human browser action.
    const resumed = await session.resumePreparation();
    assert.equal(resumed.state, 'prepared');
    assert.notEqual(resumed.preflight.document_id, oldDocument);
    assert.equal(calls(), 0);
    assert.equal(laterNavigations, 1, 'Only the human reload navigates; resume does not');
    assert.equal(navigations(), 1);
  }, { initialChallenge: true });
});

test('manual pause preserves an established selection and rejects replacement document or changed configuration', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    const prepared = await session.prepare();
    await session.page.evaluate(() => { const node=document.createElement('div');node.id='manual-challenge';node.textContent='Verify you are human';document.body.append(node); });
    await assert.rejects(session.refresh(), /needs_human/);
    await session.page.locator('#manual-challenge').evaluate(node => node.remove());
    const resumed = await session.resumePreparation();
    for (const key of ['session_id', 'document_id', 'config_id']) assert.equal(resumed.preflight[key], prepared.preflight[key]);
    assert.equal(await session.page.locator('body').getAttribute('data-increments'), '2');
    await session.page.reload({ waitUntil: 'domcontentloaded' });
    await assert.rejects(session.resumePreparation(), /native_form_changed/);
    assert.equal(calls(), 0);
  });
  await withSession(async (session, calls) => {
    await session.prepare();
    await session.page.locator('#__NEXT_DATA__').evaluate(node => { const value=JSON.parse(node.textContent);value.props.pageProps.initialReduxState.eventInfo.dates.startDate='2027-01-01T00:00:00Z';node.textContent=JSON.stringify(value); });
    await assert.rejects(session.resumePreparation(), /native_form_changed/);
    assert.equal(calls(), 0);
  });
});

test('partial preparation and foreign reservation activity never resume into an automatic Find', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    await assert.rejects(session.prepare(), /needs_human/);
    await session.page.locator('#manual-challenge').evaluate(node => node.remove());
    await assert.rejects(session.resumePreparation(), /native_form_changed/);
    assert.equal(await session.page.locator('#row-2 [role="spinbutton"]').getAttribute('aria-valuenow'), '1');
    assert.equal(calls(), 0);
  }, { pauseAfterIncrement: true });
  await withSession(async (session, calls) => {
    const prepared = await session.prepare();
    await session.page.locator('[data-testid="findTicketsBtn"]').click(); // Simulated independent human action.
    await assert.rejects(session.resumePreparation(), /native_form_changed/);
    await assert.rejects(session.commit({ attempt_id: 'foreign', dispatch_token: 'foreign', preflight_id: prepared.preflight.preflight_id }), /invalid_commit/);
    assert.equal(calls(), 0, 'The bot-owned browser blocks Find before the journaled permit');
    assert.equal(session.findClickAttempted, undefined);
  });
  await withSession(async (session, calls) => {
    await session.prepare();
    await session.page.evaluate(() => fetch('/event/1234567890/api/reserve', { method: 'POST' }));
    await assert.rejects(session.resumePreparation(), /native_form_changed/);
    assert.equal(calls(), 1);
  });
});

test('resume is forbidden after a dispatched attempt and does not clear its one-shot latch', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    const prepared = await session.prepare();
    await session.commit({ attempt_id: 'spent_resume', dispatch_token: 'spent_resume', preflight_id: prepared.preflight.preflight_id });
    await assert.rejects(session.resumePreparation(), /invalid_state/);
    assert.equal(calls(), 1);
    assert.equal(session.spent, true);
  });
});

test('a manual Find racing the authorized click reaches the native handler only once', { skip: !enabled }, async () => {
  await withSession(async (session, calls) => {
    const prepared = await session.prepare();
    const findHandle = session.driver.findHandle.bind(session.driver);
    session.driver.findHandle = async () => {
      const handle = await findHandle(), click = handle.click.bind(handle);
      handle.click = async options => {
        await session.page.locator('[data-testid="findTicketsBtn"]').click();
        return click(options);
      };
      return handle;
    };
    const result = await session.commit({ attempt_id: 'manual_race', dispatch_token: 'manual_race', preflight_id: prepared.preflight.preflight_id });
    assert.equal(result.outcome.kind, 'uncertain');
    assert.equal(calls(), 1);
    assert.equal(await session.page.locator('body').getAttribute('data-reserve-clicks'), '1');
    await session.page.locator('[data-testid="findTicketsBtn"]').click();
    assert.equal(calls(), 1, 'The consumed permit blocks all later Find activations');
  });
});

test('manual ticket changes between the verified seal, arm, and click never dispatch', { skip: !enabled }, async () => {
  for (const timing of ['before_arm', 'after_arm', 'reverted_control_change']) await withSession(async (session, calls) => {
    const prepared = await session.prepare();
    const findHandle = session.driver.findHandle.bind(session.driver);
    session.driver.findHandle = async () => {
      const handle = await findHandle(), click = handle.click.bind(handle);
      const mutate = () => session.page.locator('#row-2 [role="spinbutton"]').evaluate(node => node.setAttribute('aria-valuenow', '1'));
      if (timing === 'reverted_control_change') {
        await session.page.locator('#row-2 [data-direction="minus"]').click();
        await session.page.locator('#row-2 [data-direction="plus"]').click();
      } else if (timing === 'before_arm') await mutate();
      else handle.click = async options => { await mutate(); return click(options); };
      return handle;
    };
    const result = await session.commit({ attempt_id: timing, dispatch_token: timing, preflight_id: prepared.preflight.preflight_id });
    assert.equal(result.outcome.kind, 'uncertain');
    assert.equal(calls(), 0, timing);
  });
});

test('private diagnostics redact cloned form values and omit identity documents and unrelated response bodies', { skip: !enabled }, async () => {
  await withSession(async (session, calls, dir) => {
    const prepared = await session.prepare();
    await session.commit({ attempt_id: 'privacy', dispatch_token: 'privacy', preflight_id: prepared.preflight.preflight_id });
    const marker = 'synthetic-user-private-value';
    await session.page.evaluate(marker => {
      const box = document.createElement('div');
      box.innerHTML = '<input value="'+marker+'"><textarea>'+marker+'</textarea><div contenteditable="true">'+marker+'</div><span data-private="'+marker+'">Receipt</span><script type="application/json">{"private":"'+marker+'"}</script>';
      document.body.append(box);
    }, marker);
    await session.observe({ attempt_id: 'privacy' });
    assert.equal(await session.page.locator('input:not([type])').inputValue(), marker, 'Sanitizer never edits the live form');
    await session.context.route('https://checkout.ticketmaster.cz/account', route => route.fulfill({ status: 200,
      contentType: 'application/json', headers: { 'access-control-allow-origin': '*' }, body: JSON.stringify({ private: marker }) }));
    await session.context.route('https://checkout.prod.ticketmaster.cz/graphql', route => route.fulfill({ status: 200,
      contentType: 'application/json', headers: { 'access-control-allow-origin': '*' }, body: JSON.stringify({ private: marker }) }));
    await session.context.route('**/event/1234567890/api/reserve_ism', route => route.fulfill({ status: 200,
      contentType: 'text/html', body: '<input value="'+marker+'">' }));
    await session.page.evaluate(async () => {
      await fetch('https://checkout.ticketmaster.cz/account');
      await fetch('https://checkout.prod.ticketmaster.cz/graphql', { method: 'POST', body: JSON.stringify({ operationName: 'GCOReserve', query: 'query GCOReserve { account { payment } }', variables: {} }) });
      await fetch('/event/1234567890/api/reserve_ism', { method: 'POST' });
    });
    await session.observe({ attempt_id: 'privacy' });
    session.checkoutNavigationPolicy = url => new URL(url).hostname === 'identity.ticketmaster.cz';
    await session.context.route('https://identity.ticketmaster.cz/sign-in', route => route.fulfill({ status: 200,
      contentType: 'text/html', body: '<html><body>Identity '+marker+'<input type="password" value="'+marker+'"></body></html>' }));
    await session.page.goto('https://identity.ticketmaster.cz/sign-in');
    const observed = await session.observe({ attempt_id: 'privacy' });
    assert.equal(observed.manual_intervention, 'authentication_required');
    const sessionDir = path.join(dir, prepared.preflight.session_id);
    const records = await Promise.all((await fs.readdir(sessionDir)).map(name => fs.readFile(path.join(sessionDir, name), 'utf8')));
    assert.equal(records.join('\n').includes(marker), false);
    assert.equal(records.join('\n').includes('Identity '), false);
    assert.equal(calls(), 1);
  });
});

test('preparation failure evidence also removes values typed during a manual pause', { skip: !enabled }, async () => {
  await withSession(async (session, calls, dir) => {
    await assert.rejects(session.prepare(), /needs_human/);
    await session.page.evaluate(() => {
      const input = document.createElement('textarea');
      input.textContent = 'private-pause-marker';
      document.body.append(input);
    });
    await assert.rejects(session.resumePreparation(), /needs_human/);
    const sessionDir = path.join(dir, session.sessionId);
    const records = await Promise.all((await fs.readdir(sessionDir)).map(name => fs.readFile(path.join(sessionDir, name), 'utf8')));
    assert.equal(records.join('\n').includes('private-pause-marker'), false);
    assert.equal(await session.page.locator('textarea').inputValue(), 'private-pause-marker');
    assert.equal(calls(), 0);
  }, { initialChallenge: true });
});
