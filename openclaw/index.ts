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
 * Permission model: RTK owns the deny gate, OpenClaw owns approval.
 *
 * The plugin runs `rtk rewrite` with `RTK_REWRITE_HOST=openclaw`, which tells
 * RTK that this host applies its own exec policy (`tools.exec.mode`,
 * `security`, `ask`) to whatever the hook returns. RTK therefore stops raising
 * a prompt of its own for a command that matched no rule — without that, every
 * rewritable command raised a second approval prompt, sourced from Claude
 * Code's settings files, on a runtime that never opted into them (#3908).
 *
 * An explicit `ask` rule the user wrote is not that: it still arrives as exit
 * 3 and the plugin still prompts, so a rule the user typed is not discarded.
 *
 * A deny rule is unaffected and still blocks the call. The host name relaxes
 * only the *default* ask; see `ApprovalOwner` in src/hooks/decision.rs.
 *
 * Exit code protocol for `rtk rewrite`:
 *   0 + stdout  Rewrite it — OpenClaw's exec policy decides whether it runs
 *   1           No RTK equivalent → pass through unchanged
 *   2           Deny rule matched → block the call
 *   3 + stdout  Rewrite it, but require approval — an explicit `ask` rule, or
 *               any ask/default on an rtk that predates RTK_REWRITE_HOST
 *
 * The host travels in the environment rather than in argv on purpose: an rtk
 * that does not know a `--host` flag folds it into the command text it judges,
 * so a deny rule written against the user's command stops matching. An rtk
 * that does not know the variable ignores it and behaves exactly as it does
 * today. Nothing here requires a minimum rtk version.
 *
 * See: src/hooks/rewrite_cmd.rs, src/hooks/decision.rs
 */

import { execFileSync } from "node:child_process";

/**
 * Tells RTK that OpenClaw applies its own exec policy to the rewritten
 * command, so RTK must not raise an approval gate of its own. Must be an
 * agent name `AgentPath::lookup` knows; anything else falls back to the
 * stricter default, which is the behaviour without this variable.
 */
const RTK_REWRITE_HOST = "openclaw";

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
 *   [string]        — rewrite and apply (exit 0)
 *   [string, "ask"] — rewrite, but require user approval (exit 3)
 *   [null, "deny"]  — command matched a deny rule (exit 2)
 *   [null]          — no rewrite / passthrough (exit 1 or no change)
 */
type RewriteVerdict = "ask" | "deny";

function tryRewrite(
  command: string
): [string | null, RewriteVerdict?] {
  const options = {
    encoding: "utf-8" as const,
    timeout: 2000,
    env: { ...process.env, RTK_REWRITE_HOST },
  };
  try {
    const result = execFileSync("rtk", ["rewrite", command], options)
      .toString()
      .trim();
    // Exit 0 — no rule matched (or an explicit allow): rewrite and apply.
    return [result && result !== command ? result : null];
  } catch (e: any) {
    // Exit 2 — Deny: command matched a deny rule, block the call. Checked
    // first: a deny outranks every other outcome, on every rtk version.
    if (e?.status === 2) {
      return [null, "deny"];
    }
    // Exit 3 — `Ask`: an explicit `ask` rule the user wrote, or (on an rtk
    // that predates RTK_REWRITE_HOST) any ask/default. Require approval rather
    // than applying the rewrite silently.
    if (e?.status === 3 && e.stdout) {
      const result = e.stdout.toString().trim();
      if (result && result !== command) return [result, "ask"];
      // Exit 3 but no usable stdout — treat as passthrough
      return [null];
    }
    // Exit 1 or unknown — no rewrite, pass through
    return [null];
  }
}

export default function register(api: any) {
  const pluginConfig = api.pluginConfig ?? {};
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
        console.log(
          `[rtk] ${command} -> ${rewritten}${verdict === "ask" ? " (approval required)" : ""}`
        );
      }

      const result: {
        params: Record<string, unknown>;
        requireApproval?: {
          title: string;
          description: string;
          severity: "info";
          timeoutBehavior: "deny";
          allowedDecisions: Array<"allow-once" | "deny">;
          onResolution?: (decision: string) => void;
        };
      } = {
        params: { ...event.params, command: rewritten },
      };

      // Exit 3 — `Ask`: an explicit `ask` rule the user wrote, or (on an rtk
      // that predates RTK_REWRITE_HOST) any ask/default. RTK does not relax it,
      // so keep the prompt that existed before #3908 for the commands that
      // would previously have reached one.
      //
      // Exit 0 — no rule matched: never prompt. Whether the rewrite may run is
      // OpenClaw's own exec policy, applied after this hook returns. Asking
      // here as well was the duplicate gate #3908 describes.
      //
      // The exec tool's own checks see the rewritten string. OpenClaw carries
      // hook adjustments forward into the params handed to the exec tool, so
      // `tools.exec.mode`, `security`, `ask` and the exec-approvals allowlist
      // all match `rtk git push`, not `git push` — write those rules against
      // the `rtk` form. This was already true on the exit-0 path before #3908.
      // A trusted tool policy is the exception: OpenClaw runs those before
      // ordinary `before_tool_call` hooks, so one still sees the original
      // command.
      if (verdict === "ask") {
        result.requireApproval = {
          title: "RTK rewrite suggestion",
          description: `Rewrite: \`${command}\` → \`${rewritten}\``,
          severity: "info",
          timeoutBehavior: "deny",
          // "allow-always" omitted: OpenClaw does not auto-persist approval
          // for plugin hooks — see:
          // https://docs.openclaw.ai/plugins/plugin-permission-requests#troubleshooting
          allowedDecisions: ["allow-once", "deny"],
          onResolution: (decision: string) => {
            if (verbose) {
              console.log(`[rtk] approval ${decision}: ${command} -> ${rewritten}`);
            }
          },
        };
      }

      return result;
    },
    { priority: 10 }
  );

  if (verbose) {
    console.log("[rtk] OpenClaw plugin registered");
  }
}
