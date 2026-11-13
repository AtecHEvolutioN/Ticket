(function (root) {
  'use strict';
  const MAX_BYTES = 16 * 1024 * 1024;
  const ENUMS = {
    availability: ['available', 'unavailable', 'unknown', 'blocked'],
    evidence: ['offers', 'explicit_status', 'metadata', 'none'],
    kind: ['offer', 'displayed_category', 'displayed_seat'],
    inventory_kind: ['primary', 'resale', 'unknown'],
    quantity_kind: ['listing', 'remaining', 'selectable', 'unknown'],
    price_scope: ['per_ticket', 'total', 'unknown'],
    fees: ['included', 'excluded', 'unknown'],
  };
  const DIMENSIONS = ['ticket_type', 'inventory_kind', 'currency', 'price_scope', 'price_quantity', 'fees', 'quantity_kind', 'kind', 'seat'];
  const KIND_LABELS = { offer: 'Observed listings', displayed_category: 'Displayed categories', displayed_seat: 'Displayed seat quotes' };
  const BASIS_LABELS = { per_ticket: 'Per ticket', total: 'Group total', unknown: 'Price basis unknown' };
  const FEES_LABELS = { included: 'Fees included', excluded: 'Fees excluded', unknown: 'Fees unknown' };
  const QUANTITY_LABELS = { listing: 'Listing quantity', remaining: 'Reported remaining', selectable: 'Selection capacity', unknown: 'Quantity unknown' };
  const SVG_NS = 'http://www.w3.org/2000/svg';
  function basisLabel(series) { return series.price_scope === 'total' && series.price_quantity != null ? `Group total · ${series.price_quantity} tickets` : BASIS_LABELS[series.price_scope]; }
  function seatLabel(series) {
    if (series.kind !== 'displayed_seat') return null;
    return series.seat ? `Section ${series.seat.section} · Row ${series.seat.row} · Seat ${series.seat.number}` : 'All observed seat quotes';
  }
  function fail(message) { throw new Error(message); }
  function object(value) { return value !== null && typeof value === 'object' && !Array.isArray(value); }
  function text(value, nullable, limit) {
    return (nullable && value === null) || (typeof value === 'string' && value.length <= (limit || 512) && !/[\u0000-\u0008\u000b\u000c\u000e-\u001f]/.test(value));
  }
  function integer(value) { return Number.isSafeInteger(value) && value >= 0; }
  function timestamp(value) { return integer(value) && value <= 253402300799; }
  function price(value, fractional) {
    return value === null || (Number.isFinite(value) && value >= 0 && value <= Number.MAX_SAFE_INTEGER && (fractional || Number.isSafeInteger(value)));
  }
  function validSeat(series) {
    if (series.seat == null) return true;
    const seat = series.seat;
    return series.kind === 'displayed_seat' && series.observed_count === 1 && object(seat)
      && typeof seat.seat_key === 'string' && /^[0-9a-f]{64}$/.test(seat.seat_key)
      && ['section', 'row', 'number'].every(key => text(seat[key], false, 256) && seat[key].trim().length > 0);
  }
  function safeUrl(value) {
    try { const url = new URL(value); return url.protocol === 'https:' && !url.username && !url.password ? url.href : null; } catch { return null; }
  }
  function validateExport(data) {
    if (!object(data) || data.schema_version !== 1 || !timestamp(data.generated_at) || !Array.isArray(data.events) || data.events.length > 2000) fail('Unsupported or invalid history export. Expected schema version 1.');
    let count = 0; let entries = 0; let sourcesCount = 0;
    const eventIds = new Set();
    for (const event of data.events) {
      if (!object(event) || !text(event.event_id, false, 256) || !event.event_id || eventIds.has(event.event_id)
        || !text(event.event_url, false, 4096) || !safeUrl(event.event_url) || !text(event.name, false)
        || !text(event.artist, true) || !text(event.country, true, 64) || !text(event.city, true)
        || !timestamp(event.first_observed_at) || !timestamp(event.last_observed_at) || event.first_observed_at > event.last_observed_at
        || !Array.isArray(event.sources)) fail('An event in this export has invalid metadata.');
      eventIds.add(event.event_id);
      const sourceIds = new Set();
      for (const source of event.sources) {
        sourcesCount += 1;
        if (sourcesCount > 5000 || !object(source) || !text(source.source_id, false, 256) || !source.source_id || sourceIds.has(source.source_id)
          || !text(source.watch_id, false, 256) || !text(source.scope, false, 2048)
          || !integer(source.interval_secs) || source.interval_secs > 31536000 || !Array.isArray(source.samples)) fail('An observation source is invalid or exceeds viewer limits.');
        sourceIds.add(source.source_id);
        let lastAt = -1;
        const dimensions = new Map();
        for (const sample of source.samples) {
          count += 1;
          if (count > 10000 || !object(sample) || !timestamp(sample.observed_at) || sample.observed_at < lastAt
            || !ENUMS.availability.includes(sample.availability) || !ENUMS.evidence.includes(sample.evidence)
            || typeof sample.complete !== 'boolean' || !Array.isArray(sample.series)
            || (sample.availability === 'blocked' && sample.series.length !== 0)) fail('Sample data is invalid, out of order, or exceeds 10,000 observations.');
          lastAt = sample.observed_at;
          const ids = new Set();
          for (const series of sample.series) {
            entries += 1;
            if (entries > 200000 || !object(series) || !text(series.series_id, false, 256) || !series.series_id || ids.has(series.series_id)
              || !text(series.ticket_type, true) || !(series.currency === null || /^[A-Z]{3}$/.test(series.currency))
              || !['kind', 'inventory_kind', 'quantity_kind', 'price_scope', 'fees'].every(key => ENUMS[key].includes(series[key]))
              || !price(series.min_price_minor, false) || !price(series.max_price_minor, false) || !price(series.median_price_minor, true)
              || !integer(series.observed_count) || series.observed_count === 0 || series.observed_count > 20000
              || !(series.reported_quantity === null || (integer(series.reported_quantity) && series.reported_quantity > 0))
              || (series.kind === 'offer' && (sample.availability !== 'available' || sample.evidence !== 'offers'))
              || (series.reported_quantity !== null && (series.kind !== 'offer' || !['listing', 'remaining'].includes(series.quantity_kind)))
              || (series.price_quantity != null && (series.price_scope !== 'total' || series.kind !== 'offer' || series.quantity_kind !== 'listing'
                || !integer(series.price_quantity) || series.price_quantity < 1 || series.price_quantity > 4294967295))
              || (series.price_scope === 'total' && series.price_quantity == null && series.min_price_minor !== null)
              || !validSeat(series)
              || ((series.min_price_minor === null) !== (series.max_price_minor === null))
              || (series.min_price_minor !== null && series.max_price_minor !== null && series.min_price_minor > series.max_price_minor)
              || (series.median_price_minor !== null && (series.min_price_minor === null || series.max_price_minor === null
                || series.median_price_minor < series.min_price_minor || series.median_price_minor > series.max_price_minor))) fail('A price series contains invalid or incompatible values.');
            ids.add(series.series_id);
            const key = JSON.stringify(DIMENSIONS.map(dimension => dimension === 'seat' && series.seat
              ? [series.seat.seat_key, series.seat.section, series.seat.row, series.seat.number] : series[dimension]));
            if (dimensions.has(series.series_id) && dimensions.get(series.series_id) !== key) fail('A series mixes currencies, ticket types, price basis or other dimensions.');
            dimensions.set(series.series_id, key);
          }
        }
      }
    }
    return { data, sampleCount: count, sourceCount: sourcesCount };
  }
  function parseExport(raw) {
    if (typeof raw !== 'string' || new TextEncoder().encode(raw).byteLength > MAX_BYTES) fail('History files must be 16 MiB or smaller.');
    let parsed; try { parsed = JSON.parse(raw); } catch { fail('This file is not valid JSON. Choose a Ticket Watch history export.'); }
    return validateExport(parsed);
  }
  function liveEndpoint(value) {
    if (typeof value !== 'string' || /[\u0000-\u0020\u007f]/.test(value)) fail('Enter the full HTTPS address of your history worker.');
    let url;
    try { url = new URL(value); } catch { fail('Enter the full HTTPS address of your history worker.'); }
    const loopback = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
    if (value.length > 2048 || (url.protocol !== 'https:' && !(url.protocol === 'http:' && loopback))
      || url.username || url.password || url.pathname !== '/' || url.search || url.hash) {
      fail('Use an HTTPS worker address without a path, query or credentials. HTTP is allowed only on localhost.');
    }
    return url.origin;
  }
  function liveRequestUrl(endpoint, eventUrl, hours, now) {
    const url = new URL('/api/history', liveEndpoint(endpoint));
    if (!Number.isSafeInteger(hours) || hours < 1 || hours > 366 * 24) fail('Choose a history window between 1 hour and 366 days.');
    if (eventUrl) {
      if (eventUrl.length > 4096 || !safeUrl(eventUrl)) fail('The event filter must be a public HTTPS event URL.');
      const event = new URL(eventUrl);
      if (!event.pathname.startsWith('/event/')) fail('The event filter must use a public Ticketmaster /event/ page.');
      event.search = ''; event.hash = '';
      url.searchParams.set('event_url', event.href);
    }
    url.searchParams.set('from', new Date(now - hours * 3600000).toISOString());
    url.searchParams.set('to', new Date(now).toISOString());
    return url.href;
  }
  async function readLiveExport(response) {
    if (!/^application\/json(?:\s*;|$)/i.test(response.headers.get('content-type') || '')) fail('The worker returned an unsupported response.');
    const length = Number(response.headers.get('content-length'));
    if (Number.isFinite(length) && length > MAX_BYTES) fail('History exceeds 16 MiB. Choose a shorter window or one event.');
    if (!response.body || typeof response.body.getReader !== 'function') fail('Streaming history responses are not supported by this browser.');
    const reader = response.body.getReader(); const decoder = new TextDecoder('utf-8', { fatal: true });
    let size = 0; let raw = '';
    try {
      for (;;) {
        const { done, value } = await reader.read(); if (done) break;
        size += value.byteLength;
        if (size > MAX_BYTES) fail('History exceeds 16 MiB. Choose a shorter window or one event.');
        raw += decoder.decode(value, { stream: true });
      }
      raw += decoder.decode();
      return parseExport(raw);
    } finally { await reader.cancel().catch(() => {}); reader.releaseLock(); }
  }
  function createLiveConnection(options) {
    const fetchHistory = options.fetch || root.fetch.bind(root);
    const schedule = options.setTimeout || root.setTimeout.bind(root);
    const unschedule = options.clearTimeout || root.clearTimeout.bind(root);
    const now = options.now || Date.now;
    const onState = options.onState || (() => {}); const onData = options.onData || (() => {});
    let connection = null; let timer = null; let controller = null; let generation = 0;
    let paused = false; let busy = false; let lastSuccess = null;
    function report(phase, message) { onState({ phase, message, connected: Boolean(connection), busy, paused, lastSuccess }); }
    function cancel() {
      generation += 1;
      if (timer !== null) unschedule(timer); timer = null;
      controller?.abort(); controller = null; busy = false;
    }
    function disconnect() { cancel(); connection = null; paused = false; report('disconnected', 'Disconnected. The last chart is a snapshot; the access token has been cleared.'); }
    async function refresh() {
      if (!connection || paused || busy) return;
      if (timer !== null) unschedule(timer); timer = null;
      busy = true; const current = generation; const active = connection;
      const abort = new AbortController(); controller = abort;
      const timeout = schedule(() => abort.abort(), 35000); let delay = 30000;
      report('loading', lastSuccess ? 'Refreshing recorded history…' : 'Connecting to the history worker…');
      try {
        const url = liveRequestUrl(active.endpoint, active.eventUrl, active.hours, now());
        const response = await fetchHistory(url, { method: 'GET', headers: { Authorization: `Bearer ${active.token}`, Accept: 'application/json' },
          credentials: 'omit', redirect: 'error', cache: 'no-store', referrerPolicy: 'no-referrer', mode: 'cors', signal: abort.signal });
        if (current !== generation) return;
        if (response.status === 401 || response.status === 403) {
          await response.body?.cancel().catch(() => {});
          cancel(); connection = null; paused = false;
          report('unauthorized', 'Access rejected. The token was cleared. Enter a valid token and reconnect.'); return;
        }
        if (!response.ok) {
          const retry = Number(response.headers.get('retry-after'));
          if (response.status === 429 && Number.isFinite(retry) && retry > 0) delay = Math.max(30000, Math.min(3600000, retry * 1000));
          await response.body?.cancel().catch(() => {});
          const messages = { 400: 'The worker rejected the history filters.', 404: 'No observations match this event and time range.',
            413: 'History is too large. Reconnect with one event or a shorter window.', 429: 'The worker is busy; refresh has been delayed.',
            503: 'The history worker is temporarily unavailable.', 504: 'The history export timed out.' };
          fail(messages[response.status] || 'The worker could not return history.');
        }
        const checked = await readLiveExport(response);
        if (current !== generation) return;
        onData(checked.data); lastSuccess = now();
        report('live', 'Connected. History refreshes every 30 seconds; check each source’s observation time.');
      } catch (error) {
        if (current !== generation) return;
        // Never surface transport/server error text: it can contain addresses,
        // credentials or untrusted response bodies. Last valid data is retained.
        const safeMessages = ['The worker rejected the history filters.', 'No observations match this event and time range.',
          'History is too large. Reconnect with one event or a shorter window.', 'The worker is busy; refresh has been delayed.',
          'The history worker is temporarily unavailable.', 'The history export timed out.', 'The worker could not return history.',
          'History exceeds 16 MiB. Choose a shorter window or one event.'];
        report('error', `${safeMessages.includes(error?.message) ? error.message : 'History refresh failed or returned invalid data. Check the worker, access settings and connection.'} Last valid observations remain a snapshot.`);
      } finally {
        unschedule(timeout);
        abort.abort();
        if (current === generation) {
          busy = false; controller = null;
          if (connection && !paused) timer = schedule(refresh, delay);
          // Keep the last phase/message while making refresh controls available.
          options.onIdle?.();
        }
      }
    }
    function connect(input) {
      const endpoint = liveEndpoint(input.endpoint);
      if (typeof input.token !== 'string' || !/^[\x21-\x7e]{32,512}$/.test(input.token)) fail('Enter a read-only token with 32–512 printable characters and no spaces.');
      const eventUrl = (input.eventUrl || '').trim();
      liveRequestUrl(endpoint, eventUrl, input.hours, now());
      cancel(); connection = { endpoint, token: input.token, eventUrl, hours: input.hours };
      paused = false; lastSuccess = null;
      void refresh();
    }
    function pause() { if (!connection) return; cancel(); paused = true; report('paused', 'Refresh paused. The chart shows the last fetched snapshot.'); }
    function resume() { if (!connection || !paused) return; paused = false; void refresh(); }
    return { connect, disconnect, refresh, pause, resume };
  }
  function minorDigits(currency) {
    if (!currency) return null;
    try {
      if (typeof Intl.supportedValuesOf !== 'function' || !Intl.supportedValuesOf('currency').includes(currency)) return null;
      return new Intl.NumberFormat('en', { style: 'currency', currency }).resolvedOptions().maximumFractionDigits;
    } catch { return null; }
  }
  function money(value, currency, compact) {
    if (value === null || value === undefined || !Number.isFinite(value)) return '—';
    const digits = minorDigits(currency);
    if (digits === null) return `${new Intl.NumberFormat('en', { maximumFractionDigits: 2 }).format(value)} minor units · ${currency ? `${currency} scale unknown` : 'currency unknown'}`;
    const amount = value / 10 ** digits;
    return new Intl.NumberFormat('en', { style: 'currency', currency, currencyDisplay: compact ? 'narrowSymbol' : 'code', maximumFractionDigits: digits }).format(amount);
  }
  function seriesCatalog(source) {
    const found = new Map();
    for (const sample of source.samples) for (const series of sample.series) if (!found.has(series.series_id)) found.set(series.series_id, series);
    return [...found.values()];
  }
  function quoteVisible(sample, series) {
    if (!series || series.min_price_minor === null || sample.availability === 'blocked') return false;
    if (sample.availability === 'unknown' || sample.availability === 'unavailable') return series.kind !== 'offer';
    return true;
  }
  function bucketWidth(start, end) {
    const target = Math.max(1, (end - start) / 140);
    return [1, 10, 30, 60, 300, 900, 1800, 3600, 14400, 86400, 604800].find(value => value >= target) || 2592000;
  }
  function buildBuckets(samples, seriesId, width, gapSeconds, start, end) {
    const result = [];
    let current = null; let segment = 0; let previousTime = null;
    for (const sample of samples) {
      if (sample.observed_at < start || sample.observed_at > end) continue;
      const series = sample.series.find(value => value.series_id === seriesId);
      const visible = quoteVisible(sample, series);
      const longGap = previousTime !== null && (gapSeconds === 0 || sample.observed_at - previousTime > gapSeconds);
      if (longGap || !visible) { segment += 1; current = null; }
      const bucketStart = Math.floor(sample.observed_at / width) * width;
      // Unknown stock is distinct from an unknown displayed price. Consecutive
      // category/seat quotes retain their price continuity and original status;
      // absent quotes, blocked reads and cadence gaps still split the chart.
      if (!current || current.bucket_start !== bucketStart || current.segment !== segment) {
        current = { bucket_start: bucketStart, at: sample.observed_at, first_at: sample.observed_at,
          segment, open: null, high: null, low: null, close: null, samples: 0, gap: !visible,
          observed_count: null, reported_quantity: null, series: null, sample, long_gap_before: longGap };
        result.push(current);
      }
      current.at = sample.observed_at;
      current.sample = sample;
      current.series = series || null;
      current.samples += 1;
      current.observed_count = series && sample.availability !== 'blocked' && (sample.availability === 'available' || series.kind !== 'offer') ? series.observed_count : null;
      current.reported_quantity = series && sample.availability === 'available' ? series.reported_quantity : null;
      if (visible) {
        const value = series.min_price_minor;
        if (current.open === null) current.open = value;
        current.high = current.high === null ? value : Math.max(current.high, value);
        current.low = current.low === null ? value : Math.min(current.low, value);
        current.close = value;
      }
      if (!visible) { segment += 1; current = null; }
      previousTime = sample.observed_at;
    }
    // A gap can split one time bucket into several price segments. Only its
    // final observation supplies a count bar; earlier polls are never accumulated.
    const finalCounts = new Map();
    for (const bucket of result) {
      const earlier = finalCounts.get(bucket.bucket_start);
      if (earlier) { earlier.observed_count = null; earlier.reported_quantity = null; earlier.count_is_bucket_last = false; }
      bucket.count_is_bucket_last = true;
      finalCounts.set(bucket.bucket_start, bucket);
    }
    return result;
  }
  function demoExport(now) {
    const start = now - 48 * 3600;
    const source = { source_id: 'synthetic-source', watch_id: 'synthetic-watch', scope: 'synthetic event page', interval_secs: 300, samples: [] };
    for (let i = 0; i <= 576; i += 1) {
      const tick = i / 6;
      const blocked = tick >= 37 && tick <= 41;
      const ask = Math.round(8200 + Math.sin(tick / 8) * 700 + Math.cos(tick / 3) * 160 - tick * 8 + Math.sin(i * 2.3) * 42);
      const base = { ticket_type: 'Standard · synthetic', inventory_kind: 'resale', currency: 'EUR', price_scope: 'per_ticket', price_quantity: null, fees: 'unknown', quantity_kind: 'listing', kind: 'offer', median_price_minor: ask + 1200, observed_count: Math.max(1, Math.round(14 + Math.sin(tick / 6) * 8)), reported_quantity: null };
      source.samples.push({ observed_at: start + i * 300, availability: blocked ? 'blocked' : 'available', evidence: blocked ? 'none' : 'offers', complete: !blocked && i % 66 !== 0,
        series: blocked ? [] : [{ ...base, series_id: 'synthetic-resale', min_price_minor: ask, max_price_minor: ask + 2400 },
          { ...base, series_id: 'synthetic-primary', ticket_type: 'Standard · synthetic', inventory_kind: 'primary', min_price_minor: 7900, max_price_minor: 7900, median_price_minor: 7900, observed_count: 3 }] });
    }
    return { schema_version: 1, generated_at: now, events: [{ event_id: 'synthetic-event', event_url: 'https://www.ticketmaster.cz/', name: 'Synthetic concert · demonstration', artist: 'Fictional artist', country: 'CZ', city: 'Example venue', first_observed_at: start, last_observed_at: now, sources: [source] }] };
  }
  const helpers = { MAX_BYTES, validateExport, parseExport, buildBuckets, bucketWidth, quoteVisible, money, minorDigits, seriesCatalog, demoExport, basisLabel, seatLabel,
    liveEndpoint, liveRequestUrl, readLiveExport, createLiveConnection };
  root.TicketHistory = helpers;
  if (typeof module !== 'undefined' && module.exports) module.exports = helpers;
  if (typeof document === 'undefined') return;

  function boot() {
    const $ = id => document.getElementById(id);
    if (!$('price-chart')) return;
    const state = { data: null, demo: false, mode: 'offline', window: 'all', event: null, source: null, series: null, buckets: [], selected: 0 };
    let liveState = { phase: 'disconnected', connected: false, busy: false, paused: false, lastSuccess: null };
    let contentGeneration = 0;
    function set(id, value) { $(id).textContent = value; }
    function option(select, value, label) { const element = document.createElement('option'); element.value = value; element.textContent = label; select.appendChild(element); }
    function clear(select) { select.replaceChildren(); }
    function date(at, short) { return new Date(at * 1000).toLocaleString(undefined, short ? { month: 'short', day: 'numeric', hour: '2-digit', minute: '2-digit' } : { dateStyle: 'medium', timeStyle: 'medium' }); }
    function age(at) { const seconds = Math.floor(Date.now() / 1000) - at; return seconds < 0 ? 'ahead of this device clock' : seconds < 60 ? 'less than 1 min old' : seconds < 3600 ? `${Math.floor(seconds / 60)} min old` : seconds < 86400 ? `${Math.floor(seconds / 3600)}h old` : `${Math.floor(seconds / 86400)}d old`; }
    function sourceLabel(source) { return `${source.watch_id} · ${source.scope} · ${source.interval_secs ? `${source.interval_secs}s cadence` : 'unscheduled'}`; }
    function seriesLabel(series) { return `${series.ticket_type || 'Ticket type unknown'} · ${series.inventory_kind} · ${series.currency || 'currency unknown'} · ${basisLabel(series)} · ${FEES_LABELS[series.fees]} · ${seatLabel(series) || KIND_LABELS[series.kind]} · ${QUANTITY_LABELS[series.quantity_kind]}`; }
    function load(data, isDemo, label, live = false) {
      const checked = validateExport(data);
      const selected = live && state.mode === 'live' ? { eventId: state.event?.event_id, sourceId: state.source?.source_id, seriesId: state.series?.series_id } : {};
      state.data = data; state.demo = isDemo; state.mode = live ? 'live' : isDemo ? 'demo' : 'offline';
      $('error-banner').hidden = true;
      set('load-status', `${isDemo ? 'SYNTHETIC DEMO — invented prices and counts. ' : ''}${label} · ${data.events.length} events · ${checked.sampleCount.toLocaleString()} observations. ${live ? 'Recorded history from your connected worker; no ticket purchases are performed.' : 'File remains on this device.'}`);
      set('data-mode-badge', live ? 'Connected history · read only' : 'Local data · no uploads');
      clear($('event-select'));
      for (const event of data.events) option($('event-select'), event.event_id, `${event.name} · ${[event.city, event.country].filter(Boolean).join(', ')}`);
      $('event-select').disabled = data.events.length === 0;
      if (!data.events.length) { state.event = null; state.source = null; state.series = null; render(); return; }
      if (data.events.some(event => event.event_id === selected.eventId)) $('event-select').value = selected.eventId;
      chooseEvent(selected);
    }
    function chooseEvent(selected = {}) {
      state.event = state.data.events.find(event => event.event_id === $('event-select').value) || state.data.events[0];
      clear($('source-select'));
      for (const source of state.event.sources) option($('source-select'), source.source_id, sourceLabel(source));
      $('source-select').disabled = !state.event.sources.length;
      if (state.event.sources.some(source => source.source_id === selected.sourceId)) $('source-select').value = selected.sourceId;
      chooseSource(selected.seriesId);
    }
    function chooseSource(selectedId) {
      state.source = state.event.sources.find(source => source.source_id === $('source-select').value) || state.event.sources[0] || null;
      clear($('series-select'));
      const catalog = state.source ? seriesCatalog(state.source) : [];
      for (const series of catalog) option($('series-select'), series.series_id, seriesLabel(series));
      $('series-select').disabled = !catalog.length;
      state.series = catalog.find(series => series.series_id === selectedId) || catalog[0] || null;
      if (state.series) $('series-select').value = state.series.series_id;
      render();
    }
    function snapshotStatus() {
      if (!state.data || state.demo) {
        set('snapshot-badge', state.demo ? 'SYNTHETIC DATA' : 'Offline viewer');
        $('snapshot-badge').className = `status-badge${state.demo ? ' demo' : ''}`; return;
      }
      const sourceSample = state.source?.samples[state.source.samples.length - 1];
      const sourceAt = sourceSample?.observed_at;
      const freshLimit = Math.max(300, (state.source?.interval_secs || 0) * 3);
      const current = Math.floor(Date.now() / 1000);
      const clockInvalid = state.data.generated_at > current || (sourceAt !== undefined && sourceAt > current);
      const stale = current - state.data.generated_at > freshLimit || sourceAt === undefined || current - sourceAt > freshLimit;
      if (state.mode !== 'live') {
        set('snapshot-badge', `${clockInvalid ? 'Snapshot clock mismatch' : stale ? 'Stale snapshot' : 'Export snapshot'} · ${age(state.data.generated_at)}`);
        $('snapshot-badge').className = `status-badge${stale || clockInvalid ? ' warning' : ''}`; return;
      }
      const unavailable = !liveState.connected || liveState.paused || ['error', 'unauthorized'].includes(liveState.phase);
      const sourceWarning = sourceSample?.availability === 'blocked' || sourceSample?.availability === 'unknown' || sourceSample?.complete === false;
      const label = !liveState.connected ? 'Disconnected snapshot' : liveState.paused ? 'Paused snapshot'
        : liveState.phase === 'error' ? 'Refresh failed' : clockInvalid ? 'Connected · clock mismatch' : stale ? 'Connected · source stale'
          : sourceSample?.availability === 'blocked' ? 'Connected · source blocked'
            : sourceSample?.availability === 'unknown' ? 'Connected · stock unknown'
              : sourceSample?.complete === false ? 'Connected · partial snapshot' : 'Connected · source recent';
      set('snapshot-badge', `${label} · ${sourceAt === undefined ? 'no observations' : age(sourceAt)}`);
      $('snapshot-badge').className = `status-badge ${unavailable || stale || clockInvalid || sourceWarning ? 'warning' : 'live'}`;
      set('data-mode-badge', unavailable ? 'Worker snapshot · not updating' : 'Connected history · read only');
    }
    function liveControls() {
      const locked = liveState.connected;
      for (const id of ['live-endpoint', 'live-event-url', 'live-lookback', 'live-token', 'live-connect']) $(id).disabled = locked;
      $('live-refresh').disabled = !locked || liveState.paused || liveState.busy;
      $('live-pause').disabled = !locked;
      $('live-disconnect').disabled = !locked;
      set('live-pause', liveState.paused ? 'Resume' : 'Pause');
      $('live-status').dataset.phase = liveState.phase;
      if (liveState.message) set('live-status', liveState.message);
      snapshotStatus();
    }
    const live = createLiveConnection({
      onData: data => load(data, false, 'Worker history', true),
      onState: value => { liveState = value; liveControls(); },
      onIdle: () => { liveState.busy = false; liveControls(); },
    });
    function tag(label, style) { const node = document.createElement('span'); node.className = `tag ${style || ''}`; node.textContent = label; $('series-tags').appendChild(node); }
    function latestQuote() {
      const sample = state.source?.samples[state.source.samples.length - 1];
      const series = sample?.series.find(row => row.series_id === state.series?.series_id);
      return { sample, series };
    }
    function render() {
      const { event, source, series } = state;
      // SVGElement has no reflected HTMLElement.hidden property. Toggle the
      // actual attribute so the shared [hidden] CSS rule changes visibility.
      $('price-chart').toggleAttribute('hidden', true); $('price-chart').replaceChildren();
      $('empty-state').hidden = false; $('inspection').hidden = true;
      state.buckets = []; state.selected = 0;
      $('observation-range').max = '0'; $('observation-range').value = '0';
      set('observation-detail', 'Select a point to inspect it.');
      const hasSeries = Boolean(source && series);
      $('chart-mode').disabled = !hasSeries; $('count-mode').disabled = !hasSeries;
      for (const button of document.querySelectorAll('[data-window]')) button.disabled = !hasSeries;
      if (!event) {
        set('event-context', 'NO RECORDED EVENTS'); set('event-heading', 'No event observations yet.');
        set('event-location', 'No recorded observations in this export.'); $('event-link').hidden = true;
        for (const id of ['price-value', 'count-value', 'time-value', 'change-value']) set(id, '—');
        set('price-label', 'Latest observed lowest ask'); set('price-subtitle', 'No data loaded');
        set('count-label', 'Latest observed entries'); set('count-subtitle', 'Not ticket sales');
        set('time-subtitle', state.mode === 'live' ? 'Waiting for source observations' : 'No live connection'); $('change-value').className = '';
        for (const id of ['source-select', 'series-select']) { clear($(id)); $(id).disabled = true; }
        $('series-tags').replaceChildren(); snapshotStatus();
        set('chart-caption', 'Asking prices, not completed transactions.'); set('bucket-caption', 'No observations'); return;
      }
      set('event-context', state.demo ? 'SYNTHETIC DEMONSTRATION' : `${event.artist || 'ARTIST UNKNOWN'} / RECORDED HISTORY`);
      set('event-heading', event.name); set('event-location', [event.city, event.country].filter(Boolean).join(' · ') || 'Location unknown');
      const link = safeUrl(event.event_url); $('event-link').hidden = !link || state.demo; if (link) $('event-link').href = link;
      snapshotStatus();
      $('series-tags').replaceChildren();
      if (!source || !series) {
        for (const id of ['price-value', 'count-value', 'time-value', 'change-value']) set(id, '—');
        set('price-label', 'Latest observed lowest ask'); set('count-label', 'Latest observed entries');
        set('count-subtitle', 'No priced series recorded'); set('time-subtitle', 'No live connection'); $('change-value').className = '';
        set('price-subtitle', 'No priced series recorded for this source'); set('chart-caption', 'No priced series recorded. Unknown observations remain unknown.');
        set('bucket-caption', source ? `${source.samples.length} source observations` : 'No observations'); return;
      }
      tag(series.inventory_kind === 'unknown' ? 'Inventory channel unknown' : series.inventory_kind, series.inventory_kind);
      tag(basisLabel(series)); tag(FEES_LABELS[series.fees], series.fees === 'unknown' ? 'warning' : '');
      tag(KIND_LABELS[series.kind], series.kind !== 'offer' ? 'warning' : '');
      if (series.kind === 'displayed_seat') tag(seatLabel(series));
      tag(QUANTITY_LABELS[series.quantity_kind], ['selectable', 'unknown'].includes(series.quantity_kind) ? 'warning' : '');
      if (series.kind !== 'offer') tag('Displayed prices ≠ verified stock', 'warning');
      const latest = latestQuote();
      set('price-value', latest.sample && quoteVisible(latest.sample, latest.series) ? money(latest.series.min_price_minor, series.currency) : 'Not observed');
      set('price-subtitle', latest.sample ? `${latest.sample.availability} · ${latest.sample.complete ? 'complete' : 'partial'} snapshot` : 'No observations');
      set('price-label', series.kind === 'offer' ? 'Latest observed lowest ask' : series.seat ? 'Latest displayed seat price' : 'Latest displayed minimum price');
      const countMode = $('count-mode').value;
      set('count-label', countMode === 'reported' ? `${QUANTITY_LABELS[series.quantity_kind]} · latest` : series.seat ? 'Observed seat quote' : KIND_LABELS[series.kind]);
      const lastCount = latest.series && latest.sample.availability !== 'blocked' && (latest.sample.availability === 'available' || latest.series.kind !== 'offer') ? (countMode === 'reported' ? (latest.sample.availability === 'available' ? latest.series.reported_quantity : null) : latest.series.observed_count) : null;
      set('count-value', lastCount === null || lastCount === undefined ? 'Unknown' : lastCount.toLocaleString());
      set('count-subtitle', countMode === 'reported' ? 'Distinct entries may overlap; not event stock' : series.seat ? 'One recorded quote; availability unverified' : 'Last snapshot; polls are never summed');
      set('time-value', latest.sample ? date(latest.sample.observed_at, true) : '—');
      set('time-subtitle', latest.sample ? `${age(latest.sample.observed_at)} · ${source.interval_secs ? `${source.interval_secs}s configured cadence` : 'unscheduled source'}` : 'No observations');
      const end = source.samples.length ? source.samples[source.samples.length - 1].observed_at : state.data.generated_at;
      const start = state.window === 'all' ? (source.samples[0]?.observed_at ?? end) : end - Number(state.window);
      const width = bucketWidth(start, end);
      const gapSeconds = source.interval_secs ? Math.max(300, source.interval_secs * 3) : 0;
      state.buckets = buildBuckets(source.samples, series.series_id, width, gapSeconds, start, end);
      let limited = false;
      if (state.buckets.length > 2000) { state.buckets = state.buckets.slice(-2000); limited = true; }
      const values = state.buckets.filter(bucket => bucket.close !== null);
      const first = values[0]?.open; const last = values[values.length - 1]?.close;
      const change = first > 0 && last !== undefined ? (last - first) / first * 100 : null;
      set('change-value', change === null ? '—' : `${change > 0 ? '+' : ''}${change.toFixed(2)}%`);
      $('change-value').className = change === null ? '' : change >= 0 ? 'positive' : 'negative';
      set('chart-caption', `${state.demo ? 'SYNTHETIC · ' : ''}${series.kind === 'offer' ? 'Minimum observed asking prices' : 'Displayed minimum prices; availability unverified'} · ${countMode === 'reported' ? 'reported quantity' : KIND_LABELS[series.kind].toLowerCase()} bars.${limited ? ' Showing latest 2,000 chart segments.' : ''}`);
      set('bucket-caption', `${width < 60 ? `${width}s` : width < 3600 ? `${width / 60}m` : width < 86400 ? `${width / 3600}h` : `${width / 86400}d`} buckets · ${source.interval_secs ? `gaps > ${gapSeconds < 3600 ? `${gapSeconds / 60}m` : `${gapSeconds / 3600}h`}` : 'unscheduled; no continuity assumed'}`);
      if (!state.buckets.length) return;
      $('price-chart').toggleAttribute('hidden', false); $('empty-state').hidden = true; $('inspection').hidden = false;
      $('observation-range').max = String(state.buckets.length - 1); state.selected = state.buckets.length - 1;
      drawChart(limited ? state.buckets[0].first_at : start, end);
      inspect(state.selected);
    }
    function svg(tagName, attrs, content) { const node = document.createElementNS(SVG_NS, tagName); for (const [key, value] of Object.entries(attrs || {})) node.setAttribute(key, String(value)); if (content !== undefined) node.textContent = content; return node; }
    function drawChart(start, end) {
      const chart = $('price-chart'); chart.replaceChildren(svg('title', { id: 'chart-title' }, 'Recorded minimum ticket asking prices'), svg('desc', { id: 'chart-description' }, 'Historical asking-price observations, with gaps for missing data. Count bars use the last observation in each bucket. Use the observation slider below for keyboard-accessible details.'));
      const pixelWidth = Math.round(chart.getBoundingClientRect().width);
      const viewWidth = Math.max(360, Math.min(1100, pixelWidth || 360));
      const mobile = viewWidth < 600;
      const viewHeight = mobile ? 380 : 440;
      chart.setAttribute('viewBox', `0 0 ${viewWidth} ${viewHeight}`);
      chart.setAttribute('data-compact', String(mobile));
      state.chartPixelWidth = pixelWidth; state.chartStart = start; state.chartEnd = end;
      const left = mobile ? 82 : 77; const right = viewWidth - (mobile ? 12 : 25); const top = 28;
      const bottom = mobile ? 235 : 285; const barsTop = mobile ? 275 : 325; const barsBottom = mobile ? 330 : 395;
      const points = state.buckets.filter(bucket => bucket.close !== null);
      const rawMin = points.length ? Math.min(...points.map(bucket => bucket.low)) : 0;
      const rawMax = points.length ? Math.max(...points.map(bucket => bucket.high)) : 1;
      const pad = Math.max((rawMax - rawMin) * .18, rawMax * .015, 1); const min = Math.max(0, rawMin - pad); const max = rawMax + pad;
      const span = Math.max(1, end - start); const x = at => left + (at - start) / span * (right - left); const y = value => bottom - (value - min) / Math.max(1, max - min) * (bottom - top);
      if (points.length) {
        for (let i = 0; i <= 4; i += 1) { const value = min + (max - min) * i / 4; const py = y(value); chart.append(svg('line', { x1: left, y1: py, x2: right, y2: py, class: 'chart-grid' }), svg('text', { x: left - 11, y: py + 4, 'text-anchor': 'end', class: 'chart-axis' }, state.series.currency ? money(value, state.series.currency, true) : Math.round(value).toLocaleString())); }
      } else {
        chart.append(svg('text', { x: (left + right) / 2, y: mobile ? 135 : 160, 'text-anchor': 'middle', class: 'chart-label' }, mobile ? 'No usable price observations' : 'No usable price observations in this window'));
        if (mobile) chart.append(svg('text', { x: (left + right) / 2, y: 153, 'text-anchor': 'middle', class: 'chart-label' }, 'in this window'));
      }
      const xIntervals = mobile ? 2 : 4;
      for (let i = 0; i <= xIntervals; i += 1) {
        const at = start + span * i / xIntervals;
        const attrs = { x: x(at), y: mobile ? 352 : 422, 'text-anchor': i === 0 ? 'start' : i === xIntervals ? 'end' : 'middle', class: 'chart-axis' };
        if (mobile) {
          const instant = new Date(at * 1000);
          const label = svg('text', attrs, instant.toLocaleDateString(undefined, { month: 'short', day: 'numeric' }));
          label.append(svg('tspan', { x: x(at), dy: 15 }, instant.toLocaleTimeString(undefined, { hour: '2-digit', minute: '2-digit' })));
          chart.append(label);
        } else chart.append(svg('text', attrs, date(at, true)));
      }
      chart.append(svg('text', { x: left, y: mobile ? 260 : 310, class: 'chart-label' }, $('count-mode').value === 'reported'
        ? (mobile ? 'REPORTED QTY · NOT TOTAL STOCK' : 'REPORTED QUANTITY · NOT TOTAL STOCK')
        : `${KIND_LABELS[state.series.kind].toUpperCase()} · ${mobile ? 'LAST' : 'LAST SNAPSHOT'}`));
      const metric = $('count-mode').value === 'reported' ? 'reported_quantity' : 'observed_count';
      const maxCount = Math.max(1, ...state.buckets.map(bucket => bucket[metric] || 0));
      const width = Math.max(1, Math.min(14, (right - left) / Math.max(1, state.buckets.length) * .65));
      const lineSegments = new Map();
      state.buckets.forEach((bucket, index) => {
        const px = x(bucket.at); bucket.chart_x = px;
        if (bucket.gap || bucket.long_gap_before) chart.append(svg('line', { x1: px, x2: px, y1: top, y2: barsBottom, class: 'chart-gap' }));
        if (bucket[metric] !== null) { const height = bucket[metric] / maxCount * (barsBottom - barsTop); chart.append(svg('rect', { x: px - width / 2, y: barsBottom - height, width, height: Math.max(1, height), class: 'chart-bar' })); }
        if (bucket.close !== null) {
          if ($('chart-mode').value === 'candles') { const cls = bucket.close >= bucket.open ? 'chart-candle-up' : 'chart-candle-down'; chart.append(svg('line', { x1: px, x2: px, y1: y(bucket.low), y2: y(bucket.high), class: cls }), svg('rect', { x: px - width / 2, y: Math.min(y(bucket.open), y(bucket.close)), width, height: Math.max(2, Math.abs(y(bucket.open) - y(bucket.close))), class: cls })); }
          else { if (!lineSegments.has(bucket.segment)) lineSegments.set(bucket.segment, []); lineSegments.get(bucket.segment).push(`${px},${y(bucket.close)}`); chart.append(svg('circle', { cx: px, cy: y(bucket.close), r: 2.5, class: 'chart-dot' })); }
        }
        const hit = svg('rect', { x: px - Math.max(4, width), y: top, width: Math.max(8, width * 2), height: barsBottom - top, class: 'chart-hit', 'aria-label': `${date(bucket.at, false)} observation` });
        hit.addEventListener('pointerenter', () => inspect(index)); hit.addEventListener('click', () => inspect(index)); chart.append(hit);
      });
      for (const segment of lineSegments.values()) if (segment.length > 1) chart.append(svg('polyline', { points: segment.join(' '), class: 'chart-line', 'pointer-events': 'none' }));
      chart.append(svg('line', { id: 'chart-cursor', x1: left, x2: left, y1: top, y2: barsBottom, class: 'chart-cursor', 'pointer-events': 'none' }));
    }
    function inspect(index) {
      const bucket = state.buckets[index]; if (!bucket) return; state.selected = index; $('observation-range').value = String(index);
      const cursor = $('chart-cursor'); if (cursor) { cursor.setAttribute('x1', String(bucket.chart_x)); cursor.setAttribute('x2', String(bucket.chart_x)); }
      const quote = bucket.close === null ? 'No usable price observation' : `O ${money(bucket.open, state.series.currency)}   H ${money(bucket.high, state.series.currency)}   L ${money(bucket.low, state.series.currency)}   C ${money(bucket.close, state.series.currency)}`;
      const counts = `${KIND_LABELS[state.series.kind]}: ${bucket.observed_count === null ? 'unknown' : bucket.observed_count.toLocaleString()} · ${QUANTITY_LABELS[state.series.quantity_kind]}: ${bucket.reported_quantity === null ? 'unknown' : bucket.reported_quantity.toLocaleString()}`;
      set('observation-detail', `${date(bucket.at, false)} · ${bucket.sample.availability} · ${bucket.sample.complete ? 'complete' : 'partial'} · ${bucket.sample.evidence}\n${quote}\n${counts} · ${bucket.samples} observation${bucket.samples === 1 ? '' : 's'} in this segment`);
    }
    $('history-file').addEventListener('change', async event => {
      const file = event.target.files?.[0]; if (!file) return;
      live.disconnect(); const current = ++contentGeneration;
      try { if (file.size > MAX_BYTES) fail('History files must be 16 MiB or smaller.'); const checked = parseExport(await file.text()); if (current === contentGeneration) load(checked.data, false, file.name); }
      catch (error) { if (current === contentGeneration) { set('error-banner', error instanceof Error ? error.message : 'This history file could not be loaded.'); $('error-banner').hidden = false; } }
      event.target.value = '';
    });
    $('demo-button').addEventListener('click', () => { contentGeneration += 1; live.disconnect(); load(demoExport(Math.floor(Date.now() / 1000)), true, 'Built-in synthetic demonstration'); });
    $('live-form').addEventListener('submit', event => {
      event.preventDefault();
      try {
        const options = { endpoint: $('live-endpoint').value.trim(), token: $('live-token').value,
          eventUrl: $('live-event-url').value.trim(), hours: Number($('live-lookback').value) };
        $('live-token').value = ''; contentGeneration += 1;
        live.connect(options);
      } catch (error) { set('live-status', error.message); $('live-status').dataset.phase = 'error'; }
    });
    $('live-refresh').addEventListener('click', () => { void live.refresh(); });
    $('live-pause').addEventListener('click', () => liveState.paused ? live.resume() : live.pause());
    $('live-disconnect').addEventListener('click', () => live.disconnect());
    root.addEventListener('pagehide', () => live.disconnect());
    root.setInterval(snapshotStatus, 30000);
    $('event-select').addEventListener('change', chooseEvent); $('source-select').addEventListener('change', chooseSource);
    $('series-select').addEventListener('change', () => { state.series = seriesCatalog(state.source).find(series => series.series_id === $('series-select').value); render(); });
    $('chart-mode').addEventListener('change', render); $('count-mode').addEventListener('change', render);
    $('observation-range').addEventListener('input', event => inspect(Number(event.target.value)));
    for (const button of document.querySelectorAll('[data-window]')) button.addEventListener('click', () => { state.window = button.dataset.window; for (const other of document.querySelectorAll('[data-window]')) other.setAttribute('aria-pressed', String(other === button)); render(); });
    let resizeFrame = null;
    function resizeChart() {
      if (!state.series || !state.buckets.length || $('price-chart').hasAttribute('hidden')
        || Math.round($('price-chart').getBoundingClientRect().width) === state.chartPixelWidth || resizeFrame !== null) return;
      resizeFrame = requestAnimationFrame(() => {
        resizeFrame = null;
        if (!state.series || !state.buckets.length || $('price-chart').hasAttribute('hidden')) return;
        drawChart(state.chartStart, state.chartEnd); inspect(state.selected);
      });
    }
    // Observe width only: redraw-induced SVG height changes cannot create a loop.
    if (typeof ResizeObserver === 'function') new ResizeObserver(resizeChart).observe($('chart-area'));
    else root.addEventListener('resize', resizeChart);
    const embedded = $('ticket-history-data');
    $('live-panel').hidden = Boolean(embedded) || root.location.protocol === 'file:';
    if (embedded) { try { load(parseExport(embedded.textContent).data, false, 'Embedded history export'); } catch (error) { set('error-banner', error.message); $('error-banner').hidden = false; } }
  }
  if (document.readyState === 'loading') document.addEventListener('DOMContentLoaded', boot); else boot();
}(typeof globalThis !== 'undefined' ? globalThis : this));
