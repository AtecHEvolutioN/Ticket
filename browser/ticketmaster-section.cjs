'use strict';

const { sameEvent, eventIdentity } = require('./rendered.cjs');
const { inspectChallenge } = require('./manual-intervention.cjs');
const { prepareTicketmasterMap } = require('./ticketmaster-map.cjs');

const MAIN_MAP = 'aside[aria-label="Mapa míst"],aside[aria-label="Seat Map"],aside[aria-label="Stoelplattegrond"]';
const IDENTIFIER = /^[A-Za-z0-9_-]{1,128}$/;
const pendingInspections = new WeakMap();
const pendingScans = new WeakMap();

// The NL action path is a draft based on reviewed regional overview contracts.
// Its locale strings must agree with the current event's own configuration;
// synthetic hover regressions are not a substitute for a live seat validation.
async function dutchBinding(page, target) {
  const identity = eventIdentity(target);
  if (identity?.host !== 'ticketmaster.nl') return null;
  const binding = await page.evaluate(eventId => {
    const scripts = document.querySelectorAll('script#__NEXT_DATA__[type="application/json"]');
    if (scripts.length !== 1 || scripts[0].textContent.length > 2 * 1024 * 1024) return null;
    let next; try { next = JSON.parse(scripts[0].textContent); } catch { return null; }
    const state = next.props?.pageProps?.initialReduxState, event = state?.eventInfo;
    const selection = state?.ticketSelection, translations = state?.translations;
    if (!event || !selection || !translations || state.config?.countryCode !== 'NL'
        || state.config?.currencyCode !== 'EUR' || event.marketCode !== 'NL'
        || [next.query?.eventId, state.eventId, event.eventId, event.params, selection.params?.eventId].some(id => id !== eventId)
        || [event, selection].some(item => item.stage !== 'success' || item.inFlightParams !== null)) return null;
    const region = translations['edp.base.seatMap'];
    const dutch = region === 'Stoelplattegrond';
    const maps = [...document.querySelectorAll('aside[aria-label]')]
      .filter(node => ['Seat Map', 'Stoelplattegrond'].includes(node.getAttribute('aria-label'))
        && node.getClientRects().length);
    if (maps.length !== 1 || maps[0].getAttribute('aria-label') !== region) return null;
    if ((!dutch && region !== 'Seat Map')
        || translations['edp.resale.tooltipName'] !== 'Verified Resale Tickets'
        || translations['edp.reserve.seatInfoSec'] !== (dutch ? 'Vak' : 'Section')
        || translations['edp.reserve.seatInfoRow'] !== (dutch ? 'Rij' : 'Row')
        || translations['edp.reserve.seatInfoSeats'] !== (dutch ? 'Stoel' : 'Seat')
        || translations['edp.reserve.seatInfoLevel'] !== (dutch ? 'Niveau' : 'Level')
        || translations['edp.lowDemand.ticketsPriceRange'] !== (dutch ? '{priceRange} per stuk' : '{priceRange} each')) return null;
    const canonicals = document.querySelectorAll('link[rel="canonical"]');
    if (canonicals.length !== 1 || typeof event.canonicalUrl !== 'string') return null;
    return { eventId, region, canonical: canonicals[0].href,
      embedded: new URL(event.canonicalUrl, location.href).href };
  }, identity.id);
  return binding && sameEvent(target, binding.canonical) && sameEvent(target, binding.embedded)
    ? JSON.stringify(binding) : null;
}

// This is deliberately a separate read-only inspection path. A section click
// expands a known map display; a seat is only hovered, never selected.
function checkElement(element, expected) {
  const paint = (node, ignoredAria) => {
    if (!node?.isConnected) return false;
    let rect = node.getBoundingClientRect();
    let left = Math.max(0, rect.left), top = Math.max(0, rect.top);
    let right = Math.min(innerWidth, rect.right), bottom = Math.min(innerHeight, rect.bottom);
    for (let current = node; current; current = current.parentElement) {
      const style = getComputedStyle(current);
      if (current.hasAttribute('hidden') || current.hasAttribute('inert') || current.hasAttribute('disabled')
          || current.getAttribute('aria-disabled') === 'true'
          || (current.getAttribute('aria-hidden') === 'true' && current !== ignoredAria)
          || style.display === 'none' || ['hidden', 'collapse'].includes(style.visibility)
          || style.contentVisibility === 'hidden' || Number(style.opacity) === 0) return false;
      if (current !== node) {
        rect = current.getBoundingClientRect();
        if (/(hidden|clip|scroll|auto)/.test(style.overflowX)) {
          left = Math.max(left, rect.left + current.clientLeft);
          right = Math.min(right, rect.left + current.clientLeft + current.clientWidth);
        }
        if (/(hidden|clip|scroll|auto)/.test(style.overflowY)) {
          top = Math.max(top, rect.top + current.clientTop);
          bottom = Math.min(bottom, rect.top + current.clientTop + current.clientHeight);
        }
      }
    }
    return right > left && bottom > top;
  };
  if (document.querySelector('dialog:modal')) return null;
  const dutch = ['www.ticketmaster.nl', 'ticketmaster.nl'].includes(location.hostname);
  const labels = dutch ? ['Seat Map', 'Stoelplattegrond'] : ['Mapa míst'];
  const maps = [...document.querySelectorAll('aside[aria-label]')]
    .filter(n => labels.includes(n.getAttribute('aria-label')) && paint(n));
  if (maps.length !== 1) return null;
  const roots = maps[0].querySelectorAll('[data-component="zoomer"] svg[data-component="svg"]');
  const svg = element.closest('svg[data-component="svg"]');
  if (roots.length !== 1 || roots[0] !== svg || (expected.svg && expected.svg !== svg) || !paint(svg, svg)
      || (!expected.structureOnly && !paint(element, svg))) return null;
  if (expected.kind === 'section') {
    const paths = svg.querySelectorAll('g.polygons > path[data-component="svg__section"]');
    const matches = [...paths].filter(n => n.getAttribute('data-section-id') === expected.sectionId);
    if (matches.length !== 1 || matches[0] !== element || element.parentElement.parentElement !== svg
        || element.getAttribute('data-active') !== 'true') return null;
    const name = element.getAttribute('data-section-name')?.trim();
    if (!(dutch ? /^(?:Vak\s*)?\d{1,8}$/i : /^\d{1,8}$/).test(name || '')
        || (expected.sectionName && name !== expected.sectionName)) return null;
    return { section_id: expected.sectionId, section_name: name };
  }
  const blocks = [...svg.querySelectorAll('g.seats > g[data-component="svg__block"]')]
    .filter(n => n.getAttribute('data-section-id') === expected.sectionId);
  if (blocks.length !== 1 || blocks[0].parentElement.parentElement !== svg) return null;
  const block = blocks[0];
  if (expected.kind === 'block') return element === block ? { section_id: expected.sectionId } : null;
  if (expected.kind === 'candidate_list') {
    if (element !== block) return null;
    const nodes = [...block.querySelectorAll('[data-component="svg__seat"][type="resale"]')];
    if (nodes.length > 2000 || nodes.length !== expected.nodes.length
        || nodes.some((node, index) => node !== expected.nodes[index])) return null;
    const blockName = block.getAttribute('data-section-name');
    if (!blockName || blockName.length > 160 || /[\u0000-\u001f\u007f]/.test(blockName)) return null;
    const ids = new Map();
    for (const node of block.querySelectorAll('[data-component="svg__seat"]')) ids.set(node.id, (ids.get(node.id) || 0) + 1);
    const entries = [];
    for (const node of nodes) {
      const row = node.parentElement;
      const rowName = row?.getAttribute('data-row-name')?.trim();
      const seatName = node.getAttribute('data-seat-name')?.trim();
      if (!/^[A-Za-z0-9_-]{1,128}$/.test(node.id) || ids.get(node.id) !== 1
          || row?.parentElement !== block || row.tagName.toLowerCase() !== 'g' || node.tagName.toLowerCase() !== 'g'
          || !rowName || rowName.length > 80 || !seatName || seatName.length > 80
          || /[\u0000-\u001f\u007f]/.test(rowName + seatName)) return null;
      entries.push({ section_id: expected.sectionId, section_name: expected.sectionName,
        seat_id: node.id, row: rowName, seat: seatName, visible: paint(node, svg) });
    }
    return { block_name: blockName, entries };
  }
  const seats = [...block.querySelectorAll('[data-component="svg__seat"]')].filter(n => n.id === expected.seatId);
  const row = element.parentElement;
  const resale = element.tagName.toLowerCase() === 'g' && element.getAttribute('type') === 'resale';
  const primary = dutch && element.tagName.toLowerCase() === 'circle' && element.getAttribute('type') === 'primary';
  if (seats.length !== 1 || seats[0] !== element || row?.parentElement !== block
      || row.tagName.toLowerCase() !== 'g' || (!resale && !primary)) return null;
  const rowName = row.getAttribute('data-row-name')?.trim();
  const seatName = element.getAttribute('data-seat-name')?.trim();
  if (!rowName || rowName.length > 80 || !seatName || seatName.length > 80
      || /[\u0000-\u001f\u007f]/.test(rowName + seatName)) return null;
  return { section_id: expected.sectionId, section_name: expected.sectionName,
    seat_id: expected.seatId, row: rowName, seat: seatName };
}

function readTooltip(seat, expected) {
  if (!seat.isConnected || !seat.matches(':hover') || document.querySelector('dialog:modal')) return null;
  const svg = seat.closest('svg[data-component="svg"]');
  const dutch = ['www.ticketmaster.nl', 'ticketmaster.nl'].includes(location.hostname);
  const map = svg?.closest('aside[aria-label]');
  const region = map?.getAttribute('aria-label');
  if (!map || !(dutch ? ['Seat Map', 'Stoelplattegrond'] : ['Mapa míst']).includes(region)) return null;
  const fields = dutch ? (region === 'Seat Map'
    ? { zone: 'level', section: 'section', row: 'row', seat: 'seat' }
    : { zone: 'niveau', section: 'vak', row: 'rij', seat: 'stoel' })
    : { zone: 'zóna', section: 'sekce', row: 'řada', seat: 'místo' };
  const visible = (node, ignoredAria) => {
    let rect = node.getBoundingClientRect();
    let left = Math.max(0, rect.left), top = Math.max(0, rect.top);
    let right = Math.min(innerWidth, rect.right), bottom = Math.min(innerHeight, rect.bottom);
    for (let parent = node; parent; parent = parent.parentElement) {
      const style = getComputedStyle(parent);
      if (parent.hasAttribute('hidden') || parent.hasAttribute('inert') || parent.hasAttribute('disabled')
          || parent.getAttribute('aria-disabled') === 'true'
          || (parent.getAttribute('aria-hidden') === 'true' && parent !== ignoredAria)
          || style.display === 'none' || ['hidden', 'collapse'].includes(style.visibility)
          || style.contentVisibility === 'hidden' || Number(style.opacity) === 0) return false;
      if (parent !== node) {
        rect = parent.getBoundingClientRect();
        if (/(hidden|clip|scroll|auto)/.test(style.overflowX)) {
          left = Math.max(left, rect.left + parent.clientLeft);
          right = Math.min(right, rect.left + parent.clientLeft + parent.clientWidth);
        }
        if (/(hidden|clip|scroll|auto)/.test(style.overflowY)) {
          top = Math.max(top, rect.top + parent.clientTop);
          bottom = Math.min(bottom, rect.top + parent.clientTop + parent.clientHeight);
        }
      }
    }
    return right > left && bottom > top;
  };
  const tips = [...map.querySelectorAll('[data-component="tooltip"]')].filter(n => visible(n, n));
  if (tips.length !== 1) return null;
  const tip = tips[0];
  const normal = text => String(text || '').normalize('NFC').replace(/\s+/g, ' ').trim();
  const pairs = [...tip.querySelectorAll('dl dt')];
  if (pairs.length !== 4 || tip.querySelectorAll('dl').length !== 1) return null;
  const values = {};
  for (const dt of pairs) {
    const dd = dt.nextElementSibling;
    const key = normal(dt.textContent).toLocaleLowerCase();
    const value = normal(dd?.textContent);
    if (!Object.values(fields).includes(key) || Object.hasOwn(values, key)
        || dd?.tagName !== 'DD' || !value || value.length > 160
        || dt.childElementCount || dd.childElementCount || !visible(dt, tip) || !visible(dd, tip)) return null;
    values[key] = value;
  }
  const sectionKey = value => dutch ? value.replace(/^Vak\s*/i, '') : value;
  if (sectionKey(values[fields.section]) !== sectionKey(expected.section_name)
      || values[fields.row] !== expected.row || values[fields.seat] !== expected.seat) return null;
  const info = tip.querySelectorAll('[data-testid="ticketTypeInfo"]');
  if (info.length !== 1) return null;
  const spans = [...info[0].querySelectorAll('span')];
  if (spans.some(n => n.childElementCount || !visible(n, tip))) return null;
  const lines = spans.map(n => normal(n.textContent));
  const primary = dutch && seat.tagName.toLowerCase() === 'circle' && seat.getAttribute('type') === 'primary';
  let configuredPrimary = false;
  if (primary) {
    const scripts = document.querySelectorAll('script#__NEXT_DATA__[type="application/json"]');
    if (scripts.length !== 1 || scripts[0].textContent.length > 2 * 1024 * 1024) return null;
    try {
      const types = JSON.parse(scripts[0].textContent).props?.pageProps?.initialReduxState?.ticketSelection?.ticketTypes;
      configuredPrimary = Array.isArray(types) && types.length <= 128
        && types.filter(type => type?.title === lines[0]).length === 1
        && lines[0] !== 'Verified Resale Tickets';
    } catch { return null; }
  }
  if (![2, 3].includes(lines.length) || lines.some(s => !s || s.length > 200)
      || (primary ? !configuredPrimary : lines[0] !== (dutch ? 'Verified Resale Tickets' : 'Ověřené přeprodávané vstupenky'))
      || !(dutch ? (region === 'Seat Map' ? /^€\s*[\d,]+\.\d{2} each$/ : /^€\s*[\d.]+,\d{2} per stuk$/)
        : /^[\d\s.,]+ Kč za kus$/).test(lines.at(-1))) return null;
  return { zone: values[fields.zone], section: values[fields.section], row: values[fields.row], seat: values[fields.seat],
    ticket_type: lines[0], restriction: lines.length === 3 ? lines[1] : null, price_text: lines.at(-1) };
}

async function inspectTicketmasterSection(page, target, { sectionId, seatId }, budgetMs, validateCurrent = async () => {}) {
  let host;
  try { host = new URL(target).hostname; } catch { return { state: 'section_unsupported', opened: false }; }
  if (!IDENTIFIER.test(sectionId || '') || (seatId !== undefined && !IDENTIFIER.test(seatId))
      || !['www.ticketmaster.cz', 'ticketmaster.cz', 'www.ticketmaster.nl', 'ticketmaster.nl'].includes(host) || !sameEvent(target, target)
      || !Number.isFinite(budgetMs) || budgetMs < 1) return { state: 'section_unsupported', opened: false };
  const deadline = performance.now() + Math.min(budgetMs, 15000);
  const remaining = () => Math.max(0, Math.floor(deadline - performance.now()));
  let opened = false;
  let nlBinding;
  const guard = async (afterMapDisplayAction = false) => {
    if (!sameEvent(target, page.url())) throw new Error('unsafe_redirect');
    await validateCurrent({ afterMapDisplayAction });
    if (await inspectChallenge(page)) return 'blocked';
    if (await page.locator('dialog:modal').count()) return 'section_modal_blocked';
    if (host.endsWith('.nl')) {
      const current = await dutchBinding(page, target);
      if (!current || (nlBinding !== undefined && current !== nlBinding)) return 'section_unsupported';
      nlBinding = current;
    }
    return null;
  };
  const failure = state => ({ state, opened });
  let blocked = await guard();
  if (blocked) return failure(blocked);
  const prepared = await prepareTicketmasterMap(page, target, remaining(), validateCurrent);
  if (prepared.state !== 'map_svg') return failure(prepared.state);
  blocked = await guard();
  if (blocked) return failure(blocked);

  // Consent can appear after the map has hydrated. Only the observed rejection
  // control is eligible, and it is never substituted with another button.
  const consent = page.locator('#ketch-banner-button-secondary');
  if (await consent.count() === 1 && await consent.isVisible()
      && (host.endsWith('.nl') ? ['Reject all', 'Reject All', 'Alles weigeren'] : ['Odmítnout vše'])
        .includes((await consent.innerText()).trim())) {
    if (!remaining()) return failure('render_timeout');
    try { await consent.click({ timeout: Math.max(1, Math.min(1500, remaining())) }); }
    catch { return failure('consent_blocked'); }
    blocked = await guard(); if (blocked) return failure(blocked);
  }
  const roots = page.locator(MAIN_MAP).locator('[data-component="zoomer"] svg[data-component="svg"]');
  if (await roots.count() !== 1) return failure('section_ambiguous');
  const svg = await roots.elementHandle();
  let section, block, seat, retained = false;
  const success = inspection => {
    retained = true;
    pendingInspections.set(page, { target, inspection: JSON.stringify(inspection), svg, block, seat, nlBinding });
    return { state: 'section_ready', opened, inspection };
  };
  try {
    const paths = await svg.$$(`g.polygons > path[data-component="svg__section"][data-section-id="${sectionId}"]`);
    if (paths.length !== 1) { await Promise.all(paths.map(n => n.dispose())); return failure('section_unavailable'); }
    section = paths[0];
    const expected = { kind: 'section', sectionId, svg };
    const identity = await section.evaluate(checkElement, expected);
    if (!identity) return failure('section_unavailable');
    blocked = await guard(); if (blocked) return failure(blocked);
    if (!await section.evaluate(checkElement, { ...expected, sectionName: identity.section_name })) return failure('section_unavailable');
    if (!remaining()) return failure('render_timeout');
    try { await section.click({ timeout: Math.max(1, Math.min(2000, remaining())) }); opened = true; }
    catch { return failure('section_action_blocked'); }
    blocked = await guard(true); if (blocked) return failure(blocked);
    do {
      const blocks = await svg.$$(`g.seats > g[data-component="svg__block"][data-section-id="${sectionId}"]`);
      if (blocks.length > 1) { await Promise.all(blocks.map(n => n.dispose())); return failure('section_ambiguous'); }
      if (blocks.length === 1) {
        block = blocks[0];
        if (await block.evaluate(checkElement, { kind: 'block', sectionId, svg })) break;
        await block.dispose(); block = null;
      }
      if (!remaining()) break;
      await page.waitForTimeout(Math.min(100, remaining()));
      blocked = await guard(); if (blocked) return failure(blocked);
    } while (remaining());
    if (!block) return failure('section_unavailable');
    const inspection = { ...identity, seat_id: null, coverage: 'section_only', tooltip: null,
      stock_verified: false, adjacency_verified: false };
    if (seatId === undefined) return success(inspection);

    const matches = await block.$$(`[data-component="svg__seat"][id="${seatId}"]`);
    if (matches.length !== 1) { await Promise.all(matches.map(n => n.dispose())); return failure('section_seat_unavailable'); }
    seat = matches[0];
    const seatExpected = { kind: 'seat', sectionId, sectionName: identity.section_name, seatId, svg };
    const seatIdentity = await seat.evaluate(checkElement, seatExpected);
    if (!seatIdentity) return failure('section_seat_unavailable');
    // Require the old tooltip to disappear, so a stale matching tooltip cannot
    // become evidence for the newly requested hover.
    await page.mouse.move(1, 1);
    try { await page.locator(MAIN_MAP).locator('[data-component="tooltip"]:visible')
      .waitFor({ state: 'hidden', timeout: Math.max(1, Math.min(750, remaining())) }); }
    catch { return failure('section_tooltip_unavailable'); }
    blocked = await guard(); if (blocked) return failure(blocked);
    const currentSeat = await seat.evaluate(checkElement, seatExpected);
    if (JSON.stringify(currentSeat) !== JSON.stringify(seatIdentity)) return failure('section_seat_unavailable');
    if (!remaining()) return failure('render_timeout');
    try { await seat.hover({ timeout: Math.max(1, Math.min(1500, remaining())) }); }
    catch { return failure('section_action_blocked'); }
    inspection.seat_id = seatId;
    do {
      blocked = await guard(); if (blocked) return failure(blocked);
      const current = await seat.evaluate(checkElement, seatExpected);
      if (JSON.stringify(current) !== JSON.stringify(seatIdentity)) return failure('section_seat_unavailable');
      const tooltip = await seat.evaluate(readTooltip, seatIdentity);
      if (tooltip) { inspection.tooltip = tooltip; return success(inspection); }
      if (!remaining()) break;
      await page.waitForTimeout(Math.min(100, remaining()));
    } while (remaining());
    return failure('section_tooltip_unavailable');
  } finally {
    await Promise.all((retained ? [section] : [svg, section, block, seat]).filter(Boolean).map(n => n.dispose()));
  }
}

// Called by the worker after its HTML capture and strict document guard. The
// original handles prevent a matching replacement node from inheriting a quote.
async function assertSectionInspectionCurrent(page, target, inspection) {
  const record = pendingInspections.get(page);
  pendingInspections.delete(page);
  try {
    if (!record || record.target !== target || !sameEvent(target, page.url())
        || record.inspection !== JSON.stringify(inspection)
        || (record.nlBinding !== undefined && await dutchBinding(page, target) !== record.nlBinding)
        || await inspectChallenge(page)
        || !await record.block.evaluate(checkElement, { kind: 'block', sectionId: inspection.section_id, svg: record.svg })) {
      throw new Error('section_inspection_changed');
    }
    if (inspection.seat_id !== null) {
      const expected = { kind: 'seat', sectionId: inspection.section_id,
        sectionName: inspection.section_name, seatId: inspection.seat_id, svg: record.svg };
      const identity = await record.seat.evaluate(checkElement, expected);
      if (!identity || !inspection.tooltip || identity.row !== inspection.tooltip.row || identity.seat !== inspection.tooltip.seat
          || JSON.stringify(await record.seat.evaluate(readTooltip, identity)) !== JSON.stringify(inspection.tooltip)) {
        throw new Error('section_inspection_changed');
      }
    }
  } finally {
    if (record) await Promise.all([record.svg, record.block, record.seat].filter(Boolean).map(n => n.dispose()));
  }
}

// Reuse one already opened map. Candidate counts describe rendered resale nodes,
// not stock. Each quote is validated at its own observation time before moving
// the pointer; an entire scan is deliberately not an atomic inventory snapshot.
async function scanTicketmasterSection(page, target, { sectionId, limit }, budgetMs, validateCurrent = async () => {}) {
  if (!Number.isSafeInteger(limit) || limit < 1 || limit > 50
      || !Number.isFinite(budgetMs) || budgetMs < 1) return { state: 'scan_unsupported', opened: false };
  const startedAt = Date.now();
  const deadline = performance.now() + Math.min(budgetMs, 15000);
  const remaining = () => Math.max(0, Math.floor(deadline - performance.now()));
  const opened = await inspectTicketmasterSection(page, target, { sectionId }, remaining(), validateCurrent);
  if (opened.state !== 'section_ready') return opened;
  const record = pendingInspections.get(page);
  pendingInspections.delete(page);
  if (!record) throw new Error('scan_changed');
  const candidates = [];
  let retained = false;
  const failure = state => ({ state, opened: opened.opened });
  const guard = async () => {
    if (!sameEvent(target, page.url())) throw new Error('unsafe_redirect');
    await validateCurrent({ afterMapDisplayAction: false });
    if (await inspectChallenge(page)) throw new Error('scan_changed');
    if (!await record.block.evaluate(checkElement, { kind: 'block', sectionId, svg: record.svg })) throw new Error('scan_changed');
  };
  try {
    await guard();
    const nodes = await record.block.$$('[data-component="svg__seat"][type="resale"]');
    // Keep handles bounded even when a malformed page has excessive candidates.
    if (nodes.length > 2000) {
      await Promise.all(nodes.map(node => node.dispose()));
      return failure('scan_candidate_limit');
    }
    for (const node of nodes) candidates.push({ node });
    const candidateExpected = { kind: 'candidate_list', sectionId,
      sectionName: opened.inspection.section_name, nodes, svg: record.svg };
    const discovered = await record.block.evaluate(checkElement, candidateExpected);
    if (!discovered) return failure('scan_candidate_invalid');
    record.candidateExpected = candidateExpected;
    record.candidateIdentity = JSON.stringify({ block_name: discovered.block_name,
      entries: discovered.entries.map(({ visible, ...identity }) => identity) });
    for (let index = 0; index < candidates.length; index++) {
      const { visible: painted, ...identity } = discovered.entries[index];
      const candidate = candidates[index];
      candidate.expected = { kind: 'seat', sectionId, sectionName: opened.inspection.section_name, seatId: identity.seat_id, svg: record.svg };
      candidate.identity = identity;
      candidate.visible = painted;
    }
    candidates.sort((a, b) => a.identity.seat_id < b.identity.seat_id ? -1 : a.identity.seat_id > b.identity.seat_id ? 1 : 0);
    const visible = candidates.filter(candidate => candidate.visible);
    const results = [];
    for (const candidate of visible) {
      if (!remaining() || results.length >= limit) break;
      await guard();
      if (JSON.stringify(await candidate.node.evaluate(checkElement, candidate.expected)) !== JSON.stringify(candidate.identity)) {
        throw new Error('scan_changed');
      }
      const attemptDeadline = performance.now() + Math.min(1000, remaining());
      const attemptRemaining = () => Math.max(0, Math.floor(Math.min(deadline, attemptDeadline) - performance.now()));
      let tooltip = null;
      let observedAt;
      // An old card must disappear before any new hover may supply a quote.
      await page.mouse.move(1, 1);
      let cleared = false;
      if (attemptRemaining()) {
        try {
          await page.locator('aside[aria-label="Mapa míst"] [data-component="tooltip"]:visible')
            .waitFor({ state: 'hidden', timeout: Math.max(1, Math.min(250, attemptRemaining())) });
          cleared = true;
        } catch { /* The attempt remains explicitly unknown. */ }
      }
      if (cleared && attemptRemaining()) {
        await guard();
        if (JSON.stringify(await candidate.node.evaluate(checkElement, candidate.expected)) !== JSON.stringify(candidate.identity)) {
          throw new Error('scan_changed');
        }
        let hovered = false;
        try { await candidate.node.hover({ timeout: Math.max(1, Math.min(500, attemptRemaining())) }); hovered = true; }
        catch { /* An obstructed seat is not an unavailable ticket. */ }
        while (hovered && attemptRemaining()) {
          await guard();
          if (JSON.stringify(await candidate.node.evaluate(checkElement, candidate.expected)) !== JSON.stringify(candidate.identity)) {
            throw new Error('scan_changed');
          }
          const observed = await candidate.node.evaluate(readTooltip, candidate.identity);
          if (observed) {
            // Re-read around the document guard while the same seat is hovered.
            await guard();
            if (JSON.stringify(await candidate.node.evaluate(checkElement, candidate.expected)) !== JSON.stringify(candidate.identity)
                || JSON.stringify(await candidate.node.evaluate(readTooltip, candidate.identity)) !== JSON.stringify(observed)) {
              throw new Error('scan_changed');
            }
            tooltip = observed;
            observedAt = Date.now();
            break;
          }
          if (attemptRemaining()) await page.waitForTimeout(Math.min(50, attemptRemaining()));
        }
      }
      await guard();
      if (JSON.stringify(await candidate.node.evaluate(checkElement, { ...candidate.expected, structureOnly: true })) !== JSON.stringify(candidate.identity)) {
        throw new Error('scan_changed');
      }
      const { seat_id, row, seat } = candidate.identity;
      results.push({ seat_id, row, seat, observed_at_ms: observedAt ?? Date.now(), tooltip,
        outcome: tooltip ? 'quoted' : 'tooltip_unavailable' });
    }
    await guard();
    const scan = {
      section_id: sectionId, section_name: opened.inspection.section_name, limit,
      started_at_ms: startedAt, finished_at_ms: Date.now(), candidate_count: candidates.length,
      visible_candidate_count: visible.length, results,
      stop_reason: results.length === visible.length ? 'visible_candidates_exhausted'
        : results.length >= limit ? 'seat_limit' : 'time_budget',
      coverage: 'partial_rendered_section', stock_verified: false, adjacency_verified: false,
    };
    await validateScanNodes(record);
    pendingScans.set(page, { ...record, scan: JSON.stringify(scan), candidates });
    retained = true;
    return { state: 'scan_ready', opened: opened.opened, scan };
  } finally {
    if (!retained) await Promise.all([record.svg, record.block, ...candidates.map(c => c.node)].map(node => node.dispose()));
  }
}

async function validateScanNodes(record) {
  const current = await record.block.evaluate(checkElement, record.candidateExpected);
  if (!current || JSON.stringify({ block_name: current.block_name,
    entries: current.entries.map(({ visible, ...identity }) => identity) }) !== record.candidateIdentity) throw new Error('scan_changed');
}

// Prior quotes intentionally do not need to remain hovered. This assertion
// confirms the final captured document still contains the original bound nodes.
async function assertSectionScanCurrent(page, target, scan) {
  const record = pendingScans.get(page);
  pendingScans.delete(page);
  try {
    if (!record || record.target !== target || record.scan !== JSON.stringify(scan)
        || !sameEvent(target, page.url()) || await inspectChallenge(page)) throw new Error('scan_changed');
    await validateScanNodes(record);
  } finally {
    if (record) await Promise.all([record.svg, record.block, ...record.candidates.map(c => c.node)].map(node => node.dispose()));
  }
}

module.exports = { inspectTicketmasterSection, assertSectionInspectionCurrent,
  scanTicketmasterSection, assertSectionScanCurrent };
