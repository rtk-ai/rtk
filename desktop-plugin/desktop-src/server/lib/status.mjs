import { DEFAULTS } from "./store.mjs";
import { fmt } from "./ledger.mjs";
import { TABLE_HEAD, row } from "./pipelines.mjs";

const OS = { darwin: "macOS", win32: "Windows", linux: "Linux" };

export const EXPECT = [
  "**What to expect in a task**",
  "- 🔴 One or two steps marked *Failed*: RTK deliberately refusing a command, once per task to fetch your pipelines and when a rule hands work to a helper. The task continues.",
  "- 🤝 Delegation pays on large writing tasks and costs a little on tiny ones; the defaults keep small changes with the main model.",
  "- 🔐 Desktop asks you to approve each call of the plugin's tools on your computer and offers no \"always allow\" for them; RTK keeps those calls to one per folder task (its setup) and one per `/rtk-gain`.",
  "",
  "**Commands** `/rtk-gain` · `/rtk-pipelines` · `/rtk-pipelines-search` · `/rtk-pipeline-add <name>` · `/rtk-pipeline-remove <name>` · `/rtk-pipelines-reset` · `/rtk-laya-status` · `/rtk-pipeline-off`",
];

export function choicesText(choices) {
  if (!choices || !choices.length) return [];
  return ["**Choices**", "", ...choices.map((c, i) => `${i + 1}. **${c.label}**${c.inUse ? " (in use now)" : ""}: ${c.text}`), ""];
}

export function layaRow(laya) {
  const head = "Laya, for Laya-based pipelines";
  if (!laya.node) return [`❌ ${head}`, "needs Node.js 20 or newer (nodejs.org); RTK then sets it up by itself on first use (about 1.7 GB)"];
  const parts = [`Node.js at ${laya.node}`];
  parts.push(laya.installed ? "packages installed" : "packages not installed yet (first use)");
  parts.push(laya.model ? "model downloaded" : "model not downloaded yet (about 1.7 GB on first use)");
  if (laya.phase === "up") parts.push("running now");
  else if (laya.phase === "failed") parts.push(`last start failed: ${laya.error}`);
  if (laya.off) parts.push("labelling turned off (`/rtk-laya-status` to check)");
  return [`${laya.phase === "failed" ? "⚠️" : "✅"} ${head}`, parts.join("; ")];
}

export function statusText({ version, platform, arch, node, dataDir, rtk, gain, active, laya, tasks, reviewed, choices }) {
  const dflt = active.length === DEFAULTS.length && DEFAULTS.every((n) => active.includes(n));
  const rows = [
    ["✅ Service", `plugin ${version} on ${OS[platform] || platform} (${arch}), Node.js ${node}`],
    ["✅ Pipelines for new tasks", `**${active.join("**, **") || "none"}**${dflt ? " (the defaults; they need nothing else)" : ""}`],
  ];
  if (rtk && gain) rows.push(["✅ rtk on this computer", `${rtk.version}, ~${fmt(gain.saved)} tokens saved in Claude Code so far`]);
  else if (rtk) rows.push(["✅ rtk on this computer", `${rtk.version} at ${rtk.path} (its gain history could not be read)`]);
  else rows.push(["⚠️ rtk on this computer", "not installed: only needed to show Claude Code's own savings in `/rtk-gain` (rtk-ai.app)"]);
  rows.push(layaRow(laya));
  rows.push(["✅ Tasks recorded", tasks.length ? `${tasks.length} since ${tasks[0].at}` : "none yet (a task is recorded when `/rtk-gain` runs in it)"]);
  rows.push(["📁 Data folder", `\`${dataDir}\``]);
  const out = ["## RTK setup", "", "| | |", "|---|---|", ...rows.map(([a, b]) => `| ${a} | ${b} |`), ""];
  if (reviewed) {
    const others = reviewed.filter((e) => !active.includes(e.name));
    if (others.length) out.push("**Reviewed pipelines you can add** with `/rtk-pipeline-add <name>`", "", ...TABLE_HEAD, ...others.map((e) => row(e, active)), "");
    else out.push("**Reviewed pipelines**: all of them are already in use.", "");
  } else out.push("**Reviewed pipelines**: the list could not be fetched right now (`/rtk-pipelines-search` retries it).", "");
  return [...out, ...choicesText(choices), ...EXPECT].join("\n");
}
