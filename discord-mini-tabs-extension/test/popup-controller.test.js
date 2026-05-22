import test from "node:test";
import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createPopupApp } from "../src/popup/popup.js";
import { MESSAGE_TYPES, SHORTCUT_TYPES } from "../src/shared/constants.js";

const textUrl = "https://discord.com/channels/123456789012345678/987654321098765432";
const voiceUrl = "https://discord.com/channels/223456789012345678/887654321098765432";

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

  findByText(text) {
    if (this._textContent === text) {
      return this;
    }
    for (const child of this.children) {
      const match = child.findByText(text);
      if (match) {
        return match;
      }
    }
    return null;
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
    "resetButton",
    "searchInput",
    "textTab",
    "voiceTab",
    "textCount",
    "voiceCount",
    "shortcutList",
    "shortcutForm",
    "formTitle",
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

test("popup markup exposes resetButton id contract", async () => {
  const html = await readFile(new URL("../src/popup/popup.html", import.meta.url), "utf8");

  assert.match(html, /id="resetButton"/);
  assert.match(html, /id="formTitle"/);
  assert.doesNotMatch(html, /resetPositionButton/);
});

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
  assert.equal(document.getElementById("formTitle").textContent, "Add shortcut");
  assert.equal(document.getElementById("widthInput").value, "420");
  assert.equal(document.getElementById("heightInput").value, "900");
  assert.equal(document.getElementById("zoomInput").value, "90");
  assert.match(document.getElementById("shortcutList").textContent, /Dev Chat/);
  assert.match(document.getElementById("shortcutList").textContent, /123456789012345678 \/ 987654321098765432/);
});

test("submits exponent-style window settings as bounds and decimal zoom then refreshes", async () => {
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

  document.getElementById("widthInput").value = "5e2";
  document.getElementById("heightInput").value = "8e2";
  document.getElementById("zoomInput").value = "1e2";
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

test("switching tabs while editing does not change the edited shortcut type", async () => {
  const document = createPopupDocument();
  const chromeApi = createChromeApi(async (message) => {
    if (message.type === MESSAGE_TYPES.UPDATE_SHORTCUT) {
      return { ok: true, data: null };
    }
    return {
      ok: true,
      data: {
        shortcuts: [
          { id: "voice-1", name: "Voice Room", type: SHORTCUT_TYPES.VOICE, url: voiceUrl }
        ],
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

  document.getElementById("voiceTab").dispatchEvent("click");
  document.getElementById("editingId").value = "voice-1";
  document.getElementById("shortcutName").value = "Voice Room";
  document.getElementById("shortcutUrl").value = voiceUrl;
  document.getElementById("shortcutType").value = SHORTCUT_TYPES.VOICE;

  await document.getElementById("textTab").dispatchEvent("click");
  await document.getElementById("shortcutForm").dispatchEvent("submit");

  assert.deepEqual(chromeApi.messages[1], {
    type: MESSAGE_TYPES.UPDATE_SHORTCUT,
    payload: {
      id: "voice-1",
      name: "Voice Room",
      url: voiceUrl,
      type: SHORTCUT_TYPES.VOICE
    }
  });
});

test("edit action updates form title and cancel restores add title", async () => {
  const document = createPopupDocument();
  const chromeApi = createChromeApi(async () => ({
    ok: true,
    data: {
      shortcuts: [{ id: "text-1", name: "Dev Chat", type: SHORTCUT_TYPES.TEXT, url: textUrl }],
      windowState: {
        windowId: null,
        bounds: { width: 420, height: 900 },
        zoom: 0.9
      }
    }
  }));

  const app = createPopupApp({ document, chromeApi });
  await app.init();

  const editButton = document.getElementById("shortcutList").findByText("E");
  assert.ok(editButton);
  await editButton.dispatchEvent("click");

  assert.equal(document.getElementById("formTitle").textContent, "Edit shortcut");
  assert.equal(document.getElementById("editingId").value, "text-1");

  await document.getElementById("cancelEditButton").dispatchEvent("click");

  assert.equal(document.getElementById("formTitle").textContent, "Add shortcut");
  assert.equal(document.getElementById("editingId").value, "");
});

test("save current resets edit mode title before populating active tab", async () => {
  const activeUrl = "https://discord.com/channels/323456789012345678/787654321098765432";
  const document = createPopupDocument();
  const chromeApi = createChromeApi(async (message) => {
    if (message.type === MESSAGE_TYPES.READ_ACTIVE_DISCORD_TAB) {
      return {
        ok: true,
        data: {
          suggestedName: "Active Channel",
          url: activeUrl
        }
      };
    }
    return {
      ok: true,
      data: {
        shortcuts: [{ id: "text-1", name: "Dev Chat", type: SHORTCUT_TYPES.TEXT, url: textUrl }],
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

  const editButton = document.getElementById("shortcutList").findByText("E");
  assert.ok(editButton);
  await editButton.dispatchEvent("click");
  assert.equal(document.getElementById("formTitle").textContent, "Edit shortcut");

  await document.getElementById("saveCurrentButton").dispatchEvent("click");

  assert.equal(document.getElementById("editingId").value, "");
  assert.equal(document.getElementById("shortcutName").value, "Active Channel");
  assert.equal(document.getElementById("shortcutUrl").value, activeUrl);
  assert.equal(document.getElementById("formTitle").textContent, "Add shortcut");
});

test("search render does not overwrite unsaved settings values", async () => {
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

  document.getElementById("widthInput").value = "777";
  document.getElementById("heightInput").value = "888";
  document.getElementById("zoomInput").value = "101";
  document.getElementById("searchInput").value = "dev";
  await document.getElementById("searchInput").dispatchEvent("input");

  assert.equal(document.getElementById("widthInput").value, "777");
  assert.equal(document.getElementById("heightInput").value, "888");
  assert.equal(document.getElementById("zoomInput").value, "101");
});
