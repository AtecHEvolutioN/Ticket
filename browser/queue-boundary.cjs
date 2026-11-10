'use strict';

// The reviewed Italian event route can redirect through this queue host before
// returning a private queue token. Block that top-document request; do not solve,
// wait through, replay, or make the queue URL an allowed event destination.
const { eventIdentity } = require('./rendered.cjs');
function reviewedQueueRedirect(target, candidate) {
  if (eventIdentity(target)?.host !== 'ticketmaster.it') return false;
  try {
    const url = new URL(candidate);
    return url.protocol === 'https:' && url.hostname === 'wait.ticketmaster.it'
      && !url.username && !url.password && !url.port;
  } catch { return false; }
}
async function blockReviewedQueue(context, page, target) {
  if (eventIdentity(target)?.host !== 'ticketmaster.it') return { blocked: false, close: async () => {} };
  const session = await context.newCDPSession(page);
  let mainFrame, blocked = false, closed = false;
  const paused = event => {
    const deny = event.frameId === mainFrame && reviewedQueueRedirect(target, event.request.url);
    if (deny) blocked = true;
    // Fetch interception sees redirected requests that Playwright route handlers
    // omit. The request remains paused until failed or explicitly continued.
    void session.send(deny ? 'Fetch.failRequest' : 'Fetch.continueRequest', {
      requestId: event.requestId, ...(deny ? { errorReason: 'BlockedByClient' } : {}),
    }).catch(() => { if (!closed) void page.close().catch(() => {}); });
  };
  const close = async () => {
    if (closed) return;
    closed = true;
    session.off('Fetch.requestPaused', paused);
    await session.send('Fetch.disable').catch(() => {});
    await session.detach().catch(() => {});
  };
  try {
    const tree = await session.send('Page.getFrameTree');
    mainFrame = tree.frameTree.frame.id;
    session.on('Fetch.requestPaused', paused);
    await session.send('Fetch.enable', { patterns: [{ urlPattern: 'https://wait.ticketmaster.it/*',
      resourceType: 'Document', requestStage: 'Request' }] });
    return { get blocked() { return blocked; }, close };
  } catch { await close(); throw new Error('navigation_failed'); }
}
module.exports = { reviewedQueueRedirect, blockReviewedQueue };
