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

test("clampBounds handles null input", () => {
  assert.deepEqual(clampBounds(null), {
    left: null,
    top: null,
    width: 420,
    height: 900
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

test("normalizeWindowState handles null input", () => {
  assert.deepEqual(normalizeWindowState(null), createDefaultWindowState());
});
