'use strict';

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


module.exports = { readSurface };
