import { createServiceWorkerController } from "./service-worker-core.js";
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

function errorMessage(error) {
  return error instanceof Error ? error.message : "Unknown extension error.";
}

const controller = createServiceWorkerController({
  chromeApi: chrome,
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
  setTimeoutFn: setTimeout,
  clearTimeoutFn: clearTimeout,
  debounceMs: 400
});

chrome.runtime.onMessage.addListener((message, _sender, sendResponse) => {
  controller
    .handleMessage(message)
    .then((data) => sendResponse({ ok: true, data }))
    .catch((error) => sendResponse({ ok: false, error: errorMessage(error) }));
  return true;
});

chrome.windows.onRemoved.addListener((windowId) => {
  controller.handleWindowRemoved(windowId);
});

chrome.windows.onBoundsChanged.addListener((changedWindow) => {
  controller.handleBoundsChanged(changedWindow);
});
