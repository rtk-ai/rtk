# Discord Mini Tabs Extension Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a lightweight Chrome MV3 extension that opens saved Discord text and voice channel URLs in one reusable, configurable Chrome popup window.

**Architecture:** The extension is a standalone, dependency-light project under `discord-mini-tabs-extension/`. The popup renders shortcut/search/settings UI, while the MV3 service worker owns all Chrome API work for windows, tabs, zoom, and local storage. Discord runs only as the official Discord web app in a real Chrome popup window.

**Tech Stack:** Plain JavaScript ES modules, Chrome Manifest V3, `chrome.storage.local`, `chrome.windows`, `chrome.tabs`, popup HTML/CSS, and Node's built-in `node:test` runner for pure logic tests.

---

## Implementation Decisions

- Use plain JavaScript ES modules and no build step.
- Use `node --test` for shared logic tests.
- Support only `https://discord.com/channels/...` in the first release.
- Store shortcuts and settings locally with `chrome.storage.local`.
- Do not add content scripts.
- Do not inspect or manipulate Discord DOM.
- Do not add import/export of shortcuts in the first release.

## File Structure

- `discord-mini-tabs-extension/package.json`: local scripts and module mode.
- `discord-mini-tabs-extension/manifest.json`: Chrome MV3 extension manifest.
- `discord-mini-tabs-extension/README.md`: load-unpacked and usage notes.
- `discord-mini-tabs-extension/src/shared/constants.js`: storage keys, default bounds, min/max clamps, message types.
- `discord-mini-tabs-extension/src/shared/url.js`: Discord URL validation and formatting.
- `discord-mini-tabs-extension/src/shared/settings.js`: bounds and zoom normalization.
- `discord-mini-tabs-extension/src/shared/shortcuts.js`: shortcut creation, mutation, grouping, and search.
- `discord-mini-tabs-extension/src/background/storage.js`: promise-based storage helpers.
- `discord-mini-tabs-extension/src/background/window-manager.js`: create/focus/update/close/reset mini Discord window.
- `discord-mini-tabs-extension/src/background/service-worker.js`: Chrome runtime message routing and window event listeners.
- `discord-mini-tabs-extension/src/popup/view-model.js`: pure UI view model helpers.
- `discord-mini-tabs-extension/src/popup/popup.html`: popup markup.
- `discord-mini-tabs-extension/src/popup/popup.css`: popup styling.
- `discord-mini-tabs-extension/src/popup/popup.js`: popup controller and DOM event handling.
- `discord-mini-tabs-extension/test/*.test.js`: Node tests for pure modules and injected Chrome API fakes.
- `discord-mini-tabs-extension/MANUAL_TESTS.md`: Chrome load-unpacked and stability smoke checklist.

## Task 1: Scaffold The Standalone Extension Shell

**Files:**
- Create: `discord-mini-tabs-extension/package.json`
- Create: `discord-mini-tabs-extension/manifest.json`
- Create: `discord-mini-tabs-extension/README.md`
- Create: `discord-mini-tabs-extension/src/background/service-worker.js`
- Create: `discord-mini-tabs-extension/src/popup/popup.html`
- Create: `discord-mini-tabs-extension/src/popup/popup.css`
- Create: `discord-mini-tabs-extension/src/popup/popup.js`

- [ ] **Step 1: Create the directory tree**

Run:

```powershell
New-Item -ItemType Directory -Force `
  'discord-mini-tabs-extension/src/background', `
  'discord-mini-tabs-extension/src/popup', `
  'discord-mini-tabs-extension/src/shared', `
  'discord-mini-tabs-extension/test'
```

Expected: command exits 0 and the four directories exist.

- [ ] **Step 2: Create the project metadata**

Create `discord-mini-tabs-extension/package.json`:

```json
{
  "name": "discord-mini-tabs-extension",
  "version": "0.1.0",
  "private": true,
  "type": "module",
  "scripts": {
    "test": "node --test"
  },
  "engines": {
    "node": ">=20"
  }
}
```

Create `discord-mini-tabs-extension/manifest.json`:

```json
{
  "manifest_version": 3,
  "name": "Discord Mini Tabs",
  "version": "0.1.0",
  "description": "Open saved Discord text and voice channels in one reusable mini Chrome popup window.",
  "permissions": ["storage", "tabs", "windows", "activeTab"],
  "host_permissions": ["https://discord.com/*"],
  "background": {
    "service_worker": "src/background/service-worker.js",
    "type": "module"
  },
  "action": {
    "default_title": "Discord Mini Tabs",
    "default_popup": "src/popup/popup.html"
  }
}
```

- [ ] **Step 3: Create the initial README**

Create `discord-mini-tabs-extension/README.md`:

```markdown
# Discord Mini Tabs

A Chrome Manifest V3 extension that opens saved Discord text and voice channel URLs in one reusable Chrome popup window.

## Development

Run pure logic tests:

```bash
npm test
```

Load in Chrome:

1. Open `chrome://extensions`.
2. Enable Developer mode.
3. Click Load unpacked.
4. Select the `discord-mini-tabs-extension` directory.

## Scope

Discord runs as the official Discord web app in a real Chrome popup window. This extension does not embed Discord, inject content scripts, read Discord messages, or inspect Discord voice internals.
```

- [ ] **Step 4: Add temporary valid extension entry files**

Create `discord-mini-tabs-extension/src/background/service-worker.js`:

```js
chrome.runtime.onInstalled.addListener(() => {
  console.info("Discord Mini Tabs installed");
});
```

Create `discord-mini-tabs-extension/src/popup/popup.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Discord Mini Tabs</title>
    <link rel="stylesheet" href="./popup.css">
  </head>
  <body>
    <main class="shell">
      <h1>Discord Mini Tabs</h1>
      <p>Extension shell ready.</p>
    </main>
    <script type="module" src="./popup.js"></script>
  </body>
</html>
```

Create `discord-mini-tabs-extension/src/popup/popup.css`:

```css
:root {
  color-scheme: dark;
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
}

body {
  margin: 0;
  width: 360px;
  background: #17181c;
  color: #f3f4f8;
}

.shell {
  padding: 16px;
}

h1 {
  margin: 0 0 8px;
  font-size: 18px;
  font-weight: 700;
}

p {
  margin: 0;
  color: #aeb3c2;
}
```

Create `discord-mini-tabs-extension/src/popup/popup.js`:

```js
console.info("Discord Mini Tabs popup loaded");
```

- [ ] **Step 5: Run the initial test command**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: `node --test` exits 0 with no failing tests.

- [ ] **Step 6: Commit the scaffold**

```powershell
git add discord-mini-tabs-extension
git commit -m "feat: scaffold discord mini tabs extension"
```

## Task 2: Add Shared URL And Settings Logic With Tests

**Files:**
- Create: `discord-mini-tabs-extension/test/url.test.js`
- Create: `discord-mini-tabs-extension/test/settings.test.js`
- Create: `discord-mini-tabs-extension/src/shared/constants.js`
- Create: `discord-mini-tabs-extension/src/shared/url.js`
- Create: `discord-mini-tabs-extension/src/shared/settings.js`

- [ ] **Step 1: Write failing URL tests**

Create `discord-mini-tabs-extension/test/url.test.js`:

```js
import test from "node:test";
import assert from "node:assert/strict";
import {
  compactDiscordUrl,
  normalizeDiscordChannelUrl,
  validateDiscordChannelUrl
} from "../src/shared/url.js";

test("accepts server channel Discord URLs", () => {
  const result = validateDiscordChannelUrl("https://discord.com/channels/123/456");
  assert.equal(result.ok, true);
  assert.equal(result.url, "https://discord.com/channels/123/456");
  assert.equal(result.scope, "server");
});

test("accepts direct message Discord URLs", () => {
  const result = validateDiscordChannelUrl("https://discord.com/channels/@me/456");
  assert.equal(result.ok, true);
  assert.equal(result.url, "https://discord.com/channels/@me/456");
  assert.equal(result.scope, "dm");
});

test("normalizes trailing slash and search params", () => {
  assert.equal(
    normalizeDiscordChannelUrl("https://discord.com/channels/123/456/?jump=999"),
    "https://discord.com/channels/123/456"
  );
});

test("rejects non-discord hosts", () => {
  const result = validateDiscordChannelUrl("https://example.com/channels/123/456");
  assert.equal(result.ok, false);
  assert.match(result.error, /discord\.com/);
});

test("rejects non-channel Discord URLs", () => {
  const result = validateDiscordChannelUrl("https://discord.com/app");
  assert.equal(result.ok, false);
  assert.match(result.error, /channels/);
});

test("returns compact display labels", () => {
  assert.equal(compactDiscordUrl("https://discord.com/channels/123/456"), "123 / 456");
  assert.equal(compactDiscordUrl("https://discord.com/channels/@me/456"), "DM / 456");
});
```

- [ ] **Step 2: Write failing settings tests**

Create `discord-mini-tabs-extension/test/settings.test.js`:

```js
import test from "node:test";
import assert from "node:assert/strict";
import {
  clampBounds,
  clampZoom,
  createDefaultWindowState,
  normalizeWindowState
} from "../src/shared/settings.js";

test("creates default window state", () => {
  assert.deepEqual(createDefaultWindowState(), {
    windowId: null,
    tabId: null,
    bounds: {
      left: null,
      top: null,
      width: 420,
      height: 900
    },
    zoom: 0.9,
    lastShortcutId: null
  });
});

test("clamps bounds to supported range", () => {
  assert.deepEqual(clampBounds({ left: 20, top: 30, width: 100, height: 2000 }), {
    left: 20,
    top: 30,
    width: 320,
    height: 1400
  });
});

test("keeps null left and top when unset", () => {
  assert.deepEqual(clampBounds({ width: 500, height: 700 }), {
    left: null,
    top: null,
    width: 500,
    height: 700
  });
});

test("clamps zoom to supported range", () => {
  assert.equal(clampZoom(0.2), 0.67);
  assert.equal(clampZoom(2), 1.25);
  assert.equal(clampZoom(0.9), 0.9);
});

test("normalizes partial window state", () => {
  const state = normalizeWindowState({
    windowId: 10,
    bounds: { width: 640 },
    zoom: 1.5,
    lastShortcutId: "abc"
  });

  assert.deepEqual(state, {
    windowId: 10,
    tabId: null,
    bounds: {
      left: null,
      top: null,
      width: 640,
      height: 900
    },
    zoom: 1.25,
    lastShortcutId: "abc"
  });
});
```

- [ ] **Step 3: Run tests and verify the expected failure**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: FAIL because `src/shared/url.js` and `src/shared/settings.js` do not exist yet.

- [ ] **Step 4: Implement shared constants**

Create `discord-mini-tabs-extension/src/shared/constants.js`:

```js
export const STORAGE_KEYS = {
  SHORTCUTS: "shortcuts",
  WINDOW_STATE: "windowState"
};

export const SHORTCUT_TYPES = {
  TEXT: "text",
  VOICE: "voice"
};

export const DEFAULT_BOUNDS = {
  left: null,
  top: null,
  width: 420,
  height: 900
};

export const BOUNDS_LIMITS = {
  minWidth: 320,
  minHeight: 480,
  maxWidth: 1200,
  maxHeight: 1400
};

export const DEFAULT_ZOOM = 0.9;
export const MIN_ZOOM = 0.67;
export const MAX_ZOOM = 1.25;

export const DISCORD_HOST = "discord.com";
export const DISCORD_CHANNEL_PREFIX = "/channels/";

export const MESSAGE_TYPES = {
  GET_STATE: "GET_STATE",
  CREATE_SHORTCUT: "CREATE_SHORTCUT",
  UPDATE_SHORTCUT: "UPDATE_SHORTCUT",
  DELETE_SHORTCUT: "DELETE_SHORTCUT",
  OPEN_SHORTCUT: "OPEN_SHORTCUT",
  READ_ACTIVE_DISCORD_TAB: "READ_ACTIVE_DISCORD_TAB",
  UPDATE_WINDOW_SETTINGS: "UPDATE_WINDOW_SETTINGS",
  FOCUS_WINDOW: "FOCUS_WINDOW",
  CLOSE_WINDOW: "CLOSE_WINDOW",
  RESET_POSITION: "RESET_POSITION"
};
```

- [ ] **Step 5: Implement URL validation**

Create `discord-mini-tabs-extension/src/shared/url.js`:

```js
import { DISCORD_CHANNEL_PREFIX, DISCORD_HOST } from "./constants.js";

function parseUrl(input) {
  if (typeof input !== "string" || input.trim().length === 0) {
    return null;
  }

  try {
    return new URL(input.trim());
  } catch {
    return null;
  }
}

function getChannelParts(url) {
  const parts = url.pathname.split("/").filter(Boolean);
  if (parts.length !== 3 || parts[0] !== "channels") {
    return null;
  }

  const [, guildOrMe, channelId] = parts;
  if (!guildOrMe || !channelId) {
    return null;
  }

  return { guildOrMe, channelId };
}

export function validateDiscordChannelUrl(input) {
  const url = parseUrl(input);
  if (!url) {
    return { ok: false, error: "Enter a valid Discord channel URL." };
  }

  if (url.protocol !== "https:" || url.hostname !== DISCORD_HOST) {
    return { ok: false, error: "Only https://discord.com channel URLs are supported." };
  }

  if (!url.pathname.startsWith(DISCORD_CHANNEL_PREFIX)) {
    return { ok: false, error: "The URL must point to discord.com/channels/..." };
  }

  const parts = getChannelParts(url);
  if (!parts) {
    return { ok: false, error: "The Discord URL must include a server or DM id and channel id." };
  }

  const normalized = `https://${DISCORD_HOST}/channels/${parts.guildOrMe}/${parts.channelId}`;
  return {
    ok: true,
    url: normalized,
    guildOrMe: parts.guildOrMe,
    channelId: parts.channelId,
    scope: parts.guildOrMe === "@me" ? "dm" : "server"
  };
}

export function normalizeDiscordChannelUrl(input) {
  const result = validateDiscordChannelUrl(input);
  if (!result.ok) {
    throw new Error(result.error);
  }
  return result.url;
}

export function compactDiscordUrl(input) {
  const result = validateDiscordChannelUrl(input);
  if (!result.ok) {
    return "";
  }
  const guildLabel = result.guildOrMe === "@me" ? "DM" : result.guildOrMe;
  return `${guildLabel} / ${result.channelId}`;
}
```

- [ ] **Step 6: Implement settings normalization**

Create `discord-mini-tabs-extension/src/shared/settings.js`:

```js
import {
  BOUNDS_LIMITS,
  DEFAULT_BOUNDS,
  DEFAULT_ZOOM,
  MAX_ZOOM,
  MIN_ZOOM
} from "./constants.js";

function clampNumber(value, min, max, fallback) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    return fallback;
  }
  return Math.min(max, Math.max(min, Math.round(number)));
}

function normalizeNullableCoordinate(value) {
  const number = Number(value);
  return Number.isFinite(number) ? Math.round(number) : null;
}

export function clampBounds(bounds = {}) {
  return {
    left: normalizeNullableCoordinate(bounds.left),
    top: normalizeNullableCoordinate(bounds.top),
    width: clampNumber(
      bounds.width,
      BOUNDS_LIMITS.minWidth,
      BOUNDS_LIMITS.maxWidth,
      DEFAULT_BOUNDS.width
    ),
    height: clampNumber(
      bounds.height,
      BOUNDS_LIMITS.minHeight,
      BOUNDS_LIMITS.maxHeight,
      DEFAULT_BOUNDS.height
    )
  };
}

export function clampZoom(value) {
  const number = Number(value);
  if (!Number.isFinite(number)) {
    return DEFAULT_ZOOM;
  }
  return Math.min(MAX_ZOOM, Math.max(MIN_ZOOM, Number(number.toFixed(2))));
}

export function createDefaultWindowState() {
  return {
    windowId: null,
    tabId: null,
    bounds: { ...DEFAULT_BOUNDS },
    zoom: DEFAULT_ZOOM,
    lastShortcutId: null
  };
}

export function normalizeWindowState(input = {}) {
  const defaults = createDefaultWindowState();
  return {
    windowId: Number.isInteger(input.windowId) ? input.windowId : null,
    tabId: Number.isInteger(input.tabId) ? input.tabId : null,
    bounds: clampBounds({ ...defaults.bounds, ...input.bounds }),
    zoom: clampZoom(input.zoom ?? defaults.zoom),
    lastShortcutId:
      typeof input.lastShortcutId === "string" && input.lastShortcutId.length > 0
        ? input.lastShortcutId
        : null
  };
}
```

- [ ] **Step 7: Run tests and verify they pass**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: PASS for `url.test.js` and `settings.test.js`.

- [ ] **Step 8: Commit shared logic**

```powershell
git add discord-mini-tabs-extension
git commit -m "feat: add discord url and settings logic"
```

## Task 3: Add Shortcut Creation, Editing, Deletion, Grouping, And Search

**Files:**
- Create: `discord-mini-tabs-extension/test/shortcuts.test.js`
- Create: `discord-mini-tabs-extension/src/shared/shortcuts.js`

- [ ] **Step 1: Write failing shortcut tests**

Create `discord-mini-tabs-extension/test/shortcuts.test.js`:

```js
import test from "node:test";
import assert from "node:assert/strict";
import {
  createShortcut,
  deleteShortcut,
  filterShortcuts,
  splitShortcutsByType,
  updateShortcut
} from "../src/shared/shortcuts.js";

const fixedNow = () => "2026-05-21T00:00:00.000Z";
const fixedId = () => "shortcut-1";

test("creates normalized text shortcut", () => {
  const shortcut = createShortcut(
    {
      name: " dev chat ",
      type: "text",
      url: "https://discord.com/channels/123/456/?jump=1"
    },
    { idFactory: fixedId, now: fixedNow }
  );

  assert.deepEqual(shortcut, {
    id: "shortcut-1",
    name: "dev chat",
    type: "text",
    url: "https://discord.com/channels/123/456",
    createdAt: "2026-05-21T00:00:00.000Z",
    updatedAt: "2026-05-21T00:00:00.000Z"
  });
});

test("rejects empty shortcut name", () => {
  assert.throws(
    () => createShortcut({ name: " ", type: "text", url: "https://discord.com/channels/123/456" }),
    /name/
  );
});

test("updates shortcut while preserving id and createdAt", () => {
  const original = createShortcut(
    { name: "dev chat", type: "text", url: "https://discord.com/channels/123/456" },
    { idFactory: fixedId, now: fixedNow }
  );
  const updated = updateShortcut(original, {
    name: "team call",
    type: "voice",
    url: "https://discord.com/channels/789/111",
    now: () => "2026-05-21T01:00:00.000Z"
  });

  assert.equal(updated.id, "shortcut-1");
  assert.equal(updated.createdAt, "2026-05-21T00:00:00.000Z");
  assert.equal(updated.updatedAt, "2026-05-21T01:00:00.000Z");
  assert.equal(updated.type, "voice");
  assert.equal(updated.url, "https://discord.com/channels/789/111");
});

test("deletes shortcut by id", () => {
  const shortcuts = [
    { id: "a", name: "A", type: "text", url: "https://discord.com/channels/1/2" },
    { id: "b", name: "B", type: "voice", url: "https://discord.com/channels/3/4" }
  ];
  assert.deepEqual(deleteShortcut(shortcuts, "a").map((item) => item.id), ["b"]);
});

test("filters shortcuts by name and url", () => {
  const shortcuts = [
    { id: "a", name: "Dev Chat", type: "text", url: "https://discord.com/channels/1/2" },
    { id: "b", name: "Team Call", type: "voice", url: "https://discord.com/channels/3/4" }
  ];
  assert.deepEqual(filterShortcuts(shortcuts, "team").map((item) => item.id), ["b"]);
  assert.deepEqual(filterShortcuts(shortcuts, "channels/1").map((item) => item.id), ["a"]);
});

test("splits shortcuts by type", () => {
  const result = splitShortcutsByType([
    { id: "a", name: "Dev Chat", type: "text", url: "https://discord.com/channels/1/2" },
    { id: "b", name: "Team Call", type: "voice", url: "https://discord.com/channels/3/4" }
  ]);

  assert.deepEqual(result.text.map((item) => item.id), ["a"]);
  assert.deepEqual(result.voice.map((item) => item.id), ["b"]);
});
```

- [ ] **Step 2: Run tests and verify the expected failure**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: FAIL because `src/shared/shortcuts.js` does not exist.

- [ ] **Step 3: Implement shortcut helpers**

Create `discord-mini-tabs-extension/src/shared/shortcuts.js`:

```js
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
    const haystack = `${shortcut.name} ${shortcut.type} ${shortcut.url}`.toLowerCase();
    return haystack.includes(normalizedQuery);
  });
}

export function splitShortcutsByType(shortcuts) {
  return {
    text: shortcuts.filter((shortcut) => shortcut.type === SHORTCUT_TYPES.TEXT),
    voice: shortcuts.filter((shortcut) => shortcut.type === SHORTCUT_TYPES.VOICE)
  };
}
```

- [ ] **Step 4: Run tests and verify they pass**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: PASS for URL, settings, and shortcut tests.

- [ ] **Step 5: Commit shortcut logic**

```powershell
git add discord-mini-tabs-extension
git commit -m "feat: add shortcut management logic"
```

## Task 4: Add Storage Helpers With Tests

**Files:**
- Create: `discord-mini-tabs-extension/test/storage.test.js`
- Create: `discord-mini-tabs-extension/src/background/storage.js`

- [ ] **Step 1: Write failing storage tests**

Create `discord-mini-tabs-extension/test/storage.test.js`:

```js
import test from "node:test";
import assert from "node:assert/strict";
import {
  getExtensionState,
  setShortcuts,
  setWindowState,
  updateWindowState
} from "../src/background/storage.js";

function createFakeStorage(initial = {}) {
  const data = { ...initial };
  return {
    data,
    async get(defaults) {
      return { ...defaults, ...data };
    },
    async set(values) {
      Object.assign(data, values);
    }
  };
}

test("returns normalized default state", async () => {
  const storage = createFakeStorage();
  const state = await getExtensionState(storage);

  assert.deepEqual(state.shortcuts, []);
  assert.equal(state.windowState.bounds.width, 420);
  assert.equal(state.windowState.zoom, 0.9);
});

test("saves shortcuts", async () => {
  const storage = createFakeStorage();
  await setShortcuts([{ id: "a", name: "Dev", type: "text", url: "https://discord.com/channels/1/2" }], storage);
  assert.equal(storage.data.shortcuts.length, 1);
});

test("normalizes saved window state", async () => {
  const storage = createFakeStorage();
  await setWindowState({ bounds: { width: 100, height: 2000 }, zoom: 2 }, storage);
  assert.equal(storage.data.windowState.bounds.width, 320);
  assert.equal(storage.data.windowState.bounds.height, 1400);
  assert.equal(storage.data.windowState.zoom, 1.25);
});

test("updates window state from previous value", async () => {
  const storage = createFakeStorage({
    windowState: {
      windowId: 5,
      tabId: 6,
      bounds: { left: null, top: null, width: 420, height: 900 },
      zoom: 0.9,
      lastShortcutId: null
    }
  });

  const updated = await updateWindowState((state) => ({
    ...state,
    bounds: { ...state.bounds, width: 640 }
  }), storage);

  assert.equal(updated.windowId, 5);
  assert.equal(updated.bounds.width, 640);
  assert.equal(storage.data.windowState.bounds.width, 640);
});
```

- [ ] **Step 2: Run tests and verify the expected failure**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: FAIL because `src/background/storage.js` does not exist.

- [ ] **Step 3: Implement storage helpers**

Create `discord-mini-tabs-extension/src/background/storage.js`:

```js
import { STORAGE_KEYS } from "../shared/constants.js";
import { createDefaultWindowState, normalizeWindowState } from "../shared/settings.js";

const DEFAULT_STATE = {
  [STORAGE_KEYS.SHORTCUTS]: [],
  [STORAGE_KEYS.WINDOW_STATE]: createDefaultWindowState()
};

function normalizeShortcuts(value) {
  return Array.isArray(value) ? value : [];
}

function getDefaultStorageArea() {
  return chrome.storage.local;
}

export async function getExtensionState(storageArea = getDefaultStorageArea()) {
  const data = await storageArea.get(DEFAULT_STATE);
  return {
    shortcuts: normalizeShortcuts(data[STORAGE_KEYS.SHORTCUTS]),
    windowState: normalizeWindowState(data[STORAGE_KEYS.WINDOW_STATE])
  };
}

export async function setShortcuts(shortcuts, storageArea = getDefaultStorageArea()) {
  const normalized = normalizeShortcuts(shortcuts);
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
```

- [ ] **Step 4: Run tests and verify they pass**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: PASS for URL, settings, shortcuts, and storage tests.

- [ ] **Step 5: Commit storage helpers**

```powershell
git add discord-mini-tabs-extension
git commit -m "feat: add extension storage helpers"
```

## Task 5: Add Window Manager With Chrome API Fakes

**Files:**
- Create: `discord-mini-tabs-extension/test/window-manager.test.js`
- Create: `discord-mini-tabs-extension/src/background/window-manager.js`

- [ ] **Step 1: Write failing window manager tests**

Create `discord-mini-tabs-extension/test/window-manager.test.js`:

```js
import test from "node:test";
import assert from "node:assert/strict";
import {
  closeMiniWindow,
  focusMiniWindow,
  openShortcutInMiniWindow,
  resetMiniWindowPosition,
  saveBoundsFromWindow
} from "../src/background/window-manager.js";

function createFakeStorage(initial = {}) {
  const data = { ...initial };
  return {
    data,
    async get(defaults) {
      return { ...defaults, ...data };
    },
    async set(values) {
      Object.assign(data, values);
    }
  };
}

function createFakeChrome() {
  const calls = [];
  const windowsById = new Map();
  const tabsById = new Map();
  let nextWindowId = 100;
  let nextTabId = 200;

  return {
    calls,
    windowsById,
    tabsById,
    windows: {
      async create(createData) {
        calls.push(["windows.create", createData]);
        const windowId = nextWindowId++;
        const tabId = nextTabId++;
        const tab = { id: tabId, windowId, url: createData.url };
        const window = {
          id: windowId,
          type: createData.type,
          focused: true,
          left: createData.left,
          top: createData.top,
          width: createData.width,
          height: createData.height,
          tabs: [tab]
        };
        windowsById.set(windowId, window);
        tabsById.set(tabId, tab);
        return window;
      },
      async get(windowId, options) {
        calls.push(["windows.get", windowId, options]);
        const window = windowsById.get(windowId);
        if (!window) throw new Error("No window");
        return options?.populate ? window : { ...window, tabs: undefined };
      },
      async update(windowId, updateInfo) {
        calls.push(["windows.update", windowId, updateInfo]);
        const window = windowsById.get(windowId);
        if (!window) throw new Error("No window");
        Object.assign(window, updateInfo);
        return window;
      },
      async remove(windowId) {
        calls.push(["windows.remove", windowId]);
        windowsById.delete(windowId);
      }
    },
    tabs: {
      async update(tabId, updateInfo) {
        calls.push(["tabs.update", tabId, updateInfo]);
        const tab = tabsById.get(tabId);
        if (!tab) throw new Error("No tab");
        Object.assign(tab, updateInfo);
        return tab;
      },
      async setZoom(tabId, zoom) {
        calls.push(["tabs.setZoom", tabId, zoom]);
      }
    }
  };
}

const shortcut = {
  id: "s1",
  name: "Dev Chat",
  type: "text",
  url: "https://discord.com/channels/1/2"
};

test("creates a popup window when no valid window exists", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();

  const result = await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  assert.equal(result.windowId, 100);
  assert.equal(result.tabId, 200);
  assert.equal(storage.data.windowState.windowId, 100);
  assert.equal(storage.data.windowState.tabId, 200);
  assert.equal(storage.data.windowState.lastShortcutId, "s1");
  assert.deepEqual(chromeApi.calls[0][0], "windows.create");
  assert.equal(chromeApi.calls[0][1].type, "popup");
  assert.equal(chromeApi.calls[0][1].width, 420);
  assert.equal(chromeApi.calls[0][1].height, 900);
});

test("reuses existing mini window and tab", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  const nextShortcut = { ...shortcut, id: "s2", url: "https://discord.com/channels/3/4" };
  const result = await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut: nextShortcut });

  assert.equal(result.windowId, 100);
  assert.equal(result.tabId, 200);
  assert.ok(chromeApi.calls.some((call) => call[0] === "tabs.update" && call[2].url === nextShortcut.url));
  assert.equal(storage.data.windowState.lastShortcutId, "s2");
});

test("focuses existing window", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  await focusMiniWindow({ chromeApi, storageArea: storage });

  assert.ok(chromeApi.calls.some((call) => call[0] === "windows.update" && call[2].focused === true));
});

test("closes mini window and clears ids", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  await closeMiniWindow({ chromeApi, storageArea: storage });

  assert.equal(storage.data.windowState.windowId, null);
  assert.equal(storage.data.windowState.tabId, null);
});

test("resets position but keeps size", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  const state = await resetMiniWindowPosition({ chromeApi, storageArea: storage });

  assert.equal(state.bounds.left, null);
  assert.equal(state.bounds.top, null);
  assert.equal(state.bounds.width, 420);
});

test("saves bounds from popup windows only", async () => {
  const storage = createFakeStorage();
  const saved = await saveBoundsFromWindow(
    { id: 1, type: "popup", left: 10, top: 20, width: 500, height: 700 },
    { storageArea: storage, expectedWindowId: 1 }
  );

  assert.equal(saved.bounds.left, 10);
  assert.equal(saved.bounds.width, 500);
});
```

- [ ] **Step 2: Run tests and verify the expected failure**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: FAIL because `src/background/window-manager.js` does not exist.

- [ ] **Step 3: Implement the window manager**

Create `discord-mini-tabs-extension/src/background/window-manager.js`:

```js
import { DEFAULT_BOUNDS } from "../shared/constants.js";
import { clampBounds, normalizeWindowState } from "../shared/settings.js";
import { getExtensionState, setWindowState, updateWindowState } from "./storage.js";

function getDefaultChromeApi() {
  return chrome;
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
    const existingTab = currentWindow.tabs?.find((tab) => tab.id === windowState.tabId) ?? currentWindow.tabs?.[0] ?? null;
    if (!existingTab?.id) {
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
  const state = await getExtensionState(storageArea);
  const windowState = normalizeWindowState(state.windowState);
  const existing = await getWindowWithTab(chromeApi, windowState);

  if (existing) {
    await chromeApi.windows.update(existing.window.id, { focused: true });
    const tab = await chromeApi.tabs.update(existing.tab.id, { url: shortcut.url, active: true });
    await applyZoom(chromeApi, existing.tab.id, windowState.zoom);
    const nextState = await setWindowState(
      {
        ...windowState,
        windowId: existing.window.id,
        tabId: tab.id ?? existing.tab.id,
        lastShortcutId: shortcut.id
      },
      storageArea
    );
    return nextState;
  }

  const bounds = clampBounds(windowState.bounds ?? DEFAULT_BOUNDS);
  const createdWindow = await chromeApi.windows.create(boundsToCreateData(bounds, shortcut.url));
  const createdTab = createdWindow.tabs?.[0] ?? null;
  if (!createdWindow.id || !createdTab?.id) {
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

export async function resetMiniWindowPosition({ chromeApi = getDefaultChromeApi(), storageArea }) {
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
      await setWindowState({ ...state, windowId: null, tabId: null }, storageArea);
    }
  }

  return state;
}

export async function updateMiniWindowSettings({ chromeApi = getDefaultChromeApi(), storageArea, bounds, zoom }) {
  const state = await updateWindowState(
    (windowState) => ({
      ...windowState,
      bounds: clampBounds({ ...windowState.bounds, ...bounds }),
      zoom
    }),
    storageArea
  );

  if (Number.isInteger(state.windowId)) {
    try {
      const updateInfo = { width: state.bounds.width, height: state.bounds.height };
      if (Number.isInteger(state.bounds.left)) updateInfo.left = state.bounds.left;
      if (Number.isInteger(state.bounds.top)) updateInfo.top = state.bounds.top;
      await chromeApi.windows.update(state.windowId, updateInfo);
      if (Number.isInteger(state.tabId)) {
        await applyZoom(chromeApi, state.tabId, state.zoom);
      }
    } catch {
      await setWindowState({ ...state, windowId: null, tabId: null }, storageArea);
    }
  }

  return state;
}

export async function saveBoundsFromWindow(window, { storageArea, expectedWindowId }) {
  if (!window || window.type !== "popup" || window.id !== expectedWindowId) {
    return null;
  }

  return updateWindowState(
    (windowState) => ({
      ...windowState,
      bounds: clampBounds({
        left: window.left,
        top: window.top,
        width: window.width,
        height: window.height
      })
    }),
    storageArea
  );
}
```

- [ ] **Step 4: Run tests and verify they pass**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: PASS for all tests, including `window-manager.test.js`.

- [ ] **Step 5: Commit window manager**

```powershell
git add discord-mini-tabs-extension
git commit -m "feat: manage reusable discord mini window"
```

## Task 6: Replace The Service Worker With Runtime Message Routing

**Files:**
- Modify: `discord-mini-tabs-extension/src/background/service-worker.js`

- [ ] **Step 1: Replace the service worker implementation**

Modify `discord-mini-tabs-extension/src/background/service-worker.js`:

```js
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
    suggestedName: activeTab.title?.replace(/\s+-\s+Discord$/, "").trim() || "Discord channel"
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
    .catch((error) => sendResponse({ ok: false, error: error.message }));
  return true;
});

chrome.windows.onRemoved.addListener(async (windowId) => {
  const state = await getExtensionState();
  if (state.windowState.windowId === windowId) {
    await setWindowState({ ...state.windowState, windowId: null, tabId: null });
  }
});

chrome.windows.onBoundsChanged.addListener((changedWindow) => {
  clearTimeout(boundsSaveTimer);
  boundsSaveTimer = setTimeout(async () => {
    const state = await getExtensionState();
    await saveBoundsFromWindow(changedWindow, {
      expectedWindowId: state.windowState.windowId
    });
  }, 400);
});
```

- [ ] **Step 2: Run tests**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: PASS. The service worker itself is exercised manually in later tasks because it depends on live Chrome extension APIs.

- [ ] **Step 3: Commit service worker routing**

```powershell
git add discord-mini-tabs-extension/src/background/service-worker.js
git commit -m "feat: route extension runtime messages"
```

## Task 7: Add Popup View Model Tests

**Files:**
- Create: `discord-mini-tabs-extension/test/view-model.test.js`
- Create: `discord-mini-tabs-extension/src/popup/view-model.js`

- [ ] **Step 1: Write failing view model tests**

Create `discord-mini-tabs-extension/test/view-model.test.js`:

```js
import test from "node:test";
import assert from "node:assert/strict";
import { buildPopupModel, formatWindowStatus, formatZoomPercent } from "../src/popup/view-model.js";

const shortcuts = [
  { id: "a", name: "Dev Chat", type: "text", url: "https://discord.com/channels/1/2" },
  { id: "b", name: "Team Call", type: "voice", url: "https://discord.com/channels/3/4" }
];

test("formats window status", () => {
  assert.equal(formatWindowStatus({ windowId: null }), "Closed");
  assert.equal(formatWindowStatus({ windowId: 10 }), "Open");
});

test("formats zoom percent", () => {
  assert.equal(formatZoomPercent(0.9), "90%");
  assert.equal(formatZoomPercent(1), "100%");
});

test("builds grouped popup model", () => {
  const model = buildPopupModel({
    shortcuts,
    query: "",
    activeType: "text",
    windowState: {
      windowId: 1,
      tabId: 2,
      bounds: { left: null, top: null, width: 420, height: 900 },
      zoom: 0.9,
      lastShortcutId: "a"
    }
  });

  assert.equal(model.status, "Open");
  assert.equal(model.zoomLabel, "90%");
  assert.equal(model.activeShortcuts.length, 1);
  assert.equal(model.activeShortcuts[0].id, "a");
  assert.equal(model.textCount, 1);
  assert.equal(model.voiceCount, 1);
});

test("applies search before active type", () => {
  const model = buildPopupModel({
    shortcuts,
    query: "team",
    activeType: "voice",
    windowState: { windowId: null, zoom: 1, bounds: { width: 420, height: 900 } }
  });

  assert.equal(model.activeShortcuts.length, 1);
  assert.equal(model.activeShortcuts[0].id, "b");
});
```

- [ ] **Step 2: Run tests and verify the expected failure**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: FAIL because `src/popup/view-model.js` does not exist.

- [ ] **Step 3: Implement popup view model**

Create `discord-mini-tabs-extension/src/popup/view-model.js`:

```js
import { SHORTCUT_TYPES } from "../shared/constants.js";
import { filterShortcuts, splitShortcutsByType } from "../shared/shortcuts.js";

export function formatWindowStatus(windowState) {
  return Number.isInteger(windowState?.windowId) ? "Open" : "Closed";
}

export function formatZoomPercent(zoom) {
  return `${Math.round(Number(zoom) * 100)}%`;
}

export function buildPopupModel({ shortcuts, query, activeType, windowState }) {
  const filtered = filterShortcuts(shortcuts, query);
  const grouped = splitShortcutsByType(filtered);
  const normalizedType = activeType === SHORTCUT_TYPES.VOICE ? SHORTCUT_TYPES.VOICE : SHORTCUT_TYPES.TEXT;

  return {
    status: formatWindowStatus(windowState),
    zoomLabel: formatZoomPercent(windowState?.zoom ?? 0.9),
    boundsLabel: `${windowState?.bounds?.width ?? 420} x ${windowState?.bounds?.height ?? 900}`,
    activeType: normalizedType,
    activeShortcuts: grouped[normalizedType],
    textCount: grouped.text.length,
    voiceCount: grouped.voice.length
  };
}
```

- [ ] **Step 4: Run tests and verify they pass**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: PASS for all tests, including `view-model.test.js`.

- [ ] **Step 5: Commit view model**

```powershell
git add discord-mini-tabs-extension
git commit -m "feat: add popup view model"
```

## Task 8: Build The Popup UI And Controller

**Files:**
- Modify: `discord-mini-tabs-extension/src/popup/popup.html`
- Modify: `discord-mini-tabs-extension/src/popup/popup.css`
- Modify: `discord-mini-tabs-extension/src/popup/popup.js`

- [ ] **Step 1: Replace popup markup**

Modify `discord-mini-tabs-extension/src/popup/popup.html`:

```html
<!doctype html>
<html lang="en">
  <head>
    <meta charset="utf-8">
    <meta name="viewport" content="width=device-width, initial-scale=1">
    <title>Discord Mini Tabs</title>
    <link rel="stylesheet" href="./popup.css">
  </head>
  <body>
    <main class="shell">
      <header class="topbar">
        <div>
          <h1>Discord Mini Tabs</h1>
          <p id="windowMeta">Closed · 420 x 900 · 90%</p>
        </div>
        <button id="saveCurrentButton" type="button">Save current</button>
      </header>

      <section id="feedback" class="feedback" hidden></section>

      <section class="controls" aria-label="Mini window controls">
        <button id="focusButton" type="button">Focus</button>
        <button id="closeButton" type="button">Close</button>
        <button id="resetButton" type="button">Reset position</button>
      </section>

      <label class="field">
        <span>Search</span>
        <input id="searchInput" type="search" autocomplete="off" aria-label="Search shortcuts">
      </label>

      <div class="segments" role="tablist" aria-label="Shortcut type">
        <button id="textTab" class="segment active" type="button" data-type="text">Text <span id="textCount">0</span></button>
        <button id="voiceTab" class="segment" type="button" data-type="voice">Voice <span id="voiceCount">0</span></button>
      </div>

      <section id="shortcutList" class="shortcut-list" aria-label="Saved shortcuts"></section>

      <form id="shortcutForm" class="panel">
        <h2 id="formTitle">Add shortcut</h2>
        <input id="editingId" type="hidden">
        <label class="field">
          <span>Name</span>
          <input id="shortcutName" type="text" autocomplete="off" required aria-label="Shortcut name">
        </label>
        <label class="field">
          <span>URL</span>
          <input id="shortcutUrl" type="url" required aria-label="Discord channel URL">
        </label>
        <label class="field">
          <span>Type</span>
          <select id="shortcutType">
            <option value="text">Text</option>
            <option value="voice">Voice</option>
          </select>
        </label>
        <div class="form-actions">
          <button id="cancelEditButton" type="button" hidden>Cancel</button>
          <button type="submit">Save shortcut</button>
        </div>
      </form>

      <form id="settingsForm" class="panel settings">
        <h2>Window settings</h2>
        <div class="settings-grid">
          <label class="field">
            <span>Width</span>
            <input id="widthInput" type="number" min="320" max="1200" step="10">
          </label>
          <label class="field">
            <span>Height</span>
            <input id="heightInput" type="number" min="480" max="1400" step="10">
          </label>
          <label class="field">
            <span>Zoom</span>
            <input id="zoomInput" type="number" min="67" max="125" step="5">
          </label>
        </div>
        <button type="submit">Apply settings</button>
      </form>
    </main>
    <script type="module" src="./popup.js"></script>
  </body>
</html>
```

- [ ] **Step 2: Replace popup styling**

Modify `discord-mini-tabs-extension/src/popup/popup.css`:

```css
:root {
  color-scheme: dark;
  font-family: Inter, ui-sans-serif, system-ui, -apple-system, BlinkMacSystemFont, "Segoe UI", sans-serif;
  background: #17181c;
  color: #f4f5f8;
}

* {
  box-sizing: border-box;
}

body {
  margin: 0;
  width: 380px;
  background: #17181c;
}

button,
input,
select {
  font: inherit;
}

button {
  border: 0;
  border-radius: 6px;
  background: #5865f2;
  color: #ffffff;
  cursor: pointer;
  min-height: 32px;
  padding: 0 10px;
}

button:hover {
  background: #6974f5;
}

button.secondary,
.controls button,
.form-actions button:first-child {
  background: #2a2d36;
  color: #f4f5f8;
}

.shell {
  display: grid;
  gap: 12px;
  padding: 14px;
}

.topbar {
  display: flex;
  align-items: flex-start;
  justify-content: space-between;
  gap: 12px;
}

h1,
h2,
p {
  margin: 0;
}

h1 {
  font-size: 18px;
  line-height: 1.2;
}

h2 {
  font-size: 13px;
  color: #c9ced9;
}

p,
.meta,
.shortcut-url,
.field span {
  color: #aeb3c2;
  font-size: 12px;
}

.feedback {
  border-radius: 6px;
  background: #2d2330;
  color: #ffd6e7;
  padding: 8px 10px;
  font-size: 12px;
}

.controls,
.form-actions {
  display: flex;
  gap: 8px;
  flex-wrap: wrap;
}

.field {
  display: grid;
  gap: 5px;
}

input,
select {
  width: 100%;
  border: 1px solid #363a46;
  border-radius: 6px;
  background: #101116;
  color: #f4f5f8;
  min-height: 34px;
  padding: 7px 9px;
}

.segments {
  display: grid;
  grid-template-columns: 1fr 1fr;
  gap: 6px;
}

.segment {
  background: #242731;
  color: #c9ced9;
}

.segment.active {
  background: #5865f2;
  color: #ffffff;
}

.shortcut-list {
  display: grid;
  gap: 8px;
  max-height: 230px;
  overflow: auto;
}

.shortcut-card {
  display: grid;
  gap: 8px;
  border: 1px solid #303442;
  border-radius: 8px;
  background: #1f222b;
  padding: 10px;
}

.shortcut-main {
  min-width: 0;
}

.shortcut-name {
  font-size: 14px;
  font-weight: 700;
  overflow-wrap: anywhere;
}

.shortcut-url {
  margin-top: 2px;
  overflow-wrap: anywhere;
}

.shortcut-actions {
  display: flex;
  gap: 6px;
}

.shortcut-actions button {
  flex: 1;
}

.empty {
  border: 1px dashed #3b4050;
  border-radius: 8px;
  color: #aeb3c2;
  padding: 18px;
  text-align: center;
}

.panel {
  display: grid;
  gap: 10px;
  border-top: 1px solid #2b2f3a;
  padding-top: 12px;
}

.settings-grid {
  display: grid;
  grid-template-columns: 1fr 1fr 1fr;
  gap: 8px;
}
```

- [ ] **Step 3: Replace popup controller**

Modify `discord-mini-tabs-extension/src/popup/popup.js`:

```js
import { MESSAGE_TYPES, SHORTCUT_TYPES } from "../shared/constants.js";
import { compactDiscordUrl } from "../shared/url.js";
import { buildPopupModel } from "./view-model.js";

const elements = {
  windowMeta: document.querySelector("#windowMeta"),
  feedback: document.querySelector("#feedback"),
  saveCurrentButton: document.querySelector("#saveCurrentButton"),
  focusButton: document.querySelector("#focusButton"),
  closeButton: document.querySelector("#closeButton"),
  resetButton: document.querySelector("#resetButton"),
  searchInput: document.querySelector("#searchInput"),
  textTab: document.querySelector("#textTab"),
  voiceTab: document.querySelector("#voiceTab"),
  textCount: document.querySelector("#textCount"),
  voiceCount: document.querySelector("#voiceCount"),
  shortcutList: document.querySelector("#shortcutList"),
  shortcutForm: document.querySelector("#shortcutForm"),
  formTitle: document.querySelector("#formTitle"),
  editingId: document.querySelector("#editingId"),
  shortcutName: document.querySelector("#shortcutName"),
  shortcutUrl: document.querySelector("#shortcutUrl"),
  shortcutType: document.querySelector("#shortcutType"),
  cancelEditButton: document.querySelector("#cancelEditButton"),
  settingsForm: document.querySelector("#settingsForm"),
  widthInput: document.querySelector("#widthInput"),
  heightInput: document.querySelector("#heightInput"),
  zoomInput: document.querySelector("#zoomInput")
};

const state = {
  shortcuts: [],
  windowState: null,
  activeType: SHORTCUT_TYPES.TEXT,
  query: ""
};

async function sendMessage(type, payload = {}) {
  const response = await chrome.runtime.sendMessage({ type, payload });
  if (!response?.ok) {
    throw new Error(response?.error ?? "Extension request failed.");
  }
  return response.data;
}

function showFeedback(message) {
  elements.feedback.textContent = message;
  elements.feedback.hidden = false;
}

function clearFeedback() {
  elements.feedback.textContent = "";
  elements.feedback.hidden = true;
}

function setFormMode(shortcut = null) {
  elements.editingId.value = shortcut?.id ?? "";
  elements.shortcutName.value = shortcut?.name ?? "";
  elements.shortcutUrl.value = shortcut?.url ?? "";
  elements.shortcutType.value = shortcut?.type ?? state.activeType;
  elements.formTitle.textContent = shortcut ? "Edit shortcut" : "Add shortcut";
  elements.cancelEditButton.hidden = !shortcut;
}

function renderShortcuts(model) {
  elements.shortcutList.replaceChildren();

  if (model.activeShortcuts.length === 0) {
    const empty = document.createElement("div");
    empty.className = "empty";
    empty.textContent = "No shortcuts match this view.";
    elements.shortcutList.append(empty);
    return;
  }

  for (const shortcut of model.activeShortcuts) {
    const card = document.createElement("article");
    card.className = "shortcut-card";

    const main = document.createElement("div");
    main.className = "shortcut-main";

    const name = document.createElement("div");
    name.className = "shortcut-name";
    name.textContent = shortcut.name;

    const url = document.createElement("div");
    url.className = "shortcut-url";
    url.textContent = compactDiscordUrl(shortcut.url);

    const actions = document.createElement("div");
    actions.className = "shortcut-actions";

    const openButton = document.createElement("button");
    openButton.type = "button";
    openButton.textContent = "Open";
    openButton.addEventListener("click", () => openShortcut(shortcut.id));

    const editButton = document.createElement("button");
    editButton.type = "button";
    editButton.className = "secondary";
    editButton.textContent = "Edit";
    editButton.addEventListener("click", () => setFormMode(shortcut));

    const deleteButton = document.createElement("button");
    deleteButton.type = "button";
    deleteButton.className = "secondary";
    deleteButton.textContent = "Delete";
    deleteButton.addEventListener("click", () => deleteShortcut(shortcut.id));

    main.append(name, url);
    actions.append(openButton, editButton, deleteButton);
    card.append(main, actions);
    elements.shortcutList.append(card);
  }
}

function render() {
  const model = buildPopupModel({
    shortcuts: state.shortcuts,
    query: state.query,
    activeType: state.activeType,
    windowState: state.windowState
  });

  elements.windowMeta.textContent = `${model.status} · ${model.boundsLabel} · ${model.zoomLabel}`;
  elements.textCount.textContent = String(model.textCount);
  elements.voiceCount.textContent = String(model.voiceCount);
  elements.textTab.classList.toggle("active", model.activeType === SHORTCUT_TYPES.TEXT);
  elements.voiceTab.classList.toggle("active", model.activeType === SHORTCUT_TYPES.VOICE);
  elements.widthInput.value = state.windowState?.bounds?.width ?? 420;
  elements.heightInput.value = state.windowState?.bounds?.height ?? 900;
  elements.zoomInput.value = Math.round((state.windowState?.zoom ?? 0.9) * 100);

  renderShortcuts(model);
}

async function refresh() {
  const data = await sendMessage(MESSAGE_TYPES.GET_STATE);
  state.shortcuts = data.shortcuts;
  state.windowState = data.windowState;
  render();
}

async function runAction(action) {
  clearFeedback();
  try {
    await action();
    await refresh();
  } catch (error) {
    showFeedback(error.message);
  }
}

async function openShortcut(id) {
  await runAction(() => sendMessage(MESSAGE_TYPES.OPEN_SHORTCUT, { id }));
}

async function deleteShortcut(id) {
  await runAction(() => sendMessage(MESSAGE_TYPES.DELETE_SHORTCUT, { id }));
}

elements.searchInput.addEventListener("input", () => {
  state.query = elements.searchInput.value;
  render();
});

elements.textTab.addEventListener("click", () => {
  state.activeType = SHORTCUT_TYPES.TEXT;
  render();
});

elements.voiceTab.addEventListener("click", () => {
  state.activeType = SHORTCUT_TYPES.VOICE;
  render();
});

elements.shortcutForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  const payload = {
    id: elements.editingId.value || undefined,
    name: elements.shortcutName.value,
    url: elements.shortcutUrl.value,
    type: elements.shortcutType.value
  };

  await runAction(async () => {
    if (payload.id) {
      await sendMessage(MESSAGE_TYPES.UPDATE_SHORTCUT, payload);
    } else {
      await sendMessage(MESSAGE_TYPES.CREATE_SHORTCUT, payload);
    }
    setFormMode(null);
  });
});

elements.cancelEditButton.addEventListener("click", () => setFormMode(null));

elements.saveCurrentButton.addEventListener("click", async () => {
  await runAction(async () => {
    const active = await sendMessage(MESSAGE_TYPES.READ_ACTIVE_DISCORD_TAB);
    setFormMode({
      name: active.suggestedName,
      url: active.url,
      type: state.activeType
    });
  });
});

elements.focusButton.addEventListener("click", () => runAction(() => sendMessage(MESSAGE_TYPES.FOCUS_WINDOW)));
elements.closeButton.addEventListener("click", () => runAction(() => sendMessage(MESSAGE_TYPES.CLOSE_WINDOW)));
elements.resetButton.addEventListener("click", () => runAction(() => sendMessage(MESSAGE_TYPES.RESET_POSITION)));

elements.settingsForm.addEventListener("submit", async (event) => {
  event.preventDefault();
  await runAction(() =>
    sendMessage(MESSAGE_TYPES.UPDATE_WINDOW_SETTINGS, {
      bounds: {
        width: Number(elements.widthInput.value),
        height: Number(elements.heightInput.value)
      },
      zoom: Number(elements.zoomInput.value) / 100
    })
  );
});

refresh().catch((error) => showFeedback(error.message));
```

- [ ] **Step 4: Run tests**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: PASS for all tests.

- [ ] **Step 5: Manually load the extension shell in Chrome**

Run Chrome manually:

```text
chrome://extensions
```

Enable Developer mode, click Load unpacked, and select:

```text
C:\Users\RGB\rtk\discord-mini-tabs-extension
```

Expected: Chrome accepts the extension with no manifest error. Opening the extension action shows the popup UI.

- [ ] **Step 6: Commit popup UI**

```powershell
git add discord-mini-tabs-extension
git commit -m "feat: build discord shortcut popup"
```

## Task 9: Add Manual Verification Checklist And Run Final Checks

**Files:**
- Create: `discord-mini-tabs-extension/MANUAL_TESTS.md`

- [ ] **Step 1: Create manual verification checklist**

Create `discord-mini-tabs-extension/MANUAL_TESTS.md`:

```markdown
# Manual Tests

## Load Unpacked

1. Open `chrome://extensions`.
2. Enable Developer mode.
3. Load the `discord-mini-tabs-extension` directory.
4. Confirm Chrome shows no manifest or service worker registration errors.

## Shortcut Management

1. Add a text shortcut with `https://discord.com/channels/123/456`.
2. Confirm the shortcut appears in the Text list.
3. Edit the shortcut name.
4. Search for the new name.
5. Delete the shortcut.
6. Confirm the list updates without reopening the popup.

## Save Current Discord Channel

1. Open a real Discord channel in a normal Chrome tab.
2. Open the extension popup.
3. Click Save current.
4. Confirm the URL and suggested name populate the form.
5. Save the shortcut.

## Mini Window

1. Open a saved text shortcut.
2. Confirm one Chrome popup window opens.
3. Send a Discord chat message in that window.
4. Open another shortcut.
5. Confirm the same popup window is reused.
6. Resize the popup window.
7. Close and reopen a shortcut.
8. Confirm the remembered size is used.

## Voice URL

1. Save a Discord voice channel URL as type Voice.
2. Open it from the Voice list.
3. Confirm Discord web displays the voice channel UI.
4. Join or leave voice manually through Discord web.

## Window Controls

1. Click Focus and confirm the mini window comes forward.
2. Click Close and confirm the mini window closes.
3. Click Reset position and confirm the next open lets Chrome choose a valid position with the saved size.

## Settings

1. Set width to `420`, height to `900`, and zoom to `90`.
2. Open a shortcut.
3. Confirm the mini window size and Discord tab zoom are applied.
4. Change width to `500` and zoom to `100`.
5. Confirm the existing mini window updates.

## Stability Smoke Test

1. Switch between several text and voice shortcuts repeatedly.
2. Confirm only one Discord mini window remains open.
3. Close the mini window manually.
4. Open a shortcut again.
5. Confirm the extension recovers without errors.
```

- [ ] **Step 2: Run logic tests**

Run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: PASS for every `node:test` file.

- [ ] **Step 3: Inspect service worker errors manually**

In Chrome:

```text
chrome://extensions
```

Open the service worker inspection link for Discord Mini Tabs.

Expected: no uncaught exceptions after opening popup, adding a shortcut, opening a shortcut, focusing, closing, and changing settings.

- [ ] **Step 4: Confirm git status only contains intended files**

Run:

```powershell
git status --short -- discord-mini-tabs-extension
```

Expected: only `discord-mini-tabs-extension` files from this plan appear.

- [ ] **Step 5: Commit verification docs**

```powershell
git add discord-mini-tabs-extension/MANUAL_TESTS.md
git commit -m "docs: add discord mini tabs manual tests"
```

## Final Verification Gate

Before claiming the implementation is complete, run:

```powershell
cd discord-mini-tabs-extension
npm test
```

Expected: all Node tests pass.

Then complete the manual checklist in `discord-mini-tabs-extension/MANUAL_TESTS.md` using Chrome load-unpacked mode. The implementation is not complete until both the automated tests and the manual Chrome checks pass.

## Spec Coverage Review

- One reusable Discord popup window: Task 5 and Task 6.
- Default `420x900`, configurable size, remembered bounds: Task 2, Task 5, Task 8.
- Adjustable zoom defaulting to `90%`: Task 2, Task 5, Task 8.
- Text/Voice groups and search: Task 3, Task 7, Task 8.
- Manual URL entry: Task 8.
- Save current Discord channel: Task 6 and Task 8.
- Local storage: Task 4.
- Minimal permissions and MV3 manifest: Task 1.
- No Discord iframe, no content script, no DOM scraping: Task 1 manifest and Task 6 architecture.
- Error recovery for missing window/tab and invalid URLs: Task 2, Task 5, Task 6.
- Automated logic tests and manual stability checks: Tasks 2 through 9.
