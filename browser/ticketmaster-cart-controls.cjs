'use strict';

const { eventIdentity, sameEvent } = require('./rendered.cjs');
const { inspectChallenge } = require('./manual-intervention.cjs');
const { mapPriceOptions, mapSectionOptions } = require('./native-filter-mapping.cjs');

const IDENTIFIER = /^[A-Za-z0-9_-]{1,128}$/;
const completed = new WeakMap();
const active = new WeakSet();

function failure(code) { const error = new Error(code); error.code = code; return error; }

const { readSurface } = require('./native-cart-surface.cjs');

async function inspectTicketmasterCartControls(page, target, options, budgetMs, assertCurrent) {
  const identity = eventIdentity(target);
  if (!identity || identity.host !== 'ticketmaster.cz' || !/^[0-9]{1,20}$/.test(identity.id)
      || !options || typeof options.categoryId !== 'string' || !IDENTIFIER.test(options.categoryId)
      || !Number.isSafeInteger(options.quantity) || options.quantity < 1 || options.quantity > 20
      || !Number.isFinite(budgetMs) || budgetMs < 1 || typeof assertCurrent !== 'function') {
    return { state: 'cart_controls_unsupported' };
  }
  const selection = options.selection;
  if (selection !== undefined && (!selection || typeof selection !== 'object' || Array.isArray(selection)
      || Object.keys(selection).some(key => !['priceId', 'sectionIds'].includes(key))
      || typeof selection.priceId !== 'string' || !IDENTIFIER.test(selection.priceId)
      || !Array.isArray(selection.sectionIds) || selection.sectionIds.length > 128
      || selection.sectionIds.some(id => typeof id !== 'string' || !IDENTIFIER.test(id))
      || new Set(selection.sectionIds).size !== selection.sectionIds.length)) return { state: 'cart_controls_unsupported' };
  if (options.networkDisabled !== true || typeof options.assertNetworkDisabled !== 'function') {
    return { state: 'cart_controls_network_required' };
  }
  try { if (await options.assertNetworkDisabled() !== true) return { state: 'cart_controls_network_required' }; }
  catch { return { state: 'cart_controls_network_required' }; }
  if (active.has(page)) return { state: 'cart_controls_changed' };
  active.add(page);
  const previous = completed.get(page);
  completed.delete(page);
  await previous?.root.dispose().catch(() => {});
  const deadline = performance.now() + Math.min(budgetMs, selection ? 30000 : 15000);
  const started = Date.now();
  let root, signature, initialFilters, filterLabels;
  const args = { categoryId: options.categoryId, quantity: options.quantity, eventId: identity.id };
  const remaining = () => {
    const value = Math.floor(deadline - performance.now());
    if (value < 1) throw failure('cart_controls_timeout');
    return Math.min(value, 1500);
  };
  const baseUrl = () => { const url = new URL(page.url()); url.hash = ''; return url.href; };
  let originalUrl;
  const read = (phase, kind) => page.evaluate(readSurface, { ...args, phase, kind, filterLabels });
  const bindingMatches = binding => binding && sameEvent(target, binding.canonical)
    && sameEvent(target, binding.embedded_canonical)
    && (!signature || JSON.stringify(binding) === signature);
  const guard = async (panelAllowed = false, finalCheck = false, afterAction = false) => {
    if (!finalCheck) remaining();
    try { if (await options.assertNetworkDisabled() !== true) throw new Error(); }
    catch { throw failure('cart_controls_network_required'); }
    if (!sameEvent(target, page.url()) || baseUrl() !== originalUrl) throw failure('cart_controls_changed');
    try { await assertCurrent({ afterCartControlsAction: afterAction }); }
    catch { throw failure('cart_controls_changed'); }
    if (!await root.evaluate(node => node === document.documentElement && node.isConnected)) throw failure('cart_controls_changed');
    if (await inspectChallenge(page)) throw failure('blocked');
    const modal = await page.evaluate(allowPanel => {
      const shown = node => {
        for (let current = node; current; current = current.parentElement) {
          const style = getComputedStyle(current);
          if (current.hidden || current.hasAttribute('inert') || current.getAttribute('aria-hidden') === 'true'
              || style.display === 'none' || ['hidden', 'collapse'].includes(style.visibility) || Number(style.opacity) === 0) return false;
        }
        return node.getClientRects().length > 0;
      };
      return [...document.querySelectorAll('dialog[open],[role="dialog"]')].some(node => shown(node)
        && !(allowPanel && node.querySelector('[data-testid="panelPriceSection"]')));
    }, panelAllowed);
    if (modal) throw failure('cart_controls_modal_blocked');
    if (!bindingMatches(await read('binding'))) throw failure('cart_controls_changed');
  };
  const form = async expectedQuantity => {
    const surface = await read('form');
    if (!surface || !bindingMatches(surface.binding)
        || (expectedQuantity !== undefined && surface.quantity !== expectedQuantity)) throw failure('cart_controls_unrecognized');
    return surface;
  };
  const waitSurface = async (phase, kind, expectedQuantity) => {
    while (true) {
      await guard(true);
      const surface = await read(phase, kind);
      if (surface && bindingMatches(surface.binding)
          && (expectedQuantity === undefined || surface.quantity === expectedQuantity)) return surface;
      await page.waitForTimeout(Math.min(25, remaining()));
    }
  };
  const changeQuantity = async (from, to) => {
    const increment = to > from;
    for (let value = from; value !== to; value += increment ? 1 : -1) {
      await guard();
      const surface = await form(value);
      if (increment ? surface.increment_disabled : surface.decrement_disabled) throw failure('cart_controls_unrecognized');
      const control = await page.locator('[data-testid="quantityStepper"]').nth(surface.stepper_index)
        .locator(':scope > button').nth(increment ? 1 : 0).elementHandle();
      if (!control) throw failure('cart_controls_changed');
      try { await control.click({ timeout: remaining() }); } finally { await control.dispose(); }
      await guard(false, false, true);
      // Category rows can disappear or reorder after a native increment.
      // Resolve the selected category afresh through the semantic surface.
      await waitSurface('form', undefined, value + (increment ? 1 : -1));
      await guard();
      await form(value + (increment ? 1 : -1));
    }
  };
  const closePanel = async () => {
    let close;
    for (const [id, name] of [['panelClose', 'Hotovo'], ['panelBack', 'Zpět']]) {
      const candidates = page.locator(`button[data-testid="${id}"]`).filter({ visible: true });
      if (await candidates.count() === 1 && await candidates.isEnabled()
          && (await candidates.innerText()).replace(/\s+/g, ' ').trim() === name
          && await candidates.getAttribute('type') === 'button') { close = candidates; break; }
    }
    if (!close) throw failure('cart_controls_unrecognized');
    await guard(true);
    await close.click({ timeout: remaining() });
    await guard(true, false, true);
    await waitSurface('form', undefined, options.quantity);
    await guard();
  };
  const openPanel = async kind => {
    await guard();
    const surface = await form(options.quantity);
    const filter = surface.filters.find(item => item.kind === kind);
    if (filter.disabled) throw failure('cart_controls_unrecognized');
    await page.getByRole('button', { name: filter.label, exact: true }).click({ timeout: remaining() });
    await guard(true, false, true);
    return waitSurface('panel', kind);
  };
  const mappedPanel = async kind => {
    const panel = await read('panel', kind);
    if (!panel || !bindingMatches(panel.binding)) throw failure('cart_controls_changed');
    return { panel, mapped: kind === 'price' ? mapPriceOptions(panel.binding, panel.options)
      : mapSectionOptions(panel.binding, panel.options) };
  };
  const checkedIds = (mapped, kind) => mapped.filter(item => item.checked)
    .map(item => kind === 'price' ? item.price_id : item.section_id).sort();
  const assertChecks = (mapped, kind, expected) => {
    if (JSON.stringify(checkedIds(mapped, kind)) !== JSON.stringify([...expected].sort())
        || mapped.some(item => item.checked && item.disabled)) throw failure('cart_controls_unrecognized');
  };
  const setChecks = async (kind, desired) => {
    const idOf = item => kind === 'price' ? item.price_id : item.section_id;
    let { mapped } = await mappedPanel(kind);
    if (desired.some(id => !mapped.some(item => idOf(item) === id && !item.disabled)))
      throw failure('cart_controls_unrecognized');
    const ids = mapped.map(idOf);
    let changes = false;
    for (const id of ids) {
      await guard(true);
      ({ mapped } = await mappedPanel(kind));
      const option = mapped.find(item => idOf(item) === id);
      const wanted = desired.includes(id);
      if (!option || option.disabled && option.checked) throw failure('cart_controls_unrecognized');
      if (option.checked === wanted) continue;
      if (option.disabled) throw failure('cart_controls_unrecognized');
      const control = page.locator('[data-testid="panelPriceSection"]')
        .locator(`label[for="${option.checkbox_id}"]`);
      if (await control.count() !== 1) throw failure('cart_controls_changed');
      await control.click({ timeout: remaining() });
      changes = true;
      await guard(true, false, true);
      while (true) {
        const current = await mappedPanel(kind);
        const changed = current.mapped.find(item => idOf(item) === id);
        if (changed?.checked === wanted) break;
        await page.waitForTimeout(Math.min(25, remaining()));
        await guard(true);
      }
    }
    ({ mapped } = await mappedPanel(kind));
    assertChecks(mapped, kind, desired);
    return changes;
  };
  const applyPanel = async expectedLabels => {
    await guard(true);
    const apply = page.locator('[data-testid="panelPriceSection"]')
      .getByRole('button', { name: 'Použít filtry', exact: true });
    if (await apply.count() !== 1 || !await apply.isEnabled()
        || await apply.getAttribute('type') !== 'button') throw failure('cart_controls_unrecognized');
    await apply.click({ timeout: remaining() });
    filterLabels = expectedLabels;
    await guard(true, false, true);
    await waitSurface('form', undefined, options.quantity);
    await guard();
  };
  try {
    originalUrl = baseUrl();
    root = await page.locator('html').elementHandle();
    if (!root) throw failure('cart_controls_unrecognized');
    await guard();
    const initial = await form(0);
    signature = JSON.stringify(initial.binding);
    initialFilters = JSON.stringify(initial.filters);
    await changeQuantity(0, options.quantity);
    let panels = [];
    for (const kind of ['price', 'section']) {
      await guard();
      const surface = await form(options.quantity);
      const filter = surface.filters.find(item => item.kind === kind);
      if (filter.disabled) {
        if (selection) throw failure('cart_controls_unrecognized');
        panels.push({ kind, heading: filter.label, opened: false, disabled: true, options: [] });
        continue;
      }
      await page.getByRole('button', { name: filter.label, exact: true }).click({ timeout: remaining() });
      await guard(true, false, true);
      const panel = await waitSurface('panel', kind);
      panels.push({ kind, heading: panel.heading, opened: true, disabled: false, options: panel.options });
      await closePanel();
    }
    let preparation;
    if (selection) {
      const prices = mapPriceOptions(initial.binding, panels.find(panel => panel.kind === 'price').options);
      const sections = mapSectionOptions(initial.binding, panels.find(panel => panel.kind === 'section').options);
      assertChecks(prices, 'price', []);
      assertChecks(sections, 'section', []);
      const price = prices.find(item => item.price_id === selection.priceId);
      if (!price || price.disabled || !price.section_ids.length
          || selection.sectionIds.some(id => !price.section_ids.includes(id))
          || initial.binding.filter_translations.apply !== 'Použít filtry'
          || initial.binding.filter_translations.sections !== 'Sekce') throw failure('cart_controls_unrecognized');
      const effectiveSections = [...(selection.sectionIds.length ? selection.sectionIds : price.section_ids)].sort();
      const sectionLabel = ids => {
        if (ids.length === 1) {
          const selected = sections.find(item => item.section_id === ids[0]);
          if (!selected || selected.disabled) throw failure('cart_controls_unrecognized');
          return selected.label;
        }
        return `${ids.length} ${initial.binding.filter_translations.sections}`;
      };
      const priceLabel = price.label.split(' • ')[0];
      const automaticLabels = { price: priceLabel, section: sectionLabel(price.section_ids) };
      const preparedLabels = { price: priceLabel, section: sectionLabel(effectiveSections) };
      await openPanel('price');
      await setChecks('price', [selection.priceId]);
      await applyPanel(automaticLabels);
      await openPanel('section');
      if (await setChecks('section', effectiveSections)) await applyPanel(preparedLabels);
      else await closePanel();
      const preparedPanels = [];
      let selectedPrice, selectedSections;
      for (const kind of ['price', 'section']) {
        await openPanel(kind);
        const { panel, mapped } = await mappedPanel(kind);
        assertChecks(mapped, kind, kind === 'price' ? [selection.priceId] : effectiveSections);
        preparedPanels.push({ kind, heading: panel.heading, opened: true, disabled: false, options: panel.options });
        if (kind === 'price') selectedPrice = mapped.find(item => item.price_id === selection.priceId);
        else selectedSections = mapped.filter(item => effectiveSections.includes(item.section_id));
        await closePanel();
      }
      // Deselect the exact price through its native checkbox. The observed
      // native reducer also clears its linked section selection; verify both
      // panels after Apply rather than assuming the dependency was restored.
      await openPanel('price');
      await setChecks('price', []);
      await applyPanel(undefined);
      for (const kind of ['price', 'section']) {
        await openPanel(kind);
        const { mapped } = await mappedPanel(kind);
        assertChecks(mapped, kind, []);
        await closePanel();
      }
      panels = preparedPanels;
      preparation = {
        price_id: selection.priceId, section_ids: [...selection.sectionIds], effective_section_ids: effectiveSections,
        price_option: { checkbox_id: selectedPrice.checkbox_id, label: selectedPrice.label },
        section_options: selectedSections.map(item => ({ section_id: item.section_id,
          checkbox_id: item.checkbox_id, label: item.label })),
        price_selection_verified: true, section_selection_verified: true, filters_restored: true,
      };
    }
    await changeQuantity(options.quantity, 0);
    await guard();
    const restored = await form(0);
    if (JSON.stringify(restored.filters) !== initialFilters) throw failure('cart_controls_changed');
    const inspection = {
      event_url: `https://www.ticketmaster.cz/event/${identity.id}`, event_id: identity.id,
      category_id: options.categoryId, category_title: initial.binding.category_title,
      demo_quantity: options.quantity, initial_quantity: 0, final_quantity: 0,
      started_at_ms: started, finished_at_ms: Date.now(), network_disabled_before_changes: true,
      form_restored: true, panels, stock_verified: false, adjacency_verified: false,
      fees_complete: false, hold_verified: false, reservation_attempted: false,
      ...(preparation ? { preparation } : {}),
    };
    const raw = JSON.stringify(inspection);
    completed.set(page, { root, check: async report => {
      if (JSON.stringify(report) !== raw) throw failure('cart_controls_changed');
      await guard(false, true);
      const current = await form(0);
      if (JSON.stringify(current.filters) !== initialFilters) throw failure('cart_controls_changed');
    } });
    return { state: 'cart_controls_ready', inspection };
  } catch (error) {
    await root?.dispose().catch(() => {});
    const known = ['cart_controls_changed', 'cart_controls_network_required', 'cart_controls_unrecognized',
      'cart_controls_timeout', 'cart_controls_modal_blocked', 'blocked'];
    return { state: known.includes(error.code) ? error.code
      : error.name === 'TimeoutError' ? 'cart_controls_timeout' : 'cart_controls_changed' };
  } finally { active.delete(page); }
}

async function assertCartControlsCurrent(page, target, inspection) {
  const saved = completed.get(page);
  if (!saved || !sameEvent(target, inspection?.event_url)) throw failure('cart_controls_changed');
  await saved.check(inspection);
}

async function releaseCartControlsInspection(page) {
  const saved = completed.get(page);
  completed.delete(page);
  await saved?.root.dispose().catch(() => {});
}

module.exports = { inspectTicketmasterCartControls, assertCartControlsCurrent, releaseCartControlsInspection };
