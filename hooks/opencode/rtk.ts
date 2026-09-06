import type { Plugin } from "@opencode-ai/plugin"
import { execFile, execFileSync } from "node:child_process"

// RTK OpenCode plugin — rewrites commands to use rtk for token savings.
// Requires: rtk >= 0.23.0 in PATH.
//
// This is a thin delegating plugin: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.
//
// Note: OpenCode Desktop runs its server in an Electron utility process (Node.js)
// where Bun.$ is unavailable. The plugin falls back to child_process in that case.

function execRtk($: any, args: string[]): Promise<{ stdout: string }> {
  if ($) {
    return $`rtk ${args}`.quiet().nothrow().then((r: any) => ({
      stdout: String(r.stdout).trim(),
    }))
  }
  return new Promise((resolve) => {
    execFile("rtk", args, { encoding: "utf8", timeout: 5000 }, (_err, stdout) => {
      resolve({ stdout: (stdout ?? "").trim() })
    })
  })
}

export const RtkOpenCodePlugin: Plugin = async ({ $ }) => {
  try {
    if ($) {
      await $`which rtk`.quiet()
    } else {
      execFileSync("which", ["rtk"], { encoding: "utf8", timeout: 5000 })
    }
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
        const result = await execRtk($, ["rewrite", command])
        if (result.stdout && result.stdout !== command) {
          ;(args as Record<string, unknown>).command = result.stdout
        }
      } catch {
        // rtk rewrite failed — pass through unchanged
      }
    },
  }
}
