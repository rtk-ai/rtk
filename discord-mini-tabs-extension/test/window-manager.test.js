import test from "node:test";
import assert from "node:assert/strict";
import {
  closeMiniWindow,
  focusMiniWindow,
  openShortcutInMiniWindow,
  resetMiniWindowPosition,
  saveBoundsFromWindow,
  updateMiniWindowSettings
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

function createFakeChrome({
  failSetZoom = false,
  failWindowUpdate = false,
  tabsUpdateReturnsUndefined = false,
  windowsCreateReturnsUndefined = false
} = {}) {
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
        if (windowsCreateReturnsUndefined) {
          return undefined;
        }

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
        if (failWindowUpdate) throw new Error("Window update failed");
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
        if (tabsUpdateReturnsUndefined) {
          return undefined;
        }

        return tab;
      },
      async setZoom(tabId, zoom) {
        calls.push(["tabs.setZoom", tabId, zoom]);
        if (failSetZoom) throw new Error("Zoom failed");
      }
    }
  };
}

function callsNamed(chromeApi, name) {
  return chromeApi.calls.filter((call) => call[0] === name);
}

function createDeferred() {
  let resolve;
  let reject;
  const promise = new Promise((promiseResolve, promiseReject) => {
    resolve = promiseResolve;
    reject = promiseReject;
  });
  return { promise, resolve, reject };
}

function deferWindowCreate(chromeApi) {
  const started = createDeferred();
  const release = createDeferred();
  const createWindow = chromeApi.windows.create;
  chromeApi.windows.create = async (createData) => {
    started.resolve();
    await release.promise;
    return createWindow(createData);
  };
  return { started, release };
}

const shortcut = {
  id: "s1",
  name: "Dev Chat",
  type: "text",
  url: "https://discord.com/channels/123456789012345678/987654321098765432"
};

const nextShortcut = {
  ...shortcut,
  id: "s2",
  url: "https://discord.com/channels/223456789012345678/887654321098765432"
};

test("creates a popup window when no valid window exists", async () => {
  const chromeApi = createFakeChrome({ failSetZoom: true });
  const storage = createFakeStorage({
    windowState: {
      windowId: 999,
      tabId: 998
    }
  });

  const result = await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  assert.equal(result.windowId, 100);
  assert.equal(result.tabId, 200);
  assert.equal(storage.data.windowState.windowId, 100);
  assert.equal(storage.data.windowState.tabId, 200);
  assert.equal(storage.data.windowState.lastShortcutId, "s1");
  assert.equal(callsNamed(chromeApi, "windows.create").length, 1);
  assert.equal(callsNamed(chromeApi, "windows.create")[0][1].type, "popup");
  assert.equal(callsNamed(chromeApi, "windows.create")[0][1].width, 420);
  assert.equal(callsNamed(chromeApi, "windows.create")[0][1].height, 900);
  assert.equal(callsNamed(chromeApi, "tabs.setZoom").length, 1);
});

test("reuses existing mini window and tab", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  const result = await openShortcutInMiniWindow({
    chromeApi,
    storageArea: storage,
    shortcut: nextShortcut
  });

  assert.equal(result.windowId, 100);
  assert.equal(result.tabId, 200);
  assert.equal(callsNamed(chromeApi, "windows.create").length, 1);
  assert.ok(
    chromeApi.calls.some(
      (call) => call[0] === "windows.update" && call[1] === 100 && call[2].focused === true
    )
  );
  assert.ok(
    chromeApi.calls.some(
      (call) => call[0] === "tabs.update" && call[1] === 200 && call[2].url === nextShortcut.url
    )
  );
  assert.equal(storage.data.windowState.lastShortcutId, "s2");
});

test("serializes concurrent open calls so only one popup is created", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();

  const [first, second] = await Promise.all([
    openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut }),
    openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut: nextShortcut })
  ]);

  assert.equal(callsNamed(chromeApi, "windows.create").length, 1);
  assert.equal(first.windowId, 100);
  assert.equal(second.windowId, 100);
  assert.equal(storage.data.windowState.windowId, 100);
  assert.equal(storage.data.windowState.tabId, 200);
  assert.equal(storage.data.windowState.lastShortcutId, "s2");
});

test("serializes close after in-flight open so close wins", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  const deferredCreate = deferWindowCreate(chromeApi);

  const openPromise = openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });
  await deferredCreate.started.promise;
  const closePromise = closeMiniWindow({ chromeApi, storageArea: storage });

  deferredCreate.release.resolve();
  await Promise.all([openPromise, closePromise]);

  assert.equal(storage.data.windowState.windowId, null);
  assert.equal(storage.data.windowState.tabId, null);
  assert.equal(chromeApi.windowsById.has(100), false);
});

test("serializes settings after in-flight open so settings win", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  const deferredCreate = deferWindowCreate(chromeApi);

  const openPromise = openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });
  await deferredCreate.started.promise;
  const settingsPromise = updateMiniWindowSettings({
    chromeApi,
    storageArea: storage,
    bounds: { width: 640, height: 820 },
    zoom: 1
  });

  deferredCreate.release.resolve();
  await Promise.all([openPromise, settingsPromise]);

  assert.equal(storage.data.windowState.windowId, 100);
  assert.equal(storage.data.windowState.bounds.width, 640);
  assert.equal(storage.data.windowState.bounds.height, 820);
  assert.equal(storage.data.windowState.zoom, 1);
  assert.equal(chromeApi.windowsById.get(100).width, 640);
  assert.equal(chromeApi.windowsById.get(100).height, 820);
});

test("focuses existing window", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });
  chromeApi.windowsById.get(100).focused = false;

  await focusMiniWindow({ chromeApi, storageArea: storage });

  assert.equal(chromeApi.windowsById.get(100).focused, true);
  assert.ok(
    chromeApi.calls.some(
      (call) => call[0] === "windows.update" && call[1] === 100 && call[2].focused === true
    )
  );
});

test("closes mini window and clears ids", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  await closeMiniWindow({ chromeApi, storageArea: storage });

  assert.equal(callsNamed(chromeApi, "windows.remove").length, 1);
  assert.equal(chromeApi.windowsById.has(100), false);
  assert.equal(storage.data.windowState.windowId, null);
  assert.equal(storage.data.windowState.tabId, null);
});

test("resets position but keeps size", async () => {
  const chromeApi = createFakeChrome();
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });
  await updateMiniWindowSettings({
    chromeApi,
    storageArea: storage,
    bounds: { left: 50, top: 60, width: 640, height: 820 },
    zoom: 1.1
  });

  const state = await resetMiniWindowPosition({ chromeApi, storageArea: storage });

  assert.equal(state.bounds.left, null);
  assert.equal(state.bounds.top, null);
  assert.equal(state.bounds.width, 640);
  assert.equal(state.bounds.height, 820);
  const lastWindowUpdate = callsNamed(chromeApi, "windows.update").at(-1);
  assert.deepEqual(lastWindowUpdate[2], { width: 640, height: 820 });
});

test("returns cleared state when reset position detects stale window", async () => {
  const chromeApi = createFakeChrome({ failWindowUpdate: true });
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  const state = await resetMiniWindowPosition({ chromeApi, storageArea: storage });

  assert.equal(state.windowId, null);
  assert.equal(state.tabId, null);
  assert.equal(storage.data.windowState.windowId, null);
});

test("returns cleared state when settings update detects stale window", async () => {
  const chromeApi = createFakeChrome({ failWindowUpdate: true });
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  const state = await updateMiniWindowSettings({
    chromeApi,
    storageArea: storage,
    bounds: { width: 640, height: 820 },
    zoom: 1
  });

  assert.equal(state.windowId, null);
  assert.equal(state.tabId, null);
  assert.equal(storage.data.windowState.windowId, null);
});

test("saves bounds from popup windows only", async () => {
  const storage = createFakeStorage({
    windowState: {
      windowId: 1,
      tabId: 2,
      bounds: { left: 1, top: 2, width: 420, height: 900 },
      zoom: 0.9,
      lastShortcutId: "s1"
    }
  });

  const ignoredNormalWindow = await saveBoundsFromWindow(
    { id: 1, type: "normal", left: 10, top: 20, width: 500, height: 700 },
    { storageArea: storage, expectedWindowId: 1 }
  );
  const ignoredWrongWindow = await saveBoundsFromWindow(
    { id: 2, type: "popup", left: 10, top: 20, width: 500, height: 700 },
    { storageArea: storage, expectedWindowId: 1 }
  );
  const saved = await saveBoundsFromWindow(
    { id: 1, type: "popup", left: 10, top: 20, width: 500, height: 700 },
    { storageArea: storage, expectedWindowId: 1 }
  );

  assert.equal(ignoredNormalWindow, null);
  assert.equal(ignoredWrongWindow, null);
  assert.equal(saved.bounds.left, 10);
  assert.equal(saved.bounds.top, 20);
  assert.equal(saved.bounds.width, 500);
  assert.equal(saved.bounds.height, 700);
  assert.equal(storage.data.windowState.bounds.left, 10);
});

test("does not save bounds for a stale expected window id", async () => {
  const storage = createFakeStorage({
    windowState: {
      windowId: 2,
      tabId: 20,
      bounds: { left: 1, top: 2, width: 420, height: 900 },
      zoom: 0.9,
      lastShortcutId: "s2"
    }
  });

  const result = await saveBoundsFromWindow(
    { id: 1, type: "popup", left: 10, top: 20, width: 500, height: 700 },
    { storageArea: storage, expectedWindowId: 1 }
  );

  assert.equal(result, null);
  assert.equal(storage.data.windowState.windowId, 2);
  assert.deepEqual(storage.data.windowState.bounds, {
    left: 1,
    top: 2,
    width: 420,
    height: 900
  });
});

test("falls back to existing tab id when tabs.update returns undefined", async () => {
  const chromeApi = createFakeChrome({ tabsUpdateReturnsUndefined: true });
  const storage = createFakeStorage();
  await openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut });

  const result = await openShortcutInMiniWindow({
    chromeApi,
    storageArea: storage,
    shortcut: nextShortcut
  });

  assert.equal(result.tabId, 200);
  assert.equal(storage.data.windowState.tabId, 200);
});

test("throws custom error when chrome creates no usable window", async () => {
  const chromeApi = createFakeChrome({ windowsCreateReturnsUndefined: true });
  const storage = createFakeStorage();

  await assert.rejects(
    openShortcutInMiniWindow({ chromeApi, storageArea: storage, shortcut }),
    /usable Discord mini window/
  );
});
