/**
 * RTK (Rust Token Killer) — OpenCode plugin
 *
 * Transparently rewrites bash commands to their rtk equivalents before
 * execution, saving 60-90% of tokens on common dev operations.
 *
 * Requires: rtk >= 0.23.0 installed and on PATH.
 *
 * Install:
 *   cp rtk-rewrite.ts ~/.config/opencode/plugins/
 *   # or
 *   cp rtk-rewrite.ts .opencode/plugins/
 */

import type { Plugin } from "@opencode-ai/plugin"

export const RtkRewritePlugin: Plugin = async ({ $ }) => {
  // Verify rtk is available at startup
  const hasRtk = await $`command -v rtk`.quiet().then(
    () => true,
    () => false,
  )

  if (!hasRtk) {
    console.warn("[rtk] rtk binary not found on PATH — plugin disabled")
    console.warn("[rtk] Install: cargo install --git https://github.com/rtk-ai/rtk")
    return {}
  }

  // Version check: rtk rewrite requires >= 0.23.0
  const versionOk = await $`rtk --version`
    .quiet()
    .text()
    .then((out) => {
      const match = out.match(/(\d+)\.(\d+)\.(\d+)/)
      if (!match) return false
      const [, major, minor] = match.map(Number)
      return major > 0 || minor >= 23
    })
    .catch(() => false)

  if (!versionOk) {
    console.warn("[rtk] rtk >= 0.23.0 required for rewrite support — plugin disabled")
    console.warn("[rtk] Upgrade: cargo install --git https://github.com/rtk-ai/rtk --force")
    return {}
  }

  return {
    "tool.execute.before": async (input, output) => {
      if (input.tool !== "bash") return

      const cmd: string | undefined = output.args?.command
      if (!cmd || cmd.trim().length === 0) return

      // Skip commands already using rtk
      if (cmd.trimStart().startsWith("rtk ")) return

      // Skip heredocs
      if (cmd.includes("<<")) return

      // Delegate rewrite logic to rtk binary (single source of truth)
      // rtk rewrite exits 1 when there's no rewrite — we just keep the
      // original command in that case.
      const rewritten = await $`rtk rewrite ${cmd}`
        .quiet()
        .text()
        .then((t) => t.trim())
        .catch(() => null)

      if (!rewritten || rewritten === cmd) return

      output.args.command = rewritten
    },
  }
}
