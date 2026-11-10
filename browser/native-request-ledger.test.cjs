'use strict';
const test = require('node:test');
const assert = require('node:assert/strict');
const { PassiveNativeRequestLedger, classifyNativeRequest, LIMITS } = require('./native-request-ledger.cjs');

const eventUrl = 'https://www.ticketmaster.cz/event/synthetic/1234567890';
const endpoint = 'https://checkout.prod.ticketmaster.cz/graphql';
const secret = 'synthetic-private-marker-do-not-retain';
// Public SDK query documents; test values and provider references remain synthetic.
const SOURCE_QUERIES = {
  "GCOFinaliseReserve": "mutation GCOFinaliseReserve ( $ sessionId : SessionId ! $ queryParams : QueryParams ) { finalizeReservation ( finalizeReservationRequest : { sessionId : $ sessionId queryParams : $ queryParams } ) { ... on SuccessfulReservationResponse { __typename ... GcoReserveSuccessfulResponseFragment } ... on PollingReservationResponse { __typename ... GcoReservePollingResponseFragment } } } fragment GcoReservePollingResponseFragment on PollingReservationResponse { pollingKey } fragment GcoReserveSuccessfulResponseFragment on SuccessfulReservationResponse { accessTokenValid checkoutUrl expiresAt sessionId }",
  "GCOUpsellReserve": "mutation GCOUpsellReserve ( $ eventId : UniversalEventId $ systemEventId : SystemEventId $ upsellSelections : [ UpsellSelectionInput ! ] $ queryParams : QueryParams $ sponsorshipSessionId : String $ referrerSource : String ) { reserveTickets ( reservationRequest : { referrerSource : $ referrerSource eventReservations : [ { eventId : $ eventId systemEventId : $ systemEventId upsellSelections : $ upsellSelections sponsorshipSessionId : $ sponsorshipSessionId } ] queryParams : $ queryParams } ) { ... on SuccessfulReservationResponse { __typename ... GcoReserveSuccessfulResponseFragment } ... on PollingReservationResponse { __typename ... GcoReservePollingResponseFragment } ... on SuccessfulProvisionalReservationResponse { __typename ... GcoSuccessfulProvisionalReservationResponseFragment } } } fragment GcoReserveSuccessfulResponseFragment on SuccessfulReservationResponse { accessTokenValid checkoutUrl expiresAt sessionId } fragment GcoReservePollingResponseFragment on PollingReservationResponse { pollingKey } fragment GcoSuccessfulProvisionalReservationResponseFragment on SuccessfulProvisionalReservationResponse { sessionId expiresAt tickets { id ids eventId description isExactMatch ticketUnitPrice { amount currencyCode } quantity seats { section row seat isGeneralAdmission seatInfos { type info } } } }",
  "GCOReservePolling": "query GCOReservePolling ( $ pollingKey : PollingKey ! $ queryParams : QueryParams ) { getReserveResult ( pollingKey : $ pollingKey queryParams : $ queryParams ) { ... on SuccessfulReservationResponse { __typename ... GcoReserveSuccessfulResponseFragment } ... on PollingReservationResponse { __typename ... GcoReservePollingResponseFragment } ... on SuccessfulProvisionalReservationResponse { __typename ... GcoSuccessfulProvisionalReservationResponseFragment } } } fragment GcoReserveSuccessfulResponseFragment on SuccessfulReservationResponse { accessTokenValid checkoutUrl expiresAt sessionId } fragment GcoReservePollingResponseFragment on PollingReservationResponse { pollingKey } fragment GcoSuccessfulProvisionalReservationResponseFragment on SuccessfulProvisionalReservationResponse { sessionId expiresAt tickets { id ids eventId description isExactMatch ticketUnitPrice { amount currencyCode } quantity seats { section row seat isGeneralAdmission seatInfos { type info } } } }",
  "GCOReserve": "mutation GCOReserve ( $ totalReservationAmount : MonetaryAmountInput $ eventId : UniversalEventId $ systemEventId : SystemEventId $ reservations : [ Reservation ] $ passwordToken : PasswordToken $ cartId : SystemCartId $ queryParams : QueryParams $ requiresConfirmation : Boolean $ membershipSessionId : MembershipSessionId $ clubSiteId : String $ referrerSource : String ) { reserveTickets ( reservationRequest : { referrerSource : $ referrerSource totalReservationAmount : $ totalReservationAmount eventReservations : [ { eventId : $ eventId systemEventId : $ systemEventId reservations : $ reservations passwordToken : $ passwordToken cartId : $ cartId membershipSessionId : $ membershipSessionId clubSiteId : $ clubSiteId } ] requiresConfirmation : $ requiresConfirmation queryParams : $ queryParams } ) { ... on SuccessfulReservationResponse { __typename ... GcoReserveSuccessfulResponseFragment } ... on PollingReservationResponse { __typename ... GcoReservePollingResponseFragment } ... on SuccessfulProvisionalReservationResponse { __typename ... GcoSuccessfulProvisionalReservationResponseFragment } } } fragment GcoReserveSuccessfulResponseFragment on SuccessfulReservationResponse { accessTokenValid checkoutUrl expiresAt sessionId } fragment GcoReservePollingResponseFragment on PollingReservationResponse { pollingKey } fragment GcoSuccessfulProvisionalReservationResponseFragment on SuccessfulProvisionalReservationResponse { sessionId expiresAt tickets { id ids eventId description isExactMatch ticketUnitPrice { amount currencyCode } quantity seats { section row seat isGeneralAdmission seatInfos { type info } } } }"
};
function reserveBody(overrides = {}) {
  return {
    operationName: 'GCOReserve',
    query: SOURCE_QUERIES[overrides.operationName || 'GCOReserve'] || SOURCE_QUERIES.GCOReserve,
    variables: {
      systemEventId: '1234567890', eventId: 'public-universal-event',
      reservations: [{ acceptSplit: false, areaIds: ['4P'], priceLevelIds: ['11'],
        offers: [{ offerType: 'TICKET', id: '23', quantity: 2, unlockToken: secret }] }],
      passwordToken: secret, cartId: secret, membershipSessionId: secret,
      queryParams: { privateToken: secret }, jwtToken: secret,
      pollingKey: secret, sessionId: secret, upsellSelections: [],
    },
    ...overrides,
  };
}
function request(body = reserveBody(), url = endpoint, method = 'POST') {
  return { url: () => url, method: () => method,
    postData: () => typeof body === 'string' ? body : JSON.stringify(body),
    headers: () => { throw new Error('headers must never be read'); },
    failure: () => { throw new Error('arbitrary failure text must never be read'); } };
}
function response(req, status = 200) { return { request: () => req, status: () => status }; }
function fixture(options = {}) {
  let time = 1000;
  const ledger = new PassiveNativeRequestLedger({
    eventUrl, sourceDocumentId: 'document_fixture',
    expectedSelection: { quantity: 2, categoryId: '23', priceId: '11', sectionIds: ['4P'], adjacentRequired: true },
    clock: () => ({ wall_ms: ++time, monotonic_ms: time - 900 }), ...options,
  });
  return { ledger, arm: () => ledger.arm({ attemptId: 'attempt_fixture', clickedAtMs: 1000, monotonicMs: 100 }) };
}
const bodyRecord = ledger => ledger.snapshot().records.find(record => record.phase === 'response_body');

test('pre-click traffic and unrelated request identities cannot acquire native response lineage', () => {
  const { ledger, arm } = fixture();
  const before = request();
  assert.equal(ledger.observeRequest(before), null);
  arm();
  assert.equal(ledger.observeRequest(before), null);
  assert.equal(ledger.observeResponse(response(before), '{}'), null);
  const first = request(), twin = request();
  const firstId = ledger.observeRequest(first);
  assert.match(firstId, /^request_[0-9a-f-]+$/);
  assert.equal(ledger.observeRequest(first), firstId);
  assert.equal(ledger.observeResponse(response(twin), '{}'), null);
  const twinId = ledger.observeRequest(twin);
  assert.notEqual(twinId, firstId);
  assert.equal(ledger.snapshot().request_count, 2);
  assert.throws(arm, /invalid_native_ledger_options/);
});

test('known pre-click request start times are ignored even when request events arrive after arming', () => {
  const { ledger, arm } = fixture(); arm();
  const late = request();
  late.timing = () => ({ startTime: 999.5 });
  assert.equal(ledger.observeRequest(late), null);
  assert.equal(ledger.observeRequest(late), null);
  assert.equal(ledger.observeResponse(response(late), '{}'), null);
  assert.equal(ledger.snapshot().pre_click_requests_ignored, 1);
  const current = request();
  current.timing = () => ({ startTime: 1001.25 });
  ledger.observeRequest(current);
  assert.equal(ledger.snapshot().records[0].request_start_time_ms, 1001.25);
  assert.equal(ledger.snapshot().records[0].click_causality_verified, false);
  for (const startTime of [-1, 0]) {
    const unavailable = request();
    unavailable.timing = () => ({ startTime });
    assert.match(ledger.observeRequest(unavailable), /^request_[0-9a-f-]+$/);
    const record = ledger.snapshot().records.at(-1);
    assert.equal(record.request_start_time_ms, null);
    assert.equal(record.click_causality_verified, false);
    assert.equal(ledger.observeResponse(response(unavailable), '{}'), ledger.requestId(unavailable));
  }
  assert.equal(ledger.snapshot().request_count, 3);
  assert.equal(ledger.snapshot().pre_click_requests_ignored, 1);
});

test('source-exact operation names and endpoint scopes remain explicit without emitting arbitrary names', () => {
  const operations = [
    ['GCOReserve', 'gco_reserve', true], ['GCOReservePolling', 'gco_reserve_polling', false],
    ['GCOFinaliseReserve', 'gco_finalise_reserve', true], ['GCOUpsellReserve', 'gco_upsell_reserve', true],
  ];
  for (const [operationName, expected, mutating] of operations) {
    const result = classifyNativeRequest(request(reserveBody({ operationName })), eventUrl);
    assert.equal(result.native_endpoint, true);
    assert.equal(result.operation, expected);
    assert.equal(result.mutating, mutating);
    assert.equal(result.query_contract_verified, true);
    assert.equal(result.body, undefined);
  }
  const unknown = classifyNativeRequest(request(reserveBody({ operationName: secret })), eventUrl);
  assert.equal(unknown.operation, 'unknown_graphql');
  assert.equal(unknown.mutating, null);
  assert.ok(!JSON.stringify(unknown).includes(secret));
  for (const url of [
    'https://checkout.prod.ticketmaster.pl/graphql', 'https://attacker.example/graphql',
    'https://checkout.prod.ticketmaster.cz.evil.example/graphql', 'http://checkout.prod.ticketmaster.cz/graphql',
    'https://checkout.prod.ticketmaster.cz/graphql-extra', 'https://user:password@checkout.prod.ticketmaster.cz/graphql',
    'https://www.ticketmaster.cz/event/9999999999/api/reserve',
  ]) assert.equal(classifyNativeRequest(request({}, url), eventUrl).native_endpoint, false);
  assert.equal(classifyNativeRequest(request({}, endpoint, 'GET'), eventUrl).parse_status, 'unsupported_method');
});

test('aliases, extra operations and changed query contracts cannot spoof a known operation name', () => {
  const original = SOURCE_QUERIES.GCOReserve;
  const inputs = [
    original.replace('reserveTickets (', 'alias : reserveTickets ('),
    original.replace('reserveTickets (', 'reserveTickets : unrelatedOperation ('),
    original + ' query OtherOperation { viewer { id } }',
    'query GCOReserve { reserveTickets { __typename } }',
    'mutation GCOReserve { reserveTickets { __typename } }',
    'mutation GCOReserve { reserveTickets(arg: "' + secret + '") { __typename } }',
  ];
  for (const query of inputs) {
    const { ledger, arm } = fixture(); arm();
    const req = request(reserveBody({ query })); ledger.observeRequest(req);
    const recorded = ledger.snapshot().records[0];
    assert.equal(recorded.operation, 'unknown_graphql');
    assert.equal(recorded.parse_status, 'unrecognized_query');
    assert.equal(recorded.query_contract_verified, false);
    assert.equal(recorded.selection, undefined);
    ledger.observeResponse(response(req), '{"data":{"reserveTickets":null},"errors":[{"extensions":{"code":"U402"}}]}');
    assert.equal(bodyRecord(ledger).response_kind, 'unrecognized_operation');
    assert.ok(!JSON.stringify(ledger.snapshot()).includes(secret));
  }
  const { ledger, arm } = fixture(); arm();
  const req = request(reserveBody({ variables: { reservations: secret } })); ledger.observeRequest(req);
  assert.equal(ledger.snapshot().records[0].parse_status, 'unsupported_variables_shape');
  ledger.observeResponse(response(req), '{"data":{"reserveTickets":null},"errors":[{"extensions":{"code":"U402"}}]}');
  assert.equal(bodyRecord(ledger).response_kind, 'unrecognized_request_shape');
});

test('request metadata keeps only matched public selection values and never authentication or query contents', () => {
  const { ledger, arm } = fixture(); arm();
  const raw = reserveBody();
  raw.query += ' # ' + secret;
  raw.variables.reservations[0].areaIds.push(secret);
  raw.variables.reservations[0].priceLevelIds.push(secret);
  raw.variables.reservations[0].seats = [{ name: secret, offerId: secret }];
  raw.variables.reservations[0].row = secret;
  raw.variables.reservations[0].section = secret;
  ledger.observeRequest(request(raw));
  const snapshot = ledger.snapshot(), record = snapshot.records[0];
  assert.equal(record.evidence_scope, 'diagnostic_only');
  assert.equal(record.source_document_id, 'document_fixture');
  assert.equal(record.selection.system_event_id, '1234567890');
  assert.equal(record.selection.total_quantity, 2);
  assert.equal(record.selection.quantity_matches, true);
  assert.deepEqual(record.selection.groups[0].areas.ids, ['4P']);
  assert.equal(record.selection.groups[0].areas.unmatched_count, 1);
  assert.deepEqual(record.selection.groups[0].price_levels.ids, ['11']);
  assert.equal(record.selection.groups[0].offers[0].category_id, '23');
  const serialized = JSON.stringify(snapshot);
  for (const forbidden of [secret, 'passwordToken', 'cartId', 'membershipSessionId', 'jwtToken', 'unlockToken',
    'queryParams', 'public-universal-event', 'mutation GCOReserve']) assert.ok(!serialized.includes(forbidden), forbidden);
});

test('request mismatches and split permission are diagnostics, never adjacency evidence', () => {
  const { ledger, arm } = fixture(); arm();
  const raw = reserveBody();
  raw.variables.systemEventId = secret;
  raw.variables.reservations[0].acceptSplit = true;
  raw.variables.reservations[0].offers[0].id = secret;
  raw.variables.reservations[0].offers[0].quantity = 1;
  ledger.observeRequest(request(raw));
  const selected = ledger.snapshot().records[0].selection;
  assert.equal(selected.system_event_matches, false);
  assert.equal(selected.system_event_id, undefined);
  assert.equal(selected.quantity_matches, false);
  assert.equal(selected.groups[0].offers[0].category_matches, false);
  assert.equal(selected.groups[0].offers[0].category_id, undefined);
  assert.equal(selected.groups[0].accept_split, true);
  assert.equal(selected.groups[0].adjacent_request_conflict, true);
  assert.equal(selected.groups[0].adjacency_verified, false);
  assert.ok(!JSON.stringify(selected).includes(secret));
  const optional = fixture({ expectedSelection: undefined }); optional.arm();
  optional.ledger.observeRequest(request(raw));
  assert.equal(optional.ledger.snapshot().records[0].selection.groups[0].adjacent_request_conflict, null);
});

test('HTTP 200 and a null GraphQL reservation error stay separate from successful cart evidence', () => {
  const { ledger, arm } = fixture(); arm();
  const req = request(), reply = response(req);
  const id = ledger.observeRequest(req);
  assert.equal(ledger.observeResponse(reply), id);
  assert.equal(ledger.observeResponse(reply), id);
  ledger.observeResponse(reply, JSON.stringify({ errors: [{ message: secret,
    extensions: { code: 'U402', classification: 'UNPROCESSABLE_CONTENT', arbitrary: secret } }],
  data: { reserveTickets: null } }));
  ledger.observeResponse(reply, '{"data":{"reserveTickets":{"checkoutUrl":"' + secret + '"}}}');
  ledger.requestFinished(req); ledger.requestFinished(req); ledger.requestFailed(req);
  const records = ledger.snapshot().records;
  assert.deepEqual(records.map(record => record.phase), ['request_started', 'response_received', 'response_body', 'request_finished']);
  assert.ok(records.every(record => record.request_id === id));
  assert.equal(records[1].transport_result, 'http_success');
  assert.equal(records[2].response_kind, 'graphql_error');
  assert.equal(records[2].result_null, true);
  assert.deepEqual(records[2].error_codes, ['U402']);
  assert.equal(records[2].hold_verified, false);
  assert.equal(records[2].absence_verified, false);
  assert.equal(records[3].native_result_terminal_verified, false);
  assert.ok(!JSON.stringify(records).includes(secret));
});

test('successful, provisional, polling and partial response shapes never leak provider references', () => {
  const types = [
    ['SuccessfulReservationResponse', 'graphql_success_shape'],
    ['SuccessfulProvisionalReservationResponse', 'graphql_provisional_shape'],
    ['PollingReservationResponse', 'graphql_polling_shape'],
  ];
  for (const [__typename, expected] of types) {
    const { ledger, arm } = fixture(); arm();
    const req = request(); ledger.observeRequest(req);
    ledger.observeResponse(response(req), Buffer.from(JSON.stringify({ data: { reserveTickets: {
      __typename, sessionId: secret, checkoutUrl: 'https://checkout.ticketmaster.cz/' + secret,
      pollingKey: secret, expiresAt: '2027-01-01T00:00:00Z',
      tickets: [{ id: secret, seats: [{ row: secret }], ticketUnitPrice: { amount: 1234 } }],
    } } })));
    const result = bodyRecord(ledger);
    assert.equal(result.response_kind, expected);
    assert.equal(result.has_session_reference, true);
    assert.equal(result.ticket_group_count, 1);
    assert.equal(result.hold_verified, false);
    assert.equal(result.absence_verified, false);
    assert.ok(!JSON.stringify(ledger.snapshot()).includes(secret));
    assert.ok(!JSON.stringify(ledger.snapshot()).includes('2027-01-01'));
  }
  const { ledger, arm } = fixture(); arm();
  const req = request(); ledger.observeRequest(req);
  ledger.observeResponse(response(req), JSON.stringify({ errors: [{ extensions: { code: secret } }],
    data: { reserveTickets: { __typename: 'SuccessfulReservationResponse', sessionId: secret } } }));
  assert.equal(bodyRecord(ledger).response_kind, 'graphql_partial_result');
  assert.deepEqual(bodyRecord(ledger).error_codes, ['unrecognized']);
  assert.equal(bodyRecord(ledger).has_session_reference, true);
});

test('polling and finalization responses use their source-specific result field', () => {
  for (const [operationName, resultField] of [['GCOReservePolling', 'getReserveResult'], ['GCOFinaliseReserve', 'finalizeReservation']]) {
    const { ledger, arm } = fixture(); arm();
    const req = request(reserveBody({ operationName })); ledger.observeRequest(req);
    ledger.observeResponse(response(req), JSON.stringify({ data: { [resultField]: { __typename: 'PollingReservationResponse', pollingKey: secret } } }));
    assert.equal(bodyRecord(ledger).response_kind, 'graphql_polling_shape');
    assert.equal(bodyRecord(ledger).result_present, true);
  }
});

test('legacy status and HTTP failures retain no checkout link or arbitrary failure text', () => {
  const { ledger, arm } = fixture(); arm();
  const req = request({ privateToken: secret }, 'https://www.ticketmaster.cz/event/1234567890/api/reserve?queueittoken=' + secret);
  ledger.observeRequest(req);
  ledger.observeResponse(response(req, 503), JSON.stringify({ statusCode: 'NO_AVAILABILITY_ERROR', links: [{ href: secret }], info: { code: secret } }));
  ledger.requestFailed(req);
  const records = ledger.snapshot().records;
  assert.equal(records[0].endpoint_kind, 'mfol_reserve');
  assert.equal(records[1].transport_result, 'http_error');
  assert.equal(bodyRecord(ledger).native_status, 'NO_AVAILABILITY_ERROR');
  assert.equal(bodyRecord(ledger).absence_verified, false);
  assert.equal(records.at(-1).phase, 'request_failed');
  assert.ok(!JSON.stringify(records).includes(secret));
});

test('malformed, ambiguous, oversized and deeply nested bodies yield bounded fixed diagnostics', () => {
  const deep = {}; let current = deep;
  for (let i = 0; i < 30; i++) { current.child = {}; current = current.child; }
  const inputs = [
    ['{"passwordToken":"' + secret, 'invalid_json'],
    ['{"operationName":"GCOReserve","operationName":"' + secret + '"}', 'ambiguous_json'],
    ['{"variables":{"x":1,"\\u0078":2}}', 'ambiguous_json'],
    [' '.repeat(LIMITS.bodyBytes + 1), 'body_too_large'],
    [JSON.stringify(deep), 'json_structure_limit'], ['[]', 'unexpected_json_shape'], ['', 'empty_body'],
  ];
  for (const [body, status] of inputs) {
    const { ledger, arm } = fixture(); arm();
    const req = request(body); ledger.observeRequest(req);
    assert.equal(ledger.snapshot().records[0].parse_status, status);
    assert.ok(!JSON.stringify(ledger.snapshot()).includes(secret));
    const good = request(); ledger.observeRequest(good);
    ledger.observeResponse(response(good), body);
    assert.equal(bodyRecord(ledger).body_status, status);
  }
});

test('request/record size caps and broken sinks cannot create unbounded retained data', () => {
  const { ledger, arm } = fixture({ onRecord: () => { throw new Error(secret); } }); arm();
  for (let i = 0; i < 100; i++) {
    const req = request();
    ledger.observeRequest(req); ledger.observeRequest(req);
    ledger.observeResponse(response(req), '{}'); ledger.requestFinished(req);
  }
  let snapshot = ledger.snapshot();
  assert.equal(snapshot.request_count, LIMITS.requests);
  assert.equal(snapshot.dropped_requests, 100 - LIMITS.requests);
  assert.equal(snapshot.sink_failed, true);
  assert.ok(snapshot.records.length <= LIMITS.records);
  assert.ok(Buffer.byteLength(JSON.stringify(snapshot.records)) <= LIMITS.recordBytes + LIMITS.records + 1);
  assert.ok(!JSON.stringify(snapshot).includes(secret));

  const large = fixture(); large.arm();
  const raw = reserveBody();
  raw.variables.reservations = Array.from({ length: 20 }, () => ({
    areaIds: ['4P'], priceLevelIds: ['11'], acceptSplit: false,
    offers: Array.from({ length: 20 }, () => ({ id: '23', quantity: 1, offerType: 'TICKET' })),
  }));
  for (let i = 0; i < LIMITS.requests; i++) large.ledger.observeRequest(request(raw));
  snapshot = large.ledger.snapshot();
  assert.equal(snapshot.record_limit_reached, true);
  assert.ok(Buffer.byteLength(JSON.stringify(snapshot.records)) <= LIMITS.recordBytes + LIMITS.records + 1);
});

test('diagnostic callbacks and snapshot consumers cannot mutate retained lineage', () => {
  const { ledger, arm } = fixture({ onRecord: record => { record.attempt_id = secret; } }); arm();
  const req = request(); ledger.observeRequest(req);
  const snapshot = ledger.snapshot();
  snapshot.records[0].attempt_id = secret;
  assert.equal(ledger.snapshot().records[0].attempt_id, 'attempt_fixture');
  assert.throws(() => new PassiveNativeRequestLedger({ eventUrl, sourceDocumentId: 'document_ok',
    expectedSelection: { quantity: 2, priceId: '11', sectionIds: [] } }), /invalid_native_ledger_options/);
  const invalid = fixture({ clock: () => ({ wall_ms: 0, monotonic_ms: 0 }) }); invalid.arm();
  invalid.ledger.observeRequest(request());
  assert.equal(invalid.ledger.snapshot().records.length, 0);
  assert.equal(invalid.ledger.snapshot().clock_invalid, true);
});
