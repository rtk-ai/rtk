import type { Plugin } from "@opencode-ai/plugin"

// RTK OpenCode plugin — rewrites commands to use rtk for token savings,
// and compresses heavy tool outputs (built-in + MCP) via tool.execute.after.
//
// Requires: rtk >= 0.42.0 in PATH (matches workspace Cargo.toml).
//
// Command rewrite logic lives in `rtk rewrite` (src/discover/registry.rs).
// Output compression is handled in-plugin below.

// ─── Configuration ────────────────────────────────────────────────────────────

// Approximate token threshold before we compress output.
// ~4 chars per token is a safe heuristic for English/code.
const TOKEN_THRESHOLD = parseInt(process.env.RTK_TOKEN_THRESHOLD ?? "3000")
const CHAR_THRESHOLD = TOKEN_THRESHOLD * 4

// Set RTK_DEBUG=1 to log every tool call (size, sink, threshold, compression
// result) to /tmp/rtk-plugin.log. Off by default — no I/O on the hot path.
const RTK_DEBUG = process.env.RTK_DEBUG === "1"

function logDebug(msg: string): void {
  if (!RTK_DEBUG) return
  try {
    require("fs").appendFileSync("/tmp/rtk-plugin.log", msg + "\n")
  } catch {}
}

// Maximum output chars after compression.
const MAX_OUTPUT_CHARS = parseInt(process.env.RTK_MAX_OUTPUT_CHARS ?? "32000")

// Effective compression target. The gate below only compresses when
// text.length >= CHAR_THRESHOLD, but the strategies only trim when
// text.length > maxChars. If maxChars > CHAR_THRESHOLD there's a dead zone
// (12000–32000 with defaults) where outputs pass the gate but compress to
// nothing. Cap the target at the threshold so anything that enters actually
// gets compressed.
const EFFECTIVE_MAX_CHARS = Math.min(MAX_OUTPUT_CHARS, CHAR_THRESHOLD)

// ─── Tool Output Registry ────────────────────────────────────────────────────
// Every tool whose output can blow up context. Map: toolName → compression strategy.
//
// Strategies:
//   truncate-middle — preserves head (60%) + tail (40%), drops middle
//   truncate-tail   — preserves head, drops tail (best for docs/logs)
//   json-compact    — compact JSON: prune deep nesting, truncate arrays/strings

type Strategy = "truncate-middle" | "truncate-tail" | "json-compact" | "rtk-minimal"

const HEAVY_TOOLS: Record<string, Strategy> = {
  // ──────────── OpenCode Built-in Tools ──────────────────────────
  // These are NOT bash — they bypass the rewrite hook entirely.

  // Read — reads files. #1 token consumer (1588 calls in logs).
  // rtk-minimal: replicates `rtk read --level minimal` (src/core/filter.rs
  // MinimalFilter) — strips line/block comments (keeps doc comments),
  // normalizes 3+ blank lines → 2 — while PRESERVING opencode's original
  // `N:` line numbers so file:line references stay valid. For files just
  // over the threshold this keeps the whole file (comments stripped) instead
  // of blind middle-truncation; for huge files it falls back to truncateMiddle
  // on the comment-stripped (denser) result.
  "read": "rtk-minimal",

  // Grep — searches across files. #2 consumer (215 calls).
  // Tail-truncate: first matches are most relevant; the rest is noise.
  "grep": "truncate-tail",

  // Glob — lists files matching patterns (59 calls).
  // Tail-truncate: first matches + structure matter most.
  "glob": "truncate-tail",

  // Task — sub-agent results (45 calls). Can return full conversations.
  "task": "truncate-middle",

  // WebFetch — fetches full web pages (27 calls). HTML/markdown dump.
  "webfetch": "truncate-tail",

  // ──────────── MCP: Codegraph ───────────────────────────────────
  // Returns full source of relevant symbols across files (64 calls).
  "codegraph_codegraph_explore": "truncate-middle",

  // ──────────── MCP: Context7 ────────────────────────────────────
  // Returns library documentation chunks (can be huge).
  "context7_query-docs": "truncate-tail",

  // ──────────── Plugin: Graphify ─────────────────────────────────
  // Knowledge graph queries return large structured output.
  // NOTE: graphify is a native OpenCode *plugin*, not an MCP server, so
  // its tool ids are registered verbatim (single "graphify_" prefix) —
  // not "graphify_graphify_*". See opencode registry.ts fromPlugin(id).
  "graphify_query": "truncate-middle",
  "graphify_explain": "truncate-tail",
  "graphify_path": "truncate-middle",
  "graphify_affected": "truncate-middle",

  // ──────────── MCP: Chrome DevTools ─────────────────────────────
  // Page snapshots and network logs can be massive.
  "chrome-devtools-mcp_take_snapshot": "json-compact",
  "chrome-devtools-mcp_list_network_requests": "json-compact",
  "chrome-devtools-mcp_list_console_messages": "truncate-tail",
  "chrome-devtools-mcp_performance_analyze_insight": "truncate-tail",
  "chrome-devtools-mcp_take_heapsnapshot": "json-compact",

  // ──────────── MCP: Notebooks ───────────────────────────────────
  // Cell outputs can contain huge data frames.
  "notebooks_get_cell_outputs": "truncate-tail",
  "notebooks_get_cell_range": "truncate-middle",

  // ──────────── MCP: Engram ──────────────────────────────────────
  // mem_search / mem_context can return many observations.
  "engram_mem_search": "truncate-tail",
  "engram_mem_context": "truncate-tail",
  "engram_mem_get_observation": "truncate-tail",
}

const DEFAULT_STRATEGY: Strategy = "truncate-middle"

// ─── Compression Helpers ─────────────────────────────────────────────────────

/**
 * Approximate token count (~4 chars/token for English/code).
 */
function approxTokens(str: string): number {
  return Math.ceil(str.length / 4)
}

/**
 * Truncate from the middle, preserving head and tail for context.
 * Keeps 60% head, 40% tail — the beginning typically has the most
 * important structural info (file headers, symbol defs).
 */
function truncateMiddle(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text
  const headRatio = 0.6
  const headSize = Math.floor(maxChars * headRatio)
  const tailSize = maxChars - headSize
  const removed = text.length - maxChars
  const removedTokens = Math.ceil(removed / 4)

  return (
    text.slice(0, headSize) +
    `\n\n[… ${removedTokens} tokens truncated by rtk — ${removed} chars removed from middle …]\n\n` +
    text.slice(-tailSize)
  )
}

/**
 * Truncate from the tail, keeping the beginning intact.
 * Best for docs/logs where the most relevant content is at the start.
 */
function truncateTail(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text
  const removed = text.length - maxChars
  const removedTokens = Math.ceil(removed / 4)

  return (
    text.slice(0, maxChars) +
    `\n\n[… ${removedTokens} tokens truncated by rtk — ${removed} chars removed from end …]`
  )
}

/**
 * Compact JSON output: parse, re-serialize with minimal whitespace,
 * truncate large arrays/values, and strip deep nesting.
 */
function jsonCompact(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text

  try {
    const parsed = JSON.parse(text)
    const compacted = compactValue(parsed, 4)
    const result = JSON.stringify(compacted, null, 1)
    if (result.length <= maxChars) return result
    return truncateTail(result, maxChars)
  } catch {
    return truncateTail(text, maxChars)
  }
}

/**
 * Recursively compact a JSON value:
 * - Arrays > 20 items → keep first 15 + last 5 + marker
 * - Strings > 500 chars → truncate with marker
 * - Objects nested > maxDepth → replace with summary
 */
function compactValue(value: unknown, maxDepth: number, depth = 0): unknown {
  if (value === null || value === undefined) return value
  if (typeof value === "number" || typeof value === "boolean") return value

  if (typeof value === "string") {
    if (value.length > 500) {
      return value.slice(0, 400) + `… [${value.length - 400} chars truncated]`
    }
    return value
  }

  if (Array.isArray(value)) {
    if (depth >= maxDepth) {
      return `[Array(${value.length})]`
    }
    if (value.length > 20) {
      const head = value.slice(0, 15).map((v) => compactValue(v, maxDepth, depth + 1))
      const tail = value.slice(-5).map((v) => compactValue(v, maxDepth, depth + 1))
      return [...head, `… ${value.length - 20} items omitted …`, ...tail]
    }
    return value.map((v) => compactValue(v, maxDepth, depth + 1))
  }

  if (typeof value === "object") {
    if (depth >= maxDepth) {
      const keys = Object.keys(value as Record<string, unknown>)
      return `{${keys.length} keys: ${keys.slice(0, 5).join(", ")}${keys.length > 5 ? "…" : ""}}`
    }
    const result: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      result[k] = compactValue(v, maxDepth, depth + 1)
    }
    return result
  }

  return value
}

/**
 * Comment patterns per language — mirrors rtk's `CommentPatterns`
 * (src/core/filter.rs) and `Language::from_extension`.
 */
interface CommentPatterns {
  line: string | null
  docLine: string | null
  blockStart: string | null
  blockEnd: string | null
  docBlockStart: string | null
}

function detectCommentStyle(filePath: string): CommentPatterns | null {
  const ext = filePath.slice(filePath.lastIndexOf(".") + 1).toLowerCase()
  const cStyle: CommentPatterns = {
    line: "//", docLine: "///", blockStart: "/*", blockEnd: "*/", docBlockStart: "/**",
  }
  const hash: CommentPatterns = {
    line: "#", docLine: null, blockStart: null, blockEnd: null, docBlockStart: "###",
  }
  switch (ext) {
    case "ts": case "tsx": case "js": case "jsx": case "mjs": case "cjs":
    case "go": case "rs": case "c": case "h": case "cpp": case "hpp": case "cc":
    case "java": case "kt": case "swift": case "scala": case "dart":
    case "jsonc": case "css": case "scss": case "less": case "php":
      return cStyle
    case "py": case "pyi": case "rb": case "sh": case "bash": case "zsh":
    case "yaml": case "yml": case "toml": case "dockerfile": case "makefile":
    case "gemspec": case "rake":
      return hash
    default:
      return null
  }
}

/**
 * rtk `--level minimal` equivalent, operating on opencode's `N: <content>`
 * output format. Strips line/block comments (keeps doc comments) and
 * normalizes 3+ blank lines → 2, while PRESERVING the original `N:` prefix
 * so file:line references the model emits stay accurate. If the comment-
 * stripped result still exceeds maxChars, falls back to truncateMiddle on
 * the denser (comment-free) text.
 */
function rtkMinimal(text: string, maxChars: number, filePath?: string): string {
  const patterns = filePath ? detectCommentStyle(filePath) : null
  if (!patterns) {
    return truncateMiddle(text, maxChars)
  }

  const lines = text.split("\n")
  const kept: string[] = []
  let inBlock = false
  let blankRun = 0

  for (const raw of lines) {
    const m = /^(\d+):\s?(.*)$/.exec(raw)
    if (!m) { kept.push(raw); continue }
    const prefix = `${m[1]}: `
    const content = m[2]
    const trimmed = content.trim()

    // Block comments (C-style): drop the whole span, keep /** doc */ via docBlockStart guard
    if (patterns.blockStart && patterns.blockEnd) {
      if (!inBlock && trimmed.includes(patterns.blockStart)
          && !trimmed.startsWith(patterns.docBlockStart ?? "###")) {
        inBlock = true
      }
      if (inBlock) {
        if (trimmed.includes(patterns.blockEnd)) inBlock = false
        continue
      }
    }

    // Single-line comments: drop, but keep doc comments (/// , //! , etc.)
    if (patterns.line && trimmed.startsWith(patterns.line)) {
      if (patterns.docLine && trimmed.startsWith(patterns.docLine)) {
        kept.push(raw)
      }
      continue
    }

    // Blank-line normalization: 3+ consecutive blanks → 2 (rtk behavior)
    if (trimmed === "") {
      blankRun++
      if (blankRun <= 2) kept.push(raw)
      continue
    }
    blankRun = 0
    kept.push(raw)
  }

  let result = kept.join("\n")
  if (result.length <= maxChars) return result
  return truncateMiddle(result, maxChars)
}

/**
 * Apply the appropriate compression strategy to a tool output.
 */
function compressOutput(text: string, strategy: Strategy, filePath?: string): string {
  switch (strategy) {
    case "truncate-middle":
      return truncateMiddle(text, EFFECTIVE_MAX_CHARS)
    case "truncate-tail":
      return truncateTail(text, EFFECTIVE_MAX_CHARS)
    case "json-compact":
      return jsonCompact(text, EFFECTIVE_MAX_CHARS)
    case "rtk-minimal":
      return rtkMinimal(text, EFFECTIVE_MAX_CHARS, filePath)
  }
}

// ─── Plugin Export ────────────────────────────────────────────────────────────

const RtkOpenCodePlugin: Plugin = async ({ $ }) => {
  try {
    await $`which rtk`.quiet()
  } catch {
    console.warn("[rtk] rtk binary not found in PATH — plugin disabled")
    return {}
  }

  return {
    // ─── Command Rewriting (bash/shell only) ──────────────────────
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

    // ─── Output Compression (built-in + MCP + unlisted) ──────────
    "tool.execute.after": async (input, output) => {
      const rawTool = String(input?.tool ?? "")
      const tool = rawTool.toLowerCase()

      // Pick strategy: explicit per-tool, or DEFAULT_STRATEGY for unlisted tools
      const strategy = HEAVY_TOOLS[tool] ?? DEFAULT_STRATEGY

      // Extract text from the field this tool actually populates, and remember
      // the "sink" so we mutate the same field.
      //   - Built-in tools (read/grep/glob/task/webfetch): `output.output` (string)
      //   - MCP tools (codegraph_*/context7_*/engram_*): the hook receives the
      //     raw SDK `result` (since commit 458ec7b37) whose `content` is an array
      //     of {type:"text"|"image"|"resource", ...}. opencode builds `.output`
      //     AFTER this hook from text parts + attachments (session/tools.ts:471),
      //     so swapping `content` is what reaches the LLM. Skip when content
      //     holds non-text parts (images, resource blobs) — those become
      //     attachments and we must not drop them.
      let text = ""
      let sink: "output" | "content" | null = null
      if (typeof (output as any)?.output === "string") {
        text = (output as any).output
        sink = "output"
      } else if (Array.isArray((output as any)?.content)) {
        const parts = (output as any).content as Array<{ type: string; text?: string }>
        if (parts.every((c) => c.type === "text")) {
          text = parts.map((c) => c.text || "").join("\n")
          sink = "content"
        } else if (RTK_DEBUG) {
          const types = parts.map((p) => p.type).join(",")
          logDebug(`[${new Date().toISOString()}] ${rawTool} — skip: mixed content [${types}]`)
        }
      }

      logDebug(`[${new Date().toISOString()}] ${rawTool} — ${text.length} chars (~${approxTokens(text)} tokens), sink=${sink ?? "none"}, threshold=${CHAR_THRESHOLD} chars (~${TOKEN_THRESHOLD} tokens)`)

      if (!text || text.length < CHAR_THRESHOLD) return

      const originalTokens = approxTokens(text)
      const args = (input as any)?.args ?? {}
      const filePath = typeof args?.filePath === "string" ? args.filePath : undefined
      const compressed = compressOutput(text, strategy, filePath)
      const compressedTokens = approxTokens(compressed)
      const saved = originalTokens - compressedTokens
      const pct = Math.round((saved / originalTokens) * 100)

      // Only apply if we actually saved meaningful tokens (>10%)
      if (pct > 10) {
        const finalCompressed =
          `[rtk: compressed ${rawTool} output — ${originalTokens}→${compressedTokens} tokens (${pct}% saved)]\n\n` +
          compressed

        if (sink === "output") {
          (output as any).output = finalCompressed
        } else if (sink === "content") {
          (output as any).content = [{ type: "text", text: finalCompressed }]
        }

        logDebug(`  → COMPRESSED ${rawTool}: ${originalTokens}→${compressedTokens} tokens (${pct}% saved), sink=${sink}`)
      }
    },
  }
}

export { RtkOpenCodePlugin }
export default RtkOpenCodePlugin
