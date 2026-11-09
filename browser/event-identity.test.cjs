'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { sameEvent } = require('./rendered.cjs');

test('event identity accepts encoded display slugs and localized routes', () => {
  const expected = 'https://www.ticketmaster.de/event/12345';
  for (const path of [
    '/event/show-%7C-logen-seat/12345', '/event/bj%C3%B6rk/12345',
    '/event/björk/12345', '/en/event/changed-title/12345',
    '/en-de/changed-title/event/12345',
  ]) assert.ok(sameEvent(expected, `https://www.ticketmaster.de${path}`), path);
  for (const path of [
    '/biglietti/another-artist-roma/event/qfd2zy7m15y4',
    '/it/biglietti/another-artist-roma/event/qfd2zy7m15y4',
  ]) assert.ok(sameEvent('https://ticketmaster.it/event/qfd2zy7m15y4', `https://www.ticketmaster.it${path}`), path);
});

test('event identity never normalizes escaped routes into inventory evidence', () => {
  const expected = 'https://www.ticketmaster.de/event/12345';
  for (const path of [
    '/event/hello%2fworld/12345', '/event/hello%5cworld/12345',
    '/event/hello%252fworld/12345', '/event/%2e%2e/12345',
    '/event/%63heckout/12345', '/%65vent/show/12345',
    '/event/show/%31%32%33%34%35', '/event/show%3fcheckout/12345',
    '/event/show%00/12345', '/event/show%80/12345', '/event/show%/12345',
    '/english/event/title/12345', '/en/event/checkout/12345',
    '/biglietti/title/event/12345', '/event/show/12345/extra',
    '/event/show/12345?token=secret', '/event/show/99999',
  ]) assert.equal(sameEvent(expected, `https://www.ticketmaster.de${path}`), false, path);
});
