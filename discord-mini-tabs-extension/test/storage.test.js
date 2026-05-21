import test from "node:test";
import assert from "node:assert/strict";
import {
  getExtensionState,
  setShortcuts,
  setWindowState,
  updateWindowState
} from "../src/background/storage.js";

const textUrl = "https://discord.com/channels/123456789012345678/987654321098765432";

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
  await setShortcuts([{ id: "a", name: "Dev", type: "text", url: textUrl }], storage);
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
