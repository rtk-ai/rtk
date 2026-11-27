import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { syncBuiltinESMExports } from "node:module";
import { test } from "node:test";

let importId = 0;

async function loadPlugin(t, { pluginConfig, status = 0 } = {}) {
  const calls = [];
  const logs = [];
  let hook;
  t.mock.method(childProcess, "execFileSync", (file, args) => {
    calls.push({ file, args });
    if (file === "which") return "";
    assert.equal(file, "rtk");
    assert.deepEqual(args, ["rewrite", "git status"]);
    if (status !== 0) {
      throw Object.assign(new Error("fixture rewrite verdict"), {
        status,
        stdout: status === 3 ? "rtk git status\n" : "",
      });
    }
    return "rtk git status\n";
  });
  syncBuiltinESMExports();
  t.after(() => {
    t.mock.restoreAll();
    syncBuiltinESMExports();
  });
  t.mock.method(console, "log", (...args) => logs.push(args.join(" ")));
  const { default: register } = await import(`./index.ts?test=${++importId}`);
  // OpenClaw passes the whole host config and the validated plugin config separately.
  register({
    config: {
      plugins: {
        entries: { "rtk-rewrite": { enabled: true, config: pluginConfig } },
      },
    },
    pluginConfig,
    on(name, callback, options) {
      assert.equal(name, "before_tool_call");
      assert.equal(options.priority, 10);
      hook = callback;
    },
  });
  return { calls, logs, hook };
}

const event = {
  toolName: "exec",
  params: { command: "git status", workdir: "/synthetic/workspace" },
};

test("plugin config enabled=false prevents registration and binary probing", async (t) => {
  const { hook, calls } = await loadPlugin(t, { pluginConfig: { enabled: false } });
  assert.equal(hook, undefined);
  assert.deepEqual(calls, []);
});

test("plugin config verbose=true logs registration and rewrites", async (t) => {
  const { hook, logs } = await loadPlugin(t, { pluginConfig: { verbose: true } });
  assert.ok(hook);
  hook(event);
  assert.deepEqual(logs, [
    "[rtk] OpenClaw plugin registered",
    "[rtk] git status -> rtk git status",
  ]);
});

test("missing plugin config keeps default rewriting and quiet logging", async (t) => {
  const { hook, logs } = await loadPlugin(t);
  assert.ok(hook);
  assert.deepEqual(hook(event), {
    params: { command: "rtk git status", workdir: "/synthetic/workspace" },
  });
  assert.deepEqual(logs, []);
});

test("exit 2 still blocks when reading scoped config", async (t) => {
  const { hook } = await loadPlugin(t, { pluginConfig: {}, status: 2 });
  assert.deepEqual(hook(event), {
    block: true,
    blockReason: "RTK deny rule matched",
  });
});

test("exit 3 still requires approval when reading scoped config", async (t) => {
  const { hook } = await loadPlugin(t, { pluginConfig: {}, status: 3 });
  const result = hook(event);
  assert.equal(result.params.command, "rtk git status");
  assert.equal(result.requireApproval.timeoutBehavior, "deny");
  assert.deepEqual(result.requireApproval.allowedDecisions, ["allow-once", "deny"]);
});

test("exit 1 still passes through when reading scoped config", async (t) => {
  const { hook } = await loadPlugin(t, { pluginConfig: {}, status: 1 });
  assert.equal(hook(event), undefined);
});
