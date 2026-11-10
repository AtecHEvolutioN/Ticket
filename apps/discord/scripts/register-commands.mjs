import { pathToFileURL } from 'node:url';
import { ticketCommand } from '../lib/commands.js';
import { isSnowflake } from '../lib/config.js';

const API = 'https://discord.com/api/v10';
const MAX_BODY_BYTES = 256 * 1024;

function snowflake(value, name) {
  if (!isSnowflake(value)) {
    throw new Error(`${name} must be a positive Discord numeric ID.`);
  }
  return value;
}

export function registrationPlan(env) {
  const applicationId = snowflake(env.DISCORD_APPLICATION_ID, 'DISCORD_APPLICATION_ID');
  const guildId = snowflake(env.DISCORD_GUILD_ID, 'DISCORD_GUILD_ID');
  return {
    application_id: applicationId,
    guild_id: guildId,
    command: ticketCommand,
    operation: 'Create or update only the guild-scoped /ticket command. Other commands are preserved.',
  };
}

async function readJson(response) {
  if (Number(response.headers.get('content-length')) > MAX_BODY_BYTES) {
    await response.body?.cancel().catch(() => {});
    throw new Error('Discord returned an oversized response. Registration was not confirmed.');
  }
  const chunks = [];
  let size = 0;
  if (!response.body) throw new Error('Discord returned an empty response.');
  try {
    for await (const chunk of response.body) {
      size += chunk.byteLength;
      if (size > MAX_BODY_BYTES) throw new Error('Response too large');
      chunks.push(Buffer.from(chunk));
    }
  } catch {
    throw new Error('Discord response could not be read within size limits. Registration was not confirmed.');
  }
  try {
    return JSON.parse(Buffer.concat(chunks).toString('utf8'));
  } catch {
    throw new Error('Discord returned an invalid response. Registration was not confirmed.');
  }
}

export async function registerCommands(env, fetchImpl = fetch) {
  const plan = registrationPlan(env);
  const token = env.DISCORD_BOT_TOKEN?.trim();
  if (!token || /[^\x21-\x7e]/.test(token)) {
    throw new Error('Set DISCORD_BOT_TOKEN privately to register commands.');
  }
  async function request(path, method, body) {
    let response;
    try {
      response = await fetchImpl(`${API}${path}`, {
        method,
        headers: { authorization: `Bot ${token}`, 'content-type': 'application/json' },
        body: body === undefined ? undefined : JSON.stringify(body),
        redirect: 'error',
        signal: AbortSignal.timeout(20_000),
      });
    } catch {
      throw new Error(method === 'POST'
        ? 'Discord registration result is uncertain after a transport failure. Check the server integration before retrying.'
        : 'Unable to verify the bot application with Discord. No command was registered.');
    }
    if (!response.ok) {
      await response.body?.cancel().catch(() => {});
      const reason = {
        401: 'Discord rejected the bot token.',
        403: 'The bot lacks access to this application or server. Check its installation.',
        404: 'Discord could not find this application or server.',
        429: 'Discord rate limited setup. Wait before retrying; no automatic retry was made.',
      }[response.status] ?? 'Discord did not confirm this setup request.';
      throw new Error(`${reason} (HTTP ${response.status})`);
    }
    try {
      return await readJson(response);
    } catch {
      throw new Error(method === 'POST'
        ? 'Discord registration result is uncertain because its response could not be verified. Check the server integration before retrying.'
        : 'Discord application response could not be verified. No command was registered.');
    }
  }
  const application = await request('/oauth2/applications/@me', 'GET');
  if (application.id !== plan.application_id) {
    throw new Error('DISCORD_BOT_TOKEN belongs to a different application. No command was registered.');
  }
  const result = await request(`/applications/${plan.application_id}/guilds/${plan.guild_id}/commands`, 'POST', plan.command);
  if (result.application_id !== plan.application_id || result.guild_id !== plan.guild_id || result.name !== ticketCommand.name) {
    throw new Error('Discord returned unexpected command identifiers. Registration was not confirmed.');
  }
  const commandId = snowflake(result.id, 'Returned command ID');
  return { registered: true, application_id: plan.application_id, guild_id: plan.guild_id, command_id: commandId, command: ticketCommand.name };
}

export async function main(args = process.argv.slice(2), env = process.env) {
  if (args.length > 1 || (args.length === 1 && args[0] !== '--apply')) {
    throw new Error('Usage: node scripts/register-commands.mjs [--apply]. Default: offline preview.');
  }
  const result = args[0] === '--apply' ? await registerCommands(env) : { preview: true, ...registrationPlan(env) };
  process.stdout.write(`${JSON.stringify(result, null, 2)}\n`);
}

if (process.argv[1] && import.meta.url === pathToFileURL(process.argv[1]).href) {
  main().catch(error => {
    process.stderr.write(`${error.message}\n`);
    process.exitCode = 1;
  });
}
