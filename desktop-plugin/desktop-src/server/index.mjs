#!/usr/bin/env node
import fs from "node:fs";
import path from "node:path";
import readline from "node:readline";
import { fileURLToPath } from "node:url";
import { Store, dataDir, migrate } from "./lib/store.mjs";
import { Laya } from "./lib/laya.mjs";
import { Pipelines } from "./lib/pipelines.mjs";
import { findInstalled } from "./lib/rtklocal.mjs";
import { allTasks, gainText, localGain, recordTask, validRecord } from "./lib/ledger.mjs";
import { statusText } from "./lib/status.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
export const VERSION = (() => {
  try { return JSON.parse(fs.readFileSync(path.join(HERE, "..", "manifest.json"), "utf8")).version || "0.0.0"; } catch { return "0.0.0"; }
})();
const PLUGIN_VERSION = (() => {
  let d = HERE;
  for (let i = 0; i < 6; i++) {
    d = path.dirname(d);
    try { return JSON.parse(fs.readFileSync(path.join(d, ".claude-plugin", "plugin.json"), "utf8")).version || VERSION; } catch {}
  }
  return VERSION;
})();
const platform = process.platform, arch = process.arch;

const store = new Store(process.env.RTK_DESKTOP_STORE || dataDir());
if (!process.env.RTK_DESKTOP_STORE) migrate(store);
const laya = new Laya(store, { src: path.join(HERE, "..", "laya", "server.mjs") });
const pipes = new Pipelines(store, laya);
const rtk = process.env.RTK_DESKTOP_RTK === "none" ? null : findInstalled();
store.log(`installed rtk: ${rtk ? `${rtk.path} ${rtk.version}` : "none"}`);

const unavailable = (why) => `Tell the user once, in one short sentence: ${why}. RTK uses explore-first for this task.`;
const welcome = (names) => `Tell the user once, in one short sentence at the end of your answer: RTK is active (pipelines: ${names.join(", ")}); /rtk-setup gives a guided tour and /rtk-gain shows the savings`;

async function taskSetup(args) {
  const data = await setupData(args);
  if (!data.notice && !store.exists("welcomed")) {
    store.append("welcomed", new Date().toISOString());
    data.notice = welcome(data.active);
  }
  return data;
}

async function setupData(args) {
  const req = String(args.state || args.request || "").trim();
  const names = pipes.active();
  const d = pipes.deliverable(names);
  const data = { active: names, pipelines: d.pipelines, os: platform, arch };
  if (!d.usesLaya || laya.off() || !req) return data;
  laya.used = Date.now();
  if (!(await laya.up()) && !laya.findNode()) {
    laya.phase = "failed";
    laya.error = "Node.js 20+ not found";
    return { ...data, notice: unavailable(laya.fix.node), fallback: true };
  }
  const first = laya.firstRun();
  laya.start();
  if (!(await laya.waitUp(first ? 40 : 60))) {
    const why = laya.phase === "failed" ? laya.fix.failed.replace("%s", laya.error) : first ? laya.fix.setup : "RTK's Laya classifier is still starting; it will be ready for the next task";
    return { ...data, notice: unavailable(why), fallback: true };
  }
  if (Object.keys(d.questions).length) data.labels = await laya.label(req, d.questions);
  return data;
}

const TOOLS = [
  { name: "rtk_task_setup", description: "Set up RTK for this task: delivers the user's chosen pipelines and Laya's labels. Call it once when RTK asks, with state set to the text RTK gives you.",
    inputSchema: { type: "object", properties: { state: { type: "string" } } } },
  { name: "rtk_gain", description: "What RTK saved: this task (its record is attached by RTK's hook), all Desktop tasks and Claude Code on the user's computer. For /rtk-gain.",
    inputSchema: { type: "object", properties: { ledger: { type: "object" } } } },
  { name: "rtk_pipelines", description: "The RTK pipelines Cowork tasks use, kept on the user's computer: list them; add one by its name on the reviewed list, github:owner/repo, a https URL or a file path; remove one by name; reset to the defaults; or search the reviewed list. Only when the user asks.",
    inputSchema: { type: "object", properties: { action: { type: "string", enum: ["list", "add", "remove", "reset", "search"] }, source: { type: "string" }, name: { type: "string" }, query: { type: "string" } } } },
  { name: "rtk_laya", description: "Turn RTK's task labelling by Laya (a classifier on the user's computer) on or off, or show its status.",
    inputSchema: { type: "object", properties: { action: { type: "string", enum: ["on", "off", "status"] } } } },
  { name: "rtk_status", description: "RTK's setup checklist for this computer: pipelines in use, rtk and Laya, tasks recorded, reviewed pipelines, what to expect. For /rtk-setup.",
    inputSchema: { type: "object", properties: {} } },
];

async function status() {
  let reviewed = null, choices = [];
  try { reviewed = await pipes.allowlist(); choices = await pipes.choices(reviewed); } catch (e) { store.log(`reviewed list: ${e.message}`); }
  const node = laya.findNode();
  const l = { node, installed: laya.installed(), model: fs.existsSync(path.join(laya.home, ".cache", "receptron-laya")), phase: (await laya.up()) ? "up" : laya.phase, error: laya.error, off: laya.off() };
  return statusText({ version: PLUGIN_VERSION, platform, arch, node: process.versions.node, dataDir: store.dir, rtk, gain: localGain(rtk && rtk.path),
    active: pipes.active(), laya: l, tasks: allTasks(store), reviewed, choices });
}

const text = (t) => ({ content: [{ type: "text", text: t }] });
const state = { local: false };

async function call(name, args) {
  if (name === "rtk_task_setup") {
    try { return text(JSON.stringify(await taskSetup(args))); }
    catch (e) { store.log(`rtk_task_setup failed: ${e.message}`); return text(JSON.stringify({ active: pipes.active(), pipelines: {}, fallback: true, os: platform })); }
  }
  if (name === "rtk_gain") {
    const rec = validRecord(args.ledger) ? args.ledger : null;
    store.log(`rtk_gain record: ${rec ? rec.via || "from the model" : "none"}`);
    if (rec) recordTask(store, rec);
    return text(gainText(allTasks(store), localGain(rtk && rtk.path), rtk, rec));
  }
  if (name === "rtk_pipelines") {
    const act = args.action || "list";
    try {
      if (act === "add") return text(await pipes.add(args.source || args.name));
      if (act === "remove") return text(pipes.remove(String(args.name || args.source || "")));
      if (act === "reset") return text(pipes.reset());
      if (act === "search") return text(await pipes.search(args.query || args.source || ""));
      return text(pipes.list());
    } catch (e) { return text(`⚠️ ${e.message}`); }
  }
  if (name === "rtk_laya") {
    const act = args.action || "status";
    if (act === "on") { fs.rmSync(store.path("laya-off"), { force: true }); laya.used = Date.now(); laya.start(); }
    if (act === "off") { store.append("laya-off", "off"); laya.stop(); }
    return text(await laya.status());
  }
  if (name === "rtk_status") return text(await status());
  throw new Error(`unknown tool ${name}`);
}

async function handle(msg) {
  const m = msg.method, p = msg.params || {};
  if (m === "initialize") {
    store.log(`initialize client=${JSON.stringify(p.clientInfo)} os=${platform}/${arch} node=${process.versions.node}`);
    state.local = (p.clientInfo || {}).name === "claude-code";
    return { protocolVersion: p.protocolVersion || "2025-06-18", capabilities: { tools: {} }, serverInfo: { name: "rtk-desktop", version: VERSION } };
  }
  if (m === "tools/list") return { tools: state.local ? [] : TOOLS };
  if (m === "ping") return {};
  if (m === "tools/call") {
    const args = p.arguments || {};
    const shown = p.name === "rtk_pipelines" ? JSON.stringify({ action: args.action, source: args.source, name: args.name, query: args.query })
      : `{${Object.keys(args).join(", ")}} ${JSON.stringify(args).length} bytes`;
    store.log(`call ${p.name} ${shown}`);
    const r = await call(p.name, args);
    store.log(`${p.name} -> ${(r.content[0].text || "").length} bytes`);
    return r;
  }
  return undefined;
}

export function main() {
  store.log(`start pid=${process.pid} v${VERSION}`);
  laya.watchIdle();
  const rl = readline.createInterface({ input: process.stdin, crlfDelay: Infinity });
  rl.on("line", async (line) => {
    let msg;
    try { msg = JSON.parse(line); } catch { return; }
    if (msg.id === undefined || msg.id === null) return;
    let out;
    try {
      const res = await handle(msg);
      out = res === undefined ? { jsonrpc: "2.0", id: msg.id, error: { code: -32601, message: "method not found" } } : { jsonrpc: "2.0", id: msg.id, result: res };
    } catch (e) {
      store.log(`error ${e.stack || e.message}`);
      out = { jsonrpc: "2.0", id: msg.id, error: { code: -32603, message: e.message } };
    }
    process.stdout.write(JSON.stringify(out) + "\n");
  });
  const bye = () => { laya.stop(); process.exit(0); };
  rl.on("close", bye);
  process.on("SIGTERM", bye);
  process.on("SIGINT", bye);
}

main();
