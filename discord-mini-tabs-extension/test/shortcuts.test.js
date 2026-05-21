import test from "node:test";
import assert from "node:assert/strict";
import {
  createShortcut,
  deleteShortcut,
  filterShortcuts,
  findShortcut,
  splitShortcutsByType,
  updateShortcut
} from "../src/shared/shortcuts.js";

const fixedNow = () => "2026-05-21T00:00:00.000Z";
const fixedId = () => "shortcut-1";
const textUrl = "https://discord.com/channels/123456789012345678/987654321098765432";
const voiceUrl = "https://discord.com/channels/223457890234567890/887650987650987650";

test("creates normalized text shortcut", () => {
  const shortcut = createShortcut(
    {
      name: " dev chat ",
      type: "text",
      url: `${textUrl}/?jump=999`
    },
    { idFactory: fixedId, now: fixedNow }
  );

  assert.deepEqual(shortcut, {
    id: "shortcut-1",
    name: "dev chat",
    type: "text",
    url: textUrl,
    createdAt: "2026-05-21T00:00:00.000Z",
    updatedAt: "2026-05-21T00:00:00.000Z"
  });
});

test("rejects empty shortcut name", () => {
  assert.throws(
    () => createShortcut({ name: " ", type: "text", url: textUrl }),
    /name/
  );
});

test("updates shortcut while preserving id and createdAt", () => {
  const original = createShortcut(
    { name: "dev chat", type: "text", url: textUrl },
    { idFactory: fixedId, now: fixedNow }
  );
  const updated = updateShortcut(original, {
    name: "team call",
    type: "voice",
    url: voiceUrl,
    now: () => "2026-05-21T01:00:00.000Z"
  });

  assert.equal(updated.id, "shortcut-1");
  assert.equal(updated.createdAt, "2026-05-21T00:00:00.000Z");
  assert.equal(updated.updatedAt, "2026-05-21T01:00:00.000Z");
  assert.equal(updated.name, "team call");
  assert.equal(updated.type, "voice");
  assert.equal(updated.url, voiceUrl);
});

test("deletes shortcut by id", () => {
  const shortcuts = [
    { id: "a", name: "A", type: "text", url: textUrl },
    { id: "b", name: "B", type: "voice", url: voiceUrl }
  ];
  assert.deepEqual(deleteShortcut(shortcuts, "a").map((item) => item.id), ["b"]);
});

test("finds shortcuts by id and returns null when missing", () => {
  const shortcuts = [
    { id: "a", name: "Dev Chat", type: "text", url: textUrl },
    { id: "b", name: "Team Call", type: "voice", url: voiceUrl }
  ];

  assert.equal(findShortcut(shortcuts, "b")?.name, "Team Call");
  assert.equal(findShortcut(shortcuts, "missing"), null);
});

test("filters shortcuts by name and url", () => {
  const shortcuts = [
    { id: "a", name: "Dev Chat", type: "text", url: textUrl },
    { id: "b", name: "Team Call", type: "voice", url: voiceUrl }
  ];
  assert.deepEqual(filterShortcuts(shortcuts, "team").map((item) => item.id), ["b"]);
  assert.deepEqual(filterShortcuts(shortcuts, "123456").map((item) => item.id), ["a"]);
  assert.deepEqual(filterShortcuts(shortcuts, "voice").map((item) => item.id), []);
});

test("splits shortcuts by type", () => {
  const result = splitShortcutsByType([
    { id: "a", name: "Dev Chat", type: "text", url: textUrl },
    { id: "b", name: "Team Call", type: "voice", url: voiceUrl }
  ]);

  assert.deepEqual(result.text.map((item) => item.id), ["a"]);
  assert.deepEqual(result.voice.map((item) => item.id), ["b"]);
});

test("list helpers tolerate malformed shortcut lists", () => {
  assert.deepEqual(deleteShortcut(null, "a"), []);
  assert.equal(findShortcut(null, "a"), null);
  assert.deepEqual(filterShortcuts(null, "dev"), []);
  assert.deepEqual(splitShortcutsByType(null), { text: [], voice: [] });
});

test("list helpers skip malformed shortcut items", () => {
  const shortcuts = [
    null,
    { id: "a", name: "Dev Chat", type: "text", url: textUrl },
    { id: "b", name: "Team Call", type: "voice", url: voiceUrl },
    { id: "broken", name: null, type: "text", url: null }
  ];

  assert.deepEqual(deleteShortcut(shortcuts, "a").map((item) => item.id), ["b"]);
  assert.equal(findShortcut(shortcuts, "b")?.name, "Team Call");
  assert.deepEqual(filterShortcuts(shortcuts, "TEAM").map((item) => item.id), ["b"]);
  assert.deepEqual(splitShortcutsByType(shortcuts).text.map((item) => item.id), ["a"]);
});
