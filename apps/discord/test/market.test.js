import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import test from 'node:test';
import '../public/market.js';

const { validateExport, parseExport, demoExport, buildBuckets, money, minorDigits, seriesCatalog, basisLabel, seatLabel, MAX_BYTES } = globalThis.TicketHistory;
const at = 1794400000;
function documentFixture() { return demoExport(at); }
function row(changes = {}) {
  return { series_id: 'one-series', ticket_type: 'Standard', inventory_kind: 'resale', currency: 'EUR',
    price_scope: 'per_ticket', fees: 'included', quantity_kind: 'listing', kind: 'offer',
    min_price_minor: 10000, max_price_minor: 30000, median_price_minor: 15000,
    observed_count: 4, reported_quantity: 8, ...changes };
}
function sample(observedAt, changes = {}) {
  return { observed_at: observedAt, availability: 'available', evidence: 'offers', complete: true,
    series: [row()], ...changes };
}

test('synthetic demonstration is explicitly named and satisfies the real import contract', () => {
  const fixture = documentFixture();
  const checked = validateExport(fixture);
  assert.equal(checked.sampleCount, 577);
  assert.match(fixture.events[0].name, /Synthetic/);
  assert.equal(seriesCatalog(fixture.events[0].sources[0]).length, 2);
  const buckets = buildBuckets(fixture.events[0].sources[0].samples, 'synthetic-resale', 1800, 900, at - 48 * 3600, at);
  assert.ok(buckets.some(bucket => bucket.open !== null && bucket.open !== bucket.close && bucket.high > bucket.low));
});

test('viewer starts empty and uses local assets without CDN or automatic fetching', async () => {
  const html = await readFile(new URL('../public/market.html', import.meta.url), 'utf8');
  const script = await readFile(new URL('../public/market.js', import.meta.url), 'utf8');
  const css = await readFile(new URL('../public/market.css', import.meta.url), 'utf8');
  assert.match(html, /<!-- HISTORY_DATA -->/);
  assert.match(html, /<link rel="stylesheet" href="market.css">/);
  assert.match(html, /<script src="market.js" defer><\/script>/);
  assert.match(html, /No live data or example prices are shown by default/);
  assert.doesNotMatch(script, /XMLHttpRequest|\.innerHTML\s*=|localStorage|sessionStorage/);
  assert.match(html, /No requests are made until you connect/);
  assert.doesNotMatch(css, /@import|https?:\/\//);
});

test('live endpoint validation limits credentials to a selected HTTPS or loopback origin', () => {
  const { liveEndpoint, liveRequestUrl } = globalThis.TicketHistory;
  assert.equal(liveEndpoint('https://history.example.com'), 'https://history.example.com');
  assert.equal(liveEndpoint('http://127.0.0.1:8789'), 'http://127.0.0.1:8789');
  assert.equal(liveEndpoint('http://[::1]:8789'), 'http://[::1]:8789');
  for (const endpoint of ['http://example.com', 'http://127.0.0.1.example.com', 'https://user:pass@example.com',
    'https://example.com/api/history', 'https://example.com/?token=secret', 'https://example.com/#secret',
    'javascript:alert(1)', 'https://exam\nple.com', '//example.com']) assert.throws(() => liveEndpoint(endpoint));
  const url = new URL(liveRequestUrl('https://history.example.com', 'https://www.ticketmaster.nl/event/123?language=en-us#top', 48, at * 1000));
  assert.equal(url.pathname, '/api/history');
  assert.equal(url.searchParams.get('event_url'), 'https://www.ticketmaster.nl/event/123');
  assert.equal(Date.parse(url.searchParams.get('to')) - Date.parse(url.searchParams.get('from')), 48 * 3600000);
  assert.throws(() => liveRequestUrl('https://history.example.com', '', 9000, at * 1000));
  assert.throws(() => liveRequestUrl('https://history.example.com', 'https://www.ticketmaster.nl/payment/', 48, at * 1000));
});

function liveHarness(fetch) {
  const timers = new Map(); let sequence = 0;
  const states = []; const data = []; let idle = 0;
  const connection = globalThis.TicketHistory.createLiveConnection({ fetch, now: () => at * 1000,
    setTimeout: (fn, delay) => { const id = ++sequence; timers.set(id, { fn, delay }); return id; },
    clearTimeout: id => timers.delete(id), onState: value => states.push(value), onData: value => data.push(value), onIdle: () => { idle += 1; } });
  return { connection, states, data, timers, idle: () => idle };
}
const liveInput = { endpoint: 'https://history.example.com', token: 'synthetic-read-only-secret-123456789', eventUrl: '', hours: 48 };
const settle = () => new Promise(resolve => setImmediate(resolve));
function liveResponse(data = documentFixture(), status = 200, headers = {}) {
  return new Response(JSON.stringify(data), { status, headers: { 'content-type': 'application/json', ...headers } });
}

test('live refresh is opt-in, uses header-only credentials and retains strict transport options', async () => {
  const requests = [];
  const h = liveHarness(async (...args) => { requests.push(args); return liveResponse(); });
  assert.equal(requests.length, 0);
  h.connection.connect(liveInput); await settle();
  assert.equal(requests.length, 1); assert.equal(h.data.length, 1);
  const [url, options] = requests[0];
  assert.equal(url.includes(liveInput.token), false);
  assert.equal(options.headers.Authorization, `Bearer ${liveInput.token}`);
  for (const [key, value] of Object.entries({ credentials: 'omit', redirect: 'error', cache: 'no-store', referrerPolicy: 'no-referrer', mode: 'cors' })) assert.equal(options[key], value);
  assert.equal(h.states.at(-1).phase, 'live');
  assert.equal([...h.timers.values()].filter(timer => timer.delay === 30000).length, 1);
  h.connection.disconnect();
  assert.equal(h.timers.size, 0); assert.equal(h.states.at(-1).connected, false);
  await h.connection.refresh(); assert.equal(requests.length, 1);
  assert.equal(JSON.stringify(h.states).includes(liveInput.token), false);
});

test('overlapping refreshes and late results cannot overwrite a disconnected or newer connection', async () => {
  const pending = []; const h = liveHarness((url, options) => new Promise(resolve => pending.push({ resolve, options })));
  h.connection.connect(liveInput);
  await h.connection.refresh(); assert.equal(pending.length, 1);
  h.connection.disconnect(); assert.equal(pending[0].options.signal.aborted, true);
  h.connection.connect({ ...liveInput, endpoint: 'https://other.example.com' });
  pending[0].resolve(liveResponse()); await settle(); assert.equal(h.data.length, 0);
  pending[1].resolve(liveResponse({ ...documentFixture(), generated_at: at + 1 })); await settle();
  assert.equal(h.data.length, 1); assert.equal(h.data[0].generated_at, at + 1);
  h.connection.disconnect();
});

test('authentication failure clears connection and requires explicit reconnect without echoing server errors', async () => {
  for (const status of [401, 403]) {
    let requests = 0;
    const h = liveHarness(async () => { requests += 1; return liveResponse({ error: liveInput.token }, status); });
    h.connection.connect(liveInput); await settle();
    assert.equal(h.states.at(-1).phase, 'unauthorized'); assert.equal(h.states.at(-1).connected, false);
    assert.equal(h.timers.size, 0); assert.equal(h.data.length, 0);
    await h.connection.refresh(); h.connection.resume(); await settle(); assert.equal(requests, 1);
    assert.equal(JSON.stringify(h.states).includes(liveInput.token), false);
  }
});

test('failed or malformed refreshes preserve last valid data and respect worker backoff', async () => {
  const responses = [liveResponse(), liveResponse({ error: 'secret-body' }, 429, { 'retry-after': '90' }), liveResponse({ bad: true }), liveResponse()];
  const h = liveHarness(async () => responses.shift());
  h.connection.connect(liveInput); await settle(); assert.equal(h.data.length, 1);
  await h.connection.refresh();
  assert.equal(h.data.length, 1); assert.equal(h.states.at(-1).phase, 'error');
  assert.equal([...h.timers.values()].some(timer => timer.delay === 90000), true);
  assert.equal(JSON.stringify(h.states).includes('secret-body'), false);
  await h.connection.refresh(); assert.equal(h.data.length, 1); assert.equal(h.states.at(-1).phase, 'error');
  h.connection.pause(); assert.equal(h.timers.size, 0);
  await h.connection.refresh(); assert.equal(h.data.length, 1);
  h.connection.resume(); await settle(); assert.equal(h.data.length, 2);
  h.connection.disconnect();
});

test('streamed live responses enforce byte bounds and validate before accepting any history', async () => {
  const { readLiveExport } = globalThis.TicketHistory;
  await assert.rejects(readLiveExport(new Response('ignored', { headers: { 'content-type': 'text/html' } })), /unsupported response/);
  await assert.rejects(readLiveExport(liveResponse({}, 200, { 'content-length': String(MAX_BYTES + 1) })), /16 MiB/);
  let cancelled = false;
  const body = new ReadableStream({ start(controller) { controller.enqueue(new Uint8Array(MAX_BYTES)); controller.enqueue(new Uint8Array(1)); }, cancel() { cancelled = true; } });
  await assert.rejects(readLiveExport(new Response(body, { headers: { 'content-type': 'application/json' } })), /16 MiB/);
  assert.equal(cancelled, true);
  await assert.rejects(readLiveExport(liveResponse({ schema_version: 2 })), /schema version/);
});

test('no matching history is distinct from an unavailable database', async () => {
  const h = liveHarness(async () => liveResponse({ error: { code: 'no_history' } }, 404));
  h.connection.connect(liveInput); await settle();
  assert.equal(h.states.at(-1).phase, 'error');
  assert.match(h.states.at(-1).message, /No observations match this event and time range/);
  assert.doesNotMatch(h.states.at(-1).message, /database/);
  h.connection.disconnect();
});

test('import rejects oversized or invalid JSON without rendering its values', () => {
  assert.throws(() => parseExport('x'.repeat(MAX_BYTES + 1)), /16 MiB/);
  assert.throws(() => parseExport('{'), /not valid JSON/);
  const fixture = documentFixture(); fixture.schema_version = 2;
  assert.throws(() => validateExport(fixture), /schema version/);
  fixture.schema_version = 1; fixture.events[0].event_url = 'javascript:alert(1)';
  assert.throws(() => validateExport(fixture), /metadata/);
});

test('nullable prices/currency/type remain unknown, never default to zero or a currency', () => {
  const fixture = documentFixture();
  fixture.events[0].sources[0].samples = [sample(at, { series: [row({ ticket_type: null, currency: null,
    min_price_minor: null, max_price_minor: null, median_price_minor: null, reported_quantity: null })] })];
  validateExport(fixture);
  const buckets = buildBuckets(fixture.events[0].sources[0].samples, 'one-series', 60, 300, at, at);
  assert.equal(buckets[0].close, null);
  assert.match(money(12345, null), /12,345 minor units.*unknown/);
  assert.equal(money(null, 'EUR'), '—');
});

test('currency, fee and ticket dimensions cannot change within an imported series', () => {
  for (const changes of [{ currency: 'CZK' }, { fees: 'excluded' }, { price_scope: 'total', price_quantity: 2 }, { ticket_type: 'VIP' }, { inventory_kind: 'primary' }]) {
    const fixture = documentFixture();
    fixture.events[0].sources[0].samples = [sample(at - 60), sample(at, { series: [row(changes)] })];
    assert.throws(() => validateExport(fixture), /mixes currencies/);
  }
});

test('inconsistent inventory and numeric values are rejected before display', () => {
  const variants = [
    sample(at, { availability: 'blocked' }),
    sample(at, { availability: 'unknown' }),
    sample(at, { evidence: 'metadata' }),
    sample(at, { series: [row({ kind: 'displayed_category' })] }),
    sample(at, { series: [row({ quantity_kind: 'selectable' })] }),
    sample(at, { series: [row({ reported_quantity: 0 })] }),
    sample(at, { series: [row({ observed_count: 0 })] }),
    sample(at, { series: [row({ observed_count: 20001 })] }),
    sample(at, { series: [row({ min_price_minor: Infinity })] }),
    sample(at, { series: [row({ min_price_minor: 0.5 })] }),
    sample(at, { series: [row({ min_price_minor: null })] }),
    sample(at, { series: [row({ median_price_minor: 999999 })] }),
  ];
  for (const observation of variants) {
    const fixture = documentFixture(); fixture.events[0].sources[0].samples = [observation];
    assert.throws(() => validateExport(fixture));
  }
});

test('OHLC follows minimum asking price, not listing range, and counts use the last poll', () => {
  const samples = [sample(120, { series: [row({ min_price_minor: 11000, observed_count: 4, reported_quantity: 8 })] }),
    sample(130, { series: [row({ min_price_minor: 9000, observed_count: 7, reported_quantity: 14 })] }),
    sample(140, { series: [row({ min_price_minor: 10000, observed_count: 3, reported_quantity: 6 })] })];
  const buckets = buildBuckets(samples, 'one-series', 60, 300, 100, 200);
  assert.equal(buckets.length, 1);
  assert.deepEqual([buckets[0].open, buckets[0].high, buckets[0].low, buckets[0].close], [11000, 11000, 9000, 10000]);
  assert.equal(buckets[0].observed_count, 3);
  assert.equal(buckets[0].reported_quantity, 6);
  assert.equal(buckets[0].samples, 3);
});

test('blocked, absent and unknown observations break continuity and never carry prices forward', () => {
  for (const middle of [sample(180, { availability: 'blocked', series: [] }), sample(180, { series: [] }), sample(180, { availability: 'unknown', series: [] })]) {
    const buckets = buildBuckets([sample(120), middle, sample(240)], 'one-series', 60, 300, 100, 300);
    assert.equal(buckets.length, 3);
    assert.equal(buckets[1].close, null);
    assert.equal(buckets[1].observed_count, null);
    assert.notEqual(buckets[0].segment, buckets[2].segment);
  }
});

test('same-second gaps preserve input order and only the final bucket segment has a count bar', () => {
  const buckets = buildBuckets([sample(120), sample(120, { availability: 'blocked', series: [] }),
    sample(120, { series: [row({ min_price_minor: 12000, observed_count: 2 })] })], 'one-series', 60, 300, 100, 200);
  assert.equal(buckets.length, 3);
  assert.deepEqual(buckets.map(bucket => bucket.close), [10000, null, 12000]);
  assert.deepEqual(buckets.map(bucket => bucket.observed_count), [null, null, 2]);
  assert.notEqual(buckets[0].segment, buckets[2].segment);
  const endsUnknown = buildBuckets([sample(120), sample(130, { series: [] })], 'one-series', 60, 300, 100, 200);
  assert.ok(endsUnknown.every(bucket => bucket.observed_count === null));
});

test('long polling outages and unscheduled observations are not line connected', () => {
  let buckets = buildBuckets([sample(100), sample(101), sample(1000)], 'one-series', 60, 300, 0, 2000);
  assert.notEqual(buckets[0].segment, buckets[buckets.length - 1].segment);
  assert.equal(buckets[buckets.length - 1].long_gap_before, true);
  buckets = buildBuckets([sample(100), sample(101)], 'one-series', 60, 0, 0, 200);
  assert.equal(buckets.length, 2);
  assert.notEqual(buckets[0].segment, buckets[1].segment);
});

test('known displayed prices stay connected when stock is unknown without reporting inventory', () => {
  const quote = row({ kind: 'displayed_category', quantity_kind: 'unknown', price_scope: 'unknown', fees: 'unknown', reported_quantity: null, median_price_minor: null });
  const observations = [sample(120, { availability: 'unknown', evidence: 'metadata', series: [quote] }),
    sample(180, { availability: 'unknown', evidence: 'metadata', series: [quote] })];
  const fixture = documentFixture(); fixture.events[0].sources[0].samples = observations;
  validateExport(fixture);
  const buckets = buildBuckets(observations, 'one-series', 60, 300, 100, 200);
  assert.equal(buckets[0].close, 10000);
  assert.equal(buckets[0].reported_quantity, null);
  assert.equal(buckets[0].segment, buckets[1].segment);
});

test('separate sources are never aggregated into one series catalog or price bucket', () => {
  const one = { samples: [sample(120)] };
  const two = { samples: [sample(120, { series: [row({ series_id: 'other-source-series', currency: 'CZK' })] })] };
  assert.deepEqual(seriesCatalog(one).map(value => value.series_id), ['one-series']);
  assert.deepEqual(seriesCatalog(two).map(value => value.series_id), ['other-source-series']);
  assert.equal(buildBuckets(one.samples, 'other-source-series', 60, 300, 0, 200)[0].close, null);
});

test('currency formatting uses known minor exponents and does not guess unknown codes', () => {
  assert.equal(minorDigits('JPY'), 0);
  assert.equal(minorDigits('KWD'), 3);
  assert.equal(minorDigits('EUR'), 2);
  assert.equal(minorDigits('ZZZ'), null);
  assert.match(money(1234, 'KWD'), /1\.234/);
  assert.match(money(1234, 'JPY'), /1,234/);
  assert.match(money(1234, 'ZZZ'), /minor units.*scale unknown/);
});

test('date windows filter observations without carrying an earlier quote into an empty window', () => {
  assert.deepEqual(buildBuckets([sample(100)], 'one-series', 60, 300, 200, 300), []);
  const buckets = buildBuckets([sample(100), sample(240)], 'one-series', 60, 300, 200, 300);
  assert.equal(buckets.length, 1);
  assert.equal(buckets[0].at, 240);
});

test('total-price groups require a known consistent bundle quantity before plotting prices', () => {
  const fixture = documentFixture();
  const total = row({ price_scope: 'total', price_quantity: 2 });
  fixture.events[0].sources[0].samples = [sample(at, { series: [total] })];
  validateExport(fixture);
  assert.equal(basisLabel(total), 'Group total · 2 tickets');
  fixture.events[0].sources[0].samples.push(sample(at, { series: [{ ...total, price_quantity: 3 }] }));
  assert.throws(() => validateExport(fixture), /mixes currencies/);
  for (const changes of [{ price_quantity: undefined }, { price_quantity: null }, { price_quantity: 0 }, { price_quantity: 2.5 }]) {
    fixture.events[0].sources[0].samples = [sample(at, { series: [{ ...total, ...changes }] })];
    assert.throws(() => validateExport(fixture));
  }
  for (const changes of [{ quantity_kind: 'remaining' }, { kind: 'displayed_category', reported_quantity: null }, { kind: 'displayed_seat', reported_quantity: null }]) {
    fixture.events[0].sources[0].samples = [sample(at, { series: [{ ...total, ...changes }] })];
    assert.throws(() => validateExport(fixture));
  }
  fixture.events[0].sources[0].samples = [sample(at, { series: [{ ...total, price_quantity: undefined,
    min_price_minor: null, max_price_minor: null, median_price_minor: null }] })];
  validateExport(fixture);
  assert.equal(basisLabel(row()), 'Per ticket');
});

test('per-seat quotes have auditable labels and remain separate from aggregate map quotes', () => {
  const seat = { seat_key: 'a'.repeat(64), section: 'A', row: '4', number: '12' };
  const quoted = row({ kind: 'displayed_seat', seat, observed_count: 1, reported_quantity: null });
  const aggregate = { ...quoted, series_id: 'aggregate-seats', seat: null, observed_count: 8 };
  const source = { samples: [sample(120, { availability: 'unknown', series: [quoted, aggregate] }),
    sample(180, { availability: 'unknown', series: [{ ...quoted, min_price_minor: 11000 }, aggregate] })] };
  const fixture = documentFixture(); fixture.events[0].sources[0].samples = source.samples;
  validateExport(fixture);
  assert.equal(seatLabel(quoted), 'Section A · Row 4 · Seat 12');
  assert.equal(seatLabel(aggregate), 'All observed seat quotes');
  assert.equal(seatLabel({ ...aggregate, seat: undefined }), 'All observed seat quotes');
  assert.equal(seriesCatalog(source).length, 2);
  const individual = buildBuckets(source.samples, quoted.series_id, 60, 300, 100, 200);
  assert.deepEqual(individual.map(bucket => bucket.observed_count), [1, 1]);
  assert.ok(individual.every(bucket => bucket.reported_quantity === null));
  assert.equal(individual[0].segment, individual[1].segment);
  assert.equal(buildBuckets(source.samples, aggregate.series_id, 60, 300, 100, 200)[0].observed_count, 8);
});

test('a missing seat quote creates a gap even when the map aggregate still has prices', () => {
  const seat = { seat_key: 'a'.repeat(64), section: 'A', row: '4', number: '12' };
  const quoted = row({ kind: 'displayed_seat', seat, observed_count: 1, reported_quantity: null });
  const aggregate = { ...quoted, series_id: 'aggregate-seats', seat: null, observed_count: 8 };
  const observations = [sample(120, { availability: 'unknown', series: [quoted, aggregate] }),
    sample(180, { availability: 'unknown', series: [aggregate] }),
    sample(240, { availability: 'unknown', series: [quoted, aggregate] })];
  const buckets = buildBuckets(observations, quoted.series_id, 60, 300, 100, 300);
  assert.equal(buckets[1].close, null);
  assert.equal(buckets[1].observed_count, null);
  assert.notEqual(buckets[0].segment, buckets[2].segment);
});

test('per-seat validation rejects malformed identity, wrong kind, excess count and identity drift', () => {
  const seat = { seat_key: 'a'.repeat(64), section: 'A', row: '4', number: '12' };
  const quoted = row({ kind: 'displayed_seat', seat, observed_count: 1, reported_quantity: null });
  const fixture = documentFixture();
  for (const changes of [{ kind: 'offer' }, { kind: 'displayed_category' }, { observed_count: 2 },
    { seat: { ...seat, seat_key: 'not-a-hash' } }, { seat: { ...seat, row: '' } },
    { seat: { ...seat, number: ' '.repeat(3) } }, { seat: { ...seat, section: 'a'.repeat(257) } }]) {
    fixture.events[0].sources[0].samples = [sample(at, { series: [{ ...quoted, ...changes }] })];
    assert.throws(() => validateExport(fixture));
  }
  fixture.events[0].sources[0].samples = [sample(at - 60, { series: [quoted] }),
    sample(at, { series: [{ ...quoted, seat: { ...seat, number: '13' } }] })];
  assert.throws(() => validateExport(fixture), /mixes currencies/);
  // JSON field ordering does not change the canonical seat dimension.
  fixture.events[0].sources[0].samples[1].series[0].seat = { number: '12', row: '4', section: 'A', seat_key: 'a'.repeat(64) };
  validateExport(fixture);
});
