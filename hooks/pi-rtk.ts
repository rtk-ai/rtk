import { createBashTool, type ExtensionAPI } from "@mariozechner/pi-coding-agent";
import { spawnSync } from "node:child_process";

// RTK pi extension — rewrites bash commands to use rtk for token savings.
// Requires: rtk >= 0.23.0 in PATH.
//
// This is a thin delegating extension: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.

type RewriteResult = {
  command: string;
  rewritten: boolean;
};

const RTK_BIN = process.env.PI_RTK_BIN || "rtk";
const RTK_TIMEOUT_MS = Number.parseInt(process.env.PI_RTK_TIMEOUT_MS || "800", 10);

let rtkAvailabilityChecked = false;
let rtkAvailable = false;

function ensureRtkAvailable(env: NodeJS.ProcessEnv): boolean {
  if (rtkAvailabilityChecked) return rtkAvailable;

  const probe = spawnSync(RTK_BIN, ["--help"], {
    encoding: "utf8",
    env,
    timeout: Math.min(RTK_TIMEOUT_MS, 1000),
  });

  rtkAvailabilityChecked = true;
  rtkAvailable = probe.status === 0 || !probe.error;
  return rtkAvailable;
}

function rewriteCommand(command: string, cwd: string, env: NodeJS.ProcessEnv): RewriteResult {
  if (!command.trim() || !ensureRtkAvailable(env)) {
    return { command, rewritten: false };
  }

  const result = spawnSync(RTK_BIN, ["rewrite", command], {
    cwd,
    encoding: "utf8",
    env,
    timeout: RTK_TIMEOUT_MS,
  });

  if (result.error || result.status !== 0) {
    return { command, rewritten: false };
  }

  const rewritten = (result.stdout || "").trim();
  if (!rewritten || rewritten === command) {
    return { command, rewritten: false };
  }

  return { command: rewritten, rewritten: true };
}

export default function (pi: ExtensionAPI) {
  const bashTool = createBashTool(process.cwd(), {
    spawnHook: ({ command, cwd, env }) => {
      const rewrite = rewriteCommand(command, cwd, env);
      return {
        command: rewrite.command,
        cwd,
        env: {
          ...env,
          PI_RTK_ACTIVE: rewrite.rewritten ? "1" : "0",
        },
      };
    },
  });

  pi.registerTool({
    ...bashTool,
    label: "bash (rtk)",
    description:
      "Execute bash commands, first attempting RTK rewrite with automatic fallback to the original command.",
    execute: async (toolCallId, params, signal, onUpdate) => {
      return bashTool.execute(toolCallId, params, signal, onUpdate);
    },
  });
}
