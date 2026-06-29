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

import { spawnSync } from "node:child_process";

const MIN_SUPPORTED_RTK_MINOR = 23;
const REWRITE_TIMEOUT_MS = 2_000;

function parseSemver(raw: string): [number, number, number] | null {
  const m = raw.trim().match(/(\d+)\.(\d+)\.(\d+)/);
  if (!m) return null;
  return [parseInt(m[1], 10), parseInt(m[2], 10), parseInt(m[3], 10)];
}

function probeRtk(): boolean {
  const result = spawnSync("rtk", ["--version"], {
    encoding: "utf-8",
    timeout: REWRITE_TIMEOUT_MS,
  });
  if (result.error) {
    console.warn("[rtk] rtk probe failed unexpectedly — plugin disabled", result.error);
    return false;
  }
  if (result.status !== 0) {
    console.warn("[rtk] rtk binary not found in PATH — plugin disabled");
    return false;
  }
  const parsed = parseSemver((result.stdout ?? "").replace(/^rtk\s+/, ""));
  if (!parsed) return true;
  const [major, minor] = parsed;
  if (major === 0 && minor < MIN_SUPPORTED_RTK_MINOR) {
    console.warn(
      `[rtk] rtk ${(result.stdout ?? "").trim()} is too old (need >= 0.23.0) — plugin disabled`
    );
    return false;
  }
  return true;
}

function tryRewrite(command: string): string | null {
  const result = spawnSync("rtk", ["rewrite", command], {
    encoding: "utf-8",
    timeout: REWRITE_TIMEOUT_MS,
  });
  if (result.status !== 0 && result.status !== 3) return null;
  const stdout = (result.stdout ?? "").trim();
  return stdout && stdout !== command ? stdout : null;
}

export default function register(api: any) {
  const pluginConfig = api.config ?? {};
  const enabled = pluginConfig.enabled !== false;
  const verbose = pluginConfig.verbose === true;

  if (!enabled) return;

  if (!probeRtk()) return;

  api.on(
    "before_tool_call",
    (event: { toolName: string; params: Record<string, unknown> }) => {
      if (event.toolName !== "exec") return;

      const command = event.params?.command;
      if (typeof command !== "string" || !command.trim()) return;
      if (command.startsWith("rtk ")) return;
      if (process.env.RTK_DISABLED === "1") return;

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
