'use strict';

// A separate, stateful normal-browser adapter. Offline cart demonstrations are
// never brought online. Only commit may press Find tickets, at most once; the
// controller must durably journal that dispatch before sending its token.
const crypto = require('node:crypto');
const fs = require('node:fs/promises');
const path = require('node:path');
const readline = require('node:readline');
const { eventIdentity, sameEvent } = require('./rendered.cjs');
const { readSurface } = require('./native-cart-surface.cjs');
const { mapPriceOptions, mapSectionOptions } = require('./native-filter-mapping.cjs');
const { prepareTicketmaster } = require('./ticketmaster.cjs');
const { inspectChallenge, responseIntervention } = require('./manual-intervention.cjs');
const { observeDocuments } = require('./document-lifecycle.cjs');
const { identificationRetryAllowed, proxyOptions } = require('./worker.cjs');
const { PassiveNativeRequestLedger, classifyNativeRequest } = require('./native-request-ledger.cjs');

const id = value => typeof value === 'string' && /^[A-Za-z0-9_-]{1,64}$/.test(value);
const applicationId = value => typeof value === 'string' && /^[A-Za-z0-9_.:-]{1,128}$/.test(value);
const failure = code => Object.assign(new Error(code), { code });
const uid = prefix => `${prefix}_${crypto.randomUUID()}`;
const hash = value => crypto.createHash('sha256').update(JSON.stringify(value)).digest('hex');
async function bounded(operation, milliseconds) {
  let timer;
  try { return await Promise.race([operation, new Promise((_, reject) => {
    timer = setTimeout(() => reject(failure('capture_timeout')), milliseconds);
  })]); } finally { clearTimeout(timer); }
}
const finiteInteger = (value, min, max) => Number.isSafeInteger(value) && value >= min && value <= max;
function validateRequest(request) {
  const keys = ['event_url', 'category_id', 'price_id', 'section_ids', 'quantity', 'expected_event_start', 'configured_unit_price_minor'];
  if (!request || typeof request !== 'object' || Array.isArray(request)
      || Object.keys(request).some(key => !keys.includes(key)) || !id(request.category_id) || !id(request.price_id)
      || !Array.isArray(request.section_ids) || request.section_ids.length > 128
      || request.section_ids.some(value => !id(value)) || new Set(request.section_ids).size !== request.section_ids.length
      || !finiteInteger(request.quantity, 1, 20) || !finiteInteger(request.configured_unit_price_minor, 0, 10_000_000_000)
      || typeof request.expected_event_start !== 'string'
      || !/^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d{1,3})?Z$/.test(request.expected_event_start)
      || !Number.isFinite(Date.parse(request.expected_event_start))) throw failure('invalid_request');
  let url;
  try { url = new URL(request.event_url); } catch { throw failure('invalid_request'); }
  const identity = eventIdentity(request.event_url);
  if (!identity || identity.host !== 'ticketmaster.cz' || !/^[0-9]{1,20}$/.test(identity.id)
      || url.protocol !== 'https:' || url.username || url.password || url.port || url.hash
      || [...url.searchParams.keys()].some(key => !['language', 'locale'].includes(key))) throw failure('invalid_request');
  return Object.freeze({ ...request, section_ids: Object.freeze([...request.section_ids]) });
}

// This helper exposes only reviewed public origin/path allowlists. It never
// accepts a caller-provided checkout URL or a wildcard Ticketmaster domain.
function eventNavigationAllowed(target, candidate) {
  try {
    const url = new URL(candidate);
    const original = new URL(target);
    return url.protocol === 'https:' && !url.username && !url.password && !url.port
      && url.hostname === original.hostname && sameEvent(target, candidate)
      && [...url.searchParams.keys()].every(key => ['language', 'locale'].includes(key));
  } catch { return false; }
}

// Installed before site scripts, scoped only to this browser's Find control.
// It never invokes the control: one post-journal permit can admit one click,
// whether that first click comes from the operator or Playwright.
function installNativeFindGate({ binding, counter, control, secret }) {
  let count = 0, allowed = 0, blocked = 0, armed = false, sealed = false, invalidated = false, snapshot = null;
  const selector = 'button[data-testid="findTicketsBtn"]';
  const signature = () => JSON.stringify([
    location.href, document.querySelector('link[rel="canonical"]')?.href,
    document.querySelector('#__NEXT_DATA__')?.textContent,
    [...document.querySelectorAll('[data-testid="quantityStepper"]')].map(node => node.outerHTML),
    snapshot?.filters.map(node => node.outerHTML),
  ]);
  const notify = kind => { void window[binding](kind).catch(() => {}); };
  const invalidate = () => { if (sealed || armed) { armed = false; invalidated = true; } };
  Object.defineProperty(globalThis, counter, { get: () => count, configurable: false });
  Object.defineProperty(globalThis, control, { configurable: false, value: (key, action, args) => {
    if (key !== secret) return null;
    if (action === 'snapshot') return { attempted: count, admitted: allowed, blocked, invalidated };
    if (action === 'arm') {
      if (!sealed || count || armed || allowed || invalidated || !args?.button?.isConnected
          || args.root !== snapshot.root || args.root !== document.documentElement
          || !args.button.matches(selector) || args.button.disabled || document.querySelectorAll(selector).length !== 1
          || snapshot.signature !== signature() || !snapshot.filters.every(node => node.isConnected)) return false;
      snapshot.button = args.button;
      armed = true;
      return true;
    }
    if (action !== 'seal' || sealed || count || armed || allowed || invalidated
        || args?.root !== document.documentElement) return false;
    const labels = Object.values(args.filterLabels || {});
    const filters = labels.map(label => [...document.querySelectorAll('button')]
      .filter(node => node.textContent.replace(/\s+/g, ' ').trim() === label));
    if (labels.length !== 2 || filters.some(nodes => nodes.length !== 1)) return false;
    snapshot = { root: args.root, filters: filters.map(nodes => nodes[0]) };
    snapshot.signature = signature();
    sealed = true;
    return true;
  } });
  const ticketControl = event => event.composedPath().some(node => node instanceof Element
    && (node.matches('[data-testid="quantityStepper"],[data-testid="panelPriceSection"]')
      || snapshot?.filters.includes(node)));
  for (const type of ['input', 'change']) document.addEventListener(type, event => {
    if (ticketControl(event)) invalidate();
  }, true);
  document.addEventListener('click', event => {
    const button = event.composedPath().find(node => node instanceof Element && node.matches(selector));
    if (!button) { if (ticketControl(event)) invalidate(); return; }
    count++;
    const admit = armed && snapshot.root === document.documentElement && button === snapshot.button
      && snapshot.filters.every(node => node.isConnected) && snapshot.signature === signature();
    armed = false;
    if (admit) { allowed++; return; }
    blocked++;
    event.preventDefault();
    event.stopImmediatePropagation();
    notify('find_control_blocked');
  }, { capture: true, passive: false });
}

// Diagnostics use a clone only. Human credentials and form values must never
// reach disk; identity documents are omitted even when their form is empty.
function sanitizedDocumentSnapshot() {
  if (location.hostname === 'identity.ticketmaster.cz'
      || /(?:^|\/)(?:sign-in|signin|login|authenticate)(?:\/|$)/i.test(location.pathname)
      || document.querySelector('input[type="password"],input[autocomplete="username"],input[autocomplete="current-password"],input[autocomplete="one-time-code"]')) return null;
  const clone = document.documentElement.cloneNode(true);
  clone.querySelectorAll('script,style,link,meta,template,noscript,iframe,object,embed').forEach(node => node.remove());
  clone.querySelectorAll('input,textarea,select,[contenteditable]').forEach(node => {
    node.replaceChildren();
    for (const name of node.getAttributeNames()) node.removeAttribute(name);
    node.setAttribute('data-redacted', 'form-control');
  });
  for (const node of [clone, ...clone.querySelectorAll('*')]) {
    for (const name of node.getAttributeNames()) {
      if (!['data-testid', 'data-redacted', 'role', 'hidden', 'aria-live'].includes(name)) node.removeAttribute(name);
    }
  }
  return { url: location.origin + location.pathname, html: '<!doctype html>' + clone.outerHTML, form_values_redacted: true };
}

class NativeFormDriver {
  constructor(page, target, request, guard, nativeFindGate) {
    Object.assign(this, { page, target, request, externalGuard: guard, nativeFindGate });
    this.args = { categoryId: request.category_id, quantity: request.quantity, eventId: eventIdentity(target).id };
    this.signature = null;
    this.filterLabels = undefined;
    this.deadline = 0;
  }
  remaining() {
    const value = Math.floor(this.deadline - performance.now());
    if (value < 1) throw failure('native_form_timeout');
    return Math.min(value, 1500);
  }
  read(phase, kind, sealFind = false) { return this.page.evaluate(readSurface, { ...this.args, phase, kind,
    filterLabels: this.filterLabels, ...(sealFind ? { nativeFindGate: this.nativeFindGate } : {}) }); }
  bindingMatches(binding) {
    return binding && sameEvent(this.target, binding.canonical) && sameEvent(this.target, binding.embedded_canonical)
      && (!this.signature || JSON.stringify(binding) === this.signature);
  }
  async guard(panelAllowed = false, afterAction = false) {
    this.remaining();
    await this.externalGuard({ afterCartControlsAction: afterAction });
    if (this.root && !await this.root.evaluate(node => node === document.documentElement && node.isConnected))
      throw failure('native_form_changed');
    const challenge = await inspectChallenge(this.page);
    if (challenge) throw Object.assign(failure('needs_human'), { kind: challenge });
    const modal = await this.page.evaluate(allowed => {
      const shown = node => {
        for (let current = node; current; current = current.parentElement) {
          const style = getComputedStyle(current);
          if (current.hidden || current.hasAttribute('inert') || current.getAttribute('aria-hidden') === 'true'
              || style.display === 'none' || ['hidden', 'collapse'].includes(style.visibility) || Number(style.opacity) === 0) return false;
        }
        return node.getClientRects().length > 0;
      };
      return [...document.querySelectorAll('dialog[open],[role="dialog"]')].some(node => shown(node)
        && !(allowed && node.querySelector('[data-testid="panelPriceSection"]')));
    }, panelAllowed);
    if (modal) throw failure('native_modal_blocked');
    if (!this.bindingMatches(await this.read('binding'))) throw failure('native_form_changed');
  }
  async form(quantity = this.request.quantity, sealFind = false) {
    const result = await this.read('form', undefined, sealFind);
    if (!result || !this.bindingMatches(result.binding) || result.quantity !== quantity) throw failure('native_form_unrecognized');
    return result;
  }
  async waitForm(expectedQuantity = this.request.quantity) {
    while (true) {
      await this.guard(true);
      const result = await this.read('form');
      if (result && this.bindingMatches(result.binding) && result.quantity === expectedQuantity) return result;
      await this.page.waitForTimeout(Math.min(25, this.remaining()));
    }
  }
  async panel(kind) {
    const panel = await this.read('panel', kind);
    if (!panel || !this.bindingMatches(panel.binding)) throw failure('native_form_changed');
    return { panel, mapped: kind === 'price' ? mapPriceOptions(panel.binding, panel.options) : mapSectionOptions(panel.binding, panel.options) };
  }
  async open(kind) {
    await this.guard();
    const filter = (await this.form()).filters.find(item => item.kind === kind);
    if (filter.disabled) throw failure('native_form_unrecognized');
    await this.page.getByRole('button', { name: filter.label, exact: true }).click({ timeout: this.remaining() });
    await this.guard(true, true);
    while (true) {
      const panel = await this.read('panel', kind);
      if (panel && this.bindingMatches(panel.binding)) return this.panel(kind);
      await this.page.waitForTimeout(Math.min(25, this.remaining()));
      await this.guard(true);
    }
  }
  async closePanel() {
    let control;
    for (const [testId, name] of [['panelClose', 'Hotovo'], ['panelBack', 'Zpět']]) {
      const candidate = this.page.locator(`button[data-testid="${testId}"]`).filter({ visible: true });
      if (await candidate.count() === 1 && await candidate.isEnabled()
          && (await candidate.innerText()).replace(/\s+/g, ' ').trim() === name
          && await candidate.getAttribute('type') === 'button') { control = candidate; break; }
    }
    if (!control) throw failure('native_form_unrecognized');
    await this.guard(true);
    await control.click({ timeout: this.remaining() });
    await this.guard(true, true);
    await this.waitForm();
  }
  assertChecks(mapped, kind, desired) {
    const actual = mapped.filter(option => option.checked).map(option => kind === 'price' ? option.price_id : option.section_id).sort();
    if (JSON.stringify(actual) !== JSON.stringify([...desired].sort()) || mapped.some(option => option.checked && option.disabled))
      throw failure('native_form_changed');
  }
  async setChecks(kind, desired) {
    const key = kind === 'price' ? 'price_id' : 'section_id';
    let { mapped } = await this.panel(kind);
    if (desired.some(value => !mapped.some(option => option[key] === value && !option.disabled))) throw failure('native_form_unrecognized');
    let changed = false;
    for (const value of mapped.map(option => option[key])) {
      await this.guard(true);
      ({ mapped } = await this.panel(kind));
      const option = mapped.find(item => item[key] === value), wanted = desired.includes(value);
      if (!option || option.disabled && option.checked) throw failure('native_form_unrecognized');
      if (option.checked === wanted) continue;
      if (option.disabled) throw failure('native_form_unrecognized');
      const label = this.page.locator('[data-testid="panelPriceSection"]').locator(`label[for="${option.checkbox_id}"]`);
      if (await label.count() !== 1) throw failure('native_form_changed');
      await label.click({ timeout: this.remaining() });
      changed = true;
      await this.guard(true, true);
      while (!(await this.panel(kind)).mapped.some(item => item[key] === value && item.checked === wanted)) {
        await this.page.waitForTimeout(Math.min(25, this.remaining()));
        await this.guard(true);
      }
    }
    this.assertChecks((await this.panel(kind)).mapped, kind, desired);
    return changed;
  }
  async apply(labels) {
    await this.guard(true);
    const control = this.page.locator('[data-testid="panelPriceSection"]').getByRole('button', { name: 'Použít filtry', exact: true });
    if (await control.count() !== 1 || !await control.isEnabled() || await control.getAttribute('type') !== 'button')
      throw failure('native_form_unrecognized');
    await control.click({ timeout: this.remaining() });
    this.filterLabels = labels;
    await this.guard(true, true);
    await this.waitForm();
  }
  async prepare() {
    this.stage = 'initial_binding';
    this.deadline = performance.now() + 30000;
    this.root = await this.page.locator('html').elementHandle();
    if (!this.root) throw failure('native_form_unrecognized');
    await this.guard();
    const initial = await this.form(0);
    this.signature = JSON.stringify(initial.binding);
    const binding = initial.binding;
    const dates = binding.event_configuration.dates;
    if (dates.dateStatus !== 'singleDate' || dates.dateDisplay !== 'showDateTime'
        || typeof dates.startDate !== 'string' || dates.startDate !== dates.eventDate || dates.endDate !== dates.startDate
        || Date.parse(dates.startDate) !== Date.parse(this.request.expected_event_start)) throw failure('event_date_changed');
    this.sourceEventStart = dates.startDate;
    this.stage = 'quantity_selection';
    for (let count = 0; count < this.request.quantity; count++) {
      await this.guard();
      const surface = await this.form(count);
      if (surface.increment_disabled) throw failure('native_form_unrecognized');
      const control = await this.page.locator('[data-testid="quantityStepper"]').nth(surface.stepper_index)
        .locator(':scope > button').nth(1).elementHandle();
      if (!control) throw failure('native_form_changed');
      this.quantityActionAttempted = true;
      try { await control.click({ timeout: this.remaining() }); } finally { await control.dispose(); }
      await this.guard(false, true);
      // React can remove/reorder other category rows after an increment.
      // Rebind by validated category identity, never the previous array index.
      await this.waitForm(count + 1);
      await this.form(count + 1);
    }
    this.stage = 'filter_inspection';
    const prices = (await this.open('price')).mapped;
    this.assertChecks(prices, 'price', []);
    await this.closePanel();
    const sections = (await this.open('section')).mapped;
    this.assertChecks(sections, 'section', []);
    await this.closePanel();
    const price = prices.find(item => item.price_id === this.request.price_id);
    if (!price || price.disabled || price.amount_minor !== this.request.configured_unit_price_minor || !price.section_ids.length
        || this.request.section_ids.some(value => !price.section_ids.includes(value))
        || binding.filter_translations.apply !== 'Použít filtry' || binding.filter_translations.sections !== 'Sekce')
      throw failure('native_form_unrecognized');
    this.effectiveSections = [...(this.request.section_ids.length ? this.request.section_ids : price.section_ids)].sort();
    const sectionLabel = values => {
      if (values.length !== 1) return `${values.length} Sekce`;
      const selected = sections.find(item => item.section_id === values[0]);
      if (!selected || selected.disabled) throw failure('native_form_unrecognized');
      return selected.label;
    };
    const priceLabel = price.label.split(' • ')[0];
    this.stage = 'price_selection';
    await this.open('price');
    await this.setChecks('price', [this.request.price_id]);
    await this.apply({ price: priceLabel, section: sectionLabel(price.section_ids) });
    this.stage = 'section_selection';
    await this.open('section');
    if (await this.setChecks('section', this.effectiveSections))
      await this.apply({ price: priceLabel, section: sectionLabel(this.effectiveSections) });
    else await this.closePanel();
    this.selectionApplied = true;
    return this.verify();
  }
  async verify(sealFind = false) {
    this.stage = 'preflight_verification';
    this.deadline = performance.now() + 10000;
    const panels = [];
    for (const kind of ['price', 'section']) {
      const { panel, mapped } = await this.open(kind);
      this.assertChecks(mapped, kind, kind === 'price' ? [this.request.price_id] : this.effectiveSections);
      panels.push({ kind, options: panel.options });
      await this.closePanel();
    }
    await this.guard();
    const form = await this.form(this.request.quantity, sealFind);
    return { binding: form.binding, panels };
  }
  async findHandle() {
    this.stage = 'find_control';
    this.deadline = performance.now() + 5000;
    await this.guard();
    await this.form();
    const button = this.page.locator('button[data-testid="findTicketsBtn"]').filter({ visible: true });
    if (await button.count() !== 1 || !await button.isEnabled()
        || (await button.innerText()).replace(/\s+/g, ' ').trim() !== 'Najít vstupenky') throw failure('native_form_changed');
    return button.elementHandle();
  }
}

class NativeCartSession {
  constructor({ browser, evidenceDir, request, checkoutNavigationPolicy }) {
    if (!browser || typeof evidenceDir !== 'string' || !path.isAbsolute(evidenceDir)) throw failure('invalid_request');
    this.browser = browser;
    this.request = validateRequest(request);
    this.evidenceRoot = evidenceDir;
    this.checkoutNavigationPolicy = checkoutNavigationPolicy;
    this.sessionId = uid('session');
    this.documentId = uid('document');
    this.outcomeDocumentId = this.documentId;
    this.state = 'new';
    this.spent = false;
    this.connected = true;
    this.queue = Promise.resolve();
    this.captureQueue = Promise.resolve();
    this.pendingRequests = new WeakSet();
    this.captureCount = 0;
    this.captureBytes = 0;
    this.captureFailed = false;
  }
  serialize(operation) {
    const result = this.queue.then(operation);
    this.queue = result.catch(() => {});
    return result;
  }
  prepare() { return this.serialize(() => this.prepareOnce()); }
  refresh() { return this.serialize(async () => {
    if (this.state !== 'prepared' || this.spent || !this.connected) throw failure('invalid_state');
    try { return await this.makePreflight(await this.driver.verify()); }
    catch (error) { this.preflight = null; this.state = 'paused'; throw error; }
  }); }
  resumePreparation() { return this.serialize(async () => {
    if (!['paused', 'prepared'].includes(this.state) || this.spent || !this.connected)
      throw failure('invalid_state');
    this.preflight = null;
    try {
      await this.checkManualActions();
      await this.documents.synchronize();
      const current = this.documents.current();
      if (!current || !eventNavigationAllowed(this.request.event_url, current.url)
          || !eventNavigationAllowed(this.request.event_url, this.page.url())) throw failure('native_form_changed');
      if (current.status < 200 || current.status >= 300) throw Object.assign(failure('needs_human'), {
        kind: responseIntervention(current.status, current.headers) || 'access_blocked',
      });
      const challenge = await inspectChallenge(this.page);
      if (challenge) throw Object.assign(failure('needs_human'), { kind: challenge });
      if (this.establishedPreflight || this.driver?.selectionApplied) {
        // Reopen only display panels in the exact existing document/configuration.
        // Never reset selections or silently adopt a newly loaded document here.
        if (!this.driver || current.loader !== this.snapshot?.loader) throw failure('native_form_changed');
        return await this.makePreflight(await this.driver.verify());
      }
      // An interrupted increment is not a reusable initial form. We cannot know
      // whether a human altered its local selection while handling verification.
      if (this.driver?.quantityActionAttempted) throw failure('native_form_changed');
      await this.driver?.root?.dispose().catch(() => {});
      this.driver = null;
      return await this.prepareCurrentDocument(current);
    } catch (error) { return this.pausePreparation(error); }
  }); }
  commit(command) { return this.serialize(() => this.commitOnce(command)); }
  observe(command) { return this.serialize(async () => {
    if (!this.spent || command.attempt_id !== this.attemptId) throw failure('invalid_state');
    return this.observation();
  }); }
  disconnect() {
    this.connected = false;
    this.preflight = null;
    if (!this.spent) this.state = 'disconnected';
    // The browser and its native in-flight requests stay alive. No operation is
    // retried; the durable journal remains the authority after controller loss.
  }
  async initializeEvidence() {
    try { await fs.mkdir(this.evidenceRoot, { mode: 0o700 }); }
    catch (error) { if (error.code !== 'EEXIST') throw failure('evidence_unavailable'); }
    const stat = await fs.lstat(this.evidenceRoot);
    if (!stat.isDirectory() || stat.isSymbolicLink() || (stat.mode & 0o077) !== 0
        || typeof process.getuid === 'function' && stat.uid !== process.getuid()) throw failure('evidence_unavailable');
    this.evidenceDir = path.join(this.evidenceRoot, this.sessionId);
    await fs.mkdir(this.evidenceDir, { mode: 0o700 });
  }
  async writeEvidence(kind, data) {
    const bytes = Buffer.from(JSON.stringify(data));
    if (bytes.length > 4 * 1024 * 1024 || this.captureCount >= 48 || this.captureBytes + bytes.length > 32 * 1024 * 1024) return;
    const filename = `${String(++this.captureCount).padStart(3, '0')}-${kind}.json`;
    this.captureBytes += bytes.length;
    try { await fs.writeFile(path.join(this.evidenceDir, filename), bytes, { mode: 0o600, flag: 'wx' }); }
    catch { this.captureFailed = true; }
  }
  assertAutomationOwned() {
    if (!this.connected || this.unsafeNavigation || this.nativeDialog || this.foreignNativeAction)
      throw failure('native_form_changed');
  }
  async checkManualActions() {
    this.assertAutomationOwned();
    if (!this.findClickAttempted && this.nativeClickCounter) {
      const count = await this.page.evaluate(key => globalThis[key], this.nativeClickCounter);
      if (count !== 0) {
        this.markForeignNativeAction('find_control');
        throw failure('native_form_changed');
      }
    }
  }
  markForeignNativeAction(kind) {
    if (this.findClickAttempted || this.foreignNativeAction) return;
    this.foreignNativeAction = true;
    this.preflight = null;
    if (!this.spent) this.state = 'paused';
    this.enqueueEvidence('untracked-native-action', { kind, observed_at_ms: Date.now() });
  }
  allowedNavigation(url) {
    if (eventNavigationAllowed(this.request.event_url, url)) return true;
    return this.spent && typeof this.checkoutNavigationPolicy === 'function' && this.checkoutNavigationPolicy(url) === true;
  }
  async prepareOnce() {
    if (this.state !== 'new' || !this.connected) throw failure('invalid_state');
    this.state = 'loading';
    this.stage = 'event_navigation';
    await this.initializeEvidence();
    this.context = await this.browser.newContext({ serviceWorkers: 'block' });
    const actionBinding = uid('native_action');
    this.nativeClickCounter = uid('native_click_count');
    this.nativeGateControl = uid('native_gate');
    this.nativeGateSecret = uid('native_gate_key');
    await this.context.exposeBinding(actionBinding, ({ page, frame }) => {
      if (page !== this.page || frame !== this.page.mainFrame()) return;
      if (!this.findClickAttempted) this.markForeignNativeAction('find_control');
      else this.enqueueEvidence('find-gate', { phase: 'additional_find_blocked', observed_at_ms: Date.now() });
    });
    await this.context.addInitScript(installNativeFindGate, { binding: actionBinding, counter: this.nativeClickCounter,
      control: this.nativeGateControl, secret: this.nativeGateSecret });
    this.page = await this.context.newPage();
    this.page.setDefaultTimeout(1500);
    this.context.on('page', page => {
      if (page !== this.page) {
        this.unsafeNavigation = true;
        void page.close().catch(() => {});
      }
    });
    await this.context.route('**/*', route => {
      const outgoing = route.request();
      if (outgoing.isNavigationRequest() && outgoing.frame().parentFrame() === null
          && !this.allowedNavigation(outgoing.url())) {
        this.unsafeNavigation = true;
        if (this.spent) this.enqueueEvidence('blocked-navigation', { observed_at_ms: Date.now(), reason: 'unsupported_navigation' });
        return route.abort('blockedbyclient');
      }
      return route.fallback();
    });
    this.documents = await observeDocuments(this.context, this.page, document => {
      if (!this.allowedNavigation(document.url)) { this.unsafeNavigation = true; throw failure('unsafe_redirect'); }
    });
    this.page.on('framenavigated', frame => {
      if (frame === this.page.mainFrame() && this.spent) this.outcomeDocumentId = uid('document');
    });
    this.page.on('dialog', dialog => {
      this.nativeDialog = true;
      // Never accept reservation, information or non-adjacent confirmations.
      if (this.spent) this.enqueueEvidence('native-dialog', { observed_at_ms: Date.now(), type: dialog.type() });
    });
    this.context.on('request', request => {
      const classification = classifyNativeRequest(request, this.request.event_url);
      if (!this.findClickAttempted && classification.native_endpoint) this.markForeignNativeAction('reservation_request');
      if (this.findClickAttempted) this.pendingRequests.add(request);
      this.requestLedger?.observeRequest(request);
    });
    this.context.on('requestfinished', request => { this.requestLedger?.requestFinished(request); });
    this.context.on('requestfailed', request => { this.requestLedger?.requestFailed(request); });
    this.context.on('response', response => {
      this.requestLedger?.observeResponse(response);
      if (!this.findClickAttempted || !this.pendingRequests.has(response.request())) return;
      // Raw bodies are limited to reviewed reservation contracts. Other checkout
      // APIs, unknown GraphQL and HTML can contain account/payment form data.
      let url;
      try { url = new URL(response.url()); } catch { return; }
      const classification = classifyNativeRequest(response.request(), this.request.event_url);
      if (response.url() !== response.request().url() || url.search || url.hash
          || response.request().method() !== 'POST' || !classification.native_endpoint
          || !(classification.kind === 'mfol_reserve' || classification.kind === 'gco_graphql'
            && classification.query_contract_verified === true && classification.parse_status === 'parsed')) return;
      const type = response.headers()['content-type'] || '';
      if (!/^application\/json(?:;|$)/i.test(type)) return;
      if (this.responseCaptures >= 24) return;
      this.responseCaptures = (this.responseCaptures || 0) + 1;
      this.captureQueue = this.captureQueue.then(async () => {
        if (this.captureCount >= 48 || this.captureBytes >= 32 * 1024 * 1024) return;
        const length = response.headers()['content-length'];
        // Chunked native JSON must remain observable. The browser already
        // received these narrow-endpoint responses; bound read time, then reject
        // oversized decoded bodies before retaining or writing any evidence.
        if (length && (!/^\d+$/.test(length) || Number(length) > 2 * 1024 * 1024)) return;
        let body;
        try { body = await bounded(response.body(), 2000); } catch { this.captureFailed = true; this.requestLedger?.observeResponse(response); return; }
        if (body.length > 2 * 1024 * 1024) { this.requestLedger?.observeResponse(response); return; }
        this.requestLedger?.observeResponse(response, body);
        await this.writeEvidence('response', { observed_at_ms: Date.now(), url: url.origin + url.pathname, status: response.status(),
          content_type: type, request_correlation_id: this.requestLedger?.requestId(response.request()) || null, body: body.toString('utf8') });
      }).catch(() => { this.captureFailed = true; });
    });
    try {
      const response = await this.page.goto(this.request.event_url, { waitUntil: 'domcontentloaded', timeout: 30000 });
      if (!response) throw failure('navigation_failed');
      let document = { status: response.status(), headers: response.headers(), url: response.url() };
      const identification = value => value.status === 401 && value.headers['tm-bl']?.trim() === '1'
        && /^text\/html(?:\s*;|\s*$)/i.test(value.headers['content-type'] || '') && identificationRetryAllowed(value.headers);
      if (identification(document)) {
        document = await this.documents.waitForIdentification(15000, identification, value => {
          if (!eventNavigationAllowed(this.request.event_url, value.url)) throw failure('unsafe_redirect');
        });
      }
      if (document.status < 200 || document.status >= 300) {
        this.state = 'paused';
        const kind = responseIntervention(document.status, document.headers)
          || (document.status === 401 ? 'identification_incomplete' : 'access_blocked');
        throw Object.assign(failure('needs_human'), { kind });
      }
      await this.documents.synchronize();
      const current = this.documents.current();
      if (!current) throw failure('navigation_failed');
      return await this.prepareCurrentDocument(current);
    } catch (error) { return this.pausePreparation(error); }
  }
  async prepareCurrentDocument(current) {
    this.assertAutomationOwned();
    if (!current || current.status < 200 || current.status >= 300
        || !eventNavigationAllowed(this.request.event_url, current.url)
        || !eventNavigationAllowed(this.request.event_url, this.page.url())) throw failure('navigation_failed');
    if (this.snapshot && current.loader !== this.snapshot.loader) {
      this.documentId = uid('document');
      this.outcomeDocumentId = this.documentId;
    }
    this.snapshot = current;
    const guard = async ({ afterCartControlsAction = false, afterDisplayAction = false } = {}) => {
      await this.checkManualActions();
      if (!eventNavigationAllowed(this.request.event_url, this.page.url())) throw failure('unsafe_redirect');
      await this.documents.synchronize();
      if (afterCartControlsAction || afterDisplayAction) {
        const current = this.documents.current();
        if (!current || current.loader !== this.snapshot.loader || current.status !== this.snapshot.status) throw failure('native_form_changed');
        this.snapshot = current;
      } else this.documents.assertCurrent(this.snapshot);
    };
    // Consent mounts asynchronously after DOMContentLoaded on the observed
    // Czech page. Match the normal worker's bounded initial rendering window
    // before its exact known consent handler runs, while quantities are zero.
    // A later or unknown modal still fails closed in NativeFormDriver.guard.
    this.stage = 'initial_render';
    const renderingDeadline = performance.now() + 1500;
    while (performance.now() < renderingDeadline) {
      await guard();
      const challenge = await inspectChallenge(this.page);
      if (challenge) throw Object.assign(failure('needs_human'), { kind: challenge });
      await this.page.waitForTimeout(Math.min(100, Math.max(1, renderingDeadline - performance.now())));
    }
    await guard();
    this.stage = 'public_display';
    const display = await prepareTicketmaster(this.page, this.request.event_url, 10000, guard);
    if (display.state === 'blocked') throw Object.assign(failure('needs_human'), { kind: await inspectChallenge(this.page) || 'verification' });
    if (display.state !== 'regional') throw failure('native_form_unrecognized');
    this.driver = new NativeFormDriver(this.page, this.request.event_url, this.request, guard,
      { control: this.nativeGateControl, secret: this.nativeGateSecret });
    return await this.makePreflight(await this.driver.prepare());
  }
  async pausePreparation(error) {
    if (error.name === 'TimeoutError') error = failure(this.driver ? 'native_form_timeout' : 'navigation_failed');
    this.state = 'paused';
    this.preflight = null;
    // Failed preparation is useful private evidence, never a stock/hold claim.
    try {
      const document = await this.captureDocument();
      await this.writeEvidence('preparation-failed', { observed_at_ms: Date.now(), stage: this.driver?.stage || this.stage,
        reason: ['needs_human', 'native_form_changed', 'native_form_unrecognized', 'native_modal_blocked',
          'native_form_timeout', 'navigation_failed', 'event_date_changed', 'unsafe_redirect', 'cart_controls_unrecognized'].includes(error.code)
          ? error.code : 'session_failed', ...(document || { document_omitted: true }) });
    } catch { this.captureFailed = true; }
    throw error;
  }
  async makePreflight(surface) {
    await this.checkManualActions();
    if (!this.connected || this.spent) throw failure('invalid_state');
    const observed = Date.now();
    this.preflight = {
      session_id: this.sessionId, document_id: this.documentId, config_id: `config_${hash(surface.binding)}`,
      preflight_id: uid('preflight'), config_observed_at_ms: observed, config_expires_at_ms: observed + 5000,
      event_url: this.request.event_url, source_event_start: this.driver.sourceEventStart,
      category_id: this.request.category_id, category_title: surface.binding.category_title,
      quantity: this.request.quantity, price_id: this.request.price_id,
      configured_unit_price_minor: this.request.configured_unit_price_minor, currency: 'CZK',
      section_ids: [...this.request.section_ids],
    };
    this.establishedPreflight = true;
    this.state = 'prepared';
    return { state: 'prepared', preflight: { ...this.preflight } };
  }
  async commitOnce(command) {
    if (!command || !applicationId(command.attempt_id) || command.dispatch_token !== command.attempt_id
        || this.spent || !this.connected || this.state !== 'prepared' || !this.preflight
        || command.preflight_id !== this.preflight.preflight_id
        || (Object.hasOwn(command, 'adjacent_required') && typeof command.adjacent_required !== 'boolean')) throw failure('invalid_commit');
    if (typeof this.checkoutNavigationPolicy !== 'function') throw failure('checkout_policy_unavailable');
    if (Date.now() >= this.preflight.config_expires_at_ms) throw failure('preflight_expired');
    // Consume permission BEFORE even preparing the click. A timeout or document
    // change can never lead to another reservation attempt with this session.
    this.spent = true;
    this.attemptId = command.attempt_id;
    this.dispatchPreflight = this.preflight;
    this.preflight = null;
    this.state = 'dispatching';
    await this.writeEvidence('dispatch', { attempt_id: this.attemptId, preflight: this.dispatchPreflight, observed_at_ms: Date.now() });
    try {
      await this.driver.verify(true);
      if (!this.connected || Date.now() >= this.dispatchPreflight.config_expires_at_ms) throw failure('preflight_expired');
      const handle = await this.driver.findHandle();
      if (!handle) throw failure('native_form_changed');
      try {
        await this.driver.guard();
        if (!this.connected || Date.now() >= this.dispatchPreflight.config_expires_at_ms) throw failure('preflight_expired');
        this.requestLedger = new PassiveNativeRequestLedger({ eventUrl: this.request.event_url,
          sourceDocumentId: this.documentId, expectedSelection: { quantity: this.request.quantity,
            categoryId: this.request.category_id, priceId: this.request.price_id, sectionIds: this.driver.effectiveSections,
            // This is the controller's dispatch-bound preference, not proof of
            // adjacent seats. Legacy callers lacking it retain an unknown check.
            ...(Object.hasOwn(command, 'adjacent_required') ? { adjacentRequired: command.adjacent_required } : {}) },
          onRecord: record => this.enqueueEvidence('request-ledger', record) });
        this.requestLedger.arm({ attemptId: this.attemptId, clickedAtMs: Date.now(), monotonicMs: performance.now() });
        this.findClickAttempted = true;
        // Diagnostic markers are queued without extending the fresh preflight's
        // critical path. The journal, not these files, authorizes the dispatch.
        this.enqueueEvidence('action', { attempt_id: this.attemptId, phase: 'find_click_attempted',
          observed_at_ms: Date.now(), monotonic_ms: performance.now() });
        const armed = await this.page.evaluate(({ control, secret, button, root, filterLabels }) =>
          globalThis[control]?.(secret, 'arm', { button, root, filterLabels }), {
          control: this.nativeGateControl, secret: this.nativeGateSecret, button: handle,
          root: this.driver.root, filterLabels: this.driver.filterLabels,
        });
        if (!armed) throw failure('native_form_changed');
        await handle.click({ timeout: 1500, noWaitAfter: true });
        this.enqueueEvidence('action', { attempt_id: this.attemptId, phase: 'find_click_returned',
          observed_at_ms: Date.now(), monotonic_ms: performance.now() });
      } finally { await handle.dispose().catch(() => {}); }
      this.state = 'observing';
      await this.page.waitForTimeout(500);
      return await this.observation();
    } catch (error) {
      this.state = 'uncertain';
      return this.observation(error.code === 'needs_human' ? 'challenge' : error.name === 'TimeoutError' ? 'timeout' : 'unrecognized_response');
    }
  }
  enqueueEvidence(kind, data) {
    this.captureQueue = this.captureQueue.then(() => this.writeEvidence(kind, data)).catch(() => { this.captureFailed = true; });
  }
  async captureDocument() {
    if (!this.page || !this.allowedNavigation(this.page.url())) return null;
    return bounded(this.page.evaluate(sanitizedDocumentSnapshot), 1500);
  }
  async nativeSignal() {
    // Diagnostic only: a failed native search is not proof that no hold exists
    // or that this event has no other inventory. Never cache a prior signal.
    if (this.findClickAttempted !== true || this.unsafeNavigation || this.nativeDialog || !this.documents) return null;
    try {
      await this.documents.synchronize();
      const snapshot = this.documents.current();
      if (!snapshot || snapshot.status < 200 || snapshot.status >= 300
          || !eventNavigationAllowed(this.request.event_url, snapshot.url)
          || !eventNavigationAllowed(this.request.event_url, this.page.url())) return null;
      const result = await this.page.evaluate(eventId => {
        const shown = node => {
          if (!node?.isConnected) return false;
          for (let current = node; current; current = current.parentElement) {
            const style = getComputedStyle(current);
            if (current.hidden || current.hasAttribute('inert') || current.getAttribute('aria-hidden') === 'true'
                || style.display === 'none' || ['hidden', 'collapse'].includes(style.visibility)
                || style.contentVisibility === 'hidden' || Number(style.opacity) === 0) return false;
          }
          return [...node.getClientRects()].some(rect => rect.width > 0 && rect.height > 0);
        };
        const views = [...document.querySelectorAll('[data-testid="reserveView"]')].filter(shown);
        const errors = [...document.querySelectorAll('[data-testid="reserveError"]')].filter(shown);
        if (views.length !== 1 || errors.length !== 1 || errors[0].closest('[data-testid="reserveView"]') !== views[0]) return null;
        const message = errors[0].innerText.normalize('NFC').replace(/\s+/g, ' ').trim();
        if (message !== 'Pro provedení vašeho požadavku není k dispozici dostatek vstupenek. Pro zobrazení dostupných vstupenek upravte množství nebo typ vstupenek.') return null;
        const scripts = document.querySelectorAll('script#__NEXT_DATA__[type="application/json"]');
        const canonicals = document.querySelectorAll('link[rel="canonical"]');
        if (scripts.length !== 1 || scripts[0].textContent.length > 2 * 1024 * 1024 || canonicals.length !== 1) return null;
        try {
          const next = JSON.parse(scripts[0].textContent), state = next.props?.pageProps?.initialReduxState;
          const event = state?.eventInfo;
          if (!event || [next.query?.eventId, state.eventId, event.eventId, event.params,
            state.ticketSelection?.params?.eventId].some(value => value !== eventId)
              || event.stage !== 'success' || event.inFlightParams !== null
              || state.config?.countryCode !== 'CZ' || typeof event.canonicalUrl !== 'string') return null;
          return { canonical: canonicals[0].href, embeddedCanonical: new URL(event.canonicalUrl, location.href).href };
        } catch { return null; }
      }, eventIdentity(this.request.event_url).id);
      await this.documents.synchronize();
      this.documents.assertCurrent(snapshot);
      if (!result || !eventNavigationAllowed(this.request.event_url, result.canonical)
          || !eventNavigationAllowed(this.request.event_url, result.embeddedCanonical)) return null;
      return 'insufficient_tickets_reported';
    } catch { return null; }
  }
  async observation(reason = 'pending') {
    let challenge = null;
    try { challenge = await inspectChallenge(this.page); } catch { reason = 'unrecognized_response'; }
    try {
      const current = new URL(this.page.url());
      if (current.origin === 'https://identity.ticketmaster.cz' && current.pathname === '/sign-in') challenge = 'authentication_required';
    } catch { /* No navigation state is interpreted as absence of a hold. */ }
    if (challenge) reason = 'challenge';
    if (this.unsafeNavigation || this.nativeDialog || this.captureFailed) reason = 'unrecognized_response';
    try {
      const document = challenge === 'authentication_required' ? null : await this.captureDocument();
      const fingerprint = hash(document);
      if (document && fingerprint !== this.lastDocumentFingerprint) {
        this.lastDocumentFingerprint = fingerprint;
        this.enqueueEvidence('document', { observed_at_ms: Date.now(), document_id: this.outcomeDocumentId, ...document });
      }
    } catch { reason = 'unrecognized_response'; }
    await bounded(this.captureQueue, 2500).catch(() => { this.captureFailed = true; });
    const nativeSignal = challenge ? null : await this.nativeSignal();
    return {
      state: challenge ? 'needs_human' : 'uncertain',
      ...(challenge ? { manual_intervention: challenge } : {}),
      ...(nativeSignal ? { native_signal: nativeSignal } : {}),
      outcome: { kind: 'uncertain', receipt: { context: {
        attempt_id: this.attemptId, event_url: null, event_start: null,
        session_id: this.sessionId, source_document_id: this.documentId,
        outcome_document_id: this.outcomeDocumentId, config_id: this.dispatchPreflight.config_id,
        observed_at_ms: Date.now(), alternate_event_or_date_reported: false,
      }, reason } },
    };
  }
  async close() {
    this.disconnect();
    // Close native network/browser resources before waiting for best-effort
    // diagnostic drains, so a stalled response cannot keep shutdown alive.
    await bounded(this.context?.close().catch(() => {}), 3000).catch(() => {});
    await bounded(this.queue, 3000).catch(() => {});
    await bounded(this.captureQueue, 3000).catch(() => {});
    await this.driver?.root?.dispose().catch(() => {});
    await this.documents?.close().catch(() => {});
    this.state = 'closed';
  }
}

// Source-observed SDK build v1-209-0-7d119975-b15709894 (2026-11-10).
// This is a narrow source contract, not a claim that a live checkout succeeded.
function checkoutNavigationAllowed(target, candidate) {
  try {
    const url = new URL(candidate);
    if (url.protocol !== 'https:' || url.username || url.password || url.port || url.hash
        || url.href.length > 8192 || /%2f|%5c|%2e/i.test(url.pathname)) return false;
    const keys = [...url.searchParams.keys()];
    if (new Set(keys).size !== keys.length) return false;
    if (url.origin === 'https://checkout.ticketmaster.cz') {
      if (!/^\/[A-Za-z0-9_-]{1,256}$/.test(url.pathname)) return false;
      const allowed = ['ccp_src', 'ccp_channel', 'edp', 'f_appview', 'f_appview_ln', 'f_appview_version', 'f_layout', 'venue_owner'];
      if (keys.some(key => !allowed.includes(key))) return false;
      if (url.searchParams.has('edp') && !eventNavigationAllowed(target, url.searchParams.get('edp'))) return false;
      if (url.searchParams.has('venue_owner') && url.searchParams.get('venue_owner') !== 'LN') return false;
      return true;
    }
    if (url.origin !== 'https://identity.ticketmaster.cz' || url.pathname !== '/sign-in') return false;
    const allowed = ['lang', 'integratorId', 'placementId', 'showHeader', 'headerTitle', 'hideLeftPanel', 'redirectUri', 'timerData'];
    if (keys.some(key => !allowed.includes(key)) || url.searchParams.get('showHeader') !== 'true'
        || url.searchParams.get('headerTitle') !== 'Checkout' || url.searchParams.get('hideLeftPanel') !== 'true') return false;
    if (![['prd1741.iccp', 'cart.reserve'], ['prd1741.iccp', 'mytmlogin'], ['prd1908.CCPCheckout', 'checkout.v2']]
      .some(([integrator, placement]) => url.searchParams.get('integratorId') === integrator && url.searchParams.get('placementId') === placement)) return false;
    const redirect = url.searchParams.get('redirectUri');
    if (!redirect || new URL(redirect).origin !== 'https://checkout.ticketmaster.cz'
        || !checkoutNavigationAllowed(target, redirect)) return false;
    if (url.searchParams.has('timerData')) {
      const raw = url.searchParams.get('timerData');
      if (!/^[A-Za-z0-9+/=]{1,4096}$/.test(raw)) return false;
      const timer = JSON.parse(Buffer.from(raw, 'base64').toString('utf8'));
      if (!timer || Object.keys(timer).sort().join(',') !== 'artistName,remainingTime,timesOutRedirectURL'
          || timer.artistName !== '' || !finiteInteger(timer.remainingTime, 0, Number.MAX_SAFE_INTEGER)
          || !eventNavigationAllowed(target, timer.timesOutRedirectURL)) return false;
    }
    return true;
  } catch { return false; }
}

function launchEnvironment(env = process.env) {
  const allowed = ['PATH', 'HOME', 'TMPDIR', 'TMP', 'TEMP', 'LANG', 'LANGUAGE', 'LC_ALL', 'DISPLAY', 'XAUTHORITY',
    'XDG_RUNTIME_DIR', 'XDG_CONFIG_HOME', 'XDG_CACHE_HOME', 'FONTCONFIG_PATH', 'SSL_CERT_FILE', 'SSL_CERT_DIR',
    'HTTP_PROXY', 'HTTPS_PROXY', 'ALL_PROXY', 'NO_PROXY', 'http_proxy', 'https_proxy', 'all_proxy', 'no_proxy'];
  return Object.fromEntries(allowed.filter(key => typeof env[key] === 'string').map(key => [key, env[key]]));
}
function parseArgs(argv) {
  const options = { headed: false, disconnectGraceMs: 120000 };
  for (let index = 0; index < argv.length; index++) {
    const flag = argv[index];
    if (flag === '--headed') options.headed = true;
    else if (flag === '--executable-path') options.executablePath = argv[++index];
    else if (flag === '--evidence-dir') options.evidenceDir = argv[++index];
    else if (flag === '--disconnect-grace-ms') options.disconnectGraceMs = Number(argv[++index]);
    else throw failure('invalid_arguments');
  }
  if (!options.evidenceDir || !path.isAbsolute(options.evidenceDir)
      || !finiteInteger(options.disconnectGraceMs, 0, 1800000)
      || options.executablePath !== undefined && !path.isAbsolute(options.executablePath)) throw failure('invalid_arguments');
  return options;
}

async function main() {
  const options = parseArgs(process.argv.slice(2));
  const { chromium } = require('playwright');
  let browser, session, stopped = false, disconnected = false, disconnectTimer;
  let chain = Promise.resolve();
  const send = (message, afterClose = false) => { if (!disconnected && (!stopped || afterClose)) process.stdout.write(JSON.stringify(message) + '\n'); };
  const close = async () => {
    if (stopped) return;
    stopped = true;
    clearTimeout(disconnectTimer);
    await session?.close().catch(() => {});
    await bounded(browser?.close(), 5000);
    if (browser?.isConnected()) throw failure('browser_close_failed');
  };
  const disconnect = () => {
    if (disconnected) return;
    disconnected = true;
    session?.disconnect();
    if (stopped) return;
    if (!session?.spent) void close().catch(() => {});
    else disconnectTimer = setTimeout(() => { void close().catch(() => {}); }, options.disconnectGraceMs);
  };
  process.stdout.on('error', disconnect);
  process.once('SIGINT', () => { disconnect(); void close().catch(() => {}); });
  process.once('SIGTERM', () => { disconnect(); void close().catch(() => {}); });
  const lines = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  lines.on('line', line => {
    chain = chain.then(async () => {
      if (stopped || disconnected) return;
      let command;
      try {
        if (Buffer.byteLength(line) > 16384) throw failure('invalid_request');
        command = JSON.parse(line);
        if (!command || !finiteInteger(command.id, 1, Number.MAX_SAFE_INTEGER)
            || !['prepare', 'refresh', 'resume_preparation', 'commit', 'observe', 'close'].includes(command.op)) throw failure('invalid_request');
        const keys = { prepare: ['id', 'op', 'request'], refresh: ['id', 'op'], resume_preparation: ['id', 'op'],
          commit: ['id', 'op', 'attempt_id', 'preflight_id', 'dispatch_token', 'adjacent_required'],
          observe: ['id', 'op', 'attempt_id'], close: ['id', 'op'] }[command.op];
        if (Object.keys(command).some(key => !keys.includes(key))) throw failure('invalid_request');
        if (command.op === 'close') {
          try {
            await close();
            send({ id: command.id, ok: true, state: 'closed' }, true);
          } catch { send({ id: command.id, ok: false, error: 'browser_close_failed' }, true); }
          lines.close(); return;
        }
        let reply;
        if (command.op === 'prepare') {
          if (session) throw failure('invalid_state');
          const request = validateRequest(command.request);
          browser = await chromium.launch({ headless: !options.headed, env: launchEnvironment(),
            ...(options.executablePath ? { executablePath: options.executablePath } : {}),
            ...(proxyOptions() ? { proxy: proxyOptions() } : {}) });
          if (disconnected) { await close(); return; }
          session = new NativeCartSession({ browser, evidenceDir: options.evidenceDir, request,
            checkoutNavigationPolicy: url => checkoutNavigationAllowed(request.event_url, url) });
          reply = await session.prepare();
        } else {
          if (!session) throw failure('invalid_state');
          reply = command.op === 'refresh' ? await session.refresh()
            : command.op === 'resume_preparation' ? await session.resumePreparation()
            : command.op === 'commit' ? await session.commit(command) : await session.observe(command);
        }
        send({ id: command.id, ok: true, ...reply });
      } catch (error) {
        if (error.code === 'needs_human') send({ id: command?.id || null, ok: true, state: 'needs_human', manual_intervention: error.kind || 'verification' });
        else {
          const allowed = ['invalid_request', 'invalid_state', 'invalid_commit', 'checkout_policy_unavailable', 'preflight_expired',
            'evidence_unavailable', 'navigation_failed', 'unsafe_redirect', 'native_form_changed', 'native_form_timeout',
            'native_form_unrecognized', 'native_modal_blocked', 'event_date_changed', 'cart_controls_unrecognized'];
          send({ id: command?.id || null, ok: false, error: allowed.includes(error.code) ? error.code : 'session_failed' });
        }
      }
    }).catch(() => { send({ id: null, ok: false, error: 'session_failed' }); });
  });
  lines.on('close', disconnect);
}

module.exports = { NativeCartSession, NativeFormDriver, validateRequest, eventNavigationAllowed,
  checkoutNavigationAllowed, launchEnvironment, parseArgs };
if (require.main === module) main().catch(() => { process.exitCode = 1; });
