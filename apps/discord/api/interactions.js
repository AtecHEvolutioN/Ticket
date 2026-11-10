import { handleInteraction } from '../lib/interactions.js';

export async function POST(request) {
  try {
    return await handleInteraction(request);
  } catch {
    // Fixed diagnostic only: Discord tokens and signed bodies must not reach logs.
    console.error('discord_interaction_failed');
    return Response.json({ error: 'request_failed' }, {
      status: 500,
      headers: { 'Cache-Control': 'no-store' },
    });
  }
}
