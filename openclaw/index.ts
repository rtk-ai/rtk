/**
 * RTK Rewrite Plugin for OpenClaw
 *
 * Transparently rewrites exec tool commands to RTK equivalents
 * before execution, cutting up to 90% of the bash output that reaches the LLM context.
 *
 * All rewrite logic lives in `rtk rewrite` (src/discover/registry.rs).
 * This plugin is a thin delegate — to add or change rules, edit the
 * Rust registry, not this file.
 *
 * Permission model: the plugin calls `rtk rewrite --host openclaw`, which tells
 * RTK that OpenClaw is its own permission authority. RTK then decides whether a
 * command can be *rewritten*; whether the result may *run* is decided by
 * OpenClaw's own exec policy (`tools.exec.mode`, `security`, `ask`) after this
 * hook returns. Without the flag RTK evaluates the command against Claude
 * Code's settings files and asks for approval on anything they do not
 * explicitly allow — a second gate, sourced from another agent's config, on a
 * runtime that never opted into it. See issue #3908.
 *
 * Exit code protocol for `rtk rewrite --host openclaw`:
 *   0 + stdout  Rewrite found → apply it, OpenClaw's exec policy decides the rest
 *   1           No RTK equivalent → pass through unchanged
 *   2           Deny rule matched → block the call
 *   3 + stdout  Not emitted for this host; treated as exit 0, matching the
 *               convention the Pi, Hermes, and OpenCode adapters follow
 *
 * Requires an rtk build that understands `rtk rewrite --host` (see README).
 * An older rtk swallows the flag into the command it is asked to rewrite, fails
 * to match it, and exits 1 — rewriting stops and nothing is blocked or denied.
 *
 * See: src/hooks/rewrite_cmd.rs
 */

import { execFileSync } from "node:child_process";

/**
 * Tells RTK that OpenClaw enforces its own exec policy on the rewritten
 * command, so RTK must not add an approval gate of its own.
 */
const RTK_HOST = "openclaw";

let rtkAvailable: boolean | null = null;

function checkRtk(): boolean {
  if (rtkAvailable !== null) return rtkAvailable;
  try {
    execFileSync("which", ["rtk"], { stdio: "ignore" });
    rtkAvailable = true;
  } catch {
    rtkAvailable = false;
  }
  return rtkAvailable;
}

/**
 * Delegate to `rtk rewrite` and interpret the exit code.
 *
 * Returns a tuple `[rewritten, verdict?]`:
 *   [string]        — rewrite available, apply it (exit 0, or exit 3 from an
 *                     rtk that still emits it)
 *   [null, "deny"]  — command matched a deny rule (exit 2)
 *   [null]          — no rewrite / passthrough (exit 1 or no change)
 */
type RewriteVerdict = "deny";

function tryRewrite(
  command: string
): [string | null, RewriteVerdict?] {
  try {
    const result = execFileSync("rtk", ["rewrite", "--host", RTK_HOST, command], {
      encoding: "utf-8",
      timeout: 2000,
    })
      .toString()
      .trim();
    // Exit 0 — rewrite available
    return [result && result !== command ? result : null];
  } catch (e: any) {
    // Exit 2 — Deny: command matched a deny rule, block the call
    if (e?.status === 2) {
      return [null, "deny"];
    }
    // Exit 3 — an rtk that did not collapse ask/default for this host.
    // Exit codes 0 and 3 both mean "rewrite"; the gate is OpenClaw's, not RTK's.
    if (e?.status === 3 && e.stdout) {
      const result = e.stdout.toString().trim();
      return [result && result !== command ? result : null];
    }
    // Exit 1 or unknown — no rewrite, pass through
    return [null];
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

      const [rewritten, verdict] = tryRewrite(command);

      // Deny rule matched — block the call entirely
      if (verdict === "deny") {
        if (verbose) {
          console.log(`[rtk] DENY: ${command}`);
        }
        return {
          block: true,
          blockReason: "RTK deny rule matched",
        };
      }

      if (!rewritten) return;

      if (verbose) {
        console.log(`[rtk] ${command} -> ${rewritten}`);
      }

      // No `requireApproval`: OpenClaw's exec policy is the authority over
      // whether the rewritten command runs, and it is applied after this hook.
      return {
        params: { ...event.params, command: rewritten },
      };
    },
    { priority: 10 }
  );

  if (verbose) {
    console.log("[rtk] OpenClaw plugin registered");
  }
}
