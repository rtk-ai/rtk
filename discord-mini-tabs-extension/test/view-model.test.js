import test from "node:test";
import assert from "node:assert/strict";
import {
  buildPopupModel,
  formatWindowStatus,
  formatZoomPercent
} from "../src/popup/view-model.js";

const textUrl = "https://discord.com/channels/123456789012345678/987654321098765432";
const voiceUrl = "https://discord.com/channels/223456789012345678/887654321098765432";

const shortcuts = [
  { id: "a", name: "Dev Chat", type: "text", url: textUrl },
  { id: "b", name: "Team Call", type: "voice", url: voiceUrl }
];

test("formats mini window status", () => {
  assert.equal(formatWindowStatus({ windowId: null }), "Closed");
  assert.equal(formatWindowStatus({ windowId: 10 }), "Open");
});

test("formats zoom as rounded percent", () => {
  assert.equal(formatZoomPercent(0.9), "90%");
  assert.equal(formatZoomPercent(1), "100%");
});

test("builds popup model grouped by active text shortcuts", () => {
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

test("filters popup shortcuts before applying active type", () => {
  const model = buildPopupModel({
    shortcuts,
    query: "team",
    activeType: "voice",
    windowState: {
      windowId: null,
      zoom: 1,
      bounds: { width: 420, height: 900 }
    }
  });

  assert.equal(model.activeShortcuts.length, 1);
  assert.equal(model.activeShortcuts[0].id, "b");
});

test("defaults popup zoom label when window state has no zoom", () => {
  const model = buildPopupModel({
    shortcuts,
    query: "",
    activeType: "text",
    windowState: {
      windowId: null,
      bounds: { width: 420, height: 900 }
    }
  });

  assert.equal(model.zoomLabel, "90%");
});
