import { Plugin } from "@opencode/plugin";
import { execFile } from "node:child_process";

// RTK OpenCode plugin (V2 API) — rewrites commands to use rtk for token savings.
// Requires: rtk >= 0.49.0 in PATH.
//
// This is the OpenCode V2 counterpart of `rtk.ts`. OpenCode V2 replaced the V1
// plugin API: a plugin now default-exports `Plugin.define({ id, setup })` and
// registers hooks on a context domain (`ctx.tool.hook("execute.before", ...)`)
// instead of returning a `{ "tool.execute.before": ... }` map. V1 plugins do
// not run in V2, and V2 plugins do not run in V1.
//
// `rtk init -g --opencode` picks this file when it detects OpenCode >= 2 and
// installs it as `~/.config/opencode/plugins/rtk.ts`.
//
// This is a thin delegating plugin: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.

/**
 * `execute.before` event emitted by OpenCode V2 before a tool runs.
 *
 * Mirrors the subset of the SDK's `ToolHooks["execute.before"]` that RTK relies
 * on: the tool name and its mutable input payload. The real event also carries
 * read-only `sessionID`, `agent`, `messageID`, and `id` fields that are not
 * needed here, and the SDK does not export the event type from its Promise
 * entrypoint, so it is declared locally.
 */
interface ToolExecuteBeforeEvent {
  readonly tool: string;
  input: unknown;
}

// `rtk rewrite` reports its result on stdout and signals it through the exit
// code: 0 or 3 means a rewritten command (possibly unchanged), 1 means there is
// nothing to rewrite. The stdout must therefore be read even when the process
// exits non-zero.
function rtkRewrite(command: string): Promise<string | undefined> {
  return new Promise((resolve) => {
    execFile(
      "rtk",
      ["rewrite", command],
      { timeout: 5_000 },
      (error, stdout: string) => {
        const code = error ? ((error as { code?: number }).code ?? -1) : 0;
        if (code !== 0 && code !== 3) {
          resolve(undefined);
          return;
        }
        const rewritten = stdout.trim();
        resolve(rewritten && rewritten !== command ? rewritten : undefined);
      },
    );
  });
}

function rtkAvailable(): Promise<boolean> {
  return new Promise((resolve) => {
    execFile("rtk", ["--version"], { timeout: 5_000 }, (error) => {
      resolve(!error);
    });
  });
}

export default Plugin.define({
  id: "rtk",
  async setup(ctx) {
    if (!(await rtkAvailable())) {
      console.warn("[rtk] rtk binary not found in PATH — plugin disabled");
      return;
    }

    await ctx.tool.hook(
      "execute.before",
      async (event: ToolExecuteBeforeEvent) => {
        const tool = event.tool.toLowerCase();
        if (tool !== "bash" && tool !== "shell") return;

        const args = event.input;
        if (!args || typeof args !== "object") return;

        const command = (args as Record<string, unknown>).command;
        if (typeof command !== "string" || !command) return;

        const rewritten = await rtkRewrite(command);
        if (rewritten) {
          (args as Record<string, unknown>).command = rewritten;
        }
      },
    );
  },
});
