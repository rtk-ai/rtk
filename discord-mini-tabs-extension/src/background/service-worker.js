import { MESSAGE_TYPES } from "../shared/constants.js";
import { createShortcut, deleteShortcut, findShortcut, updateShortcut } from "../shared/shortcuts.js";
import { validateDiscordChannelUrl } from "../shared/url.js";
import { getExtensionState, setShortcuts, setWindowState } from "./storage.js";
import {
  closeMiniWindow,
  focusMiniWindow,
  openShortcutInMiniWindow,
  resetMiniWindowPosition,
  saveBoundsFromWindow,
  updateMiniWindowSettings
} from "./window-manager.js";

let boundsSaveTimer = null;

function errorMessage(error) {
  return error instanceof Error ? error.message : "Unknown extension error.";
}

function getSuggestedName(title) {
  const cleanedTitle = String(title ?? "")
    .replace(/\s*(?:-|[|])\s*Discord$/i, "")
    .trim();
  return cleanedTitle || "Discord channel";
}

async function handleCreateShortcut(payload) {
  const state = await getExtensionState();
  const shortcut = createShortcut(payload);
  const shortcuts = await setShortcuts([...state.shortcuts, shortcut]);
  return { shortcut, shortcuts };
}

async function handleUpdateShortcut(payload) {
  const state = await getExtensionState();
  const existing = findShortcut(state.shortcuts, payload.id);
  if (!existing) {
    throw new Error("Shortcut not found.");
  }

  const updated = updateShortcut(existing, payload);
  const shortcuts = await setShortcuts(
    state.shortcuts.map((shortcut) => (shortcut.id === updated.id ? updated : shortcut))
  );
  return { shortcut: updated, shortcuts };
}

async function handleDeleteShortcut(payload) {
  const state = await getExtensionState();
  const shortcuts = await setShortcuts(deleteShortcut(state.shortcuts, payload.id));
  return { shortcuts };
}

async function handleOpenShortcut(payload) {
  const state = await getExtensionState();
  const shortcut = findShortcut(state.shortcuts, payload.id);
  if (!shortcut) {
    throw new Error("Shortcut not found.");
  }

  const windowState = await openShortcutInMiniWindow({ shortcut });
  return { windowState };
}

async function handleReadActiveDiscordTab() {
  const tabs = await chrome.tabs.query({ active: true, currentWindow: true });
  const activeTab = tabs[0];
  const result = validateDiscordChannelUrl(activeTab?.url ?? "");
  if (!result.ok) {
    throw new Error("The active tab is not a supported Discord channel URL.");
  }

  return {
    url: result.url,
    title: activeTab.title ?? "",
    suggestedName: getSuggestedName(activeTab.title)
  };
}

async function handleMessage(message) {
  const payload = message?.payload ?? {};

  switch (message?.type) {
    case MESSAGE_TYPES.GET_STATE:
      return getExtensionState();
    case MESSAGE_TYPES.CREATE_SHORTCUT:
      return handleCreateShortcut(payload);
    case MESSAGE_TYPES.UPDATE_SHORTCUT:
      return handleUpdateShortcut(payload);
    case MESSAGE_TYPES.DELETE_SHORTCUT:
      return handleDeleteShortcut(payload);
    case MESSAGE_TYPES.OPEN_SHORTCUT:
      return handleOpenShortcut(payload);
    case MESSAGE_TYPES.READ_ACTIVE_DISCORD_TAB:
      return handleReadActiveDiscordTab();
    case MESSAGE_TYPES.UPDATE_WINDOW_SETTINGS:
      return {
        windowState: await updateMiniWindowSettings({
          bounds: payload.bounds,
          zoom: payload.zoom
        })
      };
    case MESSAGE_TYPES.FOCUS_WINDOW:
      return { windowId: await focusMiniWindow({}) };
    case MESSAGE_TYPES.CLOSE_WINDOW:
      return { windowState: await closeMiniWindow({}) };
    case MESSAGE_TYPES.RESET_POSITION:
      return { windowState: await resetMiniWindowPosition({}) };
    default:
      throw new Error("Unsupported message type.");
  }
}

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  handleMessage(message)
    .then((data) => sendResponse({ ok: true, data }))
    .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
  return true;
});

chrome.windows.onRemoved.addListener(async (windowId) => {
  try {
    const state = await getExtensionState();
    if (state.windowState.windowId === windowId) {
      await setWindowState({ ...state.windowState, windowId: null, tabId: null });
    }
  } catch {
    // Background window events must not surface unhandled storage failures.
  }
});

chrome.windows.onBoundsChanged.addListener((changedWindow) => {
  clearTimeout(boundsSaveTimer);
  boundsSaveTimer = setTimeout(async () => {
    try {
      const state = await getExtensionState();
      await saveBoundsFromWindow(changedWindow, {
        expectedWindowId: state.windowState.windowId
      });
    } catch {
      // Background window events must not surface unhandled storage failures.
    }
  }, 400);
});
