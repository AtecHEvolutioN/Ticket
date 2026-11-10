'use strict';

// Bounded, read-only coverage audit. Reuses the production display adapter; it
// never invokes cart, quantity, seat selection, Find tickets or checkout code.
const fs = require('node:fs/promises');
const path = require('node:path');
const crypto = require('node:crypto');
const { execFile } = require('node:child_process');
const { promisify } = require('node:util');
const { capture, validateUrl, proxyOptions } = require('./worker.cjs');
const { eventIdentity, sameEvent } = require('./rendered.cjs');
const execute = promisify(execFile);
const MAX_HTML = 4 * 1024 * 1024;
const MAX_OBSERVATION = 256 * 1024;
const DEFAULTS = { limit: 100, timeoutMs: 45000, renderWaitMs: 2000, hostGapMs: 2000, maxMinutes: 45 };
const HELP = `Read-only Ticketmaster coverage audit.
node browser/coverage.cjs --input examples/coverage-2026-11-09.json
  --output captures/audit/report.json --capture-dir captures/audit/pages
  [--binary target/release/ticket-watch] [--headed] [--limit 100]
  [--timeout-ms 45000] [--render-wait-ms 2000] [--host-gap-ms 2000]
  [--max-minutes 45] [--resume | --replay]

Live mode uses direct event navigation and recognized display controls only.
A protection response or Retry-After halts that host for this run. It is never
retried automatically. Raw HTML and metadata are private. The JSON report contains
only public target labels, transport diagnostics and normalized offline parsing.
--resume reuses existing captures and probes missing targets; cached host blocks
are retained. --replay parses existing captures without launching a browser.
Artist/venue names come from the supplied manifest and are not identity proofs.
`;
const boundedString = (v, max) => typeof v === 'string' && v.trim() === v && v.length > 0
  && v.length <= max && !/[\p{Cc}]/u.test(v);
const TRANSPORT_CODES = new Set(['invalid_capture_result', 'body_too_large', 'navigation_failed',
  'navigation_timeout', 'unsafe_redirect', 'queue_redirect', 'browser_launch_failed']);
const transportCode = value => TRANSPORT_CODES.has(value) ? value : 'capture_failed';
const code = v => typeof v === 'string' && /^[a-z][a-z0-9_]{0,79}$/.test(v) ? v : null;
const safeEnv = () => {
  const env = { ...process.env };
  for (const key of ['DISCORD_BOT_TOKEN', 'TICKETMASTER_API_KEY', 'NODE_OPTIONS']) delete env[key];
  return env;
};

function options(argv) {
  const out = { ...DEFAULTS, binary: path.resolve('target/release/ticket-watch') };
  const names = { '--input': 'input', '--output': 'output', '--capture-dir': 'captureDir', '--binary': 'binary',
    '--limit': 'limit', '--timeout-ms': 'timeoutMs', '--render-wait-ms': 'renderWaitMs',
    '--host-gap-ms': 'hostGapMs', '--max-minutes': 'maxMinutes' };
  const flags = { '--headed': 'headed', '--resume': 'resume', '--replay': 'replay' };
  const seen = new Set();
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === '--help' && argv.length === 1) return { help: true };
    if (seen.has(arg)) throw new Error('invalid_arguments');
    seen.add(arg);
    if (flags[arg]) { out[flags[arg]] = true; continue; }
    if (!names[arg] || !boundedString(argv[i + 1], 4096) || argv[i + 1].startsWith('--')) throw new Error('invalid_arguments');
    out[names[arg]] = argv[++i];
  }
  if (!out.input || !out.output || !out.captureDir || out.resume && out.replay) throw new Error('invalid_arguments');
  for (const [key, min, max] of [['limit', 1, 300], ['timeoutMs', 1000, 120000],
    ['renderWaitMs', 0, 30000], ['hostGapMs', 2000, 60000], ['maxMinutes', 1, 180]]) {
    if (!/^\d+$/.test(String(out[key]))) throw new Error('invalid_arguments');
    out[key] = Number(out[key]);
    if (!Number.isSafeInteger(out[key]) || out[key] < min || out[key] > max) throw new Error('invalid_arguments');
  }
  for (const key of ['input', 'output', 'captureDir', 'binary']) out[key] = path.resolve(out[key]);
  return out;
}

function targets(value, limit = 100) {
  if (!Array.isArray(value) || value.length < 1 || value.length > 1000
      || !Number.isInteger(limit) || limit < 1 || limit > 300) throw new Error('invalid_targets');
  const result = [], identities = new Set(), ids = new Set();
  for (const item of value) {
    if (!item || typeof item !== 'object' || Array.isArray(item)
        || !boundedString(item.id, 100) || !/^[A-Za-z0-9_-]+$/.test(item.id)
        || !boundedString(item.artist, 240)) throw new Error('invalid_target');
    const url = validateUrl(item.event_url);
    const identity = eventIdentity(url.href);
    if (!identity || url.hash) throw new Error('invalid_target');
    const key = identity.host + ':' + identity.id;
    if (identities.has(key)) continue;
    if (ids.has(item.id)) throw new Error('duplicate_target_id');
    identities.add(key); ids.add(item.id);
    const target = { id: item.id, artist: item.artist, event_url: url.href, site_host: identity.host };
    for (const key of ['venue', 'city', 'country', 'date', 'currency', 'phase']) {
      if (item[key] !== undefined && item[key] !== null) {
        if (!boundedString(item[key], 240)) throw new Error('invalid_target');
        target[key] = item[key];
      }
    }
    if (target.currency && !/^[A-Z]{3}$/.test(target.currency)) throw new Error('invalid_target');
    result.push(target);
  }
  return result.slice(0, limit);
}

async function readJson(file, limit = 1024 * 1024) {
  const stat = await fs.lstat(file);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > limit) throw new Error('invalid_file');
  return JSON.parse(await fs.readFile(file, 'utf8'));
}
async function privateDirectory(directory) {
  await fs.mkdir(directory, { recursive: true, mode: 0o700 });
  const stat = await fs.lstat(directory);
  if (!stat.isDirectory() || stat.isSymbolicLink() || stat.mode & 0o077
      || typeof process.getuid === 'function' && stat.uid !== process.getuid()) throw new Error('unsafe_capture_directory');
}
async function atomicReport(file, report) {
  await fs.mkdir(path.dirname(file), { recursive: true, mode: 0o700 });
  const temporary = file + '.' + crypto.randomUUID() + '.tmp';
  try {
    await fs.writeFile(temporary, JSON.stringify(report, null, 2) + '\n', { mode: 0o600, flag: 'wx' });
    await fs.rename(temporary, file);
  } finally { await fs.unlink(temporary).catch(() => {}); }
}
async function exists(file) {
  try { await fs.lstat(file); return true; }
  catch (error) { if (error.code === 'ENOENT') return false; throw error; }
}
function paths(directory, target) {
  return { html: path.join(directory, target.id + '.html'), meta: path.join(directory, target.id + '.json'),
    error: path.join(directory, target.id + '.error.json') };
}
function metadata(target, result, capturedAt) {
  if (!result || !sameEvent(target.event_url, result.final_url)
      || !Number.isInteger(result.status) || result.status < 100 || result.status > 599) throw new Error('invalid_capture_result');
  const html = typeof result.html === 'string' ? result.html : '';
  if (Buffer.byteLength(html) > MAX_HTML) throw new Error('body_too_large');
  return { schema_version: 1, event_url: target.event_url, final_url: validateUrl(result.final_url).href,
    captured_at: new Date(capturedAt).toISOString(), rendered: true, status: result.status,
    adapter_state: code(result.adapter_state), adapter_reason: code(result.adapter_reason),
    manual_intervention: code(result.manual_intervention),
    retry_after_secs: Number.isFinite(result.retry_after_secs) && result.retry_after_secs >= 0 ? result.retry_after_secs : null,
    elapsed_ms: Number.isInteger(result.elapsed_ms) && result.elapsed_ms >= 0 ? result.elapsed_ms : null,
    html_bytes: Buffer.byteLength(html), capture_sha256: crypto.createHash('sha256').update(html).digest('hex') };
}
function blocked(meta) {
  return [401, 403, 429].includes(meta.status) || meta.adapter_state === 'blocked'
    || Boolean(meta.manual_intervention) || meta.retry_after_secs !== null && meta.retry_after_secs > 0;
}
async function cached(directory, target) {
  const files = paths(directory, target);
  let meta;
  try { meta = await readJson(files.meta, 16384); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
  if (meta.schema_version !== 1 || !sameEvent(target.event_url, meta.event_url)
      || !sameEvent(target.event_url, meta.final_url) || !Number.isInteger(meta.status)
      || typeof meta.captured_at !== 'string' || !Number.isFinite(Date.parse(meta.captured_at))) throw new Error('invalid_cached_capture');
  const stat = await fs.lstat(files.html);
  if (!stat.isFile() || stat.isSymbolicLink() || stat.size > MAX_HTML) throw new Error('invalid_cached_capture');
  const html = await fs.readFile(files.html);
  if (crypto.createHash('sha256').update(html).digest('hex') !== meta.capture_sha256) throw new Error('capture_hash_mismatch');
  // Reconstruct the allowlisted record rather than trusting arbitrary sidecar fields.
  return metadata(target, { ...meta, html: html.toString('utf8') }, Date.parse(meta.captured_at));
}
async function cachedError(directory, target) {
  let saved;
  try { saved = await readJson(paths(directory, target).error, 16384); }
  catch (error) { if (error.code === 'ENOENT') return null; throw error; }
  if (saved.schema_version !== 1 || saved.code !== 'queue_redirect' || !sameEvent(target.event_url, saved.event_url)
      || typeof saved.observed_at !== 'string' || !Number.isFinite(Date.parse(saved.observed_at))) throw new Error('invalid_cached_diagnostic');
  return { code: 'queue_redirect', observed_at: new Date(Date.parse(saved.observed_at)).toISOString() };
}
async function parseFile(binary, target, file) {
  const args = ['inspect', '--url', target.event_url, '--html', file];
  if (target.currency) args.push('--currency', target.currency);
  const result = await execute(binary, args, { env: safeEnv(), timeout: 20000, maxBuffer: MAX_HTML, encoding: 'utf8' });
  const parsed = JSON.parse(result.stdout);
  if (parsed.source !== 'saved_html' || !sameEvent(target.event_url, parsed.event_url)
      || !parsed.observation || parsed.observation.observed_at !== 0
      || Buffer.byteLength(JSON.stringify(parsed.observation)) > MAX_OBSERVATION) throw new Error('invalid_parser_result');
  return parsed.observation;
}
function summarize(events) {
  const counts = {};
  for (const event of events) counts[event.state] = (counts[event.state] || 0) + 1;
  return { targets_reported: events.length, captured_events: events.filter(e => e.capture).length,
    fresh_probes: events.filter(e => e.source === 'live_direct_navigation').length,
    manifest_artists: new Set(events.map(e => e.artist.toLowerCase())).size,
    manifest_venues: new Set(events.filter(e => e.venue).map(e => e.site_host + ':' + e.venue + ':' + (e.city || ''))).size,
    country_sites: new Set(events.map(e => e.site_host)).size, states: counts,
    displayed_category_rows: events.reduce((n, e) => n + (e.observation?.ticket_categories?.length || 0), 0),
    observed_offer_rows: events.reduce((n, e) => n + (e.observation?.offers?.length || 0), 0),
    remaining_stock_total: null };
}
async function runCoverage(input, config, dependencies = {}) {
  const selected = targets(input, config.limit);
  const clock = dependencies.clock || Date.now;
  const sleep = dependencies.sleep || (ms => new Promise(resolve => setTimeout(resolve, ms)));
  const fetch = dependencies.capture || capture;
  const parse = dependencies.parse || ((target, file) => parseFile(config.binary, target, file));
  const started = clock(), deadline = started + config.maxMinutes * 60000;
  await privateDirectory(config.captureDir);
  if (!config.resume && !config.replay) {
    try { await fs.lstat(config.output); throw new Error('output_exists'); }
    catch (error) { if (error.code !== 'ENOENT') throw error; }
  }
  const report = { schema_version: 1, started_at: new Date(started).toISOString(), mode: config.replay ? 'offline_replay' : 'production_direct_navigation',
    target_count: selected.length, finished: false, summary: {}, events: [],
    method_notes: ['Artist and venue labels are supplied manifest metadata, not independently verified identities.',
      'Fresh captures use ordinary direct event navigation and recognized display controls; no reservations or purchases.',
      'Normalized observations are parsed from saved HTML with observed_at=0; capture time is recorded separately.',
      'Displayed categories, order capacity and observed offer rows never establish event-wide stock or guaranteed fulfillment.',
      'Access blocks and unsupported layouts are distinct from sold-out results. Host blocks are never retried automatically.'] };
  const hostStops = new Map(), hostFailures = new Map(), lastCompleted = new Map(), existing = new Map(), existingErrors = new Map(), cachedObservations = new Map();
  if (config.resume || config.replay) for (const target of selected) {
    const meta = await cached(config.captureDir, target);
    const failure = await cachedError(config.captureDir, target);
    if (meta && failure) throw new Error('conflicting_cached_capture');
    if (failure) { existingErrors.set(target.id, failure); hostStops.set(target.site_host, 'cached_host_block'); }
    if (meta) {
      existing.set(target.id, meta);
      if (blocked(meta)) hostStops.set(target.site_host, 'cached_host_block');
      else if (meta.status >= 200 && meta.status < 300 && meta.html_bytes) {
        // A parser can recognize protection text that the browser did not.
        // Inspect saved pages before any live request, even if the blocked
        // target appears later in a reordered resume manifest.
        try {
          const value = await parse(target, paths(config.captureDir, target).html);
          cachedObservations.set(target.id, { value });
          if (value.availability === 'blocked') hostStops.set(target.site_host, 'cached_parser_access_block');
        } catch { cachedObservations.set(target.id, { error: true }); }
      }
    }
  }
  let browser, context, browserUnavailable = false;
  const close = async () => { await context?.close().catch(() => {}); await browser?.close().catch(() => {}); };
  const onAbort = () => { void close(); };
  dependencies.signal?.addEventListener('abort', onAbort, { once: true });
  try {
    for (const target of selected) {
      const event = { ...target, source: existing.has(target.id) ? 'saved_capture' : existingErrors.has(target.id) ? 'saved_diagnostic' : 'not_attempted', state: 'not_attempted' };
      const files = paths(config.captureDir, target);
      let meta = existing.get(target.id);
      const priorError = existingErrors.get(target.id);
      if (priorError) { event.state = 'blocked'; event.reason = priorError.code; event.diagnostic = priorError; }
      if (!meta && !priorError) {
        if (config.replay) event.reason = 'capture_missing';
        else if (dependencies.signal?.aborted) event.reason = 'cancelled';
        else if (await exists(files.html) || await exists(files.meta) || await exists(files.error)) event.reason = 'existing_capture_requires_review';
        else if (browserUnavailable) event.reason = 'browser_unavailable';
        else if (hostStops.has(target.site_host)) event.reason = hostStops.get(target.site_host);
        else {
          const wait = Math.max(0, (lastCompleted.get(target.site_host) || 0) + config.hostGapMs - clock());
          if (clock() + wait + 1000 >= deadline) event.reason = 'run_deadline';
          else {
            if (wait) await sleep(wait);
            if (!dependencies.signal?.aborted) {
              event.source = 'live_direct_navigation';
              const began = clock();
              try {
                if (!context) {
                  if (dependencies.createBrowser) ({ browser, context } = await dependencies.createBrowser());
                  else {
                    const { chromium } = require('playwright');
                    browser = await chromium.launch({ headless: !config.headed, env: safeEnv(), timeout: Math.min(30000, deadline - clock()),
                      ...(process.env.CHROMIUM_PATH ? { executablePath: process.env.CHROMIUM_PATH } : {}),
                      ...(proxyOptions() ? { proxy: proxyOptions() } : {}) });
                    context = await browser.newContext({ serviceWorkers: 'block' });
                  }
                }
                const result = await fetch(context, { id: report.events.length + 1, url: target.event_url,
                  timeout_ms: Math.max(1, Math.min(config.timeoutMs, deadline - clock())), render_wait_ms: config.renderWaitMs,
                  max_body_bytes: MAX_HTML, adapter: 'ticketmaster' });
                if (result?.error) throw new Error(transportCode(result.error));
                meta = metadata(target, result, clock());
                hostFailures.set(target.site_host, 0);
                await fs.writeFile(files.html, result.html || '', { mode: 0o600, flag: 'wx' });
                await fs.writeFile(files.meta, JSON.stringify(meta, null, 2) + '\n', { mode: 0o600, flag: 'wx' });
                if (blocked(meta)) hostStops.set(target.site_host, 'host_blocked_after_prior_probe');
              } catch (error) {
                event.state = 'transport_error';
                event.reason = dependencies.signal?.aborted ? 'cancelled'
                  : transportCode(error.message);
                if (event.reason === 'queue_redirect') {
                  event.state = 'blocked';
                  hostStops.set(target.site_host, 'host_blocked_after_prior_probe');
                  event.diagnostic = { code: 'queue_redirect', observed_at: new Date(clock()).toISOString() };
                  try {
                    await fs.writeFile(files.error, JSON.stringify({ schema_version: 1, event_url: target.event_url,
                      ...event.diagnostic }, null, 2) + '\n', { mode: 0o600, flag: 'wx' });
                  } catch { event.diagnostic_persisted = false; }
                }
                if (!context) browserUnavailable = true;
                if (['navigation_failed', 'navigation_timeout'].includes(event.reason)) {
                  const failures = (hostFailures.get(target.site_host) || 0) + 1;
                  hostFailures.set(target.site_host, failures);
                  if (failures >= 3) hostStops.set(target.site_host, 'repeated_host_transport_failures');
                }
                event.elapsed_ms = Math.max(0, clock() - began);
              } finally { lastCompleted.set(target.site_host, clock()); }
            } else event.reason = 'cancelled';
          }
        }
      }
      if (meta) {
        event.capture = { ...meta, html_file: target.id + '.html' };
        event.event_identity_matched = true;
        if (blocked(meta)) { event.state = 'blocked'; event.reason = meta.manual_intervention || meta.adapter_reason || 'http_access_blocked'; }
        else if (meta.status < 200 || meta.status >= 300) { event.state = 'http_error'; event.reason = 'http_error_response'; }
        else if (!meta.html_bytes) { event.state = 'no_ticket_details'; event.reason = 'empty_capture'; }
        else {
          try {
            const saved = cachedObservations.get(target.id);
            if (saved?.error) throw new Error('offline_parser_failed');
            event.observation = saved?.value || await parse(target, files.html);
            if (event.observation.availability === 'blocked') {
              event.state = 'blocked';
              event.reason = 'parsed_access_block';
              hostStops.set(target.site_host, 'host_blocked_after_prior_probe');
            } else event.state = event.observation.offers?.length || event.observation.ticket_categories?.length ? 'parsed_ticket_details'
              : /unsupported/.test(event.observation.adapter || '') ? 'unsupported_layout' : 'no_ticket_details';
          } catch { event.state = 'parser_error'; event.reason = 'offline_parser_failed'; }
        }
      }
      report.events.push(event); report.summary = summarize(report.events);
      await atomicReport(config.output, report);
      dependencies.progress?.({ id: target.id, state: event.state, reason: event.reason || null, completed: report.events.length, total: selected.length });
    }
    report.finished = true; report.finished_at = new Date(clock()).toISOString();
    await atomicReport(config.output, report);
    return report;
  } finally {
    dependencies.signal?.removeEventListener('abort', onAbort);
    await close();
  }
}
async function main(argv) {
  const config = options(argv);
  if (config.help) { process.stdout.write(HELP); return; }
  const controller = new AbortController();
  const cancel = () => controller.abort();
  process.once('SIGTERM', cancel); process.once('SIGINT', cancel);
  try {
    const report = await runCoverage(await readJson(config.input), config, { signal: controller.signal,
      progress: record => process.stderr.write(JSON.stringify(record) + '\n') });
    process.stdout.write(JSON.stringify(report.summary, null, 2) + '\n');
    if (controller.signal.aborted) process.exitCode = 130;
  } finally { process.removeListener('SIGTERM', cancel); process.removeListener('SIGINT', cancel); }
}
module.exports = { options, targets, metadata, blocked, summarize, parseFile, runCoverage };
if (require.main === module) main(process.argv.slice(2)).catch(error => {
  process.stderr.write('Coverage audit failed: ' + (code(error.message) || 'audit_failed') + '\n'); process.exitCode = 1;
});
