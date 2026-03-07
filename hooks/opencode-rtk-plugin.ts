// rtk-hook-version: 2
// RTK OpenCode plugin — rewrites bash commands to use rtk for token savings.
// Requires: rtk >= 0.23.0
//
// This is a thin delegating plugin: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.
//
// Install (global):
//   cp hooks/opencode-rtk-plugin.ts ~/.config/opencode/plugin/rtk.ts
//
// Install (per-project):
//   cp hooks/opencode-rtk-plugin.ts .opencode/plugin/rtk.ts

import type { Plugin } from "@opencode-ai/plugin"

export const RtkPlugin: Plugin = async ({ $ }) => {
  // Bail early if rtk is not installed — no noise, no errors.
  const check = await $`command -v rtk`.quiet().nothrow()
  if (check.exitCode !== 0) return {}

  return {
    "tool.execute.before": async (input, output) => {
      if (input.tool !== "bash") return

      const cmd = output.args?.command
      if (!cmd || typeof cmd !== "string") return

      // Delegate all rewrite logic to the Rust binary.
      // rtk rewrite exits 1 when there's no rewrite — plugin passes through silently.
      const proc = await $`rtk rewrite ${cmd}`.quiet().nothrow()
      if (proc.exitCode !== 0) return

      const rewritten = proc.text().trim()
      if (!rewritten || rewritten === cmd) return

      output.args.command = rewritten
    },
  }
}
