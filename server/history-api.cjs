#!/usr/bin/env node
'use strict';

// Private read-only bridge from the persistent SQLite watcher to the chart UI.
// No provider credentials, ticket requests, Discord delivery or SQLite writes.
const http = require('node:http');
const crypto = require('node:crypto');
const fs = require('node:fs/promises');
const { constants } = require('node:fs');
const path = require('node:path');
const os = require('node:os');
const { spawn } = require('node:child_process');

const MAX_BYTES = 16 * 1024 * 1024;
const MAX_WINDOW_MS = 366 * 86400_000;
const DEFAULT_WINDOW_MS = 48 * 3600_000;
const PUBLIC_HOSTS = new Set(['pl', 'cz', 'com', 'co.uk', 'de', 'at', 'ch', 'be', 'nl', 'ie', 'es', 'it', 'fr', 'dk', 'no', 'se', 'fi', 'ca', 'com.au', 'co.nz', 'com.mx', 'ae', 'co.za'].map(tld => `ticketmaster.${tld}`));

class ApiError extends Error {
  constructor(status, code, message) { super(message); this.status = status; this.code = code; }
}
const invalid = () => new ApiError(400, 'invalid_request', 'Use only event_url, from and to with a valid public event URL and a time range of at most 366 days.');
const unavailable = () => new ApiError(503, 'history_unavailable', 'Saved history is unavailable. Check the worker database and API service configuration.');

function configFromEnv(env = process.env) {
  function array(name, fallback) {
    try {
      const result = env[name] === undefined ? fallback : JSON.parse(env[name]);
      if (!Array.isArray(result) || result.some(item => typeof item !== 'string')) throw new Error();
      return result;
    } catch { throw new Error(`${name} must be a JSON array of strings`); }
  }
  const config = {
    token: env.TICKET_HISTORY_TOKEN || '',
    binary: env.TICKET_HISTORY_BINARY || '/opt/ticket-watch/target/release/ticket-watch',
    databases: array('TICKET_HISTORY_DATABASES', ['/var/lib/ticket-watch-nl/ticket.sqlite']),
    allowedOrigins: array('TICKET_HISTORY_ALLOWED_ORIGINS', ['https://ticket-discord.vercel.app']),
    host: env.TICKET_HISTORY_HOST || '127.0.0.1',
    port: Number(env.TICKET_HISTORY_PORT || '8787'),
    timeoutMs: 30_000,
    minIntervalMs: 5_000,
  };
  validateConfig(config);
  return config;
}

function validateConfig(config) {
  if (typeof config.token !== 'string' || !/^[\x21-\x7e]{32,512}$/.test(config.token)) {
    throw new Error('TICKET_HISTORY_TOKEN must be a privately generated token of 32–512 printable characters without spaces');
  }
  if (!['127.0.0.1', '::1'].includes(config.host)) throw new Error('History API must bind to loopback; use an HTTPS reverse proxy for remote clients');
  if (!Number.isInteger(config.port) || config.port < 0 || config.port > 65535) throw new Error('Invalid history API port');
  if (typeof config.binary !== 'string' || !path.isAbsolute(config.binary)) throw new Error('History binary must be an absolute path');
  if (!Array.isArray(config.databases) || !config.databases.length || config.databases.length > 8 || new Set(config.databases).size !== config.databases.length || config.databases.some(p => typeof p !== 'string' || !path.isAbsolute(p))) {
    throw new Error('History databases must be one to eight distinct absolute paths');
  }
  if (!Array.isArray(config.allowedOrigins) || config.allowedOrigins.length > 16) throw new Error('Invalid history origin allowlist');
  for (const origin of config.allowedOrigins) {
    let url;
    try { url = new URL(origin); } catch { throw new Error('Invalid history origin allowlist'); }
    const local = ['localhost', '127.0.0.1', '[::1]'].includes(url.hostname);
    if (url.origin !== origin || url.username || url.password || !(url.protocol === 'https:' || (local && url.protocol === 'http:'))) throw new Error('History origins must be exact HTTPS origins (HTTP only for loopback development)');
  }
  if (!Number.isFinite(config.timeoutMs) || config.timeoutMs < 1 || config.timeoutMs > 60_000 || !Number.isFinite(config.minIntervalMs) || config.minIntervalMs < 0) throw new Error('Invalid history resource limits');
}

function parseQuery(url, nowMs) {
  for (const key of url.searchParams.keys()) {
    if (!['event_url', 'from', 'to'].includes(key) || url.searchParams.getAll(key).length !== 1) throw invalid();
  }
  function timestamp(value, fallback) {
    if (value === null) return fallback;
    // Explicit timezone and full time required; never accept Date.parse's local/date-only shortcuts.
    const parts = /^(\d{4})-(\d{2})-(\d{2})T(\d{2}):(\d{2}):(\d{2})(?:\.\d{1,3})?(?:Z|[+-](\d{2}):(\d{2}))$/.exec(value);
    if (!parts) throw invalid();
    const [, year, month, day, hour, minute, second] = parts.map((part, index) => index ? Number(part) : part);
    const offsetHour = Number(parts[7] || 0);
    const offsetMinute = Number(parts[8] || 0);
    const days = new Date(Date.UTC(year, month, 0)).getUTCDate();
    if (month < 1 || month > 12 || day < 1 || day > days || hour > 23 || minute > 59 || second > 59 || offsetHour > 23 || offsetMinute > 59) throw invalid();
    const parsed = Date.parse(value);
    if (!Number.isFinite(parsed) || parsed < 0) throw invalid();
    return parsed;
  }
  const to = timestamp(url.searchParams.get('to'), nowMs);
  const from = timestamp(url.searchParams.get('from'), Math.max(0, to - DEFAULT_WINDOW_MS));
  if (from > to || to - from > MAX_WINDOW_MS || to > nowMs + 60_000) throw invalid();
  const eventUrl = url.searchParams.get('event_url');
  if (eventUrl !== null) {
    let event;
    try { event = new URL(eventUrl); } catch { throw invalid(); }
    if (eventUrl.length > 2048 || eventUrl.trim() !== eventUrl || /[\x00-\x20\\]/.test(eventUrl) || event.protocol !== 'https:' || !PUBLIC_HOSTS.has(event.hostname.replace(/^www\./, '')) || event.username || event.password || event.port || event.hash || event.search || !event.pathname.startsWith('/event/')) throw invalid();
  }
  return { from: new Date(from).toISOString(), to: new Date(to).toISOString(), eventUrl };
}

async function checkFile(filename, executable = false) {
  // Explicit configured paths only. Reject symlinks, missing files and directories.
  const info = await fs.lstat(filename);
  if (!info.isFile() || info.isSymbolicLink() || await fs.realpath(filename) !== path.resolve(filename)) throw unavailable();
  await fs.access(filename, executable ? constants.R_OK | constants.X_OK : constants.R_OK);
}

function runExport(binary, args, directory, timeoutMs, signal) {
  return new Promise((resolve, reject) => {
    let stderr = '';
    let outputBytes = 0;
    let failure;
    const child = spawn(binary, args, { cwd: directory, env: { PATH: '/usr/bin:/bin', LANG: 'C.UTF-8', HOME: directory }, stdio: ['ignore', 'pipe', 'pipe'], windowsHide: true });
    const stop = error => { failure ||= error; child.kill('SIGKILL'); };
    const timer = setTimeout(() => stop(new ApiError(504, 'export_timeout', 'History export timed out. Select a smaller time range or one event.')), timeoutMs);
    const abort = () => stop(new ApiError(499, 'client_closed', 'Client disconnected.'));
    signal?.addEventListener('abort', abort, { once: true });
    if (signal?.aborted) abort();
    for (const stream of [child.stdout, child.stderr]) stream.on('data', chunk => {
      outputBytes += chunk.length;
      if (outputBytes > 64 * 1024) stop(unavailable());
    });
    child.stderr.on('data', chunk => { if (stderr.length < 8192) stderr += chunk.toString('utf8').slice(0, 8192 - stderr.length); });
    child.on('error', () => { failure ||= unavailable(); });
    child.on('close', code => {
      clearTimeout(timer);
      signal?.removeEventListener('abort', abort);
      if (failure) return reject(failure);
      if (code === 0) return resolve();
      if (stderr.includes('No saved history matches this event and time range')) return reject(new ApiError(404, 'no_history', 'No observations match this event and time range.'));
      if (/History (?:export )?exceeds|Combined history exceeds|History JSON exceeds/.test(stderr)) return reject(new ApiError(413, 'history_too_large', 'History exceeds the export limit. Select one event or a smaller time range.'));
      reject(unavailable()); // Never return raw stderr: paths and stored data may be private.
    });
  });
}

async function exportHistory(config, query, signal) {
  try {
    await checkFile(config.binary, true);
    for (const database of config.databases) await checkFile(database);
  } catch { throw unavailable(); }
  const directory = await fs.mkdtemp(path.join(os.tmpdir(), 'ticket-history-api-'));
  await fs.chmod(directory, 0o700);
  try {
    const output = path.join(directory, 'history.json');
    const args = ['history-export'];
    for (const database of config.databases) args.push('--database', database);
    args.push('--output', output, '--from', query.from, '--to', query.to);
    if (query.eventUrl !== null) args.push('--event-url', query.eventUrl);
    await runExport(config.binary, args, directory, config.timeoutMs, signal);
    const info = await fs.lstat(output);
    if (!info.isFile() || info.isSymbolicLink()) throw unavailable();
    if (info.size > MAX_BYTES) throw new ApiError(413, 'history_too_large', 'History exceeds the export limit. Select one event or a smaller time range.');
    return await fs.readFile(output);
  } finally { await fs.rm(directory, { recursive: true, force: true }); }
}

function summarize(body) {
  if (!Buffer.isBuffer(body) || body.length > MAX_BYTES) throw unavailable();
  let history;
  try { history = JSON.parse(body); } catch { throw unavailable(); }
  if (history.schema_version !== 1 || !Number.isSafeInteger(history.generated_at) || !Array.isArray(history.events)) throw unavailable();
  let latest = null;
  let samples = 0;
  for (const event of history.events) {
    if (!Array.isArray(event.sources)) throw unavailable();
    for (const source of event.sources) {
      if (!Array.isArray(source.samples)) throw unavailable();
      for (const sample of source.samples) {
        if (!Number.isSafeInteger(sample.observed_at) || sample.observed_at < 0) throw unavailable();
        latest = Math.max(latest ?? 0, sample.observed_at);
        samples++;
      }
    }
  }
  return { events: history.events.length, samples, latest_observed_at: latest, generated_at: history.generated_at };
}

function createHistoryServer(config, { exporter = exportHistory, clock = Date.now } = {}) {
  validateConfig(config);
  const expected = crypto.createHash('sha256').update(config.token).digest();
  const origins = new Set(config.allowedOrigins);
  let active = false;
  let nextExportAt = 0;
  let lastExport = null;
  let lastExportAt = null;
  let lastError = null;
  const server = http.createServer({ maxHeaderSize: 8192, requestTimeout: 10_000, headersTimeout: 5000, keepAliveTimeout: 5000 }, async (req, res) => {
    res.setHeader('Cache-Control', 'no-store');
    res.setHeader('Pragma', 'no-cache');
    res.setHeader('X-Content-Type-Options', 'nosniff');
    res.setHeader('Vary', 'Origin');
    res.setHeader('Referrer-Policy', 'no-referrer');
    function reply(status, data) {
      if (res.destroyed || res.writableEnded) return;
      res.writeHead(status, { 'Content-Type': 'application/json; charset=utf-8' });
      res.end(Buffer.isBuffer(data) ? data : JSON.stringify(data));
    }
    const controller = new AbortController();
    res.on('close', () => { if (!res.writableEnded) controller.abort(); });
    try {
      const origin = req.headers.origin;
      if (origin !== undefined) {
        if (typeof origin !== 'string' || !origins.has(origin)) throw new ApiError(403, 'origin_denied', 'This chart origin is not allowed.');
        res.setHeader('Access-Control-Allow-Origin', origin);
        res.setHeader('Access-Control-Expose-Headers', 'Retry-After');
      }
      if (!req.url || req.url.length > 4096 || !req.url.startsWith('/') || req.url.startsWith('//') || req.url.includes('#')) throw invalid();
      const url = new URL(req.url, 'http://127.0.0.1');
      if (!['/api/history', '/api/health'].includes(url.pathname)) throw new ApiError(404, 'not_found', 'Unknown history API route.');
      if (req.method === 'OPTIONS') {
        const headers = String(req.headers['access-control-request-headers'] || '').split(',').map(h => h.trim().toLowerCase()).filter(Boolean);
        if (!origin || req.headers['access-control-request-method'] !== 'GET' || headers.some(header => header !== 'authorization')) throw new ApiError(403, 'origin_denied', 'Preflight request is not allowed.');
        res.writeHead(204, { 'Access-Control-Allow-Methods': 'GET', 'Access-Control-Allow-Headers': 'Authorization', 'Access-Control-Max-Age': '300' });
        return res.end();
      }
      const header = req.headers.authorization;
      const duplicateAuthorization = req.rawHeaders.filter((_, index) => index % 2 === 0).filter(name => name.toLowerCase() === 'authorization').length !== 1;
      const supplied = typeof header === 'string' && header.startsWith('Bearer ') && header.length <= 519 ? header.slice(7) : '';
      if (duplicateAuthorization || !crypto.timingSafeEqual(expected, crypto.createHash('sha256').update(supplied).digest())) {
        res.setHeader('WWW-Authenticate', 'Bearer realm="ticket-history"');
        throw new ApiError(401, 'unauthorized', 'A valid read-only history token is required.');
      }
      if (req.method !== 'GET') { res.setHeader('Allow', 'GET, OPTIONS'); throw new ApiError(405, 'method_not_allowed', 'This API is read-only.'); }
      if (req.headers['transfer-encoding'] || (req.headers['content-length'] && req.headers['content-length'] !== '0')) {
        res.setHeader('Connection', 'close');
        throw invalid();
      }
      if (url.pathname === '/api/health') {
        if (url.search) throw invalid();
        const observationTime = lastExport?.latest_observed_at;
        const now = Math.floor(clock() / 1000);
        const clockInvalid = observationTime != null && observationTime > now;
        return reply(200, {
          service: 'ticket-history-api', status: lastError ? 'export_error' : lastExport ? 'serving_history' : 'awaiting_first_export', watcher_status: 'unverified',
          last_export_at: lastExportAt, last_export: lastExport,
          latest_observation_age_secs: observationTime == null || clockInvalid ? null : now - observationTime,
          observation_clock_invalid: clockInvalid,
          observation_age_measures: 'any_observation_including_blocked_or_unknown',
          observation_scope: 'last_successful_filtered_history_request', last_error: lastError,
        });
      }
      const query = parseQuery(url, clock());
      const wait = nextExportAt - clock();
      if (active || wait > 0) {
        res.setHeader('Retry-After', String(Math.max(1, Math.ceil(wait / 1000), active ? 5 : 0)));
        throw new ApiError(429, 'export_busy', 'History export is busy. Retry after the indicated delay.');
      }
      active = true;
      nextExportAt = clock() + config.minIntervalMs;
      try {
        const body = await exporter(config, query, controller.signal);
        const summary = summarize(body);
        lastExport = summary;
        lastExportAt = Math.floor(clock() / 1000);
        lastError = null;
        reply(200, body);
      } catch (error) {
        lastError = error instanceof ApiError ? error.code : 'history_unavailable';
        throw error;
      } finally { active = false; }
    } catch (error) {
      const safe = error instanceof ApiError ? error : unavailable();
      if (safe.status !== 499) reply(safe.status, { error: { code: safe.code, message: safe.message } });
    }
  });
  server.maxConnections = 32;
  return server;
}

if (require.main === module) {
  try {
    const config = configFromEnv();
    const server = createHistoryServer(config);
    server.on('error', () => { console.error('History API could not listen; check its loopback address and port.'); process.exitCode = 1; });
    server.listen(config.port, config.host, () => console.log('Read-only history API listening on loopback. Use an HTTPS reverse proxy for remote charts.'));
    const stop = () => { server.close(); server.closeAllConnections(); };
    process.once('SIGTERM', stop);
    process.once('SIGINT', stop);
  } catch (error) {
    console.error(error.message);
    process.exitCode = 1;
  }
}

module.exports = { ApiError, MAX_BYTES, configFromEnv, createHistoryServer, exportHistory, parseQuery, summarize };
