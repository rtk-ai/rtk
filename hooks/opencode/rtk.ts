import type { Plugin } from "@opencode-ai/plugin"
import { execFile } from "node:child_process"

// RTK OpenCode plugin — rewrites commands to use rtk for token savings.
// Requires: rtk >= 0.23.0 in PATH.
//
// This is a thin delegating plugin: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.

function runRtk(args: string[]): Promise<string> {
  return new Promise((resolve) => {
    execFile("rtk", args, { windowsHide: true }, (_error, stdout) => {
      // `rtk rewrite` may return a non-zero status while still emitting a rewrite.
      resolve(String(stdout ?? "").trim())
    })
  })
}

export const RtkOpenCodePlugin: Plugin = async () => {
  const version = await runRtk(["--version"])
  if (!version) {
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

      const rewritten = await runRtk(["rewrite", command])
      if (rewritten && rewritten !== command) {
        ;(args as Record<string, unknown>).command = rewritten
      }
    },
  }
}
