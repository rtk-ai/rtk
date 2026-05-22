import { DEFAULT_BOUNDS } from "../shared/constants.js";
import { clampBounds, normalizeWindowState } from "../shared/settings.js";
import { getExtensionState, setWindowState, updateWindowState } from "./storage.js";

let openQueue = Promise.resolve();

function getDefaultChromeApi() {
  if (typeof chrome === "undefined") {
    throw new Error("Chrome API is unavailable.");
  }
  return chrome;
}

function enqueueOpen(operation) {
  const next = openQueue.catch(() => undefined).then(operation);
  openQueue = next.catch(() => undefined);
  return next;
}

function boundsToCreateData(bounds, url) {
  const createData = {
    url,
    type: "popup",
    focused: true,
    width: bounds.width,
    height: bounds.height
  };

  if (Number.isInteger(bounds.left)) createData.left = bounds.left;
  if (Number.isInteger(bounds.top)) createData.top = bounds.top;

  return createData;
}

async function getWindowWithTab(chromeApi, windowState) {
  if (!Number.isInteger(windowState.windowId)) {
    return null;
  }

  try {
    const currentWindow = await chromeApi.windows.get(windowState.windowId, { populate: true });
    if (currentWindow.type !== "popup") {
      return null;
    }

    const existingTab =
      currentWindow.tabs?.find((tab) => tab.id === windowState.tabId) ??
      currentWindow.tabs?.[0] ??
      null;
    if (!Number.isInteger(existingTab?.id)) {
      return null;
    }

    return { window: currentWindow, tab: existingTab };
  } catch {
    return null;
  }
}

async function applyZoom(chromeApi, tabId, zoom) {
  try {
    await chromeApi.tabs.setZoom(tabId, zoom);
    return true;
  } catch {
    return false;
  }
}

export async function openShortcutInMiniWindow({
  chromeApi = getDefaultChromeApi(),
  storageArea,
  shortcut
}) {
  return enqueueOpen(() =>
    openShortcutInMiniWindowUnlocked({ chromeApi, storageArea, shortcut })
  );
}

async function openShortcutInMiniWindowUnlocked({ chromeApi, storageArea, shortcut }) {
  const state = await getExtensionState(storageArea);
  const windowState = normalizeWindowState(state.windowState);
  const existing = await getWindowWithTab(chromeApi, windowState);

  if (existing) {
    await chromeApi.windows.update(existing.window.id, { focused: true });
    const tab = await chromeApi.tabs.update(existing.tab.id, {
      url: shortcut.url,
      active: true
    });
    const tabId = Number.isInteger(tab?.id) ? tab.id : existing.tab.id;
    await applyZoom(chromeApi, tabId, windowState.zoom);

    return setWindowState(
      {
        ...windowState,
        windowId: existing.window.id,
        tabId,
        lastShortcutId: shortcut.id
      },
      storageArea
    );
  }

  const bounds = clampBounds(windowState.bounds ?? DEFAULT_BOUNDS);
  const createdWindow = await chromeApi.windows.create(boundsToCreateData(bounds, shortcut.url));
  const createdTab = createdWindow?.tabs?.[0] ?? null;
  if (!Number.isInteger(createdWindow?.id) || !Number.isInteger(createdTab?.id)) {
    throw new Error("Chrome did not return a usable Discord mini window.");
  }

  await applyZoom(chromeApi, createdTab.id, windowState.zoom);

  return setWindowState(
    {
      ...windowState,
      windowId: createdWindow.id,
      tabId: createdTab.id,
      bounds,
      lastShortcutId: shortcut.id
    },
    storageArea
  );
}

export async function focusMiniWindow({ chromeApi = getDefaultChromeApi(), storageArea }) {
  const state = await getExtensionState(storageArea);
  const existing = await getWindowWithTab(chromeApi, state.windowState);
  if (!existing) {
    await setWindowState({ ...state.windowState, windowId: null, tabId: null }, storageArea);
    return null;
  }

  await chromeApi.windows.update(existing.window.id, { focused: true });
  return existing.window.id;
}

export async function closeMiniWindow({ chromeApi = getDefaultChromeApi(), storageArea }) {
  const state = await getExtensionState(storageArea);
  if (Number.isInteger(state.windowState.windowId)) {
    try {
      await chromeApi.windows.remove(state.windowState.windowId);
    } catch {
      // The window may already be closed. Clearing stored ids is still correct.
    }
  }

  return setWindowState({ ...state.windowState, windowId: null, tabId: null }, storageArea);
}

export async function resetMiniWindowPosition({
  chromeApi = getDefaultChromeApi(),
  storageArea
}) {
  const state = await updateWindowState(
    (windowState) => ({
      ...windowState,
      bounds: { ...windowState.bounds, left: null, top: null }
    }),
    storageArea
  );

  if (Number.isInteger(state.windowId)) {
    try {
      await chromeApi.windows.update(state.windowId, {
        width: state.bounds.width,
        height: state.bounds.height
      });
    } catch {
      return setWindowState({ ...state, windowId: null, tabId: null }, storageArea);
    }
  }

  return state;
}

export async function updateMiniWindowSettings({
  chromeApi = getDefaultChromeApi(),
  storageArea,
  bounds,
  zoom
}) {
  const state = await updateWindowState(
    (windowState) => ({
      ...windowState,
      bounds: clampBounds({ ...windowState.bounds, ...bounds }),
      zoom: zoom ?? windowState.zoom
    }),
    storageArea
  );

  if (Number.isInteger(state.windowId)) {
    try {
      const updateInfo = {
        width: state.bounds.width,
        height: state.bounds.height
      };
      if (Number.isInteger(state.bounds.left)) updateInfo.left = state.bounds.left;
      if (Number.isInteger(state.bounds.top)) updateInfo.top = state.bounds.top;

      await chromeApi.windows.update(state.windowId, updateInfo);
      if (Number.isInteger(state.tabId)) {
        await applyZoom(chromeApi, state.tabId, state.zoom);
      }
    } catch {
      return setWindowState({ ...state, windowId: null, tabId: null }, storageArea);
    }
  }

  return state;
}

export async function saveBoundsFromWindow(window, { storageArea, expectedWindowId }) {
  if (!window || window.type !== "popup" || window.id !== expectedWindowId) {
    return null;
  }

  const state = await getExtensionState(storageArea);
  const windowState = normalizeWindowState(state.windowState);
  if (windowState.windowId !== window.id) {
    return null;
  }

  return setWindowState(
    {
      ...windowState,
      bounds: clampBounds({
        left: window.left,
        top: window.top,
        width: window.width,
        height: window.height
      })
    },
    storageArea
  );
}
