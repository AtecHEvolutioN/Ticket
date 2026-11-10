'use strict';

// Read-only mapping of an already bound Czech form's rendered controls. React
// checkbox IDs are ephemeral. A configured ID is accepted only when every
// option has a unique, complete label/amount match; list order is irrelevant.
const MAX_MONEY_MINOR = 10_000_000_000;
const IDENTIFIER = /^[A-Za-z0-9_-]{1,64}$/;
const CHECKBOX_ID = /^[A-Za-z0-9_:-]{1,128}$/;

function fail() {
  const error = new Error('cart_controls_unrecognized');
  error.code = 'cart_controls_unrecognized';
  throw error;
}

function list(value, maximum, minimum = 0) {
  if (!Array.isArray(value) || value.length < minimum || value.length > maximum) fail();
  return value;
}

function label(value, maximum = 1024) {
  if (typeof value !== 'string' || !value.length || Buffer.byteLength(value, 'utf8') > maximum
      || /[\u0000-\u001f\u007f-\u009f]/.test(value)) fail();
  const normalized = value.normalize('NFC').replace(/\s+/gu, ' ').trim();
  if (!normalized.length) fail();
  return normalized;
}

function id(value) {
  if (typeof value !== 'string' || !IDENTIFIER.test(value)) fail();
  return value;
}

function keyed(values, maximum) {
  const result = new Map();
  for (const entry of list(values, maximum)) {
    const key = id(entry?.id);
    if (result.has(key)) fail();
    result.set(key, entry);
  }
  return result;
}

// String conversion preserves the numeric JSON value we received; integer
// decimal arithmetic avoids rounding a sub-minor-unit value into a price.
function money(value) {
  if (typeof value !== 'number' || !Number.isFinite(value) || Object.is(value, -0)) fail();
  const parts = /^(0|[1-9][0-9]*)(?:\.([0-9]{1,2}))?$/.exec(String(value));
  if (!parts) fail();
  const amount = BigInt(parts[1]) * 100n + BigInt((parts[2] || '').padEnd(2, '0'));
  if (amount > BigInt(MAX_MONEY_MINOR)) fail();
  return Number(amount);
}

function configuredPrice(price) {
  const priceId = id(price?.id);
  const face = money(price.faceValue);
  const service = money(price.serviceFeeChargesValue);
  const upsell = money(price.upsellFeeChargesValue);
  let breakdown = 0;
  for (const item of list(price.priceBreakdown, 32, 1)) {
    label(item?.name);
    if (!Number.isSafeInteger(item?.value) || item.value < 0 || item.value > MAX_MONEY_MINOR) fail();
    breakdown += item.value;
    if (breakdown > MAX_MONEY_MINOR) fail();
  }
  const amount = face + service + upsell;
  if (breakdown !== face || amount > MAX_MONEY_MINOR) fail();
  return { price_id: priceId, amount_minor: amount };
}

function configuration(binding) {
  if (!binding || binding.event_configuration?.currencyCode !== 'CZK'
      || binding.fee_configuration?.feeRollup !== 'ROLLUP'
      || binding.fee_configuration?.priceDisplays?.primary !== 'ROLLUP') fail();
  money(binding.fee_configuration.handlingFeeValue);
  const categories = keyed(binding.categories, 128);
  const selected = categories.get(id(binding.category_id));
  // The reviewed STANDARD path uses price-level IDs. Platinum can render an
  // area-list keyed by ticket-type IDs, while collector pricing adds a separate
  // surcharge. Neither alternate contract is inferred from a similar label.
  if (!selected || selected.discountType !== 'STANDARD'
      || selected.locked !== false || selected.membershipLocked !== false) fail();
  const title = label(selected.title);
  if (title !== label(binding.category_title)
      || [...categories.values()].filter(category => label(category.title) === title).length !== 1) fail();
  const levels = keyed(binding.price_levels, 256);
  for (const level of levels.values()) label(level.title);
  const prices = list(selected.prices, 128, 1).map(configuredPrice);
  if (new Set(prices.map(price => price.price_id)).size !== prices.length) fail();
  for (const price of prices) if (!levels.has(price.price_id)) fail();
  return { category_id: selected.id, category_title: title, prices, levels };
}

function options(values) {
  const seen = new Set();
  return list(values, 128, 1).map(option => {
    if (!option || typeof option.checkbox_id !== 'string' || !CHECKBOX_ID.test(option.checkbox_id)
        || seen.has(option.checkbox_id) || typeof option.checked !== 'boolean'
        || typeof option.disabled !== 'boolean') fail();
    seen.add(option.checkbox_id);
    return { checkbox_id: option.checkbox_id, label: label(option.label, 500),
      checked: option.checked, disabled: option.disabled };
  });
}

function bijection(rendered, expected, key) {
  if (rendered.length !== expected.size) fail();
  const used = new Set();
  const result = rendered.map(option => {
    const match = expected.get(key(option));
    if (!match || used.has(match)) fail();
    used.add(match);
    return { ...option, ...match };
  });
  if (used.size !== expected.size) fail();
  return result;
}

function renderedPrice(option) {
  // Supported Czech display: a nonnegative amount with two decimal places,
  // ordinary or normalized nonbreaking grouping spaces, Kč, then one subtitle.
  const parts = /^(0|[1-9][0-9]*|[1-9][0-9]{0,2}(?: [0-9]{3})+),([0-9]{2}) Kč • (.+)$/u.exec(option.label);
  if (!parts) fail();
  const amount = BigInt(parts[1].replace(/ /g, '')) * 100n + BigInt(parts[2]);
  if (amount > BigInt(MAX_MONEY_MINOR)) fail();
  return JSON.stringify([Number(amount), parts[3]]);
}

function mapPriceOptions(binding, values) {
  const config = configuration(binding);
  const mappedSections = sectionConfiguration(binding, config);
  const expected = new Map();
  for (const price of config.prices) {
    const title = label(config.levels.get(price.price_id).title);
    const key = JSON.stringify([price.amount_minor, `${config.category_title} ${title}`]);
    if (expected.has(key)) fail();
    expected.set(key, { ...price, category_id: config.category_id,
      category_title: config.category_title, price_level_title: title,
      section_ids: [...mappedSections.byPrice.get(price.price_id)].sort() });
  }
  return bijection(options(values), expected, renderedPrice);
}

function sectionConfiguration(binding, config) {
  const sections = keyed(binding.sections, 256);
  for (const section of sections.values()) { id(section.rawId); label(section.title); }
  const allowedPrices = new Set(config.prices.map(price => price.price_id));
  const byPrice = new Map(config.prices.map(price => [price.price_id, new Set()]));
  const seenMappings = new Set();
  for (const entry of list(binding.price_to_sections, 2048)) {
    const price = id(entry?.priceIdentifier), section = id(entry?.sectionIdentifier);
    const key = JSON.stringify([price, section]);
    if (!config.levels.has(price) || !sections.has(section) || seenMappings.has(key)) fail();
    seenMappings.add(key);
    if (allowedPrices.has(price)) byPrice.get(price).add(section);
  }
  return { sections, byPrice };
}

function mapSectionOptions(binding, values) {
  const config = configuration(binding);
  const { sections, byPrice } = sectionConfiguration(binding, config);
  const selectedSections = new Set([...byPrice.values()].flatMap(value => [...value]));
  const expected = new Map();
  for (const sectionId of selectedSections) {
    const title = label(sections.get(sectionId).title);
    if (expected.has(title)) fail();
    expected.set(title, { section_id: sectionId, section_title: title });
  }
  return bijection(options(values), expected, option => option.label);
}

module.exports = { mapPriceOptions, mapSectionOptions };
