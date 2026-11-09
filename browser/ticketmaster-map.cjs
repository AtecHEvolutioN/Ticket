'use strict';

const { sameEvent, challengeText } = require('./rendered.cjs');

// Exact display labels observed on the captured regional pages. These controls
// change the map view; ticket/seat selection and search are outside this helper.
const MAP_LABELS = ['Mapa míst', 'Zobacz miejsca na mapie', 'View seat map',
  'Ver mapa de asientos', 'Bekijk plattegrond'];
const MAP_REGIONS = ['Mapa míst', 'Mapa Obiektu', 'Seat Map', 'Arenalayout',
  'Mapa de Asientos', 'Saalplan', 'Stoelplattegrond'];
const REJECT_COOKIES = ['Reject all', 'Reject All', 'Odrzuć wszystko',
  'Reject non-essential cookies', 'Alles weigeren', 'Odmítnout vše'];

async function inspectMap(page) {
  return page.evaluate(({ labels, regions, reject }) => {
    const normal = text => String(text || '').replace(/\s+/g, ' ').trim().toLocaleLowerCase();
    const painted = (element, graphic = false) => {
      if (!element) return false;
      for (let node = element; node; node = node.parentElement) {
        const style = getComputedStyle(node);
        if (node.hidden || node.hasAttribute('hidden') || node.hasAttribute('inert')
            || (node.getAttribute('aria-hidden') === 'true' && !(graphic && node === element))
            || style.display === 'none' || ['hidden', 'collapse'].includes(style.visibility)
            || style.contentVisibility === 'hidden' || Number(style.opacity) === 0) return false;
      }
      return [...element.getClientRects()].some(rect => rect.width > 0 && rect.height > 0);
    };
    const enabled = node => !node.matches(':disabled') && !node.closest('[aria-disabled="true"]');
    const maps = [...document.querySelectorAll('aside[aria-label]')]
      .filter(node => painted(node) && regions.some(label => normal(label) === normal(node.getAttribute('aria-label'))));
    const candidates = maps.flatMap(map => [...map.querySelectorAll('button[aria-pressed],[role="tab"]')])
      .filter(node => painted(node) && enabled(node)
        && labels.some(label => normal(label) === normal(node.textContent)));
    const rejectButtons = [...document.querySelectorAll('#ketch-banner-button-secondary,#onetrust-reject-all-handler')]
      .filter(node => painted(node) && enabled(node) && reject.some(label => normal(label) === normal(node.textContent)));
    let surface = null;
    if (maps.length === 1) {
      const map = maps[0];
      // aria-hidden on the observed graphical SVG root is an accessibility
      // attribute, not a paint signal. All outside ancestors remain guarded.
      const svgs = [...map.querySelectorAll('svg[data-component="svg"]')]
        .filter(svg => painted(svg, true) && svg.querySelector('path[data-component="svg__section"][data-section-id][data-section-name]'));
      if (svgs.length > 16) surface = 'map_ambiguous';
      else if (svgs.length) surface = 'map_svg';
      else if ([...map.querySelectorAll('canvas')].some(node => painted(node))) surface = 'map_canvas';
      else if ([...map.querySelectorAll('iframe')].some(node => painted(node))) surface = 'map_iframe';
      else if ([...map.querySelectorAll('img')].some(node => painted(node) && node.complete && node.naturalWidth > 0)) surface = 'map_static';
    }
    let interactiveAdvertised = false;
    const state = document.querySelector('script#__NEXT_DATA__[type="application/json"]')?.textContent;
    if (state && state.length <= 2 * 1024 * 1024) {
      try { interactiveAdvertised = JSON.parse(state).props?.pageProps?.initialReduxState?.eventInfo?.isIsmEnabled === true; }
      catch { /* Only a bounded wait hint; this is never map or inventory evidence. */ }
    }
    return {
      text: `${document.title}\n${document.body?.innerText || ''}`.slice(0, 80000),
      modal: Boolean(document.querySelector('dialog:modal')),
      mapCount: maps.length,
      mapLabel: maps.length === 1 ? maps[0].getAttribute('aria-label') : null,
      controlCount: candidates.length,
      controlLabel: candidates.length === 1 ? candidates[0].textContent.replace(/\s+/g, ' ').trim() : null,
      selected: candidates.length === 1 && (candidates[0].getAttribute('aria-pressed') === 'true'
        || candidates[0].getAttribute('aria-selected') === 'true'),
      rejectId: rejectButtons.length === 1 ? rejectButtons[0].id : null,
      surface,
      interactiveAdvertised,
    };
  }, { labels: MAP_LABELS, regions: MAP_REGIONS, reject: REJECT_COOKIES });
}

function exactLabel(label) {
  const words = label.split(' ').map(word => word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
  return new RegExp(`^\\s*${words.join('\\s+')}\\s*$`);
}

/** Prepare only a known public map display. Surface names describe rendering,
 * not whether any seats are available or whether geometry can be extracted. */
async function prepareTicketmasterMap(page, target, budgetMs, validateCurrent = async () => {}) {
  if (!Number.isFinite(budgetMs) || budgetMs < 1) return { state: 'render_timeout', opened: false };
  const deadline = performance.now() + Math.min(budgetMs, 10000);
  const remaining = () => Math.max(0, Math.floor(deadline - performance.now()));
  let opened = false;
  const guard = async () => {
    if (!sameEvent(target, page.url())) throw new Error('unsafe_redirect');
    // Only the recognized map click below permits a fragment-only rebind in the
    // caller. New loaders, pending navigations and non-fragment changes fail.
    await validateCurrent({ afterMapDisplayAction: opened });
  };
  await guard();
  let state = await inspectMap(page);
  if (challengeText(state.text)) return { state: 'blocked', opened };
  if (state.rejectId && remaining()) {
    try { await page.locator(`#${state.rejectId}`).click({ timeout: Math.max(1, Math.min(1500, remaining())) }); }
    catch { return { state: 'consent_blocked', opened }; }
    await guard();
    state = await inspectMap(page);
    if (challengeText(state.text)) return { state: 'blocked', opened };
  }
  const hydrationDeadline = Math.min(deadline, performance.now() + 1500);
  while (!state.mapCount && performance.now() < hydrationDeadline) {
    await page.waitForTimeout(Math.min(100, Math.max(0, hydrationDeadline - performance.now())));
    await guard(); state = await inspectMap(page);
    if (challengeText(state.text)) return { state: 'blocked', opened };
  }
  if (state.modal) return { state: 'map_modal_blocked', opened };
  if (state.mapCount > 1 || state.controlCount > 1 || state.surface === 'map_ambiguous') {
    return { state: 'map_ambiguous', opened };
  }
  if (!state.mapCount) return { state: 'unsupported_layout', opened };
  if (state.surface && state.surface !== 'map_static' && (state.selected || !state.controlCount)) {
    return { state: state.surface, opened };
  }
  if (state.controlLabel && !state.selected && remaining()) {
    // Bind the action to the inspected landmark as well as its display label.
    // An unrelated aside cannot become the click target if the map is replaced.
    const map = page.getByRole('complementary', { name: state.mapLabel, exact: true });
    const control = map.locator('button:visible[aria-pressed]:not(:disabled):not([aria-disabled="true"]),[role="tab"]:visible:not([aria-disabled="true"])')
      .filter({ hasText: exactLabel(state.controlLabel) });
    try { await control.click({ timeout: Math.max(1, Math.min(2000, remaining())) }); opened = true; }
    catch { return { state: 'view_control_blocked', opened }; }
    await guard();
  }
  const settleUntil = performance.now() + (opened ? 250 : 0);
  do {
    state = await inspectMap(page); await guard();
    if (challengeText(state.text)) return { state: 'blocked', opened };
    if (state.modal) return { state: 'map_modal_blocked', opened };
    if (state.mapCount !== 1 || state.controlCount > 1 || state.surface === 'map_ambiguous') {
      return { state: 'map_ambiguous', opened };
    }
    if (state.surface && state.surface !== 'map_static') return { state: state.surface, opened };
    if (state.surface === 'map_static' && !state.interactiveAdvertised && performance.now() >= settleUntil) {
      return { state: 'map_static', opened };
    }
    if (!remaining()) break;
    await page.waitForTimeout(Math.min(100, remaining()));
  } while (remaining());
  return { state: state.surface || 'map_unavailable', opened };
}

module.exports = { prepareTicketmasterMap, MAP_LABELS, MAP_REGIONS };
