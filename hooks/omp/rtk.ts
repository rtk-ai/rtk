// RTK - Rust Token Killer
// OMP hook: rewrite bash tool calls through `rtk rewrite`.
//
// Installed to .omp/hooks/pre/rtk.ts (project) or
// ~/.omp/agent/hooks/pre/rtk.ts (global).
// OMP auto-discovers and loads hooks from these paths.
//
// Fail-open: if rtk is unavailable or rewrite fails, commands run raw.

import type { HookAPI } from "@oh-my-pi/pi-coding-agent/extensibility/hooks";
import { $which } from "@oh-my-pi/pi-utils";

type Verdict = "allow" | "ask";

export interface RewriteResult {
    rewritten: string;
    verdict: Verdict;
}

/** Thin wrapper over Bun.spawn for test injection. */
export const spawner = {
    spawn: (cmd: string[], opts: Parameters<typeof Bun.spawn>[1]) => Bun.spawn(cmd, opts),
};

export async function rewrite(command: string): Promise<RewriteResult | null> {
    // `rtk rewrite` exit codes:
    //   0 = rewrite allowed (auto-apply)
    //   1 = no RTK equivalent (passthrough)
    //   2 = deny rule matched (passthrough — host tool handles denial)
    //   3 = ask rule matched (prompt user before applying)
    let timeout: ReturnType<typeof setTimeout> | undefined;
    let timedOut = false;
    try {
        const proc = spawner.spawn(["rtk", "rewrite", command], {
            stdout: "pipe", stderr: "pipe",
        });
        timeout = setTimeout(() => {
            timedOut = true;
            proc.kill();
        }, 2_000);
        const [exitCode, stdout, stderr] = await Promise.all([
            proc.exited,
            new Response(proc.stdout).text(),
            new Response(proc.stderr).text(),
        ]);

        if (timedOut) {
            warn("rtk rewrite timed out");
            return null;
        }

        const rewritten = stdout.trim();

        if (exitCode === 1 || exitCode === 2) return null;
        if (exitCode === 0 || exitCode === 3) {
            if (rewritten && rewritten !== command) {
                return { rewritten, verdict: exitCode === 3 ? "ask" : "allow" };
            }
            return null;
        }

        const details = `rtk rewrite failed with exit ${exitCode}`;
        const errDetail = stderr.trim();
        warn(errDetail ? `${details}: ${errDetail}` : details);
    } catch (e) {
        warn(`rtk rewrite error: ${e instanceof Error ? e.message : String(e)}`);
    } finally {
        if (timeout !== undefined) clearTimeout(timeout);
    }
    return null;
}

function warn(message: string): void {
    console.error(`rtk: omp hook warning: ${message}`);
}

let rtkMissingWarned = false;

export default function (pi: HookAPI): void {
    const hasRtk = Boolean($which("rtk"));

    if (!hasRtk && !rtkMissingWarned) {
        rtkMissingWarned = true;
        warn("rtk binary not found in PATH; OMP hook not registered");
    }

    pi.on("tool_call", async (event, ctx) => {
        if (event.toolName !== "bash") return;
        if (!hasRtk) return;
        const result = await rewrite(event.input.command as string);
        if (!result) return;

        if (result.verdict === "ask" && ctx.hasUI) {
            const ok = await ctx.ui.confirm(
                "RTK rewrite",
                `Rewrote:\n  ${event.input.command}\nto:\n  ${result.rewritten}\n\nUse rewritten command?`,
            );
            if (!ok) return; // user declined — passthrough original
        }

        event.input.command = result.rewritten;
    });
}
