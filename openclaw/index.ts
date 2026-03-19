/**
 * RTK Rewrite Plugin for OpenClaw
 *
 * Transparently rewrites exec tool commands to RTK equivalents
 * before execution, achieving 60-90% LLM token savings.
 *
 * All rewrite logic lives in `rtk rewrite` (src/discover/registry.rs).
 * This plugin is a thin delegate — to add or change rules, edit the
 * Rust registry, not this file.
 */

import { execSync } from "node:child_process";

let rtkAvailable: boolean | null = null;

function checkRtk(): boolean {
  if (rtkAvailable !== null) return rtkAvailable;
  try {
    execSync("which rtk", { stdio: "ignore" });
    rtkAvailable = true;
  } catch {
    rtkAvailable = false;
  }
  return rtkAvailable;
}

function tryRewrite(command: string): string | null {
  try {
    const result = execSync(`rtk rewrite ${JSON.stringify(command)}`, {
      encoding: "utf-8",
      timeout: 2000,
    }).trim();
    return result && result !== command ? result : null;
  } catch {
    return null;
  }
}

export default function register(api: any) {
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
    (event: { toolName: string; params: Record<string, unknown> }) => {
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
