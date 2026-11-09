'use strict';

// Read-only Chromium lifecycle observation. Response headers alone do not prove
// that a document committed, especially when identification reloads the same URL.
async function observeDocuments(context, page, validateNavigation = () => {}) {
  const session = await context.newCDPSession(page);
  const records = new Map();
  const requests = new Map();
  const pending = new Set();
  const listeners = [];
  const waiters = new Set();
  let mainFrame;
  let committed;
  let latestRequested;
  let generation = 0;
  let fatal;
  let closed = false;
  const notify = () => { for (const waiter of [...waiters]) waiter(); };
  const listen = (name, callback) => { session.on(name, callback); listeners.push([name, callback]); };
  const record = loader => {
    if (!records.has(loader)) {
      if (!loader || records.size >= 16) { fatal = 'navigation_failed'; notify(); return null; }
      records.set(loader, { loader, ready: false, committed: false, failed: false });
    }
    return records.get(loader);
  };
  const close = async () => {
    if (closed) return;
    closed = true;
    fatal = 'navigation_failed';
    notify();
    for (const [name, callback] of listeners) session.off(name, callback);
    waiters.clear();
    records.clear();
    requests.clear();
    pending.clear();
    await session.detach().catch(() => {});
  };
  try {
    await session.send('Page.enable');
    const [, , tree] = await Promise.all([
      session.send('Network.enable'),
      session.send('Page.setLifecycleEventsEnabled', { enabled: true }),
      session.send('Page.getFrameTree'),
    ]);
    mainFrame = tree.frameTree.frame.id;
    listen('Network.requestWillBeSent', event => {
      if (event.type !== 'Document' || event.frameId !== mainFrame) return;
      generation++;
      const value = record(event.loaderId);
      if (!value) return;
      latestRequested = event.loaderId;
      requests.set(event.requestId, event.loaderId);
      pending.add(event.loaderId);
      Object.assign(value, { url: event.request.url, status: null, headers: null,
        ready: false, committed: false, failed: false });
      notify();
    });
    listen('Network.responseReceived', event => {
      if (event.type !== 'Document' || event.frameId !== mainFrame) return;
      const value = record(event.loaderId);
      if (!value) return;
      value.url = event.response.url;
      value.status = event.response.status;
      value.headers = {};
      for (const [name, content] of Object.entries(event.response.headers)) {
        const key = name.toLowerCase();
        value.headers[key] = Object.hasOwn(value.headers, key)
          ? value.headers[key] + ', ' + String(content) : String(content);
      }
      notify();
    });
    listen('Page.frameNavigated', event => {
      if (event.frame.id !== mainFrame || event.frame.parentId) return;
      if (latestRequested) {
        try {
          const failed = records.get(latestRequested);
          // A failed allowed request may commit Chromium's internal error page.
          // This differs from a website navigating to an unrelated empty page.
          const browserError = event.frame.url.startsWith('chrome-error://')
            && event.frame.unreachableUrl && failed?.failed && failed.url === event.frame.unreachableUrl;
          validateNavigation({ url: browserError ? event.frame.unreachableUrl : event.frame.url });
        } catch {
          fatal = 'unsafe_redirect';
          notify();
          return;
        }
      }
      generation++;
      committed = event.frame.loaderId;
      const value = record(committed);
      if (!value) return;
      value.committed = true;
      value.commitUrl = event.frame.url;
      pending.delete(committed);
      notify();
    });
    listen('Page.navigatedWithinDocument', event => {
      if (event.frameId !== mainFrame) return;
      try { validateNavigation({ url: event.url }); }
      catch { fatal = 'unsafe_redirect'; notify(); return; }
      generation++;
      const value = records.get(committed);
      if (value) value.commitUrl = event.url;
      notify();
    });
    listen('Page.lifecycleEvent', event => {
      if (event.frameId !== mainFrame || event.name !== 'DOMContentLoaded') return;
      const value = record(event.loaderId);
      if (value) value.ready = true;
      notify();
    });
    listen('Network.loadingFailed', event => {
      const loader = requests.get(event.requestId);
      if (!loader) return;
      const value = records.get(loader);
      if (value) value.failed = true;
      pending.delete(loader);
      notify();
    });

    const lastCommittedResponse = () => {
      const value = records.get(committed);
      if (!value?.committed || !Number.isInteger(value.status) || !value.headers) return null;
      return { loader: committed, generation, status: value.status, headers: value.headers,
        url: value.commitUrl, response_url: value.url };
    };
    const current = () => {
      if (fatal) throw new Error(fatal);
      if (records.get(latestRequested)?.failed) throw new Error('navigation_failed');
      const value = records.get(committed);
      if (!value || committed !== latestRequested || pending.size || value.failed || !value.committed
          || !value.ready || !Number.isInteger(value.status) || !value.headers) return null;
      // Fragment changes do not alter the response URL. All other history changes
      // are ambiguous unless a new document and response have been observed.
      const withoutFragment = url => { const parsed = new URL(url); parsed.hash = ''; return parsed.href; };
      if (withoutFragment(value.url) !== withoutFragment(value.commitUrl)) throw new Error('unsafe_redirect');
      return { loader: committed, generation, status: value.status, headers: value.headers, url: value.commitUrl };
    };
    const synchronize = async () => {
      if (closed) throw new Error('navigation_failed');
      const tree = await session.send('Page.getFrameTree');
      if (tree.frameTree.frame.id !== mainFrame || (committed && tree.frameTree.frame.loaderId !== committed)) {
        throw new Error('navigation_failed');
      }
    };
    const assertCurrent = snapshot => {
      const value = current();
      if (!value || value.loader !== snapshot.loader || value.generation !== snapshot.generation) {
        throw new Error('navigation_failed');
      }
    };
    const waitForIdentification = (timeoutMs, continueWaiting, validate) => new Promise((resolve, reject) => {
      let timer;
      const finish = (error, value) => {
        clearTimeout(timer);
        waiters.delete(check);
        if (error) reject(error); else resolve(value);
      };
      const check = (expired = false) => {
        try {
          // Error metadata is already conclusive once committed. Do not wait for
          // a blocked/rate-limited document's scripts or body to finish loading.
          const blocked = lastCommittedResponse();
          if (blocked && [403, 429].includes(blocked.status)) {
            validate(blocked);
            return finish(null, blocked);
          }
          const value = current();
          if (value) {
            validate(value);
            if (expired || !continueWaiting(value)) return finish(null, value);
          }
          if (expired) finish(new Error('navigation_timeout'));
        } catch (error) { finish(error); }
      };
      waiters.add(check);
      timer = setTimeout(() => check(true), Math.max(0, timeoutMs));
      check();
    });
    return { current, lastCommittedResponse, synchronize, assertCurrent, waitForIdentification, close };
  } catch (error) {
    await close();
    throw error;
  }
}

module.exports = { observeDocuments };
