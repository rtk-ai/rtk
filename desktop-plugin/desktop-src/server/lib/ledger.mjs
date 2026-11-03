import { spawnSync } from "node:child_process";

export function readTasks(store) {
  const rows = new Map();
  for (const line of store.readText("tasks.jsonl").split("\n")) {
    if (!line.trim()) continue;
    try { const r = JSON.parse(line); if (r && r.sid) rows.set(r.sid, r); } catch {}
  }
  return rows;
}

export function validRecord(rec) {
  return !!rec && typeof rec === "object" && /^[\w-]{1,80}$/.test(String(rec.sid || "")) && rec.v === 1;
}

export function recordTask(store, rec) {
  if (!validRecord(rec)) return false;
  const clean = { ...rec, at: String(rec.at || new Date().toISOString()).slice(0, 10), recorded: new Date().toISOString() };
  store.append("tasks.jsonl", JSON.stringify(clean));
  return true;
}

export function allTasks(store) {
  const out = [];
  for (const r of readTasks(store).values()) {
    const c = r.compression || {}, d = r.device || {}, cost = r.cost || {};
    out.push({ sid: r.sid, at: r.at, commands: (c.commands || 0) + (d.outputs || 0), saved: (c.saved || 0) + (d.saved || 0), kept: (r.delegation || {}).kept || 0,
      helpers_usd: cost.helpers_usd || 0, main_usd: cost.main_usd || 0, rules: Object.values(r.rules || {}).reduce((a, b) => a + b, 0), helpers: ((r.delegation || {}).helpers || []).length });
  }
  return out.sort((a, b) => (a.at || "").localeCompare(b.at || ""));
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
  const c = rec.compression || {}, d = rec.device || {}, dl = rec.delegation || {}, cost = rec.cost || {};
  const rows = [];
  const saved = (c.saved || 0) + (d.saved || 0), cmds = (c.commands || 0) + (d.outputs || 0);
  rows.push(["🗜️ Compression", `${cmds} commands, **~${fmt(saved)} tokens saved**` + (d.outputs ? ` (${c.commands || 0} in the workspace, ${d.outputs} from the connected folder)` : "")]);
  const helpers = dl.helpers || [];
  if (!helpers.length) rows.push(["🤝 Delegation", "none"]);
  helpers.forEach((h, i) => rows.push([i ? "" : "🤝 Delegation",
    `${h.type} on ${h.model} read ~${fmt(h.read || 0)}${h.wrote ? `, produced ~${fmt(h.wrote)}` : ""}, reported ~${fmt(h.report || 0)}: **~${fmt(h.kept || 0)} tokens kept out of the main context**, helper cost ${usd(h.usd || 0)}`]));
  const spent = Object.entries(cost.by_model || {}).filter(([, v]) => v > 0).sort((a, b) => b[1] - a[1]);
  if (spent.length) rows.push(["💵 Cost", spent.map(([m, v]) => `${m} ${usd(v)}`).join(", ") + " (API list prices, estimate)"]);
  const rules = Object.entries(rec.rules || {});
  if (rules.length) rows.push(["📏 Rules", rules.map(([r, n]) => `${r} ×${n}`).join(", ")]);
  const ctx = [rec.off ? "pipelines off" : `pipelines: ${(rec.pipelines || []).join(", ") || "none"}`];
  const laya = Object.entries(rec.laya || {});
  if (laya.length) ctx.push("Laya: " + laya.map(([q, a]) => `${q}=${a}`).join(", "));
  return table(`This task · ${ctx.join(" · ")}`, rows);
}

export function gainText(tasks, local, rtk, rec) {
  const out = ["## RTK gain", ""];
  out.push(rec ? taskText(rec) : "⚠️ The record of this task did not arrive.", "");
  const rows = [["Claude Code", local ? `**~${fmt(local.saved)} tokens saved** over ${count(local.commands)} commands (rtk gain)`
    : rtk ? `rtk ${rtk.version} at ${rtk.path} did not answer \`rtk gain --format json\`` : "rtk is not installed here, so no history"]];
  if (!tasks.length) rows.push(["Desktop tasks", "none recorded yet"]);
  else {
    const sum = (k) => tasks.reduce((a, t) => a + (t[k] || 0), 0);
    const helpers = sum("helpers"), withHelpers = tasks.filter((t) => t.kept > 0).length;
    rows.push(["Desktop tasks", `${tasks.length} since ${tasks[0].at || "?"}: **~${fmt(sum("saved"))} tokens compressed** over ${sum("commands")} commands`]);
    if (withHelpers) rows.push(["Delegation", `${withHelpers} tasks, ${helpers} helpers: **~${fmt(sum("kept"))} tokens kept out of the main model's context**; helpers ${usd(sum("helpers_usd"))} against ${usd(sum("main_usd"))} for the main model (API list prices, estimates)`]);
    if (sum("rules")) rows.push(["Rules fired", String(sum("rules"))]);
  }
  out.push(table("On this computer", rows), "", "_A task is recorded when `/rtk-gain` runs in it._");
  return out.join("\n");
}
