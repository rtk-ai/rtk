/**
 * RTK Rewrite Plugin for OpenClaw
 *
 * Transparently rewrites exec tool commands to RTK equivalents
 * before execution, achieving 60-90% LLM token savings.
 *
 * All rewrite logic lives in `rtk rewrite` (src/discover/registry.rs).
 * This plugin is a thin delegate — to add or change rules, edit the
 * Rust registry, not this file.
 *
 * Compiled from upstream index.ts (rtk v0.42.3) — types stripped only,
 * logic identical. OpenClaw 2026.6.5 managed installs require JS output.
 */

import { execFileSync } from "node:child_process";

let rtkAvailable = null;

function checkRtk() {
  if (rtkAvailable !== null) return rtkAvailable;
  try {
    execFileSync("which", ["rtk"], { stdio: "ignore" });
    rtkAvailable = true;
  } catch {
    rtkAvailable = false;
  }
  return rtkAvailable;
}

// `rtk rewrite` exit-code protocol (hooks/claude/rtk-rewrite.sh):
//   0 + stdout  rewrite found, allow-classified
//   1           no RTK equivalent — pass through unchanged
//   2           deny rule matched — pass through so native policy sees the original
//   3 + stdout  rewrite found, ask-classified — rewrite; OpenClaw exec policy still governs
function tryRewrite(command) {
  let stdout = null;
  try {
    stdout = execFileSync("rtk", ["rewrite", command], {
      encoding: "utf-8",
      timeout: 2000,
    });
  } catch (err) {
    if (err && err.status === 3 && err.stdout) {
      stdout = err.stdout;
    } else {
      return null;
    }
  }
  const result = stdout.toString().trim();
  return result && result !== command ? result : null;
}

export default function register(api) {
  const pluginConfig = api.config ?? {};
  const enabled = pluginConfig.enabled !== false;
  const verbose = pluginConfig.verbose === true;

  if (!enabled) return;

  if (!checkRtk()) {
    console.warn("[rtk] rtk binary not found in PATH — plugin disabled");
    return;
  }

  api.on(
    "before_tool_call",
    (event) => {
      if (event.toolName !== "exec") return;

      const command = event.params?.command;
      if (typeof command !== "string") return;

      const rewritten = tryRewrite(command);
      if (!rewritten) return;

      if (verbose) {
        console.log(`[rtk] ${command} -> ${rewritten}`);
      }

      return { params: { ...event.params, command: rewritten } };
    },
    { priority: 10 }
  );

  if (verbose) {
    console.log("[rtk] OpenClaw plugin registered");
  }
}
