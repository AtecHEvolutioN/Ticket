const SNOWFLAKE = /^[1-9][0-9]{0,19}$/;
const MAX_SNOWFLAKE = (1n << 64n) - 1n;

export function isSnowflake(value) {
  return typeof value === 'string' && SNOWFLAKE.test(value) && BigInt(value) <= MAX_SNOWFLAKE;
}

// Read on each request so tests and deployments cannot reuse stale configuration.
// The public health route never returns this object or its validation details.
export function readConfig(env = process.env) {
  const publicKey = env.DISCORD_PUBLIC_KEY;
  const applicationId = env.DISCORD_APPLICATION_ID;
  const guildId = env.DISCORD_GUILD_ID;
  const users = env.DISCORD_ALLOWED_USER_IDS;
  const allowedUserIds = typeof users === 'string' && users.length <= 2100
    ? users.split(',').map(value => value.trim())
    : [];
  const signatureReady = typeof publicKey === 'string' && /^[0-9a-fA-F]{64}$/.test(publicKey)
    && isSnowflake(applicationId);
  const commandsReady = signatureReady && isSnowflake(guildId)
    && allowedUserIds.length > 0 && allowedUserIds.length <= 100
    && allowedUserIds.every(isSnowflake);
  return { publicKey, applicationId, guildId, allowedUserIds, signatureReady, commandsReady };
}
