import { Plugin } from "@opencode-ai/plugin"
import { spawn } from "child_process"

// RTK OpenCode v2 plugin — rewrites commands to use rtk for token savings.
// Requires: rtk >= 0.23.0 in PATH.
//
// OpenCode 2.0 uses a new plugin API (Plugin.define + ctx.tool.hook) and does
// not provide the v1 bun-shell `$` helper, so this variant spawns `rtk`
// directly. `rtk rewrite` exits non-zero even on success, so we read stdout
// from the child instead of relying on exit codes.
//
// This is a thin delegating plugin: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.

function rtkRewrite(command: string): Promise<string> {
  return new Promise((resolve) => {
    const proc = spawn("rtk", ["rewrite", command])
    let out = ""
    proc.stdout.on("data", (d) => {
      out += d
    })
    proc.on("error", () => resolve(""))
    proc.on("close", () => resolve(out.trim()))
  })
}

export default Plugin.define({
  id: "rtk.rewrite",
  setup: async (ctx) => {
    await ctx.tool.hook("execute.before", async (event) => {
      const tool = String(event.tool ?? "").toLowerCase()
      if (tool !== "bash" && tool !== "shell") return
      if (!event.input || typeof event.input !== "object") return

      const command = (event.input as Record<string, unknown>).command
      if (typeof command !== "string" || !command) return

      const rewritten = await rtkRewrite(command)
      if (rewritten && rewritten !== command) {
        ;(event.input as Record<string, unknown>).command = rewritten
      }
    })
  },
})