/**
 * opencode-rtk — OpenCode plugin for rtk (Rust Token Killer)
 *
 * Transparently rewrites bash commands to use rtk equivalents,
 * reducing LLM token consumption by 60-90% on common dev commands.
 *
 * Requires: rtk >= 0.23.0 (https://github.com/rtk-ai/rtk)
 *
 * Equivalent to rtk's Claude Code PreToolUse hook, adapted for
 * OpenCode's plugin API (tool.execute.before hook).
 */

import { execFileSync, execSync } from "child_process";

/**
 * Check if rtk binary is available and meets minimum version.
 * Returns true if available, null if unavailable.
 */
const checkRtk = () => {
  try {
    const version = execSync("rtk --version 2>/dev/null", {
      encoding: "utf8",
      timeout: 5000,
    }).trim();

    const match = version.match(/(\d+)\.(\d+)\.(\d+)/);
    if (!match) return null;

    const [, major, minor] = match.map(Number);
    if (major === 0 && minor < 23) {
      console.warn(
        `[opencode-rtk] rtk ${match[0]} too old (need >= 0.23.0). Upgrade: brew upgrade rtk`,
      );
      return null;
    }

    return true;
  } catch {
    return null;
  }
};

/**
 * Call `rtk rewrite` on a command string.
 * Returns the rewritten command, or null if no rewrite available.
 */
const rewriteCommand = (cmd) => {
  try {
    const rewritten = execFileSync("rtk", ["rewrite", cmd], {
      encoding: "utf8",
      timeout: 5000,
      stdio: ["pipe", "pipe", "pipe"],
    }).trim();

    if (rewritten && rewritten !== cmd) {
      return rewritten;
    }
    return null;
  } catch {
    return null;
  }
};

export const RtkPlugin = async (_ctx) => {
  const available = checkRtk();
  if (!available) {
    console.warn("[opencode-rtk] rtk binary not found or too old — plugin disabled");
    return {};
  }

  const debug = process.env.RTK_DEBUG === "1";

  return {
    "tool.execute.before": async (input, output) => {
      if (input.tool !== "bash") return;

      const cmd = output.args?.command;
      if (!cmd || typeof cmd !== "string") return;

      const rewritten = rewriteCommand(cmd);
      if (rewritten) {
        output.args.command = rewritten;
        if (debug) {
          console.error(`[opencode-rtk] ${cmd} → ${rewritten}`);
        }
      }
    },
  };
};
