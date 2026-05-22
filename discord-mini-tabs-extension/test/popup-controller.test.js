import test from "node:test";
import assert from "node:assert/strict";
import { createPopupApp } from "../src/popup/popup.js";
import { MESSAGE_TYPES, SHORTCUT_TYPES } from "../src/shared/constants.js";

const textUrl = "https://discord.com/channels/123456789012345678/987654321098765432";

class FakeClassList {
  constructor() {
    this.values = new Set();
  }

  add(value) {
    this.values.add(value);
  }

  remove(value) {
    this.values.delete(value);
  }

  toggle(value, force) {
    if (force) {
      this.add(value);
    } else {
      this.remove(value);
    }
  }

  contains(value) {
    return this.values.has(value);
  }
}

class FakeElement {
  constructor(tagName = "div") {
    this.tagName = tagName.toUpperCase();
    this.children = [];
    this.listeners = new Map();
    this.classList = new FakeClassList();
    this.dataset = {};
    this.attributes = new Map();
    this.hidden = false;
    this.value = "";
    this.type = "";
    this.id = "";
    this._textContent = "";
  }

  get textContent() {
    return `${this._textContent}${this.children.map((child) => child.textContent).join("")}`;
  }

  set textContent(value) {
    this._textContent = String(value ?? "");
    this.children = [];
  }

  append(...children) {
    this.children.push(...children);
  }

  replaceChildren(...children) {
    this._textContent = "";
    this.children = children;
  }

  setAttribute(name, value) {
    this.attributes.set(name, String(value));
  }

  addEventListener(type, handler) {
    const handlers = this.listeners.get(type) ?? [];
    handlers.push(handler);
    this.listeners.set(type, handlers);
  }

  async dispatchEvent(type) {
    const event = { preventDefault() {}, target: this };
    const handlers = this.listeners.get(type) ?? [];
    await Promise.all(handlers.map((handler) => handler(event)));
  }
}

class FakeDocument {
  constructor(ids) {
    this.elements = new Map(ids.map((id) => [id, new FakeElement()]));
    for (const [id, element] of this.elements) {
      element.id = id;
    }
  }

  getElementById(id) {
    return this.elements.get(id) ?? null;
  }

  createElement(tagName) {
    return new FakeElement(tagName);
  }
}

function createPopupDocument() {
  return new FakeDocument([
    "windowMeta",
    "saveCurrentButton",
    "feedback",
    "focusButton",
    "closeButton",
    "resetPositionButton",
    "searchInput",
    "textTab",
    "voiceTab",
    "textCount",
    "voiceCount",
    "shortcutList",
    "shortcutForm",
    "editingId",
    "shortcutName",
    "shortcutUrl",
    "shortcutType",
    "cancelEditButton",
    "settingsForm",
    "widthInput",
    "heightInput",
    "zoomInput"
  ]);
}

function createChromeApi(respond) {
  const messages = [];
  return {
    messages,
    runtime: {
      async sendMessage(message) {
        messages.push(message);
        return respond(message);
      }
    }
  };
}

test("initializes from service worker state and renders popup controls", async () => {
  const document = createPopupDocument();
  const chromeApi = createChromeApi(async () => ({
    ok: true,
    data: {
      shortcuts: [{ id: "text-1", name: "Dev Chat", type: SHORTCUT_TYPES.TEXT, url: textUrl }],
      windowState: {
        windowId: 12,
        bounds: { width: 420, height: 900 },
        zoom: 0.9
      }
    }
  }));

  const app = createPopupApp({ document, chromeApi });
  await app.init();

  assert.deepEqual(chromeApi.messages, [{ type: MESSAGE_TYPES.GET_STATE, payload: {} }]);
  assert.equal(document.getElementById("windowMeta").textContent, "Open | 420 x 900 | 90%");
  assert.equal(document.getElementById("textCount").textContent, "1");
  assert.equal(document.getElementById("voiceCount").textContent, "0");
  assert.equal(document.getElementById("widthInput").value, "420");
  assert.equal(document.getElementById("heightInput").value, "900");
  assert.equal(document.getElementById("zoomInput").value, "90");
  assert.match(document.getElementById("shortcutList").textContent, /Dev Chat/);
  assert.match(document.getElementById("shortcutList").textContent, /123456789012345678 \/ 987654321098765432/);
});

test("submits window settings as bounds and decimal zoom then refreshes", async () => {
  const document = createPopupDocument();
  const chromeApi = createChromeApi(async (message) => {
    if (message.type === MESSAGE_TYPES.UPDATE_WINDOW_SETTINGS) {
      return { ok: true, data: null };
    }
    return {
      ok: true,
      data: {
        shortcuts: [],
        windowState: {
          windowId: null,
          bounds: { width: 420, height: 900 },
          zoom: 0.9
        }
      }
    };
  });

  const app = createPopupApp({ document, chromeApi });
  await app.init();

  document.getElementById("widthInput").value = "500";
  document.getElementById("heightInput").value = "800";
  document.getElementById("zoomInput").value = "100";
  await document.getElementById("settingsForm").dispatchEvent("submit");

  assert.deepEqual(chromeApi.messages, [
    { type: MESSAGE_TYPES.GET_STATE, payload: {} },
    {
      type: MESSAGE_TYPES.UPDATE_WINDOW_SETTINGS,
      payload: { bounds: { width: 500, height: 800 }, zoom: 1 }
    },
    { type: MESSAGE_TYPES.GET_STATE, payload: {} }
  ]);
});
