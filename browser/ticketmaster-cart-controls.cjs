'use strict';

const { eventIdentity, sameEvent } = require('./rendered.cjs');
const { inspectChallenge } = require('./manual-intervention.cjs');
const { mapPriceOptions, mapSectionOptions } = require('./native-filter-mapping.cjs');

const IDENTIFIER = /^[A-Za-z0-9_-]{1,128}$/;
const completed = new WeakMap();
const active = new WeakSet();

function failure(code) { const error = new Error(code); error.code = code; return error; }

// This helper runs in the page. Only bounded, non-secret form metadata leaves it.
function readSurface({ categoryId, eventId, quantity, phase, kind, filterLabels }) {
  const text = node => String(node?.innerText ?? node?.textContent ?? '').normalize('NFC').replace(/\s+/g, ' ').trim();
  const label = (value, max = 500) => typeof value === 'string' && value.length > 0
    && value.length <= max && value.trim() === value && !/[\u0000-\u001f\u007f]/.test(value);
  const shown = (node, ignoreOwnAria = false) => {
    if (!node?.isConnected) return false;
    for (let current = node; current; current = current.parentElement) {
      const style = getComputedStyle(current);
      if (current.hidden || current.hasAttribute('inert')
          || (current.getAttribute('aria-hidden') === 'true' && !(ignoreOwnAria && current === node))
          || style.display === 'none' || ['hidden', 'collapse'].includes(style.visibility)
          || style.contentVisibility === 'hidden' || Number(style.opacity) === 0) return false;
    }
    return [...node.getClientRects()].some(rect => rect.width > 0 && rect.height > 0);
  };
  const disabled = node => node.matches(':disabled') || Boolean(node.closest('[aria-disabled="true"], [inert]'));
  const integer = value => typeof value === 'string' && /^(0|[1-9][0-9]{0,3})$/.test(value)
    && Number(value) <= 1000 ? Number(value) : null;
  const typeTitle = node => {
    if (!shown(node)) return null;
    if (!node.children.length) return text(node);
    // The observed Platinum title contains one description button whose
    // screen-reader text otherwise contaminates the displayed category title.
    // Only that exact structure is recognized; no arbitrary suffix is removed.
    const own = [...node.childNodes].filter(child => child.nodeType === Node.TEXT_NODE)
      .map(child => child.textContent).join('').normalize('NFC').replace(/\s+/g, ' ').trim();
    if (node.children.length !== 1 || node.firstElementChild.tagName !== 'BUTTON'
        || text(node.firstElementChild) !== `Otevřít detailní popis pro: ${own}`) return null;
    return own;
  };
  const scripts = document.querySelectorAll('script#__NEXT_DATA__[type="application/json"]');
  if (scripts.length !== 1 || scripts[0].textContent.length > 2 * 1024 * 1024) return null;
  let next;
  try { next = JSON.parse(scripts[0].textContent); } catch { return null; }
  const state = next.props?.pageProps?.initialReduxState;
  const event = state?.eventInfo, selection = state?.ticketSelection;
  const types = selection?.ticketTypes;
  if (!event || !selection || !Array.isArray(types) || types.length < 1 || types.length > 128
      || [next.query?.eventId, state.eventId, event.eventId, event.params, selection.params?.eventId].some(id => id !== eventId)
      || [event, selection].some(item => item.stage !== 'success' || item.inFlightParams !== null)
      || state.config?.countryCode !== 'CZ' || state.config?.currencyCode !== 'CZK'
      || !label(event.name) || !Number.isSafeInteger(selection.eventTicketLimit)
      || selection.eventTicketLimit < quantity || selection.eventTicketLimit > 1000) return null;
  const canonical = document.querySelectorAll('link[rel="canonical"]');
  if (canonical.length !== 1 || typeof event.canonicalUrl !== 'string') return null;
  const ids = new Set();
  for (const type of types) {
    if (typeof type.id !== 'string' || !/^[A-Za-z0-9_-]{1,128}$/.test(type.id) || ids.has(type.id)
        || !label(type.title) || type.selectedQuantity !== 0) return null;
    ids.add(type.id);
  }
  const matches = types.filter(type => type.id === categoryId);
  if (matches.length !== 1) return null;
  const category = matches[0];
  if (types.filter(type => type.title === category.title).length !== 1
      || category.locked !== false || category.membershipLocked !== false
      || !Array.isArray(category.quantities) || category.quantities.length > 1001
      || new Set(category.quantities).size !== category.quantities.length
      || category.quantities.some(n => !Number.isSafeInteger(n) || n < 0 || n > selection.eventTicketLimit)
      || !category.quantities.includes(0) || !category.quantities.includes(quantity)) return null;
  if ((Array.isArray(state.basket?.items) && state.basket.items.length)
      || (state.basket?.itemsById && Object.keys(state.basket.itemsById).length)
      || state.reserve?.info || state.reserve?.stage) return null;
  const pick = (value, keys) => Object.fromEntries(keys.map(key => [key, value?.[key]]));
  const list = (value, project) => Array.isArray(value) ? value.map(project) : value;
  const binding = {
    event_name: event.name, category_id: category.id, category_title: category.title,
    quantities: category.quantities, event_ticket_limit: selection.eventTicketLimit,
    canonical: canonical[0].href, embedded_canonical: new URL(event.canonicalUrl, location.href).href,
    event_configuration: {
      ...pick(event, ['timeZone', 'eventChangeStatus', 'currencyCode']),
      dates: pick(event.dates, ['dateStatus', 'dateDisplay', 'eventDate', 'startDate', 'endDate']),
    },
    categories: types.map(type => ({
      ...pick(type, ['id', 'title', 'discountType', 'locked', 'membershipLocked', 'quantities', 'selectedQuantity']),
      prices: list(type.prices, price => ({
        ...pick(price, ['id', 'faceValue', 'serviceFeeChargesValue', 'upsellFeeChargesValue']),
        priceBreakdown: list(price.priceBreakdown, item => pick(item, ['name', 'value'])),
      })),
    })),
    price_levels: list(selection.priceLevels, item => pick(item, ['id', 'title'])),
    sections: list(selection.sections, item => pick(item, ['id', 'title', 'rawId'])),
    price_to_sections: list(selection.priceToSections, item => pick(item, ['priceIdentifier', 'sectionIdentifier'])),
    fee_configuration: pick(selection, ['handlingFeeValue', 'feeRollup', 'priceDisplays']),
    filter_translations: {
      sections: state.translations?.['edp.tselection.sections'],
      apply: next.props.pageProps.globalTranslations?.global?.filters?.apply,
      reset: next.props.pageProps.globalTranslations?.global?.filters?.reset,
    },
  };
  if (phase === 'binding') return binding;
  const panels = [...document.querySelectorAll('[data-testid="panelPriceSection"]')].filter(node => shown(node));
  if (phase === 'panel') {
    if (panels.length !== 1) return null;
    const panel = panels[0];
    const headings = [...panel.querySelectorAll('h1')].filter(node => shown(node));
    const heading = headings.length === 1 ? text(headings[0]) : '';
    if (!(kind === 'price' ? ['Cena', 'Ceny'] : ['Sekce']).includes(heading)) return null;
    const fields = [...panel.querySelectorAll('fieldset')].filter(node => shown(node));
    if (fields.length !== 1) return null;
    const inputs = [...fields[0].querySelectorAll('input[type="checkbox"]')];
    if (inputs.length < 1 || inputs.length > 128) return null;
    const options = [], seen = new Set();
    for (const input of inputs) {
      if (!/^[A-Za-z0-9_:-]{1,128}$/.test(input.id) || seen.has(input.id)
          || [...document.querySelectorAll('[id]')].filter(node => node.id === input.id).length !== 1
          || input.hasAttribute('hidden') || input.hasAttribute('inert')) return null;
      seen.add(input.id);
      const labels = [...fields[0].querySelectorAll('label')].filter(node => node.htmlFor === input.id);
      if (labels.length !== 1 || !shown(labels[0]) || !labels[0].contains(input)) return null;
      const value = text(labels[0]);
      if (!label(value)) return null;
      options.push({ checkbox_id: input.id, label: value, checked: input.checked, disabled: disabled(input) });
    }
    return { binding, heading, options };
  }
  if (panels.length) return null;
  const headings = [...document.querySelectorAll('h1')].filter(node => shown(node));
  if (headings.length !== 1 || text(headings[0]) !== event.name) return null;
  if ([...document.querySelectorAll('[data-testid="basketItem"]')].some(node => shown(node))) return null;
  const submit = [...document.querySelectorAll('button[data-testid="findTicketsBtn"]')].filter(node => shown(node));
  if (submit.length !== 1 || text(submit[0]) !== 'Najít vstupenky') return null;
  const allSteppers = [...document.querySelectorAll('[data-testid="quantityStepper"]')];
  const steppers = allSteppers.filter(node => shown(node));
  if (!steppers.length || steppers.length > 128) return null;
  if (allSteppers.filter(node => !shown(node)).some(node =>
    [...node.querySelectorAll('[role="spinbutton"]')].some(spin => integer(spin.getAttribute('aria-valuenow')) !== 0))) return null;
  let selected = null;
  for (const stepper of steppers) {
    const row = stepper.closest('li');
    const info = row && [...row.querySelectorAll('[data-testid="ticketTypeInfo"]')].filter(node => shown(node));
    const spins = [...stepper.querySelectorAll('[role="spinbutton"]')].filter(node => shown(node));
    if (!row || row.getAttribute('role') === 'button' || info.length !== 1 || spins.length !== 1) return null;
    const spin = spins[0];
    const matchingTitles = types.filter(type => [...info[0].querySelectorAll('span')]
      .some(node => typeTitle(node) === type.title));
    // An unrelated display row can merge multiple configured types under one
    // title. It is safe to leave that row at zero, never to choose one of its IDs.
    if (!matchingTitles.length || new Set(matchingTitles.map(type => type.title)).size !== 1) return null;
    const current = integer(spin.getAttribute('aria-valuenow'));
    const minimum = integer(spin.getAttribute('aria-valuemin'));
    const maximum = integer(spin.getAttribute('aria-valuemax'));
    const buttons = [...stepper.children].filter(node => node.tagName === 'BUTTON');
    if (current === null || minimum !== 0 || maximum === null || current > maximum
        || maximum > selection.eventTicketLimit || buttons.length !== 2
        || stepper.querySelectorAll('button').length !== 2
        || spin.previousElementSibling !== buttons[0] || spin.nextElementSibling !== buttons[1]
        || buttons.some(button => button.getAttribute('type') !== 'button' || !shown(button, true)
          || button.hasAttribute('data-testid') || button.getAttribute('role') === 'submit')
        || !spin.getAttribute('aria-label')?.startsWith(matchingTitles[0].title + ' ')) return null;
    if (matchingTitles.some(type => type.id === categoryId)) {
      if (matchingTitles.length !== 1) return null;
      if (selected || maximum < quantity || !category.quantities.includes(current)) return null;
      selected = { stepper_index: allSteppers.indexOf(stepper), quantity: current,
        decrement_disabled: disabled(buttons[0]), increment_disabled: disabled(buttons[1]) };
    } else if (current !== 0) return null;
  }
  if (!selected) return null;
  const filters = [];
  for (const [filterKind, originalName] of [['price', 'Všechny ceny'], ['section', 'Všechny sekce']]) {
    const name = filterLabels?.[filterKind] ?? originalName;
    const buttons = [...document.querySelectorAll('button')].filter(node => shown(node) && text(node) === name);
    if (buttons.length !== 1 || buttons[0].getAttribute('type') !== 'button') return null;
    filters.push({ kind: filterKind, label: name, disabled: disabled(buttons[0]) });
  }
  return { binding, ...selected, filters };
}

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
      await page.waitForFunction(({ index, expected }) =>
        document.querySelectorAll('[data-testid="quantityStepper"]')[index]
          ?.querySelector('[role="spinbutton"]')?.getAttribute('aria-valuenow') === String(expected),
      { index: surface.stepper_index, expected: value + (increment ? 1 : -1) }, { timeout: remaining() });
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
