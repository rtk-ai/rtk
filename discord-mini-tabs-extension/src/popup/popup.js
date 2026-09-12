import {
  DEFAULT_BOUNDS,
  DEFAULT_ZOOM,
  MESSAGE_TYPES,
  SHORTCUT_TYPES
} from "../shared/constants.js";
import { compactDiscordUrl } from "../shared/url.js";
import { buildPopupModel } from "./view-model.js";

const CONTROL_IDS = {
  windowMeta: "windowMeta",
  saveCurrentButton: "saveCurrentButton",
  feedback: "feedback",
  focusButton: "focusButton",
  closeButton: "closeButton",
  resetButton: "resetButton",
  searchInput: "searchInput",
  textTab: "textTab",
  voiceTab: "voiceTab",
  textCount: "textCount",
  voiceCount: "voiceCount",
  shortcutList: "shortcutList",
  shortcutForm: "shortcutForm",
  formTitle: "formTitle",
  editingId: "editingId",
  shortcutName: "shortcutName",
  shortcutUrl: "shortcutUrl",
  shortcutType: "shortcutType",
  cancelEditButton: "cancelEditButton",
  settingsForm: "settingsForm",
  widthInput: "widthInput",
  heightInput: "heightInput",
  zoomInput: "zoomInput"
};

function toNumber(value, fallback) {
  const parsed = Number(value);
  return Number.isFinite(parsed) ? parsed : fallback;
}

function toZoomPercent(zoom) {
  return Math.round(Number(zoom ?? DEFAULT_ZOOM) * 100);
}

function getSafeWindowState(windowState) {
  const bounds = windowState?.bounds ?? {};
  const width = Number(bounds.width);
  const height = Number(bounds.height);
  return {
    ...windowState,
    bounds: {
      ...DEFAULT_BOUNDS,
      ...bounds,
      width: Number.isFinite(width) ? width : DEFAULT_BOUNDS.width,
      height: Number.isFinite(height) ? height : DEFAULT_BOUNDS.height
    },
    zoom: Number.isFinite(Number(windowState?.zoom)) ? Number(windowState.zoom) : DEFAULT_ZOOM
  };
}

function getRequiredElement(documentRef, id) {
  const element = documentRef.getElementById(id);
  if (!element) {
    throw new Error(`Missing popup element: ${id}`);
  }
  return element;
}

export function createPopupApp({ document, chromeApi }) {
  const elements = Object.fromEntries(
    Object.entries(CONTROL_IDS).map(([key, id]) => [key, getRequiredElement(document, id)])
  );
  const state = {
    shortcuts: [],
    windowState: getSafeWindowState(null),
    query: "",
    activeType: SHORTCUT_TYPES.TEXT
  };

  async function sendMessage(type, payload = {}) {
    const response = await chromeApi.runtime.sendMessage({ type, payload });
    if (!response?.ok) {
      throw new Error(response?.error || "Popup request failed.");
    }
    return response.data;
  }

  function showError(error) {
    elements.feedback.textContent = error?.message || String(error || "Something went wrong.");
    elements.feedback.hidden = false;
  }

  function clearFeedback() {
    elements.feedback.textContent = "";
    elements.feedback.hidden = true;
  }

  function setText(element, value) {
    element.textContent = String(value);
  }

  function setActiveType(type) {
    state.activeType = type === SHORTCUT_TYPES.VOICE ? SHORTCUT_TYPES.VOICE : SHORTCUT_TYPES.TEXT;
    if (!elements.editingId.value.trim()) {
      elements.shortcutType.value = state.activeType;
    }
    render();
  }

  function clearShortcutForm() {
    elements.formTitle.textContent = "Add shortcut";
    elements.editingId.value = "";
    elements.shortcutName.value = "";
    elements.shortcutUrl.value = "";
    elements.shortcutType.value = state.activeType;
  }

  function populateShortcutForm(shortcut) {
    elements.formTitle.textContent = "Edit shortcut";
    elements.editingId.value = shortcut.id;
    elements.shortcutName.value = shortcut.name;
    elements.shortcutUrl.value = shortcut.url;
    elements.shortcutType.value = shortcut.type;
  }

  function createButton(label, title, handler) {
    const button = document.createElement("button");
    button.type = "button";
    button.textContent = label;
    button.setAttribute("title", title);
    button.addEventListener("click", handler);
    return button;
  }

  function renderShortcut(shortcut) {
    const card = document.createElement("article");
    card.className = "shortcut-card";

    const content = document.createElement("div");
    const name = document.createElement("div");
    name.className = "shortcut-name";
    name.textContent = shortcut.name;

    const url = document.createElement("div");
    url.className = "shortcut-url";
    url.textContent = compactDiscordUrl(shortcut.url) || shortcut.url;

    content.append(name, url);

    const actions = document.createElement("div");
    actions.className = "shortcut-actions";
    actions.append(
      createButton("O", "Open", () => runAction(async () => {
        await sendMessage(MESSAGE_TYPES.OPEN_SHORTCUT, { id: shortcut.id });
        await refresh();
      })),
      createButton("E", "Edit", () => {
        clearFeedback();
        populateShortcutForm(shortcut);
      }),
      createButton("D", "Delete", () => runAction(async () => {
        await sendMessage(MESSAGE_TYPES.DELETE_SHORTCUT, { id: shortcut.id });
        await refresh();
      }))
    );

    card.append(content, actions);
    return card;
  }

  function renderEmpty() {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "No shortcuts";
    return empty;
  }

  function render() {
    const model = buildPopupModel({
      shortcuts: state.shortcuts,
      query: state.query,
      activeType: state.activeType,
      windowState: state.windowState
    });

    setText(elements.windowMeta, `${model.status} | ${model.boundsLabel} | ${model.zoomLabel}`);
    setText(elements.textCount, model.textCount);
    setText(elements.voiceCount, model.voiceCount);
    elements.textTab.classList.toggle("active", model.activeType === SHORTCUT_TYPES.TEXT);
    elements.voiceTab.classList.toggle("active", model.activeType === SHORTCUT_TYPES.VOICE);
    const shortcutNodes = model.activeShortcuts.map(renderShortcut);
    elements.shortcutList.replaceChildren(...(shortcutNodes.length ? shortcutNodes : [renderEmpty()]));
  }

  function renderSettingsInputs() {
    const bounds = state.windowState.bounds;
    elements.widthInput.value = String(bounds.width);
    elements.heightInput.value = String(bounds.height);
    elements.zoomInput.value = String(toZoomPercent(state.windowState.zoom));
  }

  async function refresh() {
    const data = await sendMessage(MESSAGE_TYPES.GET_STATE);
    state.shortcuts = Array.isArray(data?.shortcuts) ? data.shortcuts : [];
    state.windowState = getSafeWindowState(data?.windowState);
    renderSettingsInputs();
    render();
  }

  async function runAction(action) {
    try {
      clearFeedback();
      await action();
    } catch (error) {
      showError(error);
    }
  }

  function bindEvents() {
    elements.searchInput.addEventListener("input", () => {
      state.query = elements.searchInput.value;
      render();
    });
    elements.textTab.addEventListener("click", () => setActiveType(SHORTCUT_TYPES.TEXT));
    elements.voiceTab.addEventListener("click", () => setActiveType(SHORTCUT_TYPES.VOICE));
    elements.cancelEditButton.addEventListener("click", () => {
      clearFeedback();
      clearShortcutForm();
    });

    elements.focusButton.addEventListener("click", () => runAction(async () => {
      await sendMessage(MESSAGE_TYPES.FOCUS_WINDOW);
      await refresh();
    }));
    elements.closeButton.addEventListener("click", () => runAction(async () => {
      await sendMessage(MESSAGE_TYPES.CLOSE_WINDOW);
      await refresh();
    }));
    elements.resetButton.addEventListener("click", () => runAction(async () => {
      await sendMessage(MESSAGE_TYPES.RESET_POSITION);
      await refresh();
    }));

    elements.saveCurrentButton.addEventListener("click", () => runAction(async () => {
      const data = await sendMessage(MESSAGE_TYPES.READ_ACTIVE_DISCORD_TAB);
      clearShortcutForm();
      elements.shortcutName.value = data?.suggestedName ?? "";
      elements.shortcutUrl.value = data?.url ?? "";
      elements.shortcutType.value = state.activeType;
    }));

    elements.shortcutForm.addEventListener("submit", (event) => runAction(async () => {
      event.preventDefault();
      const id = elements.editingId.value.trim();
      const payload = {
        name: elements.shortcutName.value.trim(),
        url: elements.shortcutUrl.value.trim(),
        type: elements.shortcutType.value === SHORTCUT_TYPES.VOICE
          ? SHORTCUT_TYPES.VOICE
          : SHORTCUT_TYPES.TEXT
      };
      if (id) {
        await sendMessage(MESSAGE_TYPES.UPDATE_SHORTCUT, { id, ...payload });
      } else {
        await sendMessage(MESSAGE_TYPES.CREATE_SHORTCUT, payload);
      }
      state.activeType = payload.type;
      clearShortcutForm();
      await refresh();
    }));

    elements.settingsForm.addEventListener("submit", (event) => runAction(async () => {
      event.preventDefault();
      const payload = {
        bounds: {
          width: toNumber(elements.widthInput.value, DEFAULT_BOUNDS.width),
          height: toNumber(elements.heightInput.value, DEFAULT_BOUNDS.height)
        },
        zoom: toNumber(elements.zoomInput.value, toZoomPercent(DEFAULT_ZOOM)) / 100
      };
      await sendMessage(MESSAGE_TYPES.UPDATE_WINDOW_SETTINGS, payload);
      await refresh();
    }));
  }

  async function init() {
    bindEvents();
    clearShortcutForm();
    await runAction(refresh);
  }

  return {
    init,
    refresh,
    sendMessage
  };
}

if (globalThis.document && globalThis.chrome?.runtime?.sendMessage) {
  const app = createPopupApp({
    document: globalThis.document,
    chromeApi: globalThis.chrome
  });
  if (globalThis.document.readyState === "loading") {
    globalThis.document.addEventListener("DOMContentLoaded", () => {
      app.init();
    });
  } else {
    app.init();
  }
}
