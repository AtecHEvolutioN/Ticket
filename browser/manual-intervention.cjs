'use strict';

// Extension point for an operator UI/notification, not a verification solver.
// Handlers receive fixed reasons only. Only a fresh page inspection can clear a pause.
const MESSAGES = Object.freeze({
  captcha: 'Complete the verification yourself in the open browser.',
  queue: 'Wait for the website to admit you in the open browser.',
  session_expired: 'The page reports an expired session. Restore access manually.',
  authentication_required: 'The page requires authentication. Operator attention is needed.',
  access_blocked: 'The website has blocked access. Operator attention is needed.',
  verification: 'The website is checking access. Inspect the open browser.',
});

const MARKERS = [
  ['captcha', ['verify you are human', 'verify that you are human', "verify you're human",
    'complete the captcha', 'solve the captcha', 'complete the recaptcha',
    'ověřte, že jste člověk', 'ověřte, že nejste robot']],
  ['queue', ['you are in the queue', "you're in the queue", 'you are now in line',
    'your place in the queue', 'waiting room', 'jste ve frontě', 'čekárna']],
  ['session_expired', ['your session has expired', 'vaše relace vypršela']],
  ['authentication_required', ['sign in to continue', 'log in to continue']],
  ['access_blocked', ['your browsing activity has been paused', "we've paused activity",
    'we’ve paused activity', 'access denied', 'request blocked', 'pardon the interruption']],
  ['verification', ['checking your browser', 'enable cookies to continue']],
];

function challengeKind(text) {
  const visible = String(text || '').normalize('NFC').replace(/\s+/g, ' ').toLowerCase();
  return MARKERS.find(([, markers]) => markers.some(marker => visible.includes(marker)))?.[0] || null;
}

async function inspectChallenge(page) {
  // Read visible text only. Ordinary reCAPTCHA attribution is not a challenge.
  const text = await page.evaluate(() => (document.title + '\n' + (document.body?.innerText || '')).slice(0, 40000));
  return challengeKind(text);
}

/**
 * inspect(): validates event/response/session, then returns a fixed kind or null.
 * requestHuman({ kind, message, attempt, signal }): notify and await human input.
 * Its return value is deliberately ignored; inspect() decides whether to resume.
 * Without a handler, return needs_human immediately (background-safe default).
 */
async function waitForManualClearance({ inspect, requestHuman, signal, timeoutMs = 300000, maxPrompts = 3 }) {
  if (typeof inspect !== 'function' || (requestHuman !== undefined && typeof requestHuman !== 'function')
      || !Number.isSafeInteger(timeoutMs) || timeoutMs < 1 || timeoutMs > 1800000
      || !Number.isSafeInteger(maxPrompts) || maxPrompts < 1 || maxPrompts > 10) {
    throw new Error('invalid_manual_intervention');
  }
  const controller = new AbortController();
  const cancel = () => controller.abort(new Error('manual_intervention_cancelled'));
  signal?.addEventListener('abort', cancel, { once: true });
  if (signal?.aborted) cancel();
  const timer = setTimeout(() => controller.abort(new Error('manual_intervention_timeout')), timeoutMs);
  let onAbort;
  const aborted = new Promise((_, reject) => {
    onAbort = () => reject(controller.signal.reason);
    if (controller.signal.aborted) onAbort();
    else controller.signal.addEventListener('abort', onAbort, { once: true });
  });
  aborted.catch(() => {});
  const bounded = operation => {
    controller.signal.throwIfAborted();
    return Promise.race([Promise.resolve().then(() => {
      controller.signal.throwIfAborted();
      return operation();
    }), aborted]);
  };
  try {
    let attempts = 0;
    while (true) {
      const kind = await bounded(inspect);
      if (kind === null) return { state: 'clear' };
      if (!Object.hasOwn(MESSAGES, kind)) throw new Error('invalid_manual_intervention');
      if (!requestHuman || attempts >= maxPrompts) return { state: 'needs_human', kind };
      await bounded(() => requestHuman(Object.freeze({
        kind, message: MESSAGES[kind], attempt: ++attempts, signal: controller.signal,
      })));
    }
  } finally {
    clearTimeout(timer);
    signal?.removeEventListener('abort', cancel);
    controller.signal.removeEventListener('abort', onAbort);
  }
}

module.exports = { challengeKind, inspectChallenge, waitForManualClearance, MESSAGES };
