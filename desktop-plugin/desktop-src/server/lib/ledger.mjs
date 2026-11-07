import { spawnSync } from "node:child_process";

export function readTasks(store) {
  const rows = new Map();
  for (const line of store.readText("tasks.jsonl").split("\n")) {
    if (!line.trim()) continue;
    try { const r = JSON.parse(line); if (r && r.sid) rows.set(r.sid, r); } catch {}
  }
  return rows;
}

export const MAX_RECORD = 65536;

export function validRecord(rec) {
  if (!rec || typeof rec !== "object" || Array.isArray(rec) || !/^[\w-]{1,80}$/.test(String(rec.sid || "")) || rec.v !== 1) return false;
  const helpers = (rec.delegation || {}).helpers;
  if (helpers !== undefined && (!Array.isArray(helpers) || helpers.length > 50)) return false;
  try { return JSON.stringify(rec).length <= MAX_RECORD; } catch { return false; }
}

export const cell = (s) => String(s ?? "").replace(/[|\r\n]+/g, " ").trim();
export const num = (x) => (typeof x === "number" && Number.isFinite(x) ? x : Number(x) || 0);
const obj = (x) => (x && typeof x === "object" && !Array.isArray(x) ? x : {});

export function recordTask(store, rec) {
  if (!validRecord(rec)) return false;
  const clean = { ...rec, at: String(rec.at || new Date().toISOString()).slice(0, 10), recorded: new Date().toISOString() };
  store.append("tasks.jsonl", JSON.stringify(clean));
  return true;
}

export function allTasks(store) {
  const out = [];
  for (const r of readTasks(store).values()) {
    const c = obj(r.compression), d = obj(r.device), cost = obj(r.cost), dl = obj(r.delegation);
    out.push({ sid: r.sid, at: String(r.at || ""), commands: num(c.commands) + num(d.outputs), saved: num(c.saved) + num(d.saved), kept: num(dl.kept),
      helpers_usd: num(cost.helpers_usd), main_usd: num(cost.main_usd), rules: Object.values(obj(r.rules)).reduce((a, b) => a + num(b), 0), helpers: Array.isArray(dl.helpers) ? dl.helpers.length : 0 });
  }
  return out.sort((a, b) => a.at.localeCompare(b.at));
}

export function localGain(rtkPath) {
  if (!rtkPath) return null;
  try {
    const r = spawnSync(rtkPath, ["gain", "--format", "json"], { encoding: "utf8", timeout: 15000 });
    const s = JSON.parse(r.stdout).summary;
    return { saved: Number(s.total_saved || 0), commands: Number(s.total_commands || 0) };
  } catch { return null; }
}

export const fmt = (n) => (n >= 1e6 ? (n / 1e6).toFixed(1) + "M" : n >= 1e3 ? (n / 1e3).toFixed(1) + "K" : String(Math.round(n)));
export const usd = (n) => "$" + n.toFixed(2);
export const count = (n) => Number(n || 0).toLocaleString("en-US");
const table = (header, rows) => [`| | ${header} |`, "|---|---|", ...rows.map(([a, b]) => `| ${a} | ${b} |`)].join("\n");

export function taskText(rec) {
  const c = obj(rec.compression), d = obj(rec.device), dl = obj(rec.delegation), cost = obj(rec.cost);
  const rows = [];
  const saved = num(c.saved) + num(d.saved), cmds = num(c.commands) + num(d.outputs);
  rows.push(["🗜️ Compression", `${cmds} commands, **~${fmt(saved)} tokens saved**` + (num(d.outputs) ? ` (${num(c.commands)} in the workspace, ${num(d.outputs)} from the connected folder)` : "")]);
  const helpers = (Array.isArray(dl.helpers) ? dl.helpers : []).filter((h) => h && typeof h === "object" && !Array.isArray(h));
  if (!helpers.length) rows.push(["🤝 Delegation", "none"]);
  helpers.forEach((h, i) => rows.push([i ? "" : "🤝 Delegation",
    `${cell(h.type)} on ${cell(h.model)} read ~${fmt(num(h.read))}${num(h.wrote) ? `, produced ~${fmt(num(h.wrote))}` : ""}, reported ~${fmt(num(h.report))}: **~${fmt(num(h.kept))} tokens kept out of the main context**, helper cost ${usd(num(h.usd))}`]));
  const spent = Object.entries(obj(cost.by_model)).map(([m, v]) => [m, num(v)]).filter(([, v]) => v > 0).sort((a, b) => b[1] - a[1]);
  if (spent.length) rows.push(["💵 Cost", spent.map(([m, v]) => `${cell(m)} ${usd(v)}`).join(", ") + " (API list prices, estimate)"]);
  const rules = Object.entries(obj(rec.rules));
  if (rules.length) rows.push(["📏 Rules", rules.map(([r, n]) => `${cell(r)} ×${num(n)}`).join(", ")]);
  const ctx = [rec.off ? "pipelines off" : `pipelines: ${(Array.isArray(rec.pipelines) ? rec.pipelines : []).map(cell).join(", ") || "none"}`];
  const laya = Object.entries(obj(rec.laya));
  if (laya.length) ctx.push("Laya: " + laya.map(([q, a]) => `${cell(q)}=${cell(a)}`).join(", "));

  return table(`This task · ${ctx.join(" · ")}`, rows);
}

export function gainText(tasks, local, rtk, rec) {
  const out = ["## RTK gain", ""];
  out.push(rec ? taskText(rec) : "⚠️ The record of this task did not arrive.", "");
  const rows = [["Claude Code", local ? `**~${fmt(local.saved)} tokens saved** over ${count(local.commands)} commands (rtk gain)`
    : rtk ? `rtk ${rtk.version} at ${rtk.path} did not answer \`rtk gain --format json\`` : "rtk is not installed here, so no history"]];
  if (!tasks.length) rows.push(["Desktop tasks", "none recorded yet"]);
  else {
    const sum = (k) => tasks.reduce((a, t) => a + num(t[k]), 0);

    const helpers = sum("helpers"), withHelpers = tasks.filter((t) => t.kept > 0).length;
    rows.push(["Desktop tasks", `${tasks.length} since ${tasks[0].at || "?"}: **~${fmt(sum("saved"))} tokens compressed** over ${sum("commands")} commands`]);
    if (withHelpers) rows.push(["Delegation", `${withHelpers} tasks, ${helpers} helpers: **~${fmt(sum("kept"))} tokens kept out of the main model's context**; helpers ${usd(sum("helpers_usd"))} against ${usd(sum("main_usd"))} for the main model (API list prices, estimates)`]);
    if (sum("rules")) rows.push(["Rules fired", String(sum("rules"))]);
  }
  out.push(table("On this computer", rows), "", "_A task is recorded when `/rtk-gain` runs in it._");
  return out.join("\n");
}
