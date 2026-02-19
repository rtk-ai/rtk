// RTK (Rust Token Killer) plugin for OpenCode
// Transparently rewrites bash commands to their rtk equivalents,
// reducing LLM token consumption by 60-90%.
//
// Install globally:  ~/.config/opencode/plugins/rtk.ts
// Install per-project: .opencode/plugins/rtk.ts
//
// Requires: rtk binary in PATH (https://github.com/rtk-ai/rtk)

import type { Plugin } from "@opencode-ai/plugin"

/** Check if a command is already prefixed with rtk */
function isAlreadyRtk(cmd: string): boolean {
  return /^(rtk\s|.*\/rtk\s)/.test(cmd)
}

/** Check if a command contains heredocs */
function hasHeredoc(cmd: string): boolean {
  return cmd.includes("<<")
}

/**
 * Strip leading environment variable assignments for pattern matching.
 * e.g., "TEST_SESSION_ID=2 npx playwright test" → { prefix: "TEST_SESSION_ID=2 ", body: "npx playwright test" }
 */
function stripEnvPrefix(cmd: string): { prefix: string; body: string } {
  const match = cmd.match(/^([A-Za-z_][A-Za-z0-9_]*=[^ ]* +)+/)
  if (match) {
    const prefix = match[0]
    return { prefix, body: cmd.slice(prefix.length) }
  }
  return { prefix: "", body: cmd }
}

/**
 * Strip git global flags for subcommand matching.
 * Removes -C <path>, -c <key=val>, --no-pager, --no-optional-locks, --bare, --literal-pathspecs
 */
function stripGitFlags(subcmd: string): string {
  return subcmd
    .replace(/(-C|-c)\s+\S+\s*/g, "")
    .replace(/--[a-z-]+=\S+\s*/g, "")
    .replace(/--(no-pager|no-optional-locks|bare|literal-pathspecs)\s*/g, "")
    .trim()
}

/**
 * Port of rtk-rewrite.sh rewrite rules to TypeScript.
 * Returns the rewritten command string, or null if no rewrite applies.
 */
function rewriteCommand(cmd: string): string | null {
  const trimmed = cmd.trim()

  // Skip if already using rtk or contains heredocs
  if (isAlreadyRtk(trimmed) || hasHeredoc(trimmed)) {
    return null
  }

  const { prefix, body } = stripEnvPrefix(trimmed)
  const matchCmd = body

  // --- Git commands ---
  if (/^git\s/.test(matchCmd)) {
    const gitRest = matchCmd.replace(/^git\s+/, "")
    const subcmd = stripGitFlags(gitRest)
    if (
      /^(status|diff|log|add|commit|push|pull|branch|fetch|stash|show)(\s|$)/.test(
        subcmd,
      )
    ) {
      return `${prefix}rtk ${body}`
    }
    return null
  }

  // --- GitHub CLI ---
  if (/^gh\s+(pr|issue|run|api|release)(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^gh /, "rtk gh ")}`
  }

  // --- Cargo ---
  if (/^cargo\s/.test(matchCmd)) {
    const cargoRest = matchCmd.replace(/^cargo\s+(\+\S+\s+)?/, "")
    if (/^(test|build|clippy|check|install|fmt)(\s|$)/.test(cargoRest)) {
      return `${prefix}rtk ${body}`
    }
    return null
  }

  // --- File operations ---
  if (/^cat\s+/.test(matchCmd)) {
    return `${prefix}${body.replace(/^cat /, "rtk read ")}`
  }
  if (/^(rg|grep)\s+/.test(matchCmd)) {
    return `${prefix}${body.replace(/^(rg|grep) /, "rtk grep ")}`
  }
  if (/^ls(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^ls/, "rtk ls")}`
  }
  if (/^tree(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^tree/, "rtk tree")}`
  }
  if (/^find\s+/.test(matchCmd)) {
    return `${prefix}${body.replace(/^find /, "rtk find ")}`
  }
  if (/^diff\s+/.test(matchCmd)) {
    return `${prefix}${body.replace(/^diff /, "rtk diff ")}`
  }

  // --- head → rtk read with --max-lines ---
  {
    let headMatch = matchCmd.match(/^head\s+-(\d+)\s+(.+)$/)
    if (headMatch) {
      return `${prefix}rtk read ${headMatch[2]} --max-lines ${headMatch[1]}`
    }
    headMatch = matchCmd.match(/^head\s+--lines=(\d+)\s+(.+)$/)
    if (headMatch) {
      return `${prefix}rtk read ${headMatch[2]} --max-lines ${headMatch[1]}`
    }
  }

  // --- JS/TS tooling ---
  if (/^(pnpm\s+)?(npx\s+)?vitest(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^(pnpm )?(npx )?vitest( run)?/, "rtk vitest run")}`
  }
  if (/^pnpm\s+test(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^pnpm test/, "rtk vitest run")}`
  }
  if (/^npm\s+test(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^npm test/, "rtk npm test")}`
  }
  if (/^npm\s+run\s+/.test(matchCmd)) {
    return `${prefix}${body.replace(/^npm run /, "rtk npm ")}`
  }
  if (/^(npx\s+)?vue-tsc(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^(npx )?vue-tsc/, "rtk tsc")}`
  }
  if (/^pnpm\s+tsc(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^pnpm tsc/, "rtk tsc")}`
  }
  if (/^(npx\s+)?tsc(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^(npx )?tsc/, "rtk tsc")}`
  }
  if (/^pnpm\s+lint(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^pnpm lint/, "rtk lint")}`
  }
  if (/^(npx\s+)?eslint(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^(npx )?eslint/, "rtk lint")}`
  }
  if (/^(npx\s+)?prettier(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^(npx )?prettier/, "rtk prettier")}`
  }
  if (/^(npx\s+)?playwright(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^(npx )?playwright/, "rtk playwright")}`
  }
  if (/^pnpm\s+playwright(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^pnpm playwright/, "rtk playwright")}`
  }
  if (/^(npx\s+)?prisma(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^(npx )?prisma/, "rtk prisma")}`
  }

  // --- Containers ---
  if (/^docker\s/.test(matchCmd)) {
    if (/^docker\s+compose(\s|$)/.test(matchCmd)) {
      return `${prefix}${body.replace(/^docker /, "rtk docker ")}`
    }
    const dockerRest = matchCmd
      .replace(/^docker\s+/, "")
      .replace(/(-H|--context|--config)\s+\S+\s*/g, "")
      .replace(/--[a-z-]+=\S+\s*/g, "")
      .trim()
    if (
      /^(ps|images|logs|run|build|exec)(\s|$)/.test(dockerRest)
    ) {
      return `${prefix}${body.replace(/^docker /, "rtk docker ")}`
    }
    return null
  }
  if (/^kubectl\s/.test(matchCmd)) {
    const kubeRest = matchCmd
      .replace(/^kubectl\s+/, "")
      .replace(/(--context|--kubeconfig|--namespace|-n)\s+\S+\s*/g, "")
      .replace(/--[a-z-]+=\S+\s*/g, "")
      .trim()
    if (/^(get|logs|describe|apply)(\s|$)/.test(kubeRest)) {
      return `${prefix}${body.replace(/^kubectl /, "rtk kubectl ")}`
    }
    return null
  }

  // --- Network ---
  if (/^curl\s+/.test(matchCmd)) {
    return `${prefix}${body.replace(/^curl /, "rtk curl ")}`
  }
  if (/^wget\s+/.test(matchCmd)) {
    return `${prefix}${body.replace(/^wget /, "rtk wget ")}`
  }

  // --- pnpm package management ---
  if (/^pnpm\s+(list|ls|outdated)(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^pnpm /, "rtk pnpm ")}`
  }

  // --- Python tooling ---
  if (/^pytest(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^pytest/, "rtk pytest")}`
  }
  if (/^python\s+-m\s+pytest(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^python -m pytest/, "rtk pytest")}`
  }
  if (/^ruff\s+(check|format)(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^ruff /, "rtk ruff ")}`
  }
  if (/^pip\s+(list|outdated|install|show)(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^pip /, "rtk pip ")}`
  }
  if (/^uv\s+pip\s+(list|outdated|install|show)(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^uv pip /, "rtk pip ")}`
  }

  // --- Go tooling ---
  if (/^go\s+test(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^go test/, "rtk go test")}`
  }
  if (/^go\s+build(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^go build/, "rtk go build")}`
  }
  if (/^go\s+vet(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^go vet/, "rtk go vet")}`
  }
  if (/^golangci-lint(\s|$)/.test(matchCmd)) {
    return `${prefix}${body.replace(/^golangci-lint/, "rtk golangci-lint")}`
  }

  // No rewrite matched
  return null
}

/**
 * RTK Plugin for OpenCode.
 *
 * Intercepts bash commands via the tool.execute.before hook and transparently
 * rewrites them to their rtk equivalents for token-optimized output.
 *
 * This is the OpenCode equivalent of Claude Code's PreToolUse hook
 * (hooks/rtk-rewrite.sh).
 */
export const RTKPlugin: Plugin = async ({ $ }) => {
  // Guard: skip if rtk binary is not installed
  const check = await $`which rtk`.quiet().nothrow()
  if (check.exitCode !== 0) {
    return {}
  }

  return {
    "tool.execute.before": async (input, output) => {
      // Only intercept bash tool calls
      if (input.tool !== "bash") return

      const cmd = output.args.command as string
      if (!cmd) return

      const rewritten = rewriteCommand(cmd)
      if (rewritten) {
        output.args.command = rewritten
      }
    },
  }
}

// Also export as default for single-file plugin usage
export default RTKPlugin
