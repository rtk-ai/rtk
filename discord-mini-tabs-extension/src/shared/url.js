import { DISCORD_CHANNEL_PREFIX, DISCORD_HOST } from "./constants.js";

function parseUrl(input) {
  if (typeof input !== "string" || input.trim().length === 0) {
    return null;
  }

  try {
    return new URL(input.trim());
  } catch {
    return null;
  }
}

function isSnowflake(value) {
  return /^\d+$/.test(value);
}

function getChannelParts(url) {
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts.length !== 3 || parts[0] !== "channels") {
    return null;
  }

  const [, guildOrMe, channelId] = parts;
  if (!guildOrMe || !channelId) {
    return null;
  }

  if (!isSnowflake(channelId) || (guildOrMe !== "@me" && !isSnowflake(guildOrMe))) {
    return null;
  }

  return { guildOrMe, channelId };
}

export function validateDiscordChannelUrl(input) {
  const url = parseUrl(input);
  if (!url) {
    return { ok: false, error: "Enter a valid Discord channel URL." };
  }

  if (url.protocol !== "https:" || url.hostname !== DISCORD_HOST) {
    return { ok: false, error: "Only https://discord.com channel URLs are supported." };
  }

  if (!url.pathname.startsWith(DISCORD_CHANNEL_PREFIX)) {
    return { ok: false, error: "The URL must point to discord.com/channels/..." };
  }

  const parts = getChannelParts(url);
  if (!parts) {
    return { ok: false, error: "The Discord URL must include a server or DM id and channel id." };
  }

  const normalized = `https://${DISCORD_HOST}/channels/${parts.guildOrMe}/${parts.channelId}`;
  return {
    ok: true,
    url: normalized,
    guildOrMe: parts.guildOrMe,
    channelId: parts.channelId,
    scope: parts.guildOrMe === "@me" ? "dm" : "server"
  };
}

export function normalizeDiscordChannelUrl(input) {
  const result = validateDiscordChannelUrl(input);
  if (!result.ok) {
    throw new Error(result.error);
  }
  return result.url;
}

export function compactDiscordUrl(input) {
  const result = validateDiscordChannelUrl(input);
  if (!result.ok) {
    return "";
  }
  const guildLabel = result.guildOrMe === "@me" ? "DM" : result.guildOrMe;
  return `${guildLabel} / ${result.channelId}`;
}
