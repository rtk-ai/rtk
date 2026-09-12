import type { Plugin } from "@opencode-ai/plugin"
import { execFile } from "node:child_process"
import { promisify } from "node:util"

// RTK OpenCode plugin — rewrites commands to use rtk for token savings.
// Supports both OpenCode 1.0 (legacy plugin format) and OpenCode 2.0 (event-driven plugin API).
// Requires: rtk in PATH.
//
// This is a thin delegating plugin: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.

const execFileAsync = promisify(execFile)

async function checkRtk(): Promise<boolean> {
  try {
    await execFileAsync("which", ["rtk"])
    return true
  } catch {
    return false
  }
}

async function rewriteCommand(command: string): Promise<string> {
  if (!command || typeof command !== "string") return command
  try {
    const { stdout } = await execFileAsync("rtk", ["rewrite", command], { timeout: 3000 })
    const rewritten = String(stdout).trim()
    return rewritten || command
  } catch {
    return command
  }
}

// OpenCode 1.0 hook implementation
export const RtkOpenCodePlugin: Plugin = async ({ $ } = {} as any) => {
  let hasRtk = false
  try {
    if ($) {
      await $`which rtk`.quiet()
      hasRtk = true
    } else {
      hasRtk = await checkRtk()
    }
  } catch {
    hasRtk = false
  }

  if (!hasRtk) {
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
        const rewritten = await rewriteCommand(command)
        if (rewritten && rewritten !== command) {
          ;(args as Record<string, unknown>).command = rewritten
        }
      } catch {
        // rtk rewrite failed — pass through unchanged
      }
    },
  }
}

// OpenCode 2.0 plugin implementation
const rtkPlugin = {
  id: "rtk",
  server: RtkOpenCodePlugin,
  setup: async (context: any) => {
    const hasRtk = await checkRtk()
    if (!hasRtk) {
      console.warn("[rtk] rtk binary not found in PATH — plugin disabled")
      return
    }

    // Hook tool execution for OpenCode 2.0 (tool: 'shell', 'bash', or 'execute')
    if (context?.tool?.hook) {
      await context.tool.hook("execute.before", async (event: any) => {
        const tool = String(event?.tool ?? "").toLowerCase()
        if (tool !== "bash" && tool !== "shell" && tool !== "execute") return
        const input = event?.input
        if (!input || typeof input !== "object") return

        const command = (input as Record<string, unknown>).command
        if (typeof command !== "string" || !command) return

        try {
          const rewritten = await rewriteCommand(command)
          if (rewritten && rewritten !== command) {
            ;(input as Record<string, unknown>).command = rewritten
          }
        } catch {
          // rtk rewrite failed — pass through unchanged
        }
      })
    }

    // Hook shell process creation if available
    if (context?.shell?.hook) {
      await context.shell.hook("create.before", async (event: any) => {
        const command = event?.command
        if (typeof command !== "string" || !command) return

        try {
          const rewritten = await rewriteCommand(command)
          if (rewritten && rewritten !== command) {
            event.command = rewritten
          }
        } catch {
          // rtk rewrite failed — pass through unchanged
        }
      })
    }
  },
}

export default rtkPlugin
