import type { Plugin } from "@opencode-ai/plugin"

// RTK OpenCode plugin — rewrites commands to use rtk for token savings.
// Requires: an rtk with the `rtk hook opencode` subcommand (newer than
// v0.51). An older rtk answers nothing, so commands pass through unrewritten
// rather than breaking.
//
// This is a thin delegating plugin: all rewrite and permission logic lives
// in `rtk hook opencode`, which is the single source of truth
// (src/discover/registry.rs). It judges the command against OpenCode's own
// permission rules and answers `{}` whenever the rewrite would change what
// those rules decide — OpenCode evaluates the final command itself, so a
// rewrite RTK does return never lifts a deny, silences an ask, or blocks an
// allow. To add or change rewrite rules, edit the Rust registry — not this
// file.

type Answer = { command?: string }

export const RtkOpenCodePlugin: Plugin = async ({ $ }) => {
  try {
    await $`which rtk`.quiet()
  } catch {
    console.warn("[rtk] rtk binary not found in PATH — plugin disabled")
    return {}
  }

  return {
    "tool.execute.before": async (input, output) => {
      const tool = String(input?.tool ?? "").toLowerCase()
      if (tool !== "bash" && tool !== "shell") return
      const args = output?.args
      if (!args || typeof args !== "object") return

      const command = (args as Record<string, unknown>).command
      if (typeof command !== "string" || !command) return

      try {
        const result = await $`rtk hook opencode ${command}`.quiet().nothrow()
        const answer = JSON.parse(String(result.stdout).trim() || "{}") as Answer
        if (answer.command && answer.command !== command) {
          ;(args as Record<string, unknown>).command = answer.command
        }
      } catch {
        // rtk hook opencode failed or answered nothing — pass through unchanged
      }
    },
  }
}
