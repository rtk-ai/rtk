import { STORAGE_KEYS } from "../shared/constants.js";
import { createDefaultWindowState, normalizeWindowState } from "../shared/settings.js";
import { normalizeShortcutList } from "../shared/shortcuts.js";

const DEFAULT_STATE = {
  [STORAGE_KEYS.SHORTCUTS]: [],
  [STORAGE_KEYS.WINDOW_STATE]: createDefaultWindowState()
};

function getDefaultStorageArea() {
  return chrome.storage.local;
}

export async function getExtensionState(storageArea = getDefaultStorageArea()) {
  const data = await storageArea.get(DEFAULT_STATE);
  return {
    shortcuts: normalizeShortcutList(data[STORAGE_KEYS.SHORTCUTS]),
    windowState: normalizeWindowState(data[STORAGE_KEYS.WINDOW_STATE])
  };
}

export async function setShortcuts(shortcuts, storageArea = getDefaultStorageArea()) {
  const normalized = normalizeShortcutList(shortcuts);
  await storageArea.set({ [STORAGE_KEYS.SHORTCUTS]: normalized });
  return normalized;
}

export async function setWindowState(windowState, storageArea = getDefaultStorageArea()) {
  const normalized = normalizeWindowState(windowState);
  await storageArea.set({ [STORAGE_KEYS.WINDOW_STATE]: normalized });
  return normalized;
}

export async function updateWindowState(updater, storageArea = getDefaultStorageArea()) {
  const state = await getExtensionState(storageArea);
  const nextWindowState = normalizeWindowState(updater(state.windowState));
  await setWindowState(nextWindowState, storageArea);
  return nextWindowState;
}
