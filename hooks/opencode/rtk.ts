import type { Plugin } from "@opencode-ai/plugin"

const MIN_SUPPORTED_RTK_MINOR = 23

// RTK OpenCode plugin — rewrites commands to use rtk for token savings.
// Requires: rtk >= 0.23.0 in PATH.
//
// This is a thin delegating plugin: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.

export const RtkOpenCodePlugin: Plugin = async ({ $ }) => {
  if (!await probeRtk($)) return {}

  return {
    "tool.execute.before": async (input, output) => {
      const tool = String((input as any)?.tool ?? "").toLowerCase()
      if (tool !== "bash" && tool !== "shell") return

      const args = output?.args
      if (!args || typeof args !== "object") return

      const command = (args as Record<string, unknown>).command
      if (typeof command !== "string" || !command.trim()) return
      if (command.startsWith("rtk ")) return
      if (process.env.RTK_DISABLED === "1") return

      const rewritten = await rewriteCommand($, command)
      if (rewritten) {
        ;(args as Record<string, unknown>).command = rewritten
      }
    },
  }
}

async function probeRtk($: any): Promise<boolean> {
  try {
    const result = await $`rtk --version`.quiet().nothrow()
    if (result.exitCode !== 0) {
      console.warn("[rtk] rtk binary not found in PATH — plugin disabled")
      return false
    }

    const parsed = parseSemver(String(result.stdout).replace(/^rtk\s+/, ""))
    if (!parsed) return true

    const [major, minor] = parsed
    if (major === 0 && minor < MIN_SUPPORTED_RTK_MINOR) {
      console.warn(`[rtk] rtk ${String(result.stdout).trim()} is too old (need >= 0.23.0) — plugin disabled`)
      return false
    }

    return true
  } catch (err) {
    console.warn("[rtk] rtk probe failed unexpectedly — plugin disabled", err)
    return false
  }
}

async function rewriteCommand($: any, command: string): Promise<string | null> {
  const result = await $`rtk rewrite ${command}`
    .quiet()
    .nothrow()

  if (result.exitCode !== 0 && result.exitCode !== 3) return null

  const rewritten = String(result.stdout).trim()
  return rewritten && rewritten !== command ? rewritten : null
}

function parseSemver(raw: string): [number, number, number] | null {
  const m = raw.trim().match(/(\d+)\.(\d+)\.(\d+)/)
  if (!m) return null
  return [parseInt(m[1], 10), parseInt(m[2], 10), parseInt(m[3], 10)]
}
