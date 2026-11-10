import { readConfig } from '../lib/config.js';
import { json } from '../lib/interactions.js';

export function GET() {
  const configured = readConfig().commandsReady;
  return json({
    service: 'ticket-discord-commands',
    status: configured ? 'configured' : 'needs_setup',
    watcher_status: 'unverified',
  }, configured ? 200 : 503);
}
