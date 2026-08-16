// RTK Pi / Prime Agent extension — rewrites bash commands to use rtk.
// Requires: rtk >= 0.23.0 in PATH.
//
// This is a thin delegating extension: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.
//
// Exit code contract for `rtk rewrite`:
//   0 + stdout  Rewrite found → mutate command
//   1           No RTK equivalent → pass through unchanged
//   3 + stdout  Rewrite (advisory) → mutate command
//   2           Deny rule matched → pass through (this extension does not gate)

import type { ExtensionAPI } from "@earendil-works/pi-coding-agent"
import { isToolCallEventType } from "@earendil-works/pi-coding-agent"

const REWRITE_TIMEOUT_MS = 2_000
const MIN_SUPPORTED_RTK_MINOR = 23
const MAX_BASH_LINES = 200

const BASH_CELL = /^(%%bash(?:[ \t]+[^\r\n]*)?)(\r?\n)([\s\S]*)$/

// Parse "X.Y.Z" semver, return [major, minor, patch] or null.
function parseSemver(raw: string): [number, number, number] | null {
  const m = raw.trim().match(/(\d+)\.(\d+)\.(\d+)/)
  if (!m) return null
  return [parseInt(m[1], 10), parseInt(m[2], 10), parseInt(m[3], 10)]
}

// Calls `rtk rewrite`; returns the rewritten command or null (pass through).
async function rewriteCommand(
  pi: ExtensionAPI,
  cmd: string,
  signal?: AbortSignal
): Promise<string | null> {
  const result = await pi.exec("rtk", ["rewrite", cmd], {
    timeout: REWRITE_TIMEOUT_MS,
    signal,
  })
  if (result.killed) return null
  if (result.code !== 0 && result.code !== 3) return null
  return result.stdout.trim() || null
}

// Rewrite one line inside a %%bash cell. Empty/comment/continuation lines are
// left untouched; everything else is delegated to `rtk rewrite`.
async function rewriteBashCellLine(
  pi: ExtensionAPI,
  line: string,
  signal?: AbortSignal
): Promise<string> {
  const trimmed = line.trimEnd()
  if (!trimmed || trimmed.startsWith("#") || trimmed.endsWith("\\")) return line

  const indent = line.match(/^\s*/)?.[0] ?? ""
  const command = line.slice(indent.length)
  const rewritten = await rewriteCommand(pi, command, signal)
  return rewritten ? indent + rewritten : line
}

// Rewrite a multi-line %%bash ipython cell. Returns undefined when the cell
// should be left unchanged.
async function rewriteBashCell(
  pi: ExtensionAPI,
  code: string,
  signal?: AbortSignal
): Promise<string | undefined> {
  const match = code.match(BASH_CELL)
  if (!match) return undefined

  const [, header, newline, body] = match
  if (!body.trim()) return undefined

  const tokens = body.match(/.*(?:\r?\n|$)/g) ?? []
  const lineCount = tokens.filter((t) => t.length > 0).length
  if (lineCount > MAX_BASH_LINES) {
    console.warn(`[rtk] %%bash cell has ${lineCount} lines; skipping rewrite`)
    return undefined
  }

  const rewrittenLines: string[] = []
  for (const token of tokens) {
    if (!token) continue
    if (signal?.aborted) break
    const newlineMatch = token.match(/\r?\n$/)
    const newline = newlineMatch?.[0] ?? ""
    const content = token.slice(0, token.length - newline.length)
    const rewrittenContent = await rewriteBashCellLine(pi, content, signal)
    rewrittenLines.push(rewrittenContent + newline)
  }
  const rewrittenBody = rewrittenLines.join("")

  // Never replace a non-empty cell with an empty body.
  return rewrittenBody.trim() ? `${header}${newline}${rewrittenBody}` : undefined
}

export default async function (pi: ExtensionAPI) {
  // Probe rtk version at load time; disables extension if missing or too old.
  const ver = await pi.exec("rtk", ["--version"], { timeout: REWRITE_TIMEOUT_MS })
  if (ver.code !== 0) {
    console.warn("[rtk] rtk binary not found in PATH — extension disabled")
    return
  }

  // Warn and bail if rtk predates 0.23.0 (when `rtk rewrite` was introduced).
  const parsed = parseSemver(ver.stdout.replace(/^rtk\s+/, ""))
  if (parsed) {
    const [major, minor] = parsed
    if (major === 0 && minor < MIN_SUPPORTED_RTK_MINOR) {
      console.warn(`[rtk] rtk ${ver.stdout.trim()} is too old (need >= 0.23.0) — extension disabled`)
      return
    }
  }

  pi.on("tool_call", async (event, ctx) => {
    try {
      if (process.env.RTK_DISABLED === "1") return

      // Pi: bash tool with a single command.
      if (isToolCallEventType("bash", event)) {
        const cmd = event.input.command
        if (typeof cmd !== "string" || cmd.trim() === "") return
        if (cmd.startsWith("rtk ")) return

        const rewritten = await rewriteCommand(pi, cmd, ctx.signal)
        if (rewritten && rewritten !== cmd) {
          event.input.command = rewritten
        }
        return
      }

      // Prime Agent: ipython tool containing a %%bash cell.
      if (event.toolName === "ipython" && typeof event.input?.code === "string") {
        const original = event.input.code
        const rewritten = await rewriteBashCell(pi, original, ctx.signal)
        if (rewritten && rewritten !== original) {
          event.input.code = rewritten
        }
      }
    } catch (err) {
      // Fail open: never block execution on an unexpected error.
      console.warn("[rtk] unexpected error in tool_call handler; passing through", err)
    }
  })
}
