'use strict';

const test = require('node:test');
const assert = require('node:assert/strict');
const { mapPriceOptions, mapSectionOptions } = require('./native-filter-mapping.cjs');

function binding() {
  return {
    category_id: '2', category_title: 'Sezení',
    event_configuration: { currencyCode: 'CZK' },
    fee_configuration: { handlingFeeValue: 0, feeRollup: 'ROLLUP', priceDisplays: { primary: 'ROLLUP' } },
    categories: [{ id: '2', title: 'Sezení', discountType: 'STANDARD', locked: false, membershipLocked: false,
      prices: [
        { id: '4', faceValue: 1750, serviceFeeChargesValue: 375, upsellFeeChargesValue: 0,
          priceBreakdown: [{ name: 'Face value', value: 175000 }] },
        { id: '5', faceValue: 1750, serviceFeeChargesValue: 375, upsellFeeChargesValue: 0,
          priceBreakdown: [{ name: 'Face value', value: 175000 }] },
        { id: '6', faceValue: 1550, serviceFeeChargesValue: 339, upsellFeeChargesValue: 0,
          priceBreakdown: [{ name: 'Face value', value: 155000 }] },
      ] }],
    price_levels: [{ id: '4', title: '3. kategorie' }, { id: '5', title: '4. kategorie' },
      { id: '6', title: '5. kategorie' }, { id: '1', title: 'Stání' }],
    sections: [{ id: '1P', title: '1. POSCHODÍ', rawId: '1P' },
      { id: '4P', title: '4. POSCHODÍ', rawId: '4P' }, { id: 'PL', title: 'Plocha', rawId: 'PL' }],
    price_to_sections: [{ priceIdentifier: '4', sectionIdentifier: '1P' },
      { priceIdentifier: '5', sectionIdentifier: '4P' }, { priceIdentifier: '6', sectionIdentifier: '4P' },
      { priceIdentifier: '1', sectionIdentifier: 'PL' }],
  };
}

const option = (checkbox_id, label, checked = false, disabled = false) => ({ checkbox_id, label, checked, disabled });
const prices = () => [
  option('_r_a_-input', '2 125,00 Kč • Sezení 4. kategorie'),
  option('_r_d_-input', '2 125,00 Kč • Sezení 3. kategorie'),
  option('_r_g_-input', '1 889,00 Kč • Sezení 5. kategorie'),
];
const sections = () => [option('_r_k_-input', '1. POSCHODÍ', false, true),
  option('_r_n_-input', '4. POSCHODÍ', true)];
const rejects = run => assert.throws(run, error => error.message === 'cart_controls_unrecognized'
  && error.code === 'cart_controls_unrecognized');

test('equal prices and changed React checkbox IDs map by complete labels, never position', () => {
  const source = binding(), rendered = prices();
  const before = structuredClone({ source, rendered });
  const mapped = mapPriceOptions(source, rendered);
  assert.deepEqual(mapped.map(entry => entry.price_id), ['5', '4', '6']);
  assert.deepEqual(mapped.map(entry => entry.amount_minor), [212500, 212500, 188900]);
  assert.deepEqual(mapped.map(entry => entry.section_ids), [['4P'], ['1P'], ['4P']]);
  assert.deepEqual({ source, rendered }, before);
  rendered.reverse();
  rendered.forEach((entry, index) => { entry.checkbox_id = `new-${index}`; });
  assert.deepEqual(mapPriceOptions(source, rendered).map(entry => entry.price_id), ['6', '4', '5']);
});

test('normalizes Unicode labels but preserves exact amounts and category/level titles', () => {
  const source = binding(), rendered = prices();
  rendered[0].label = '2\u00a0125,00 Kč • Sezeni\u0301   4. kategorie';
  assert.equal(mapPriceOptions(source, rendered)[0].price_id, '5');
  for (const malformed of ['2 125,01 Kč • Sezení 4. kategorie', '2 125,00 EUR • Sezení 4. kategorie',
    '2 125,00 Kč • Stání 4. kategorie', '2 125,00 Kč • Sezení 3. kategorie',
    '2 125,00 Kč • sezení 4. kategorie', '21 25,00 Kč • Sezení 4. kategorie',
    '2 125.00 Kč • Sezení 4. kategorie', '2 125,00 Kč • Sezení 4. kategorie\u0085']) {
    const changed = prices(); changed[0].label = malformed;
    rejects(() => mapPriceOptions(source, changed));
  }
});

test('requires a complete one-to-one mapping, including disabled options', () => {
  const source = binding();
  for (const render of [prices().slice(1), [...prices(), option('extra', '1 889,00 Kč • Sezení 5. kategorie')],
    [prices()[0], prices()[0], prices()[2]]]) rejects(() => mapPriceOptions(source, render));
  const changed = prices(); changed[1].disabled = true;
  assert.equal(mapPriceOptions(source, changed)[1].disabled, true);
  for (const change of [
    input => { input.categories[0].prices.push(structuredClone(input.categories[0].prices[0])); },
    input => { input.categories[0].prices[1].id = 'another'; input.price_levels.push({ id: 'another', title: '3. kategorie' }); },
    input => { input.price_levels.push(structuredClone(input.price_levels[0])); },
    input => { input.categories.push({ ...structuredClone(input.categories[0]), id: 'other' }); },
    input => { input.categories[0].locked = true; },
    input => { input.category_title = 'Other'; },
  ]) { const invalid = binding(); change(invalid); rejects(() => mapPriceOptions(invalid, prices())); }
});

test('configuration money uses exact minor units and validates the face-value breakdown', () => {
  for (const value of [-1, -0, NaN, Infinity, 0.001, 100000000.01, '1750']) {
    const source = binding(); source.categories[0].prices[0].faceValue = value;
    rejects(() => mapPriceOptions(source, prices()));
  }
  for (const change of [
    input => { input.categories[0].prices[0].priceBreakdown[0].value = 174999; },
    input => { input.categories[0].prices[0].serviceFeeChargesValue = 375.001; },
    input => { input.categories[0].prices[0].priceBreakdown = []; },
    input => { input.categories[0].prices[0].priceBreakdown[0].value = 175000.5; },
    input => { input.fee_configuration.handlingFeeValue = -1; },
    input => { input.event_configuration.currencyCode = 'EUR'; },
    input => { input.fee_configuration.feeRollup = 'NONE'; },
  ]) { const invalid = binding(); change(invalid); rejects(() => mapPriceOptions(invalid, prices())); }
  const source = binding(); source.categories[0].prices[0].serviceFeeChargesValue = 375.15;
  const rendered = prices(); rendered[1].label = '2 125,15 Kč • Sezení 3. kategorie';
  assert.equal(mapPriceOptions(source, rendered)[1].amount_minor, 212515);
});

test('alternate ticket-type and area-selector contracts cannot masquerade as standard price IDs', () => {
  for (const discountType of ['PLATINUM', 'COLLECTOR', 'BART', 'standard', '', undefined]) {
    const source = binding(); source.categories[0].discountType = discountType;
    rejects(() => mapPriceOptions(source, prices()));
    rejects(() => mapSectionOptions(source, sections()));
  }
});

test('section choices map only to the selected category floor metadata without guessing seat sections', () => {
  const mapped = mapSectionOptions(binding(), sections().reverse());
  assert.deepEqual(mapped.map(entry => entry.section_id), ['4P', '1P']);
  assert.equal(mapped[1].disabled, true);
  assert.equal(mapped[0].checked, true);
  for (const change of [
    values => { values[0].label = '110'; },
    values => { values[0].label = '1P'; },
    values => { values.pop(); },
    values => { values.push(option('floor-extra', 'Plocha')); },
    values => { values[1].label = values[0].label; },
  ]) { const invalid = sections(); change(invalid); rejects(() => mapSectionOptions(binding(), invalid)); }
});

test('section mapping rejects ambiguous metadata, missing mappings and unknown references', () => {
  for (const change of [
    input => { input.sections[1].title = input.sections[0].title; },
    input => { input.sections.push(structuredClone(input.sections[0])); },
    input => { input.price_to_sections.push(structuredClone(input.price_to_sections[0])); },
    input => { input.price_to_sections[0].sectionIdentifier = 'UNKNOWN'; },
    input => { input.price_to_sections[0].priceIdentifier = 'UNKNOWN'; },
    input => { input.price_to_sections = []; },
  ]) { const invalid = binding(); change(invalid); rejects(() => mapSectionOptions(invalid, sections())); }
});

test('malformed input and option state fail with fixed diagnostics, without echoing input', () => {
  for (const input of [null, {}, { private: 'private-marker' }]) rejects(() => mapPriceOptions(input, prices()));
  for (const change of [
    values => { values[0].checkbox_id = 'private-marker<script>'; },
    values => { values[0].checked = 'yes'; },
    values => { values[0].disabled = null; },
    values => { values[0].label = 'private-marker'.repeat(100); },
  ]) { const invalid = prices(); change(invalid); rejects(() => mapPriceOptions(binding(), invalid)); }
});
