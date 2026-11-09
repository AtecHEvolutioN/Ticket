'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { EventEmitter } = require('node:events');
const { observeDocuments } = require('./document-lifecycle.cjs');

test('response headers and old document load events cannot promote an uncommitted response', async () => {
  const url = 'https://www.ticketmaster.cz/event/synthetic/12345678';
  const session = new EventEmitter();
  let loader = 'blank';
  let detached = false;
  session.send = async () => ({ frameTree: { frame: { id: 'main', loaderId: loader } } });
  session.detach = async () => { detached = true; };
  const observer = await observeDocuments({ newCDPSession: async () => session }, {});
  const begin = (id, status) => {
    session.emit('Network.requestWillBeSent', { type: 'Document', frameId: 'main', loaderId: id,
      requestId: id, request: { url } });
    session.emit('Network.responseReceived', { type: 'Document', frameId: 'main', loaderId: id,
      response: { status, url, headers: { 'Content-Type': 'text/html', 'TM-BL': '1', 'tm-bl': '0' } } });
  };
  const commit = id => {
    loader = id;
    session.emit('Page.frameNavigated', { frame: { id: 'main', loaderId: id, url } });
  };
  const ready = id => session.emit('Page.lifecycleEvent', { frameId: 'main', loaderId: id, name: 'DOMContentLoaded' });
  try {
    begin('initial', 401); commit('initial'); ready('initial');
    const initial = observer.current();
    assert.equal(initial.status, 401);
    assert.equal(initial.headers['tm-bl'], '1, 0');
    begin('next', 200);
    ready('initial');
    assert.equal(observer.current(), null);
    await assert.rejects(observer.waitForIdentification(10, value => value.status === 401, () => {}), /navigation_timeout/);
    commit('next');
    assert.equal(observer.current(), null);
    ready('next');
    const final = observer.current();
    assert.equal(final.status, 200);
    assert.throws(() => observer.assertCurrent(initial), /navigation_failed/);
    observer.assertCurrent(final);
    session.emit('Page.navigatedWithinDocument', { frameId: 'main', url: url + '#changed' });
    assert.throws(() => observer.assertCurrent(final), /navigation_failed/);
    begin('blocked', 429); commit('blocked');
    assert.equal(observer.current(), null); // No DOMContentLoaded for this error.
    const blocked = await observer.waitForIdentification(1000, () => true, () => {});
    assert.equal(blocked.status, 429);
    assert.equal(blocked.response_url, url);
    begin('after-block', 200); commit('after-block'); ready('after-block');
    const pending = observer.waitForIdentification(10000, () => true, () => {});
    await observer.close();
    await assert.rejects(pending, /navigation_failed/);
    assert.equal(detached, true);
    assert.equal(session.eventNames().length, 0);
  } finally { await observer.close(); }
});
