'use strict';

// Passive diagnostics for ordinary native browser traffic. No request is sent,
// changed, replayed or treated as proof of a hold, release or absence of a hold.
const { randomUUID, createHash } = require('node:crypto');
const { eventIdentity } = require('./rendered.cjs');
const LIMITS = Object.freeze({ bodyBytes: 65536, requests: 16, records: 72, recordBytes: 131072 });
const APP_ID = /^[A-Za-z0-9_.:-]{1,128}$/;
const PUBLIC_ID = /^[A-Za-z0-9_-]{1,64}$/;
const OPERATIONS = Object.freeze({
  // Token fingerprints of the public Czech SDK v1-209-0-7d119975-b15709894
  // reviewed on 2026-11-10. Ignore whitespace/comments, not aliases, fields or
  // extra operations. A source change becomes unrecognized until reviewed.
  GCOReserve: { operation: 'gco_reserve', mutating: true, resultField: 'reserveTickets',
    queryHash: 'cc40221dccc3a4070e1b3f70d16cc3ff4f00613a23ef1fe80a43ec09528dd012' },
  GCOReservePolling: { operation: 'gco_reserve_polling', mutating: false, resultField: 'getReserveResult',
    queryHash: 'b14162aa5066d59db7a2ba7f295489562f776757a549634c979fa41e617884a7' },
  GCOFinaliseReserve: { operation: 'gco_finalise_reserve', mutating: true, resultField: 'finalizeReservation',
    queryHash: '7beae4164b7d054d82d4fe5baea4c07237bc023619149448027a2e0415ce73f7' },
  GCOUpsellReserve: { operation: 'gco_upsell_reserve', mutating: true, resultField: 'reserveTickets',
    queryHash: '8d2a7b29b9363d9fa0c26ec77fd94e570e0295ef403497f5931888bb238c01c0' },
});
const object = value => value !== null && typeof value === 'object' && !Array.isArray(value);
const integer = (value, min, max) => Number.isSafeInteger(value) && value >= min && value <= max;
const fail = () => { throw new Error('invalid_native_ledger_options'); };

function queryFingerprint(query) {
  if (typeof query !== 'string' || query.length > LIMITS.bodyBytes) return null;
  const tokens = [];
  const token = /(?:[\s,\uFEFF]+|#[^\r\n]*|(\.\.\.|[_A-Za-z][_0-9A-Za-z]*|[!$():=@\[\]{|}&]))/gy;
  while (token.lastIndex < query.length) {
    const match = token.exec(query);
    if (!match) return null;
    if (match[1]) tokens.push(match[1]);
    if (tokens.length > 2048) return null;
  }
  return createHash('sha256').update(tokens.join('\x1f')).digest('hex');
}

function parseBody(raw) {
  if (!Buffer.isBuffer(raw) && typeof raw !== 'string') return { status: 'body_unavailable' };
  if (raw.length > LIMITS.bodyBytes || Buffer.byteLength(raw) > LIMITS.bodyBytes) return { status: 'body_too_large' };
  const text = Buffer.isBuffer(raw) ? raw.toString('utf8') : raw;
  if (!text.length) return { status: 'empty_body' };
  let value;
  try { value = JSON.parse(text); } catch { return { status: 'invalid_json' }; }
  // JSON.parse discards duplicate keys. Reject those before interpreting any
  // operation/selection fields; the already bounded input limits this scan.
  const stack = [];
  let nodes = 0;
  for (let i = 0; i < text.length; i++) {
    const char = text[i];
    if (char === '"') {
      const start = i++;
      while (i < text.length) {
        if (text[i] === '\\') { i += 2; continue; }
        if (text[i] === '"') break;
        i++;
      }
      const top = stack.at(-1);
      if (top?.kind === 'object' && top.key) {
        const key = JSON.parse(text.slice(start, i + 1));
        if (top.keys.has(key)) return { status: 'ambiguous_json' };
        top.keys.add(key);
        top.key = false;
      }
    } else if (char === '{' || char === '[') {
      if (++nodes > 2048 || stack.length >= 24) return { status: 'json_structure_limit' };
      stack.push(char === '{' ? { kind: 'object', keys: new Set(), key: true } : { kind: 'array' });
    } else if (char === '}' || char === ']') stack.pop();
    else if (char === ',' && stack.at(-1)?.kind === 'object') stack.at(-1).key = true;
  }
  return object(value) ? { status: 'parsed', value } : { status: 'unexpected_json_shape' };
}

function endpoint(request, eventUrl) {
  try {
    const event = eventIdentity(eventUrl);
    if (!event || event.host !== 'ticketmaster.cz' || !/^[0-9]{1,20}$/.test(event.id)) return null;
    const raw = request.url();
    if (typeof raw !== 'string' || raw.length > 8192 || raw.includes('\\')) return null;
    const url = new URL(raw);
    if (url.protocol !== 'https:' || url.username || url.password || url.port || url.hash) return null;
    if (url.hostname === 'checkout.prod.ticketmaster.cz' && url.pathname === '/graphql') {
      return { kind: 'gco_graphql', method: request.method(), eventId: event.id };
    }
    if (['ticketmaster.cz', 'www.ticketmaster.cz'].includes(url.hostname)
        && (url.pathname === '/event/' + event.id + '/api/reserve'
          || url.pathname === '/event/' + event.id + '/api/reserve_ism')) {
      return { kind: 'mfol_reserve', method: request.method(), eventId: event.id };
    }
  } catch { /* Fixed classifications never retain exceptions or URLs. */ }
  return null;
}

function classify(request, eventUrl, includeBody) {
  const matched = endpoint(request, eventUrl);
  if (!matched) return { native_endpoint: false };
  const basic = { native_endpoint: true, kind: matched.kind,
    operation: matched.kind === 'mfol_reserve' ? 'mfol_reserve' : 'unknown_graphql',
    mutating: matched.kind === 'mfol_reserve' ? true : null };
  if (matched.method !== 'POST') return { ...basic, parse_status: 'unsupported_method' };
  let parsed;
  try { parsed = parseBody(request.postData()); } catch { parsed = { status: 'body_unavailable' }; }
  if (parsed.status !== 'parsed') return { ...basic, parse_status: parsed.status };
  if (matched.kind === 'mfol_reserve') return { ...basic, parse_status: 'parsed',
    ...(includeBody ? { body: parsed.value } : {}) };
  const operation = typeof parsed.value.operationName === 'string'
    && Object.hasOwn(OPERATIONS, parsed.value.operationName) ? OPERATIONS[parsed.value.operationName] : null;
  if (!operation) return { ...basic, parse_status: 'unrecognized_operation' };
  if (queryFingerprint(parsed.value.query) !== operation.queryHash) {
    return { ...basic, parse_status: 'unrecognized_query', query_contract_verified: false };
  }
  const variables = parsed.value.variables;
  const variablesSupported = object(variables) && (
    operation.operation === 'gco_reserve'
      ? Array.isArray(variables.reservations) && variables.reservations.length > 0 && variables.reservations.length <= 20
        && variables.reservations.every(group => object(group) && typeof group.acceptSplit === 'boolean'
          && Array.isArray(group.offers) && group.offers.length > 0 && group.offers.length <= 20
          && group.offers.every(offer => object(offer) && typeof offer.id === 'string'
            && PUBLIC_ID.test(offer.id) && integer(offer.quantity, 1, 20)))
      : operation.operation === 'gco_reserve_polling'
        ? typeof variables.pollingKey === 'string' && variables.pollingKey.length > 0 && variables.pollingKey.length <= 4096
        : operation.operation === 'gco_finalise_reserve'
          ? typeof variables.sessionId === 'string' && variables.sessionId.length > 0 && variables.sessionId.length <= 4096
          : Array.isArray(variables.upsellSelections) && variables.upsellSelections.length <= 20
  );
  return { ...basic, operation: operation.operation, mutating: operation.mutating,
    parse_status: variablesSupported ? 'parsed' : 'unsupported_variables_shape',
    query_contract_verified: true,
    ...(includeBody ? { body: parsed.value, resultField: operation.resultField } : {}) };
}

/** Source-query shape diagnostic only; query text is never executed or retained. */
function classifyNativeRequest(request, eventUrl) {
  return classify(request, eventUrl, false);
}

function knownIds(value, expected) {
  if (!Array.isArray(value) || value.length > 128) return { present: value !== undefined, valid: false };
  const ids = [];
  let unmatched = 0;
  for (const id of value) {
    if (typeof id !== 'string' || !PUBLIC_ID.test(id)) { unmatched++; continue; }
    if (!expected.includes(id)) { unmatched++; continue; }
    ids.push(id);
  }
  return { present: true, valid: true, ids, unmatched_count: unmatched };
}

function selectionSummary(classification, eventId, expected) {
  if (classification.kind !== 'gco_graphql' || classification.operation !== 'gco_reserve') return undefined;
  const variables = classification.body?.variables;
  if (!object(variables)) return { status: 'missing_variables' };
  const eventMatch = variables.systemEventId === undefined ? null : variables.systemEventId === eventId;
  const result = { status: 'observed_request_fields', system_event_matches: eventMatch,
    ...(eventMatch ? { system_event_id: eventId } : {}),
    universal_event_id_present: typeof variables.eventId === 'string' };
  if (!Array.isArray(variables.reservations) || variables.reservations.length < 1
      || variables.reservations.length > 20) return { ...result, status: 'unsupported_reservations_shape' };
  let total = 0, validQuantity = true;
  result.groups = variables.reservations.map(group => {
    if (!object(group)) { validQuantity = false; return { status: 'unsupported_group_shape' }; }
    const split = typeof group.acceptSplit === 'boolean' ? group.acceptSplit : null;
    const summary = { accept_split: split, adjacency_verified: false,
      adjacent_request_conflict: typeof expected?.adjacentRequired === 'boolean' && split !== null
        ? expected.adjacentRequired && split : null,
      price_levels: knownIds(group.priceLevelIds, expected ? [expected.priceId] : []),
      areas: knownIds(group.areaIds, expected?.sectionIds || []),
      physical_section_present: typeof group.section === 'string',
      row_present: typeof group.row === 'string',
      explicit_seats_present: Array.isArray(group.seats) && group.seats.length > 0 };
    if (!Array.isArray(group.offers) || group.offers.length < 1 || group.offers.length > 20) {
      validQuantity = false;
      return { ...summary, status: 'unsupported_offers_shape' };
    }
    summary.offers = group.offers.map(offer => {
      if (!object(offer)) { validQuantity = false; return { status: 'unsupported_offer_shape' }; }
      const quantity = integer(offer.quantity, 1, 20) ? offer.quantity : null;
      if (quantity === null) validQuantity = false; else total += quantity;
      const matches = expected ? offer.id === expected.categoryId : null;
      return { quantity, category_matches: matches,
        ...(matches ? { category_id: expected.categoryId } : {}),
        offer_type: offer.offerType === 'TICKET' ? 'ticket' : 'other' };
    });
    return summary;
  });
  result.total_quantity = validQuantity && integer(total, 1, 20) ? total : null;
  result.quantity_matches = expected && result.total_quantity !== null ? result.total_quantity === expected.quantity : null;
  return result;
}

function responseSummary(classification, body) {
  const parsed = parseBody(body);
  if (parsed.status !== 'parsed') return { body_status: parsed.status };
  const value = parsed.value;
  if (classification.kind === 'mfol_reserve') {
    const known = ['OK', 'NO_AVAILABILITY_ERROR', 'BIGGEST_SEAT_BLOCK_ERROR',
      'NON_ADJACENT_SEATS_REQUIRED_ERROR', 'UNKNOWN_ERROR', 'PASSWORD_LIMIT_REACHED_ERROR'];
    return { body_status: 'parsed', response_kind: 'mfol_response',
      native_status: known.includes(value.statusCode) ? value.statusCode : 'unrecognized',
      handoff_present: Array.isArray(value.links) && value.links.some(link => object(link) && typeof link.href === 'string'),
      hold_verified: false, absence_verified: false };
  }
  if (!classification.resultField) return { body_status: 'parsed', response_kind: 'unrecognized_operation' };
  const errors = Array.isArray(value.errors) ? value.errors : [];
  const resultPresent = object(value.data) && Object.hasOwn(value.data, classification.resultField);
  const result = resultPresent ? value.data[classification.resultField] : undefined;
  const codes = errors.slice(0, 16).map(error => {
    const code = object(error?.extensions) ? error.extensions.code : undefined;
    return ['U001', 'U003', 'U401', 'U402'].includes(code) ? code : 'unrecognized';
  });
  const typename = object(result) ? result.__typename : undefined;
  const type = ['SuccessfulReservationResponse', 'PollingReservationResponse',
    'SuccessfulProvisionalReservationResponse'].includes(typename) ? typename : 'unrecognized';
  const kind = classification.parse_status !== 'parsed' ? 'unrecognized_request_shape' : errors.length
    ? (object(result) ? 'graphql_partial_result' : 'graphql_error')
    : type === 'PollingReservationResponse' ? 'graphql_polling_shape'
      : type === 'SuccessfulProvisionalReservationResponse' ? 'graphql_provisional_shape'
        : type === 'SuccessfulReservationResponse' ? 'graphql_success_shape' : 'unrecognized_result';
  return { body_status: 'parsed', response_kind: kind, error_count: errors.length,
    error_codes: codes, error_codes_truncated: errors.length > codes.length,
    result_present: resultPresent, result_null: resultPresent && result === null,
    response_type: type, has_checkout_handoff: object(result) && typeof result.checkoutUrl === 'string',
    has_session_reference: object(result) && typeof result.sessionId === 'string',
    has_polling_reference: object(result) && typeof result.pollingKey === 'string',
    has_expiry: object(result) && typeof result.expiresAt === 'string',
    ticket_group_count: object(result) && Array.isArray(result.tickets) ? result.tickets.length : null,
    hold_verified: false, absence_verified: false };
}

class PassiveNativeRequestLedger {
  constructor({ eventUrl, sourceDocumentId, expectedSelection, onRecord = () => {},
    clock = () => ({ wall_ms: Date.now(), monotonic_ms: performance.now() }) }) {
    const event = eventIdentity(eventUrl);
    if (!event || event.host !== 'ticketmaster.cz' || !/^[0-9]{1,20}$/.test(event.id)
        || typeof sourceDocumentId !== 'string' || !APP_ID.test(sourceDocumentId)
        || typeof onRecord !== 'function' || typeof clock !== 'function') fail();
    if (expectedSelection && (!integer(expectedSelection.quantity, 1, 20)
        || typeof expectedSelection.categoryId !== 'string' || !PUBLIC_ID.test(expectedSelection.categoryId)
        || typeof expectedSelection.priceId !== 'string' || !PUBLIC_ID.test(expectedSelection.priceId)
        || !Array.isArray(expectedSelection.sectionIds) || expectedSelection.sectionIds.length > 128
        || expectedSelection.sectionIds.some(id => typeof id !== 'string' || !PUBLIC_ID.test(id))
        || expectedSelection.adjacentRequired !== undefined && typeof expectedSelection.adjacentRequired !== 'boolean')) fail();
    this.eventUrl = eventUrl;
    this.eventId = event.id;
    this.sourceDocumentId = sourceDocumentId;
    this.expected = expectedSelection ? { ...expectedSelection, sectionIds: [...expectedSelection.sectionIds] } : undefined;
    this.onRecord = onRecord;
    this.clock = clock;
    this.requests = new WeakMap();
    this.ignoredRequests = new WeakSet();
    this.records = [];
    this.requestCount = 0;
    this.recordBytes = 0;
    this.droppedRequests = 0;
    this.preClickRequestsIgnored = 0;
    this.recordLimitReached = false;
    this.sinkFailed = false;
    this.clockInvalid = false;
    this.armInfo = null;
  }
  arm({ attemptId, clickedAtMs, monotonicMs }) {
    if (this.armInfo || typeof attemptId !== 'string' || !APP_ID.test(attemptId)
        || !integer(clickedAtMs, 0, Number.MAX_SAFE_INTEGER)
        || typeof monotonicMs !== 'number' || !Number.isFinite(monotonicMs) || monotonicMs < 0) fail();
    this.armInfo = { attemptId, clickedAtMs, monotonicMs };
  }
  emit(state, phase, details) {
    let time;
    try { time = this.clock(); } catch { time = null; }
    if (!time || !integer(time.wall_ms, this.armInfo.clickedAtMs, Number.MAX_SAFE_INTEGER)
        || typeof time.monotonic_ms !== 'number' || !Number.isFinite(time.monotonic_ms)
        || time.monotonic_ms < this.armInfo.monotonicMs) { this.clockInvalid = true; return; }
    const record = { schema_version: 1, evidence_scope: 'diagnostic_only', sequence: this.records.length + 1,
      attempt_id: this.armInfo.attemptId, source_document_id: this.sourceDocumentId, request_id: state.id,
      phase, observed_at_ms: time.wall_ms, monotonic_ms: time.monotonic_ms, ...details };
    const serialized = JSON.stringify(record);
    const size = Buffer.byteLength(serialized);
    if (this.records.length >= LIMITS.records || this.recordBytes + size > LIMITS.recordBytes) {
      this.recordLimitReached = true; return;
    }
    this.recordBytes += size;
    this.records.push(record);
    try {
      const pending = this.onRecord(JSON.parse(serialized));
      if (pending && typeof pending.then === 'function') pending.catch(() => { this.sinkFailed = true; });
    } catch { this.sinkFailed = true; }
  }
  observeRequest(request) {
    if (!request || (typeof request !== 'object' && typeof request !== 'function')) return null;
    if (!this.armInfo) { this.ignoredRequests.add(request); return null; }
    if (this.ignoredRequests.has(request)) return null;
    const previous = this.requests.get(request);
    if (previous) return previous.id;
    const classification = classify(request, this.eventUrl, true);
    if (!classification.native_endpoint) return null;
    let startedAt = null;
    try {
      const value = request.timing?.().startTime;
      // Playwright initializes startTime to 0 until response timing is available.
      if (typeof value === 'number' && Number.isFinite(value) && value > 0 && value <= Number.MAX_SAFE_INTEGER) startedAt = value;
    } catch { /* Delivery time alone cannot establish the actual request start. */ }
    if (startedAt !== null && startedAt < this.armInfo.clickedAtMs) {
      this.ignoredRequests.add(request);
      this.preClickRequestsIgnored++;
      return null;
    }
    if (this.requestCount >= LIMITS.requests) {
      this.ignoredRequests.add(request);
      this.droppedRequests = Math.min(Number.MAX_SAFE_INTEGER, this.droppedRequests + 1);
      return null;
    }
    const state = { id: 'request_' + randomUUID(), classification: { ...classification, body: undefined },
      response: null, responseSeen: false, bodySeen: false, finished: false };
    this.requests.set(request, state);
    this.requestCount++;
    const selection = selectionSummary(classification, this.eventId, this.expected);
    this.emit(state, 'request_started', { endpoint_kind: classification.kind, operation: classification.operation,
      mutating: classification.mutating, parse_status: classification.parse_status,
      request_start_time_ms: startedAt, click_causality_verified: false,
      query_contract_verified: classification.query_contract_verified === true,
      ...(selection ? { selection } : {}) });
    return state.id;
  }
  requestId(request) { return request && this.requests.get(request)?.id || null; }
  observeResponse(response, body) {
    let state, status;
    try { state = this.requests.get(response.request()); status = response.status(); } catch { return null; }
    if (!state || !integer(status, 100, 599) || state.response && state.response !== response) return null;
    state.response = response;
    if (!state.responseSeen) {
      state.responseSeen = true;
      this.emit(state, 'response_received', { http_status: status,
        transport_result: status >= 200 && status < 300 ? 'http_success' : 'http_error',
        native_result_verified: false });
    }
    if (body !== undefined && !state.bodySeen) {
      state.bodySeen = true;
      this.emit(state, 'response_body', responseSummary(state.classification, body));
    }
    return state.id;
  }
  finish(request, failed) {
    const state = request && this.requests.get(request);
    if (!state || state.finished) return null;
    state.finished = true;
    this.emit(state, failed ? 'request_failed' : 'request_finished', {
      response_seen: state.responseSeen, response_body_seen: state.bodySeen, native_result_terminal_verified: false });
    return state.id;
  }
  requestFinished(request) { return this.finish(request, false); }
  requestFailed(request) { return this.finish(request, true); }
  snapshot() {
    return JSON.parse(JSON.stringify({ schema_version: 1, evidence_scope: 'diagnostic_only',
      armed: this.armInfo !== null, request_count: this.requestCount, dropped_requests: this.droppedRequests,
      pre_click_requests_ignored: this.preClickRequestsIgnored,
      record_limit_reached: this.recordLimitReached, sink_failed: this.sinkFailed, clock_invalid: this.clockInvalid,
      records: this.records }));
  }
}

module.exports = { PassiveNativeRequestLedger, classifyNativeRequest, LIMITS };
