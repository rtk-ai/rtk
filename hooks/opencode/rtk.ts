import type { Plugin } from "@opencode-ai/plugin"

// RTK OpenCode plugin — rewrites commands to use rtk for token savings.
// Requires: rtk >= 0.23.0 in PATH.
//
// This is a thin delegating plugin: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.
//
// OpenCode v2 (>=2.0.x) requires the default export to satisfy
// `{ id, setup | effect }`. We export a default that carries both:
//   - `id` + `setup` so the v2 loader's schema decode succeeds
//   - `server` so OpenCode v1 hosts keep working
// The hook logic lives once in `RtkOpenCodePlugin` and is reused by v1.

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
        const result = await $`rtk rewrite ${command}`.quiet().nothrow()
        const rewritten = String(result.stdout).trim()
        if (rewritten && rewritten !== command) {
          ;(args as Record<string, unknown>).command = rewritten
        }
      } catch {
        // rtk rewrite failed — pass through unchanged
      }
    },
  }
}

export default {
  id: "rtk",
  // v2 hosts require `setup` (or `effect`). The real v1 hook logic is in
  // `server` below; v2 currently ignores `server`. A future revision will
  // re-register the hook through the v2 PluginContext when its tool APIs
  // stabilize (see https://github.com/anomalyco/opencode/issues/42878).
  setup: async () => {},
  server: RtkOpenCodePlugin,
}
