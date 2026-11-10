'use strict';

const { sameEvent } = require('./rendered.cjs');
const { inspectChallenge } = require('./manual-intervention.cjs');
const { prepareTicketmasterMap } = require('./ticketmaster-map.cjs');

const MAIN_MAP = 'aside[aria-label="Mapa míst"] [data-component="zoomer"] svg[data-component="svg"]';
const IDENTIFIER = /^[A-Za-z0-9_-]{1,128}$/;
const pendingInspections = new WeakMap();

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
  const maps = [...document.querySelectorAll('aside[aria-label="Mapa míst"]')].filter(n => paint(n));
  if (maps.length !== 1) return null;
  const roots = maps[0].querySelectorAll('[data-component="zoomer"] svg[data-component="svg"]');
  const svg = element.closest('svg[data-component="svg"]');
  if (roots.length !== 1 || roots[0] !== svg || !paint(svg, svg) || !paint(element, svg)) return null;
  if (expected.kind === 'section') {
    const paths = svg.querySelectorAll('g.polygons > path[data-component="svg__section"]');
    const matches = [...paths].filter(n => n.getAttribute('data-section-id') === expected.sectionId);
    if (matches.length !== 1 || matches[0] !== element || element.parentElement.parentElement !== svg
        || element.getAttribute('data-active') !== 'true') return null;
    const name = element.getAttribute('data-section-name')?.trim();
    if (!/^\d{1,8}$/.test(name || '') || (expected.sectionName && name !== expected.sectionName)) return null;
    return { section_id: expected.sectionId, section_name: name };
  }
  const blocks = [...svg.querySelectorAll('g.seats > g[data-component="svg__block"]')]
    .filter(n => n.getAttribute('data-section-id') === expected.sectionId);
  if (blocks.length !== 1 || blocks[0].parentElement.parentElement !== svg) return null;
  const block = blocks[0];
  if (expected.kind === 'block') return element === block ? { section_id: expected.sectionId } : null;
  const seats = [...block.querySelectorAll('[data-component="svg__seat"]')].filter(n => n.id === expected.seatId);
  const row = element.parentElement;
  if (seats.length !== 1 || seats[0] !== element || row?.parentElement !== block
      || row.tagName.toLowerCase() !== 'g' || element.tagName.toLowerCase() !== 'g'
      || element.getAttribute('type') !== 'resale') return null;
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
  const map = svg?.closest('aside[aria-label="Mapa míst"]');
  if (!map) return null;
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
    if (!['zóna', 'sekce', 'řada', 'místo'].includes(key) || Object.hasOwn(values, key)
        || dd?.tagName !== 'DD' || !value || value.length > 160
        || dt.childElementCount || dd.childElementCount || !visible(dt, tip) || !visible(dd, tip)) return null;
    values[key] = value;
  }
  if (values.sekce !== expected.section_name || values['řada'] !== expected.row || values['místo'] !== expected.seat) return null;
  const info = tip.querySelectorAll('[data-testid="ticketTypeInfo"]');
  if (info.length !== 1) return null;
  const spans = [...info[0].querySelectorAll('span')];
  if (spans.some(n => n.childElementCount || !visible(n, tip))) return null;
  const lines = spans.map(n => normal(n.textContent));
  if (![2, 3].includes(lines.length) || lines.some(s => !s || s.length > 200)
      || lines[0] !== 'Ověřené přeprodávané vstupenky'
      || !/^[\d\s.,]+ Kč za kus$/.test(lines.at(-1))) return null;
  return { zone: values['zóna'], section: values.sekce, row: values['řada'], seat: values['místo'],
    ticket_type: lines[0], restriction: lines.length === 3 ? lines[1] : null, price_text: lines.at(-1) };
}

async function inspectTicketmasterSection(page, target, { sectionId, seatId }, budgetMs, validateCurrent = async () => {}) {
  let host;
  try { host = new URL(target).hostname; } catch { return { state: 'section_unsupported', opened: false }; }
  if (!IDENTIFIER.test(sectionId || '') || (seatId !== undefined && !IDENTIFIER.test(seatId))
      || !['www.ticketmaster.cz', 'ticketmaster.cz'].includes(host) || !sameEvent(target, target)
      || !Number.isFinite(budgetMs) || budgetMs < 1) return { state: 'section_unsupported', opened: false };
  const deadline = performance.now() + Math.min(budgetMs, 15000);
  const remaining = () => Math.max(0, Math.floor(deadline - performance.now()));
  let opened = false;
  const guard = async (afterMapDisplayAction = false) => {
    if (!sameEvent(target, page.url())) throw new Error('unsafe_redirect');
    await validateCurrent({ afterMapDisplayAction });
    if (await inspectChallenge(page)) return 'blocked';
    if (await page.locator('dialog:modal').count()) return 'section_modal_blocked';
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
      && (await consent.innerText()).trim() === 'Odmítnout vše') {
    if (!remaining()) return failure('render_timeout');
    try { await consent.click({ timeout: Math.max(1, Math.min(1500, remaining())) }); }
    catch { return failure('consent_blocked'); }
    blocked = await guard(); if (blocked) return failure(blocked);
  }
  const roots = page.locator(MAIN_MAP);
  if (await roots.count() !== 1) return failure('section_ambiguous');
  const svg = await roots.elementHandle();
  let section, block, seat, retained = false;
  const success = inspection => {
    retained = true;
    pendingInspections.set(page, { target, inspection: JSON.stringify(inspection), svg, block, seat });
    return { state: 'section_ready', opened, inspection };
  };
  try {
    const paths = await svg.$$(`g.polygons > path[data-component="svg__section"][data-section-id="${sectionId}"]`);
    if (paths.length !== 1) { await Promise.all(paths.map(n => n.dispose())); return failure('section_unavailable'); }
    section = paths[0];
    const expected = { kind: 'section', sectionId };
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
        if (await block.evaluate(checkElement, { kind: 'block', sectionId })) break;
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
    const seatExpected = { kind: 'seat', sectionId, sectionName: identity.section_name, seatId };
    const seatIdentity = await seat.evaluate(checkElement, seatExpected);
    if (!seatIdentity) return failure('section_seat_unavailable');
    // Require the old tooltip to disappear, so a stale matching tooltip cannot
    // become evidence for the newly requested hover.
    await page.mouse.move(1, 1);
    try { await page.locator('aside[aria-label="Mapa míst"] [data-component="tooltip"]:visible')
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
        || await inspectChallenge(page)
        || !await record.block.evaluate(checkElement, { kind: 'block', sectionId: inspection.section_id })) {
      throw new Error('section_inspection_changed');
    }
    if (inspection.seat_id !== null) {
      const expected = { kind: 'seat', sectionId: inspection.section_id,
        sectionName: inspection.section_name, seatId: inspection.seat_id };
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

module.exports = { inspectTicketmasterSection, assertSectionInspectionCurrent };
