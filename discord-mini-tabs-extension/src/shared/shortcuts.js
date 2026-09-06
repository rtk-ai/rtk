import { SHORTCUT_TYPES } from "./constants.js";
import { normalizeDiscordChannelUrl, validateDiscordChannelUrl } from "./url.js";

const VALID_TYPES = new Set([SHORTCUT_TYPES.TEXT, SHORTCUT_TYPES.VOICE]);

function defaultIdFactory() {
  if (globalThis.crypto?.randomUUID) {
    return globalThis.crypto.randomUUID();
  }
  return `shortcut-${Date.now()}-${Math.random().toString(16).slice(2)}`;
}

function defaultNow() {
  return new Date().toISOString();
}

function normalizeName(name) {
  const value = String(name ?? "").trim();
  if (value.length === 0) {
    throw new Error("Shortcut name is required.");
  }
  return value;
}

function normalizeType(type) {
  if (!VALID_TYPES.has(type)) {
    throw new Error("Shortcut type must be text or voice.");
  }
  return type;
}

function isShortcutLike(shortcut) {
  return (
    shortcut !== null &&
    typeof shortcut === "object" &&
    typeof shortcut.id === "string" &&
    typeof shortcut.name === "string" &&
    typeof shortcut.type === "string" &&
    typeof shortcut.url === "string" &&
    VALID_TYPES.has(shortcut.type) &&
    validateDiscordChannelUrl(shortcut.url).ok
  );
}

export function normalizeShortcutList(shortcuts) {
  if (!Array.isArray(shortcuts)) {
    return [];
  }
  return shortcuts.filter(isShortcutLike);
}

export function createShortcut(input, options = {}) {
  const idFactory = options.idFactory ?? defaultIdFactory;
  const now = options.now ?? defaultNow;
  const timestamp = now();

  return {
    id: idFactory(),
    name: normalizeName(input.name),
    type: normalizeType(input.type),
    url: normalizeDiscordChannelUrl(input.url),
    createdAt: timestamp,
    updatedAt: timestamp
  };
}

export function updateShortcut(existing, input) {
  const now = input.now ?? defaultNow;
  return {
    ...existing,
    name: normalizeName(input.name ?? existing.name),
    type: normalizeType(input.type ?? existing.type),
    url: normalizeDiscordChannelUrl(input.url ?? existing.url),
    updatedAt: now()
  };
}

export function deleteShortcut(shortcuts, id) {
  return normalizeShortcutList(shortcuts).filter((shortcut) => shortcut.id !== id);
}

export function findShortcut(shortcuts, id) {
  return normalizeShortcutList(shortcuts).find((shortcut) => shortcut.id === id) ?? null;
}

export function filterShortcuts(shortcuts, query) {
  const normalizedShortcuts = normalizeShortcutList(shortcuts);
  const normalizedQuery = String(query ?? "").trim().toLowerCase();
  if (!normalizedQuery) {
    return normalizedShortcuts;
  }

  return normalizedShortcuts.filter((shortcut) => {
    const haystack = `${shortcut.name} ${shortcut.url}`.toLowerCase();
    return haystack.includes(normalizedQuery);
  });
}

export function splitShortcutsByType(shortcuts) {
  const normalizedShortcuts = normalizeShortcutList(shortcuts);
  return {
    text: normalizedShortcuts.filter((shortcut) => shortcut.type === SHORTCUT_TYPES.TEXT),
    voice: normalizedShortcuts.filter((shortcut) => shortcut.type === SHORTCUT_TYPES.VOICE)
  };
}
