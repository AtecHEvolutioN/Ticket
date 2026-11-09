'use strict';

const { challengeKind } = require('./manual-intervention.cjs');

// Shared by the background worker and the explicitly operator-driven capture tool.
// These helpers preserve ordinary page behavior; they neither click nor change the live DOM.
const BLOCKED_SEGMENTS = new Set([
  'login', 'account', 'payment', 'checkout', 'cart', 'artist', 'help',
  'recommendation', 'recommendations', 'search',
]);

function eventIdentity(input) {
  if (typeof input !== 'string' || input.length > 4096 || input.trim() !== input || /[\\\p{Cc}]/u.test(input)) return null;
  let url;
  try { url = new URL(input); } catch { return null; }
  if (url.protocol !== 'https:' || url.username || url.password || (url.port && url.port !== '443')
      || [...url.searchParams.keys()].some(key => !['language', 'locale', 'page'].includes(key))) return null;
  const rawPath = input.replace(/^https:\/\/[^/]+/i, '').split(/[?#]/, 1)[0];
  if (!rawPath.startsWith('/') || rawPath.includes('//')) return null;
  const parts = rawPath.replace(/\/$/, '').slice(1).split('/');
  let decoded;
  try { decoded = parts.map(part => decodeURIComponent(part)); } catch { return null; }
  if (decoded.some(part => !part || part === '.' || part === '..' || /[/\\%?#\p{Cc}]/u.test(part))) return null;
  const slug = index => decoded[index].toLowerCase() !== 'event' && !BLOCKED_SEGMENTS.has(decoded[index].toLowerCase());
  const locale = value => /^[a-z]{2}(?:-[a-z]{2})?$/i.test(value);
  const host = url.hostname.replace(/^www\./, '');
  // Structural route markers and IDs stay literal. Only the display slug may
  // contain encoded Unicode or punctuation; it does not determine identity.
  const ordinary = (parts.length === 2 && parts[0] === 'event')
    || (parts.length === 3 && parts[0] === 'event' && slug(1))
    || (parts.length === 3 && parts[1] === 'event' && slug(0))
    || (parts.length === 4 && locale(parts[0]) && parts[1] === 'event' && slug(2))
    || (parts.length === 4 && locale(parts[0]) && parts[2] === 'event' && slug(1));
  const italian = host === 'ticketmaster.it' && (
    (parts.length === 4 && parts[0] === 'biglietti' && parts[2] === 'event' && slug(1))
    || (parts.length === 5 && locale(parts[0]) && parts[1] === 'biglietti' && parts[3] === 'event' && slug(2)));
  if (!ordinary && !italian) return null;
  const id = parts.at(-1);
  if (!(/^[0-9]{1,20}$/.test(id) || (/^[A-Za-z0-9]{8,64}$/.test(id) && /[0-9]/.test(id) && /[A-Za-z]/.test(id)))) return null;
  return { host, id: id.toUpperCase() };
}

function sameEvent(expected, actual) {
  // Public host policy lives in the worker/capture URL validator. Identity alone must
  // never authorize a request to a host; it binds already-validated event pages.
  const first = eventIdentity(expected);
  const second = eventIdentity(actual);
  return first !== null && second !== null && first.host === second.host && first.id === second.id;
}

function challengeText(text) {
  return challengeKind(text) !== null;
}

async function captureRendered(page, maximum) {
  if (!Number.isSafeInteger(maximum) || maximum < 1024 || maximum > 16 * 1024 * 1024) {
    throw new Error('invalid_request');
  }
  const result = await page.evaluate(limit => {
    const root = document.documentElement;
    if (!root) return { html: '', error: null };
    const encoder = new TextEncoder();
    if (encoder.encode(root.outerHTML).length > limit) return { error: 'body_too_large' };
    const originals = [root, ...root.querySelectorAll('*')];
    if (originals.length > 50000) return { error: 'body_too_large' };
    const copy = root.cloneNode(true);
    const clones = [copy, ...copy.querySelectorAll('*')];
    const boxes = new WeakMap();
    const disabled = new WeakMap();
    // A display:contents wrapper and a zero-height parent with visible positioned
    // children still represent readable content. Look at descendant geometry too.
    for (let index = originals.length - 1; index >= 0; index--) {
      const original = originals[index];
      let hasBox = [...original.getClientRects()].some(rect => rect.width > 0 && rect.height > 0);
      for (const child of original.children) hasBox ||= Boolean(boxes.get(child));
      boxes.set(original, hasBox);
    }
    for (let index = 0; index < originals.length; index++) {
      const original = originals[index];
      const clone = clones[index];
      for (const attribute of [...clone.attributes]) {
        if (attribute.name.startsWith('data-ticketwatch-')) clone.removeAttribute(attribute.name);
      }
      const style = getComputedStyle(original);
      // Options have no client rect while a native select is closed, but remain
      // legitimate selectable choices unless explicitly hidden or disabled.
      const nonvisual = /^(HEAD|META|LINK|SCRIPT|STYLE|TITLE|TEMPLATE|NOSCRIPT|OPTION|OPTGROUP)$/.test(original.tagName);
      const hidden = original.hidden || original.getAttribute('aria-hidden') === 'true'
        || style.display === 'none' || style.visibility === 'hidden' || style.visibility === 'collapse'
        || style.contentVisibility === 'hidden' || Number(style.opacity) === 0
        || (!nonvisual && !boxes.get(original));
      if (hidden) clone.setAttribute('hidden', '');
      const inactive = Boolean(original.matches(':disabled'))
        || original.getAttribute('aria-disabled') === 'true'
        || Boolean(disabled.get(original.parentElement));
      disabled.set(original, inactive);
      if (inactive) {
        clone.setAttribute('disabled', '');
        clone.setAttribute('aria-disabled', 'true');
      }
    }
    const html = '<!DOCTYPE html>\n' + copy.outerHTML;
    return encoder.encode(html).length > limit ? { error: 'body_too_large' } : { html, error: null };
  }, maximum);
  if (!result || typeof result !== 'object' || typeof result.html !== 'string') {
    throw new Error(result?.error === 'body_too_large' ? 'body_too_large' : 'capture_failed');
  }
  if (Buffer.byteLength(result.html, 'utf8') > maximum) throw new Error('body_too_large');
  return result.html;
}

module.exports = { captureRendered, sameEvent, eventIdentity, challengeText };
