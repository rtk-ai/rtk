import type { Plugin } from "@opencode-ai/plugin"

// RTK OpenCode plugin — rewrites commands to use rtk for token savings.
// Requires: rtk >= 0.23.0 in PATH.
//
// This is a thin delegating plugin: all rewrite logic lives in `rtk hook opencode`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.

type Answer = { command?: string; status?: "allow" | "ask" | "deny" }

const MAX_PENDING = 256

export const RtkOpenCodePlugin: Plugin = async ({ $ }) => {
  try {
    await $`which rtk`.quiet()
  } catch {
    console.warn("[rtk] rtk binary not found in PATH — plugin disabled")
    return {}
  }

  const pending = new Map<string, "allow" | "ask" | "deny">()

  const remember = (callID: string | undefined, status: Answer["status"]) => {
    if (!callID || !status) return
    if (pending.size >= MAX_PENDING) {
      const oldest = pending.keys().next()
      if (!oldest.done) pending.delete(oldest.value)
    }
    pending.set(callID, status)
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
        remember(input?.callID, answer.status)
        if (answer.command && answer.command !== command) {
          ;(args as Record<string, unknown>).command = answer.command
        }
      } catch {
        // rtk rewrite failed — pass through unchanged
      }
    },

    "permission.ask": async (input, output) => {
      const callID = input?.callID
      if (!callID) return
      const status = pending.get(callID)
      if (!status) return
      pending.delete(callID)
      output.status = status
    },
  }
}
