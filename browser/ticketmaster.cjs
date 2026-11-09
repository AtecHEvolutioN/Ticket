'use strict';

// Only display controls are used here. Listing selection, quantity controls,
// Find tickets and checkout are deliberately outside this adapter's action set.
const { sameEvent, challengeText } = require('./rendered.cjs');
// These exact display labels were observed on the regional event pages captured
// during the 2026-11-09 coverage check. Do not guess purchase-button translations.
const BEST_AVAILABLE = [
  'See best available', 'Wybierz najlepsze dostępne miejsca',
  'Nejlepší dostupné', 'Ver lo mejor disponible', 'Best beschikbaar',
];
const REJECT_COOKIES = [
  'Reject all', 'Reject All', 'Odrzuć wszystko', 'Reject non-essential cookies',
  'Alles weigeren',
];

async function inspect(page) {
  return page.evaluate(({ best, reject }) => {
    const normalize = value => String(value || '').replace(/\s+/g, ' ').trim().toLocaleLowerCase();
    const visible = element => {
      if (!element || element.closest('[hidden],[aria-hidden="true"]')) return false;
      const style = getComputedStyle(element);
      return style.display !== 'none' && style.visibility !== 'hidden' && style.visibility !== 'collapse'
        && Number(style.opacity) !== 0 && [...element.getClientRects()].some(rect => rect.width && rect.height);
    };
    const enabled = element => !element.matches(':disabled') && !element.closest('[aria-disabled="true"]');
    const buttons = [...document.querySelectorAll('button,[role="tab"]')];
    const candidates = buttons.filter(element =>
      visible(element) && enabled(element) && best.some(label => normalize(label) === normalize(element.textContent)));
    const rejectButton = [...document.querySelectorAll('#ketch-banner-button-secondary,#onetrust-reject-all-handler')]
      .find(element => visible(element) && enabled(element)
        && reject.some(label => normalize(label) === normalize(element.textContent)));
    return {
      text: `${document.title}\n${document.body?.innerText || ''}`.slice(0, 80000),
      regional: [...document.querySelectorAll('[data-testid="ticketTypeInfo"]')].some(visible),
      listings: [...document.querySelectorAll('li[data-listing-id]')].some(visible),
      bestLabel: candidates.length === 1 ? candidates[0].textContent.replace(/\s+/g, ' ').trim() : null,
      bestCount: candidates.length,
      rejectId: rejectButton?.id || null,
    };
  }, { best: BEST_AVAILABLE, reject: REJECT_COOKIES });
}

/** Prepare a public event's ticket display, within the caller's remaining budget. */
async function prepareTicketmaster(page, target, budgetMs) {
  if (!Number.isFinite(budgetMs) || budgetMs < 1) return { state: 'render_timeout', opened: false };
  const deadline = performance.now() + Math.min(budgetMs, 10000);
  const remaining = () => Math.max(0, Math.floor(deadline - performance.now()));
  const guard = () => { if (!sameEvent(target, page.url())) throw new Error('unsafe_redirect'); };
  guard();
  let state = await inspect(page);
  if (challengeText(state.text)) return { state: 'blocked', opened: false };
  if (state.rejectId && remaining() > 0) {
    try { await page.locator(`#${state.rejectId}`).click({ timeout: Math.max(1, Math.min(1500, remaining())) }); }
    catch { return { state: 'consent_blocked', opened: false }; }
    guard();
    state = await inspect(page);
    if (challengeText(state.text)) return { state: 'blocked', opened: false };
  }
  // Hydration can reveal a known display tab after DOMContentLoaded. A brief
  // bounded DOM-only wait avoids declaring the template unsupported too early.
  const readinessDeadline = Math.min(deadline, performance.now() + 1500);
  while (!state.regional && !state.listings && state.bestCount === 0
    && performance.now() < readinessDeadline) {
    await page.waitForTimeout(Math.min(100, Math.max(0, readinessDeadline - performance.now())));
    guard();
    state = await inspect(page);
    if (challengeText(state.text)) return { state: 'blocked', opened: false };
  }
  if (state.regional) return { state: 'regional', opened: false };
  if (state.listings) return { state: 'listings', opened: false };
  if (state.bestCount > 1) return { state: 'ambiguous_view_control', opened: false };
  if (state.bestLabel === null) return { state: 'unsupported_layout', opened: false };
  if (!remaining()) return { state: 'render_timeout', opened: false };
  // Re-match the exact recognized display label so a DOM insertion cannot shift an
  // index onto another control. A duplicate match fails closed under strict click.
  try {
    const words = state.bestLabel.split(' ').map(word => word.replace(/[.*+?^${}()|[\]\\]/g, '\\$&'));
    const exact = new RegExp(`^\\s*${words.join('\\s+')}\\s*$`);
    await page.locator('button:visible:not(:disabled):not([aria-disabled="true"]),[role="tab"]:visible:not([aria-disabled="true"])')
      .filter({ hasText: exact }).click({ timeout: Math.max(1, Math.min(2000, remaining())) });
  } catch { return { state: 'view_control_blocked', opened: false }; }
  guard();
  // The view may fetch asynchronously. Stop on a challenge, and do not resubmit a search.
  while (remaining() > 0) {
    state = await inspect(page);
    guard();
    if (challengeText(state.text)) return { state: 'blocked', opened: true };
    if (state.regional) return { state: 'regional', opened: true };
    if (state.listings) return { state: 'listings', opened: true };
    await page.waitForTimeout(Math.min(100, remaining()));
  }
  return { state: 'no_visible_offers', opened: true };
}

module.exports = { prepareTicketmaster, BEST_AVAILABLE };
