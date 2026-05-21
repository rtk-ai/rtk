import { SHORTCUT_TYPES } from "./constants.js";
import { normalizeDiscordChannelUrl } from "./url.js";

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
  return shortcuts.filter((shortcut) => shortcut.id !== id);
}

export function findShortcut(shortcuts, id) {
  return shortcuts.find((shortcut) => shortcut.id === id) ?? null;
}

export function filterShortcuts(shortcuts, query) {
  const normalizedQuery = String(query ?? "").trim().toLowerCase();
  if (!normalizedQuery) {
    return shortcuts;
  }

  return shortcuts.filter((shortcut) => {
    const haystack = `${shortcut.name} ${shortcut.url}`.toLowerCase();
    return haystack.includes(normalizedQuery);
  });
}

export function splitShortcutsByType(shortcuts) {
  return {
    text: shortcuts.filter((shortcut) => shortcut.type === SHORTCUT_TYPES.TEXT),
    voice: shortcuts.filter((shortcut) => shortcut.type === SHORTCUT_TYPES.VOICE)
  };
}
