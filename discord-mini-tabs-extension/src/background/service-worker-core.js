import { MESSAGE_TYPES } from "../shared/constants.js";

function getSuggestedName(title) {
  const cleanedTitle = String(title ?? "")
    .replace(/\s*(?:-|[|])\s*Discord$/i, "")
    .trim();
  return cleanedTitle || "Discord channel";
}

export function createServiceWorkerController({
  chromeApi,
  getExtensionState,
  setShortcuts,
  setWindowState,
  createShortcut,
  deleteShortcut,
  findShortcut,
  updateShortcut,
  validateDiscordChannelUrl,
  closeMiniWindow,
  focusMiniWindow,
  openShortcutInMiniWindow,
  resetMiniWindowPosition,
  saveBoundsFromWindow,
  updateMiniWindowSettings,
  setTimeoutFn,
  clearTimeoutFn,
  debounceMs = 400
}) {
  const boundsSaveTimers = new Map();
  const boundsSaveTokens = new Map();
  let shortcutMutationQueue = Promise.resolve();

  function enqueueShortcutMutation(operation) {
    const next = shortcutMutationQueue.catch(() => undefined).then(operation);
    shortcutMutationQueue = next.catch(() => undefined);
    return next;
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

    const windowState = await openShortcutInMiniWindow({ chromeApi, shortcut });
    return { windowState };
  }

  async function handleReadActiveDiscordTab() {
    const tabs = await chromeApi.tabs.query({ active: true, currentWindow: true });
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
        return enqueueShortcutMutation(() => handleCreateShortcut(payload));
      case MESSAGE_TYPES.UPDATE_SHORTCUT:
        return enqueueShortcutMutation(() => handleUpdateShortcut(payload));
      case MESSAGE_TYPES.DELETE_SHORTCUT:
        return enqueueShortcutMutation(() => handleDeleteShortcut(payload));
      case MESSAGE_TYPES.OPEN_SHORTCUT:
        return handleOpenShortcut(payload);
      case MESSAGE_TYPES.READ_ACTIVE_DISCORD_TAB:
        return handleReadActiveDiscordTab();
      case MESSAGE_TYPES.UPDATE_WINDOW_SETTINGS:
        return {
          windowState: await updateMiniWindowSettings({
            chromeApi,
            bounds: payload.bounds,
            zoom: payload.zoom
          })
        };
      case MESSAGE_TYPES.FOCUS_WINDOW:
        return { windowId: await focusMiniWindow({ chromeApi }) };
      case MESSAGE_TYPES.CLOSE_WINDOW:
        return { windowState: await closeMiniWindow({ chromeApi }) };
      case MESSAGE_TYPES.RESET_POSITION:
        return { windowState: await resetMiniWindowPosition({ chromeApi }) };
      default:
        throw new Error("Unsupported message type.");
    }
  }

  async function handleWindowRemoved(windowId) {
    try {
      const state = await getExtensionState();
      if (state.windowState.windowId === windowId) {
        await setWindowState({ ...state.windowState, windowId: null, tabId: null });
      }
    } catch {
      // Background window events must not surface unhandled storage failures.
    }
  }

  function handleBoundsChanged(changedWindow) {
    if (!changedWindow || changedWindow.type !== "popup" || !Number.isInteger(changedWindow.id)) {
      return Promise.resolve();
    }

    const windowId = changedWindow.id;
    const token = Symbol("bounds-save");
    boundsSaveTokens.set(windowId, token);
    clearTimeoutFn(boundsSaveTimers.get(windowId));
    const timerId = setTimeoutFn(async () => {
      if (boundsSaveTimers.get(windowId) === timerId) {
        boundsSaveTimers.delete(windowId);
      }
      if (boundsSaveTokens.get(windowId) !== token) {
        return;
      }

      try {
        const state = await getExtensionState();
        if (boundsSaveTokens.get(windowId) !== token) {
          return;
        }

        await saveBoundsFromWindow(changedWindow, {
          expectedWindowId: state.windowState.windowId,
          shouldSave: () => boundsSaveTokens.get(windowId) === token
        });
      } catch {
        // Background window events must not surface unhandled storage failures.
      } finally {
        if (boundsSaveTokens.get(windowId) === token) {
          boundsSaveTokens.delete(windowId);
        }
      }
    }, debounceMs);
    boundsSaveTimers.set(windowId, timerId);
    return Promise.resolve();
  }

  return {
    handleMessage,
    handleWindowRemoved,
    handleBoundsChanged
  };
}
