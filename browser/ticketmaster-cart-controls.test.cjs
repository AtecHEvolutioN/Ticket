'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { inspectTicketmasterCartControls, assertCartControlsCurrent,
  releaseCartControlsInspection } = require('./ticketmaster-cart-controls.cjs');

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
  lostPriceAfterSection = false, multiplePriceSections = false, onCheckbox = '', discountType = 'STANDARD' } = {}) {
  let html = fixture();
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

async function browserTest(run) {
  const { chromium } = require('playwright');
  const browser = await chromium.launch({ headless: true,
    ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}) });
  try {
    const context = await browser.newContext({ serviceWorkers: 'block' });
    try { await run(context); } finally { await context.close(); }
  } finally { await browser.close(); }
}

async function open(context, body) {
  const page = await context.newPage();
  await page.route('**/*', route => route.fulfill({ status: 200, contentType: 'text/html', body }));
  await page.goto(target);
  await context.setOffline(true);
  let attempts = 0;
  page.on('request', () => { attempts += 1; });
  await page.route('**/*', route => route.abort());
  return { page, networkAttempts: () => attempts };
}

const options = () => ({ categoryId: '2', quantity: 2, networkDisabled: true,
  assertNetworkDisabled: async () => true });
const current = async () => {};
async function counters(page) { return page.locator('body').evaluate(node => ({ ...node.dataset })); }
async function noPurchaseActions(page, networkAttempts) {
  const counts = await counters(page);
  for (const key of ['reserveClicks', 'seatClicks', 'resaleClicks', 'checkboxChanges', 'escapes', 'submitCalls', 'descriptionClicks'])
    assert.equal(counts[key], '0', key);
  assert.equal(networkAttempts(), 0, 'No reserve or external request was dispatched');
}

test('native controls reject unsupported inputs and absent network proof before touching a page', async () => {
  const cases = [
    [target, { ...options(), categoryId: '2"] button' }, 100],
    [target, { ...options(), quantity: 0 }, 100],
    [target, { ...options(), quantity: 2.5 }, 100],
    [target, { ...options(), quantity: 1001 }, 100],
    [target, options(), 0],
    [target, options(), NaN],
    ['https://www.ticketmaster.pl/event/synthetic/1234567890', options(), 100],
    ['not a URL', options(), 100],
  ];
  for (const [url, request, budget] of cases) {
    const result = await inspectTicketmasterCartControls({}, url, request, budget, current);
    assert.equal(result.state, 'cart_controls_unsupported');
    assert.equal(result.inspection, undefined);
  }
  for (const request of [
    { ...options(), networkDisabled: false },
    { ...options(), assertNetworkDisabled: undefined },
    { ...options(), assertNetworkDisabled: async () => false },
  ]) {
    const result = await inspectTicketmasterCartControls({}, target, request, 100, current);
    assert.equal(result.state, 'cart_controls_network_required');
  }
});

test('native controls inspect both checkbox panels then restore zero quantity without submitting', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const { page, networkAttempts } = await open(context, fixture({ mergedOther: true }));
    let networkChecks = 0, documentChecks = 0;
    const request = { ...options(), assertNetworkDisabled: async () => { networkChecks += 1; return true; } };
    const result = await inspectTicketmasterCartControls(page, target, request, 8000, async () => { documentChecks += 1; });
    assert.equal(result.state, 'cart_controls_ready');
    const report = result.inspection;
    assert.equal(report.event_id, eventId);
    assert.equal(report.category_id, '2');
    assert.equal(report.category_title, 'Sezení');
    assert.equal(report.demo_quantity, 2);
    assert.equal(report.initial_quantity, 0);
    assert.equal(report.final_quantity, 0);
    assert.equal(report.network_disabled_before_changes, true);
    assert.equal(report.form_restored, true);
    assert.ok(report.finished_at_ms >= report.started_at_ms && report.started_at_ms > 0);
    for (const key of ['stock_verified', 'adjacency_verified', 'fees_complete', 'hold_verified', 'reservation_attempted'])
      assert.equal(report[key], false, key);
    assert.deepEqual(report.panels.map(panel => panel.kind), ['price', 'section']);
    assert.deepEqual(report.panels.map(panel => panel.heading), ['Cena', 'Sekce']);
    assert.deepEqual(report.panels[0].options.map(choice => choice.label), prices);
    assert.deepEqual(report.panels[1].options.map(choice => choice.label), sections);
    for (const panel of report.panels) {
      assert.equal(panel.opened, true);
      assert.equal(panel.disabled, false);
      assert.ok(panel.options.every(choice => typeof choice.checkbox_id === 'string' && !choice.checked && !choice.disabled));
    }
    const counts = await counters(page);
    assert.equal(counts.increments, '2');
    assert.equal(counts.decrements, '2');
    assert.equal(counts.panelOpens, '2');
    assert.equal(counts.backClicks, '2');
    assert.equal(await page.locator('#row-2 [role=spinbutton]').getAttribute('aria-valuenow'), '0');
    assert.equal(await page.locator('#native-form').isVisible(), true);
    assert.equal(await page.locator('#panel-container').isVisible(), false);
    assert.ok(networkChecks >= 8 && documentChecks >= 8);
    await noPurchaseActions(page, networkAttempts);
    await assertCartControlsCurrent(page, target, report);
    await assert.rejects(assertCartControlsCurrent(page, target, { ...report, stock_verified: true }),
      /cart_controls_changed/);
    await assertCartControlsCurrent(page, target, report);
    await page.locator('#row-2 [role=spinbutton]').evaluate(spin => spin.setAttribute('aria-valuenow', '1'));
    await assert.rejects(assertCartControlsCurrent(page, target, report), /cart_controls_(changed|unrecognized)/);
    await page.locator('#row-2 [role=spinbutton]').evaluate(spin => spin.setAttribute('aria-valuenow', '0'));
    await assertCartControlsCurrent(page, target, report);
    await releaseCartControlsInspection(page);
    await assert.rejects(assertCartControlsCurrent(page, target, report), /cart_controls_changed/);
  });
});

test('native controls report a disabled section filter without guessing choices', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const { page, networkAttempts } = await open(context, fixture({ disabledSection: true }));
    const result = await inspectTicketmasterCartControls(page, target, options(), 8000, current);
    assert.equal(result.state, 'cart_controls_ready');
    const section = result.inspection.panels.find(panel => panel.kind === 'section');
    assert.equal(section.opened, false);
    assert.equal(section.disabled, true);
    assert.deepEqual(section.options, []);
    assert.equal(result.inspection.final_quantity, 0);
    assert.equal((await counters(page)).panelOpens, '1');
    await noPurchaseActions(page, networkAttempts);
  });
});

test('native controls refuse pre-existing selections, merged types and unknown layouts before quantity changes', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const fixtureOptions of [
      { initialQuantity: 1 }, { otherQuantity: 1 }, { mergedOther: true, otherQuantity: 1 }, { merged: true },
      { mergedOther: true, descriptionLabelMismatch: true },
      { wrongEvent: true }, { unexpectedStepper: true },
      { request: { categoryId: 'missing-type' } }, { request: { quantity: 7 } },
      { extra: '<style>#row-2{display:none}</style>' },
      { extra: '<script>document.getElementById("row-2").setAttribute("inert","")</script>' },
      { extra: '<script>document.querySelector("#row-2 [role=spinbutton]").setAttribute("aria-valuenow","1")</script>' },
      { extra: '<script>document.querySelector("#row-1 [role=spinbutton]").setAttribute("aria-valuenow","1")</script>' },
      { extra: '<script>document.querySelector("#row-1 [role=spinbutton]").setAttribute("aria-valuenow","1");document.getElementById("row-1").hidden=true</script>' },
    ]) {
      const { page, networkAttempts } = await open(context, fixture(fixtureOptions));
      const result = await inspectTicketmasterCartControls(page, target,
        { ...options(), ...fixtureOptions.request }, 1000, current);
      assert.notEqual(result.state, 'cart_controls_ready', JSON.stringify(fixtureOptions));
      assert.equal(result.inspection, undefined);
      assert.equal((await counters(page)).stepClicks, '0');
      await noPurchaseActions(page, networkAttempts);
      await page.close();
    }
  });
});

test('native controls stop on challenges, open dialogs and lost network or document guarantees', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const [extra, expected] of [
      ['<p>Ověřte, že jste člověk</p>', 'blocked'],
      ['<dialog id="warning">Upozornění: vstupenky nemusí být vedle sebe<button onclick="reserve()">Přijmout a pokračovat</button></dialog><script>document.getElementById("warning").showModal()</script>', 'cart_controls_modal_blocked'],
    ]) {
      const { page, networkAttempts } = await open(context, fixture({ extra }));
      const result = await inspectTicketmasterCartControls(page, target, options(), 1000, current);
      assert.equal(result.state, expected);
      assert.equal((await counters(page)).stepClicks, '0');
      await noPurchaseActions(page, networkAttempts);
      await page.close();
    }
    const first = await open(context, fixture());
    const request = { ...options(), assertNetworkDisabled: async () => Number((await counters(first.page)).stepClicks) === 0 };
    const lostNetwork = await inspectTicketmasterCartControls(first.page, target, request, 1000, current);
    assert.equal(lostNetwork.state, 'cart_controls_network_required');
    assert.equal((await counters(first.page)).increments, '1');
    assert.equal((await counters(first.page)).panelOpens, '0');
    await noPurchaseActions(first.page, first.networkAttempts);
    const second = await open(context, fixture());
    const lostDocument = await inspectTicketmasterCartControls(second.page, target, options(), 1000, async () => {
      if (Number((await counters(second.page)).stepClicks) > 0) throw new Error('fixture document changed');
    });
    assert.equal(lostDocument.state, 'cart_controls_changed');
    assert.equal((await counters(second.page)).increments, '1');
    assert.equal((await counters(second.page)).panelOpens, '0');
    await noPurchaseActions(second.page, second.networkAttempts);
  });
});

test('native controls reject ambiguous or oversized filter evidence without a successful report', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const fixtureOptions of [{ duplicatePanel: true }, { invalidLabel: true }]) {
      const { page, networkAttempts } = await open(context, fixture(fixtureOptions));
      const result = await inspectTicketmasterCartControls(page, target, options(), 1500, current);
      assert.notEqual(result.state, 'cart_controls_ready');
      assert.equal(result.inspection, undefined);
      await noPurchaseActions(page, networkAttempts);
      await page.close();
    }
  });
});

test('native controls reject event-date and configured-price changes during demo selection', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const change of [
      'state.eventInfo.dates={dateStatus:"singleDate",eventDate:"2026-11-16T18:30:00Z"}',
      'state.ticketSelection.ticketTypes[0].prices=[{id:"changed",faceValue:1000}]',
    ]) {
      const mutate = `const source=document.getElementById('__NEXT_DATA__');const metadata=JSON.parse(source.textContent);
        const state=metadata.props.pageProps.initialReduxState;${change};source.textContent=JSON.stringify(metadata);`;
      const { page, networkAttempts } = await open(context, fixture({ mutate }));
      const result = await inspectTicketmasterCartControls(page, target, options(), 2000, current);
      assert.equal(result.state, 'cart_controls_changed');
      assert.equal(result.inspection, undefined);
      assert.equal((await counters(page)).increments, '1');
      assert.equal((await counters(page)).panelOpens, '0');
      await noPurchaseActions(page, networkAttempts);
      await page.close();
    }
  });
});

async function noPurchaseDuringPreparation(page, networkAttempts) {
  const counts = await counters(page);
  for (const key of ['reserveClicks', 'seatClicks', 'resaleClicks', 'escapes', 'submitCalls', 'descriptionClicks'])
    assert.equal(counts[key], '0', key);
  assert.equal(networkAttempts(), 0, 'No provider request was made during the offline rehearsal');
}

test('native preparation rejects malformed explicit filters before touching a page', async () => {
  for (const selection of [null, [], {}, { priceId: '6', sectionIds: ['4P', '4P'] },
    { priceId: '6', sectionIds: ['4P"] button'] }, { priceId: '6', sectionIds: [], reserve: true }]) {
    const result = await inspectTicketmasterCartControls({}, target, { ...options(), selection }, 1000, current);
    assert.equal(result.state, 'cart_controls_unsupported');
  }
});

test('native preparation selects exact equal-price level and linked floor, reads back then restores all state', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const { page, networkAttempts } = await open(context, preparationFixture());
    const result = await inspectTicketmasterCartControls(page, target,
      { ...options(), selection: { priceId: '5', sectionIds: ['4P'] } }, 15000, current);
    assert.equal(result.state, 'cart_controls_ready');
    const report = result.inspection;
    assert.deepEqual(report.preparation.section_ids, ['4P']);
    assert.deepEqual(report.preparation.effective_section_ids, ['4P']);
    assert.equal(report.preparation.price_id, '5');
    assert.equal(report.preparation.price_option.label, '2 125,00 Kč • Sezení 4. kategorie');
    const pricePanel = report.panels.find(panel => panel.kind === 'price');
    const selected = pricePanel.options.filter(option => option.checked);
    assert.equal(selected.length, 1);
    assert.equal(selected[0].checkbox_id, report.preparation.price_option.checkbox_id);
    assert.equal(selected[0].label, report.preparation.price_option.label);
    assert.deepEqual(report.panels.find(panel => panel.kind === 'section').options.filter(option => option.checked)
      .map(option => option.label), ['4. POSCHODÍ']);
    assert.equal(report.preparation.section_options[0].label, '4. POSCHODÍ');
    assert.equal(report.preparation.filters_restored, true);
    assert.equal(report.form_restored, true);
    assert.equal(report.final_quantity, 0);
    assert.equal((await counters(page)).checkboxChanges, '2');
    assert.equal(await page.getByRole('button', { name: 'Všechny ceny', exact: true }).isDisabled(), true);
    assert.equal(await page.getByRole('button', { name: 'Všechny sekce', exact: true }).isDisabled(), true);
    await assertCartControlsCurrent(page, target, report);
    await assert.rejects(assertCartControlsCurrent(page, target,
      { ...report, preparation: { ...report.preparation, price_id: '4' } }), /cart_controls_changed/);
    await noPurchaseDuringPreparation(page, networkAttempts);
  });
});

test('native preparation records auto-linked floor scope for an empty request and honors a chosen compatible subset', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const requested of [[], ['1P']]) {
      const { page, networkAttempts } = await open(context, preparationFixture({ multiplePriceSections: true }));
      const result = await inspectTicketmasterCartControls(page, target,
        { ...options(), selection: { priceId: '6', sectionIds: requested } }, 15000, current);
      assert.equal(result.state, 'cart_controls_ready');
      assert.deepEqual(result.inspection.preparation.section_ids, requested);
      assert.deepEqual(result.inspection.preparation.effective_section_ids, requested.length ? requested : ['1P', '4P']);
      assert.equal(result.inspection.final_quantity, 0);
      await noPurchaseDuringPreparation(page, networkAttempts);
      await page.close();
    }
  });
});

test('native preparation refuses incompatible, unknown or preselected filters before changing any checkbox', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const [setup, selection] of [
      [{}, { priceId: '6', sectionIds: ['1P'] }],
      [{}, { priceId: 'absent', sectionIds: [] }],
      [{ preselected: true }, { priceId: '6', sectionIds: ['4P'] }],
      [{ discountType: 'PLATINUM' }, { priceId: '6', sectionIds: [] }],
    ]) {
      const { page, networkAttempts } = await open(context, preparationFixture(setup));
      const result = await inspectTicketmasterCartControls(page, target, { ...options(), selection }, 10000, current);
      assert.equal(result.state, 'cart_controls_unrecognized');
      assert.equal(result.inspection, undefined);
      assert.equal((await counters(page)).checkboxChanges, '0');
      await noPurchaseDuringPreparation(page, networkAttempts);
      await page.close();
    }
  });
});

test('native preparation rejects missing Apply, ignored changes and preference drift without claiming restoration', { skip: !enabled }, async () => {
  await browserTest(async context => {
    for (const setup of [{ missingApply: true }, { brokenApply: true },
      { multiplePriceSections: true, lostPriceAfterSection: true }]) {
      const { page, networkAttempts } = await open(context, preparationFixture(setup));
      const result = await inspectTicketmasterCartControls(page, target,
        { ...options(), selection: { priceId: '6', sectionIds: setup.multiplePriceSections ? ['1P'] : [] } }, 4000, current);
      assert.notEqual(result.state, 'cart_controls_ready');
      assert.equal(result.inspection, undefined);
      await noPurchaseDuringPreparation(page, networkAttempts);
      await page.close();
    }
  });
});

test('native preparation stops when the network fence is lost during a checkbox change', { skip: !enabled }, async () => {
  await browserTest(async context => {
    const { page, networkAttempts } = await open(context, preparationFixture());
    const result = await inspectTicketmasterCartControls(page, target,
      { ...options(), selection: { priceId: '6', sectionIds: [] },
        assertNetworkDisabled: async () => Number((await counters(page)).checkboxChanges) === 0 }, 10000, current);
    assert.equal(result.state, 'cart_controls_network_required');
    assert.equal(result.inspection, undefined);
    assert.equal((await counters(page)).checkboxChanges, '1');
    await noPurchaseDuringPreparation(page, networkAttempts);
  });
});
