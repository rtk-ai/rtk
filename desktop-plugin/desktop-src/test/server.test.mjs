import { test } from "node:test";
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { spawn } from "node:child_process";

const HERE = path.dirname(new URL(import.meta.url).pathname);
const SERVER = path.join(HERE, "..", "server", "index.mjs");

function client(store) {
  const p = spawn(process.execPath, [SERVER], { env: { ...process.env, RTK_DESKTOP_STORE: store, RTK_DESKTOP_RTK: "none", RTK_DESKTOP_NODE: "none", RTK_DESKTOP_LAYA_PORT: "41799" }, stdio: ["pipe", "pipe", "pipe"] });
  let buf = "", id = 0;
  const waiting = new Map();
  p.stdout.on("data", (d) => {
    buf += d;
    let i;
    while ((i = buf.indexOf("\n")) >= 0) {
      const line = buf.slice(0, i); buf = buf.slice(i + 1);
      try { const m = JSON.parse(line); if (waiting.has(m.id)) { waiting.get(m.id)(m); waiting.delete(m.id); } } catch {}
    }
  });
  const stderr = [];
  p.stderr.on("data", (d) => stderr.push(String(d)));
  return {
    call: (method, params) => new Promise((resolve) => { const i = ++id; waiting.set(i, resolve); p.stdin.write(JSON.stringify({ jsonrpc: "2.0", id: i, method, params }) + "\n"); }),
    notify: (method, params) => p.stdin.write(JSON.stringify({ jsonrpc: "2.0", method, params }) + "\n"),
    close: () => p.stdin.end(),
    stderr,
    proc: p,
  };
}

test("initialize, tools, pipelines, setup and gain over stdio", async (t) => {
  const store = fs.mkdtempSync(path.join(os.tmpdir(), "rtk-desktop-e2e-"));
  const c = client(store);
  t.after(() => c.proc.kill());
  const init = await c.call("initialize", { protocolVersion: "2025-06-18", clientInfo: { name: "claude-desktop", version: "x" } });
  assert.equal(init.result.serverInfo.name, "rtk-desktop");
  c.notify("notifications/initialized", {});
  const tools = (await c.call("tools/list", {})).result.tools.map((t) => t.name);
  assert.deepEqual(tools, ["rtk_task_setup", "rtk_gain", "rtk_pipelines", "rtk_laya", "rtk_status"]);

  const list = (await c.call("tools/call", { name: "rtk_pipelines", arguments: { action: "list" } })).result.content[0].text;
  assert.equal(list, "New tasks use: **default-bash-compress**, **explore-first**.");

  const toml = path.join(HERE, "fixtures", "laya-tiered.toml");
  const add = (await c.call("tools/call", { name: "rtk_pipelines", arguments: { action: "add", source: toml } })).result.content[0].text;
  assert.match(add, /^✅ Added \*\*laya-tiered\*\* from .*New tasks use: \*\*default-bash-compress\*\*, \*\*laya-tiered\*\*\. Note: it replaces explore-first/);
  assert.ok(fs.existsSync(path.join(store, "pipelines", "laya-tiered.toml")));

  const setup = JSON.parse((await c.call("tools/call", { name: "rtk_task_setup", arguments: { state: "command: ls\ngoal: add a test\nrecent_calls: Bash ls\ntool: shell\n" } })).result.content[0].text);
  assert.deepEqual(setup.active, ["default-bash-compress", "laya-tiered"]);
  assert.ok(setup.pipelines["laya-tiered"].includes('name    = "laya-tiered"'));
  assert.equal(setup.os, process.platform);
  assert.deepEqual(Object.keys(setup).sort(), ["active", "arch", "fallback", "notice", "os", "pipelines"]);
  assert.ok(setup.fallback && /Node\.js 20/.test(setup.notice));

  const reset = (await c.call("tools/call", { name: "rtk_pipelines", arguments: { action: "reset" } })).result.content[0].text;
  assert.equal(reset, "✅ Back to the defaults: **default-bash-compress**, **explore-first**.");
  const setup2 = JSON.parse((await c.call("tools/call", { name: "rtk_task_setup", arguments: { state: "x" } })).result.content[0].text);
  assert.deepEqual(setup2.pipelines, {});
  assert.match(setup2.notice, /^Tell the user once, in one short sentence at the end of your answer: RTK is active \(pipelines: default-bash-compress, explore-first\); \/rtk-setup gives a guided tour/);
  const setup3 = JSON.parse((await c.call("tools/call", { name: "rtk_task_setup", arguments: { state: "x" } })).result.content[0].text);
  assert.ok(!setup3.notice);

  fs.writeFileSync(path.join(store, "browse-cache.json"), JSON.stringify({ at: Date.now(), entries: [{ name: "laya-tiered", owner: "o", repo: "r", sha: "a".repeat(40), tier: "text", rtk: "v1.0.1", suite: "SkillsBench", model: "claude-opus-5-5", baseline: "stock Claude Code", trials: 44, cost: { baseline: 4.312, pipeline: 3.44 }, quality: { metric: "reward", baseline: 0.66, pipeline: 0.64 } }] }));
  fs.mkdirSync(path.join(store, "reviewed"));
  fs.copyFileSync(toml, path.join(store, "reviewed", `laya-tiered-${"a".repeat(7)}.toml`));
  const check = (await c.call("tools/call", { name: "rtk_status", arguments: {} })).result.content[0].text;
  assert.match(check, /\n\*\*Choices\*\*\n\n1\. \*\*Keep the defaults\*\* \(in use now\): default-bash-compress \+ explore-first: compresses command output; .*Needs nothing\.\n2\. \*\*Add laya-tiered\*\*: cost -20% vs stock Claude Code, reward 0\.66 → 0\.64 \(SkillsBench, claude-opus-5-5, 44 trials\/arm\)\. Laya, a classifier on your computer, labels each task; searches go to a Haiku helper; implementations go to a Sonnet helper; some rules call a model directly \(needs an API key\); needs Node\.js 20\+ for Laya\.\n\n\*\*What to expect/);
  assert.match(check, /^## RTK setup\n\n\| \| \|\n\|---\|---\|\n\| ✅ Service \| plugin [\w.-]+ on \w+ \(\w+\), Node\.js \d+/);
  assert.match(check, /\n\| ✅ Pipelines for new tasks \| \*\*default-bash-compress\*\*, \*\*explore-first\*\* \(the defaults; they need nothing else\) \|\n/);
  assert.match(check, /\n\| ⚠️ rtk on this computer \| not installed/);
  assert.match(check, /\n\| ❌ Laya, for Laya-based pipelines \| needs Node\.js 20 or newer/);
  assert.match(check, /\n\| ✅ Tasks recorded \| none yet/);
  assert.match(check, /\n\*\*Reviewed pipelines you can add\*\* with `\/rtk-pipeline-add <name>`\n\n\| Pipeline \| Cost \|.*\n\|---.*\n\| \*\*laya-tiered\*\* \| 🟢 -20% vs stock Claude Code \| reward 0\.66 → 0\.64 \| SkillsBench, claude-opus-5-5, 44 trials\/arm \| text \| o \| `\/rtk-pipeline-add laya-tiered` \|\n/);
  assert.match(check, /\n\*\*What to expect in a task\*\*\n- 🔴 /);
  assert.match(check, /\n\*\*Commands\*\* `\/rtk-gain`/);
  const search = (await c.call("tools/call", { name: "rtk_pipelines", arguments: { action: "search", query: "" } })).result.content[0].text;
  assert.match(search, /^\*\*Reviewed RTK pipelines\*\* \(rtk-ai\.app\/pipelines\), 1 of 1\n\n\| Pipeline \| Cost \| Quality \| Benchmark \| Tier \| By \| Add \|\n\|---\|---\|---\|---\|---\|---\|---\|\n\| \*\*laya-tiered\*\* \| 🟢 -20%/);

  const rec = { v: 1, sid: "task-1", at: "2026-11-02", compression: { commands: 4, saved: 1200 }, delegation: { kept: 0, helpers: [] }, cost: { main_usd: 0.1, helpers_usd: 0 }, rules: {} };
  const gain = (await c.call("tools/call", { name: "rtk_gain", arguments: {} })).result.content[0].text;
  assert.match(gain, /^## RTK gain\n\n⚠️ The record of this task did not arrive\.\n/);
  assert.match(gain, /\n\| Claude Code \| rtk is not installed here, so no history \|\n\| Desktop tasks \| none recorded yet \|\n\n_A task is recorded when `\/rtk-gain` runs in it\._$/);
  const gain2 = (await c.call("tools/call", { name: "rtk_gain", arguments: { ledger: { ...rec, via: "hook" } } })).result.content[0].text;
  assert.match(gain2, /^## RTK gain\n\n\| \| This task · pipelines: none \|\n\|---\|---\|\n\| 🗜️ Compression \| 4 commands, \*\*~1\.2K tokens saved\*\* \|\n\| 🤝 Delegation \| none \|\n\n\| \| On this computer \|/);
  assert.match(gain2, /\n\| Desktop tasks \| 1 since 2026-11-02: \*\*~1\.2K tokens compressed\*\* over 4 commands \|\n/);
  assert.match((await c.call("tools/call", { name: "rtk_status", arguments: {} })).result.content[0].text, /\n\| ✅ Tasks recorded \| 1 since 2026-11-02 \|\n/);
  const status = (await c.call("tools/call", { name: "rtk_laya", arguments: { action: "status" } })).result.content[0].text;
  assert.match(status, /^⚠️ Laya on your computer: \*\*failed\*\*: Node\.js 20\+ not found · task labelling on · 0 labels so far\.$/);

  const bad = await c.call("nope", {});
  assert.equal(bad.error.code, -32601);
  c.close();
  await new Promise((r) => c.proc.on("exit", r));
  assert.equal(c.stderr.join(""), "");
  assert.match(fs.readFileSync(path.join(store, "server.log"), "utf8"), /initialize client=/);
});

test("a local Claude Code session gets no tools", async (t) => {
  const store = fs.mkdtempSync(path.join(os.tmpdir(), "rtk-desktop-e2e-"));
  const c = client(store);
  t.after(() => c.proc.kill());
  await c.call("initialize", { protocolVersion: "2025-06-18", clientInfo: { name: "claude-code", version: "2.1.316" } });
  assert.deepEqual((await c.call("tools/list", {})).result.tools, []);
  c.close();
  await new Promise((r) => c.proc.on("exit", r));
});
