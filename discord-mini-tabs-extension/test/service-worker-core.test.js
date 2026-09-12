import test from "node:test";
import assert from "node:assert/strict";
import { createServiceWorkerController } from "../src/background/service-worker-core.js";
import { getExtensionState, setShortcuts, setWindowState } from "../src/background/storage.js";
import { saveBoundsFromWindow } from "../src/background/window-manager.js";
import { MESSAGE_TYPES } from "../src/shared/constants.js";
import { createShortcut, deleteShortcut, findShortcut, updateShortcut } from "../src/shared/shortcuts.js";
import { validateDiscordChannelUrl } from "../src/shared/url.js";

const firstUrl = "https://discord.com/channels/123456789012345678/987654321098765432";
const secondUrl = "https://discord.com/channels/223456789012345678/887654321098765432";

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

function createDeferred() {
  let resolve;
  const promise = new Promise((promiseResolve) => {
    resolve = promiseResolve;
  });
  return { promise, resolve };
}

async function flushMicrotasks() {
  await Promise.resolve();
  await Promise.resolve();
  await Promise.resolve();
}

function createController(storage, overrides = {}) {
  return createServiceWorkerController({
    chromeApi: {
      tabs: {
        async query() {
          return [];
        }
      }
    },
    getExtensionState: () => getExtensionState(storage),
    setShortcuts: (shortcuts) => setShortcuts(shortcuts, storage),
    setWindowState: (windowState) => setWindowState(windowState, storage),
    createShortcut,
    deleteShortcut,
    findShortcut,
    updateShortcut,
    validateDiscordChannelUrl,
    openShortcutInMiniWindow: async ({ shortcut }) => ({ lastShortcutId: shortcut.id }),
    focusMiniWindow: async () => 1,
    closeMiniWindow: async () => ({}),
    resetMiniWindowPosition: async () => ({}),
    updateMiniWindowSettings: async ({ bounds, zoom }) => ({ bounds, zoom }),
    saveBoundsFromWindow: (window, options) =>
      saveBoundsFromWindow(window, { storageArea: storage, ...options }),
    ...overrides
  });
}

test("serializes concurrent shortcut creates and preserves both shortcuts", async () => {
  const storage = createFakeStorage();
  const controller = createController(storage);

  await Promise.all([
    controller.handleMessage({
      type: MESSAGE_TYPES.CREATE_SHORTCUT,
      payload: { name: "First", type: "text", url: firstUrl }
    }),
    controller.handleMessage({
      type: MESSAGE_TYPES.CREATE_SHORTCUT,
      payload: { name: "Second", type: "text", url: secondUrl }
    })
  ]);

  assert.equal(storage.data.shortcuts.length, 2);
  assert.deepEqual(
    storage.data.shortcuts.map((shortcut) => shortcut.name).sort(),
    ["First", "Second"]
  );
});

test("ignores unrelated window bounds events without cancelling a pending mini save", async () => {
  const storage = createFakeStorage({
    windowState: {
      windowId: 1,
      tabId: 10,
      bounds: { left: 1, top: 2, width: 420, height: 900 },
      zoom: 0.9,
      lastShortcutId: null
    }
  });
  const scheduled = [];
  let nextTimerId = 1;
  const controller = createController(storage, {
    setTimeoutFn(callback) {
      const timer = { id: nextTimerId++, callback, cancelled: false };
      scheduled.push(timer);
      return timer.id;
    },
    clearTimeoutFn(timerId) {
      const timer = scheduled.find((item) => item.id === timerId);
      if (timer) {
        timer.cancelled = true;
      }
    },
    debounceMs: 400
  });

  await controller.handleBoundsChanged({
    id: 1,
    type: "popup",
    left: 50,
    top: 60,
    width: 640,
    height: 820
  });
  await controller.handleBoundsChanged({
    id: 2,
    type: "normal",
    left: 500,
    top: 600,
    width: 700,
    height: 800
  });

  await Promise.all(scheduled.filter((timer) => !timer.cancelled).map((timer) => timer.callback()));

  assert.deepEqual(storage.data.windowState.bounds, {
    left: 50,
    top: 60,
    width: 640,
    height: 820
  });
});

test("preserves mini bounds event order when storage reads resolve out of order", async () => {
  const storage = createFakeStorage({
    windowState: {
      windowId: 1,
      tabId: 10,
      bounds: { left: 1, top: 2, width: 420, height: 900 },
      zoom: 0.9,
      lastShortcutId: null
    }
  });
  const scheduled = [];
  const readDeferrals = [];
  let nextTimerId = 1;
  let deferReads = true;
  const readState = () => ({
    shortcuts: storage.data.shortcuts ?? [],
    windowState: storage.data.windowState
  });
  const controller = createController(storage, {
    getExtensionState() {
      if (!deferReads) {
        return Promise.resolve(readState());
      }

      const deferred = createDeferred();
      readDeferrals.push(deferred);
      return deferred.promise;
    },
    setTimeoutFn(callback) {
      const timer = { id: nextTimerId++, callback, cancelled: false };
      scheduled.push(timer);
      return timer.id;
    },
    clearTimeoutFn(timerId) {
      const timer = scheduled.find((item) => item.id === timerId);
      if (timer) {
        timer.cancelled = true;
      }
    },
    debounceMs: 400
  });

  const firstEvent = controller.handleBoundsChanged({
    id: 1,
    type: "popup",
    left: 10,
    top: 60,
    width: 640,
    height: 820
  });
  const secondEvent = controller.handleBoundsChanged({
    id: 1,
    type: "popup",
    left: 20,
    top: 70,
    width: 650,
    height: 830
  });
  await Promise.resolve();
  deferReads = false;

  if (readDeferrals.length >= 2) {
    readDeferrals[1].resolve(readState());
    await secondEvent;
    readDeferrals[0].resolve(readState());
    await firstEvent;
  } else {
    await Promise.all([firstEvent, secondEvent]);
  }

  await Promise.all(scheduled.filter((timer) => !timer.cancelled).map((timer) => timer.callback()));

  assert.equal(storage.data.windowState.bounds.left, 20);
});

test("unrelated popup bounds events do not cancel pending mini save", async () => {
  const storage = createFakeStorage({
    windowState: {
      windowId: 1,
      tabId: 10,
      bounds: { left: 1, top: 2, width: 420, height: 900 },
      zoom: 0.9,
      lastShortcutId: "s1"
    }
  });
  const scheduled = [];
  let nextTimerId = 1;
  const controller = createController(storage, {
    setTimeoutFn(callback) {
      const timer = { id: nextTimerId++, callback, cancelled: false };
      scheduled.push(timer);
      return timer.id;
    },
    clearTimeoutFn(timerId) {
      const timer = scheduled.find((item) => item.id === timerId);
      if (timer) {
        timer.cancelled = true;
      }
    },
    debounceMs: 400
  });

  await controller.handleBoundsChanged({
    id: 1,
    type: "popup",
    left: 20,
    top: 30,
    width: 500,
    height: 700
  });
  await controller.handleBoundsChanged({
    id: 2,
    type: "popup",
    left: 99,
    top: 99,
    width: 600,
    height: 800
  });

  await Promise.all(scheduled.filter((timer) => !timer.cancelled).map((timer) => timer.callback()));

  assert.equal(storage.data.windowState.bounds.left, 20);
  assert.equal(storage.data.windowState.bounds.top, 30);
});

test("ignores stale in-flight bounds callbacks for the same window", async () => {
  const storage = createFakeStorage({
    windowState: {
      windowId: 1,
      tabId: 10,
      bounds: { left: 1, top: 2, width: 420, height: 900 },
      zoom: 0.9,
      lastShortcutId: "s1"
    }
  });
  const scheduled = [];
  const readDeferrals = [];
  let nextTimerId = 1;
  const readState = () => ({
    shortcuts: storage.data.shortcuts ?? [],
    windowState: storage.data.windowState
  });
  const controller = createController(storage, {
    getExtensionState() {
      const deferred = createDeferred();
      readDeferrals.push(deferred);
      return deferred.promise;
    },
    setTimeoutFn(callback) {
      const timer = { id: nextTimerId++, callback, cancelled: false };
      scheduled.push(timer);
      return timer.id;
    },
    clearTimeoutFn(timerId) {
      const timer = scheduled.find((item) => item.id === timerId);
      if (timer) {
        timer.cancelled = true;
      }
    },
    debounceMs: 400
  });

  await controller.handleBoundsChanged({
    id: 1,
    type: "popup",
    left: 10,
    top: 30,
    width: 500,
    height: 700
  });
  const firstCallback = scheduled.at(-1).callback();
  await flushMicrotasks();

  await controller.handleBoundsChanged({
    id: 1,
    type: "popup",
    left: 20,
    top: 40,
    width: 510,
    height: 710
  });
  const secondCallback = scheduled.at(-1).callback();
  await flushMicrotasks();

  readDeferrals[1].resolve(readState());
  await secondCallback;
  readDeferrals[0].resolve(readState());
  await firstCallback;

  assert.equal(storage.data.windowState.bounds.left, 20);
  assert.equal(storage.data.windowState.bounds.top, 40);
});

test("ignores stale bounds callbacks that are already inside save", async () => {
  const storage = createFakeStorage({
    windowState: {
      windowId: 1,
      tabId: 10,
      bounds: { left: 1, top: 2, width: 420, height: 900 },
      zoom: 0.9,
      lastShortcutId: "s1"
    }
  });
  const scheduled = [];
  const saveDeferrals = [];
  let nextTimerId = 1;
  const controller = createController(storage, {
    saveBoundsFromWindow(window, options) {
      const deferred = createDeferred();
      saveDeferrals.push({ deferred, window, options });
      return deferred.promise.then(() =>
        saveBoundsFromWindow(window, { storageArea: storage, ...options })
      );
    },
    setTimeoutFn(callback) {
      const timer = { id: nextTimerId++, callback, cancelled: false };
      scheduled.push(timer);
      return timer.id;
    },
    clearTimeoutFn(timerId) {
      const timer = scheduled.find((item) => item.id === timerId);
      if (timer) {
        timer.cancelled = true;
      }
    },
    debounceMs: 400
  });

  await controller.handleBoundsChanged({
    id: 1,
    type: "popup",
    left: 10,
    top: 30,
    width: 500,
    height: 700
  });
  const firstCallback = scheduled.at(-1).callback();
  await flushMicrotasks();

  await controller.handleBoundsChanged({
    id: 1,
    type: "popup",
    left: 20,
    top: 40,
    width: 510,
    height: 710
  });
  const secondCallback = scheduled.at(-1).callback();
  await flushMicrotasks();

  saveDeferrals[1].deferred.resolve();
  await secondCallback;
  saveDeferrals[0].deferred.resolve();
  await firstCallback;

  assert.equal(storage.data.windowState.bounds.left, 20);
  assert.equal(storage.data.windowState.bounds.top, 40);
});
