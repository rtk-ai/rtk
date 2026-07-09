import type { Plugin } from "@opencode-ai/plugin"

// RTK OpenCode plugin — two independent responsibilities:
//   1. Rewrite bash/shell commands to their `rtk` equivalents (tool.execute.before)
//   2. Compress heavy tool outputs before they reach the LLM (tool.execute.after)
//
// Requires: rtk >= 0.42.0 in PATH (matches workspace Cargo.toml).
//
// Command-rewrite logic lives in `rtk rewrite` (src/discover/registry.rs), the
// single source of truth. Output compression is handled in-plugin below.

// ─── Tunables ─────────────────────────────────────────────────────────────────

/** Chars-per-token heuristic (safe average for English + code). */
const CHARS_PER_TOKEN = 4

/** Outputs below this many tokens are left untouched. */
const TOKEN_THRESHOLD = parseInt(process.env.RTK_TOKEN_THRESHOLD ?? "3000")

/** Hard ceiling on post-compression size, before the threshold cap below. */
const MAX_OUTPUT_CHARS = parseInt(process.env.RTK_MAX_OUTPUT_CHARS ?? "32000")

/** Compression is only applied when it saves strictly more than this. */
const MIN_SAVINGS_PCT = 10

/** truncate-middle keeps this fraction from the head, the rest from the tail. */
const HEAD_RATIO = 0.6

const CHAR_THRESHOLD = TOKEN_THRESHOLD * CHARS_PER_TOKEN

// The gate only compresses when text.length >= CHAR_THRESHOLD, but the
// strategies only trim when text.length > maxChars. If maxChars > CHAR_THRESHOLD
// there is a dead zone (12000–32000 with defaults) where an output passes the
// gate yet compresses to nothing. Cap the target at the threshold so anything
// that enters actually shrinks.
const EFFECTIVE_MAX_CHARS = Math.min(MAX_OUTPUT_CHARS, CHAR_THRESHOLD)

// json-compact tuning: prune deep nesting and long arrays/strings.
const JSON_MAX_DEPTH = 4
const JSON_ARRAY_LIMIT = 20
const JSON_ARRAY_HEAD = 15
const JSON_ARRAY_TAIL = 5
const JSON_STRING_LIMIT = 500
const JSON_STRING_HEAD = 400
const JSON_OBJECT_KEY_PREVIEW = 5

// ─── Debug logging (opt-in) ───────────────────────────────────────────────────

// Set RTK_DEBUG=1 to trace every tool call (size, sink, threshold, result) to
// /tmp/rtk-plugin.log. Off by default — no I/O on the hot path.
const RTK_DEBUG = process.env.RTK_DEBUG === "1"

function logDebug(message: string): void {
  if (!RTK_DEBUG) return
  try {
    require("fs").appendFileSync("/tmp/rtk-plugin.log", `[${new Date().toISOString()}] ${message}\n`)
  } catch {}
}

// ─── Tool → strategy registry ──────────────────────────────────────────────────

type Strategy = "truncate-middle" | "truncate-tail" | "json-compact" | "rtk-minimal"

// Per-tool overrides for OpenCode's built-in tools, mapped to how we shrink
// their output:
//   truncate-middle — keep head + tail, drop the middle (structured dumps)
//   truncate-tail   — keep head, drop tail (docs/logs: first lines matter most)
//   json-compact    — prune deep nesting + long arrays/strings (JSON payloads)
//   rtk-minimal     — strip comments, keep `N:` line prefixes (source files)
//
// Only the built-ins get a hand-picked strategy — they ship with every
// OpenCode install, so tuning them benefits everyone. Any other tool (MCP
// servers, third-party plugins) isn't listed and falls back to
// DEFAULT_STRATEGY, which handles arbitrary output safely.
const HEAVY_TOOLS: Record<string, Strategy> = {
  read: "rtk-minimal", // #1 token consumer; strip comments, keep the file
  grep: "truncate-tail", // first matches are the relevant ones
  glob: "truncate-tail", // first matches + structure matter most
  task: "truncate-middle", // sub-agent results can be whole conversations
  webfetch: "truncate-tail", // raw HTML/markdown page dumps
}

// Fallback for every tool not in HEAVY_TOOLS (MCP servers, plugins, unknown
// tools). truncate-middle keeps both ends, which is the safest default when we
// don't know the output's shape.
const DEFAULT_STRATEGY: Strategy = "truncate-middle"

// ─── Token accounting + truncation markers ──────────────────────────────────────

function tokensFromChars(chars: number): number {
  return Math.ceil(chars / CHARS_PER_TOKEN)
}

function approxTokens(text: string): number {
  return tokensFromChars(text.length)
}

function truncationMarker(removedChars: number, where: "middle" | "end"): string {
  return `[… ${tokensFromChars(removedChars)} tokens truncated by rtk — ${removedChars} chars removed from ${where} …]`
}

// ─── Compression strategies ─────────────────────────────────────────────────────

// Keep HEAD_RATIO from the head and the remainder from the tail; the head
// usually carries the most structure (file headers, symbol definitions).
function truncateMiddle(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text
  const headSize = Math.floor(maxChars * HEAD_RATIO)
  const tailSize = maxChars - headSize
  const removed = text.length - maxChars
  return `${text.slice(0, headSize)}\n\n${truncationMarker(removed, "middle")}\n\n${text.slice(-tailSize)}`
}

// Keep the head, drop the tail — best for docs/logs where the start matters most.
function truncateTail(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text
  const removed = text.length - maxChars
  return `${text.slice(0, maxChars)}\n\n${truncationMarker(removed, "end")}`
}

// Parse, prune, and re-serialize JSON compactly; fall back to tail-truncation
// when the input isn't valid JSON or is still too large after pruning.
function jsonCompact(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text
  try {
    const compacted = compactValue(JSON.parse(text), JSON_MAX_DEPTH)
    const result = JSON.stringify(compacted, null, 1)
    return result.length <= maxChars ? result : truncateTail(result, maxChars)
  } catch {
    return truncateTail(text, maxChars)
  }
}

// Recursively shrink a parsed JSON value:
//   long strings          → truncated with a marker
//   arrays over the limit  → first JSON_ARRAY_HEAD + last JSON_ARRAY_TAIL + marker
//   values past maxDepth   → collapsed to a one-line summary
function compactValue(value: unknown, maxDepth: number, depth = 0): unknown {
  if (typeof value === "string") {
    return value.length > JSON_STRING_LIMIT
      ? `${value.slice(0, JSON_STRING_HEAD)}… [${value.length - JSON_STRING_HEAD} chars truncated]`
      : value
  }

  if (Array.isArray(value)) {
    if (depth >= maxDepth) return `[Array(${value.length})]`
    const shrink = (v: unknown) => compactValue(v, maxDepth, depth + 1)
    if (value.length > JSON_ARRAY_LIMIT) {
      const head = value.slice(0, JSON_ARRAY_HEAD).map(shrink)
      const tail = value.slice(-JSON_ARRAY_TAIL).map(shrink)
      return [...head, `… ${value.length - JSON_ARRAY_LIMIT} items omitted …`, ...tail]
    }
    return value.map(shrink)
  }

  if (value !== null && typeof value === "object") {
    const keys = Object.keys(value as Record<string, unknown>)
    if (depth >= maxDepth) {
      const preview = keys.slice(0, JSON_OBJECT_KEY_PREVIEW).join(", ")
      return `{${keys.length} keys: ${preview}${keys.length > JSON_OBJECT_KEY_PREVIEW ? "…" : ""}}`
    }
    const result: Record<string, unknown> = {}
    for (const [k, v] of Object.entries(value as Record<string, unknown>)) {
      result[k] = compactValue(v, maxDepth, depth + 1)
    }
    return result
  }

  // null, number, boolean, undefined, etc. — passed through unchanged.
  return value
}

// ─── rtk-minimal (source-file compaction) ────────────────────────────────────────

interface CommentPatterns {
  line: string | null
  docLine: string | null
  blockStart: string | null
  blockEnd: string | null
  docBlockStart: string | null
}

// Mirrors rtk's CommentPatterns (src/core/filter.rs) + Language::from_extension.
const C_STYLE: CommentPatterns = {
  line: "//",
  docLine: "///",
  blockStart: "/*",
  blockEnd: "*/",
  docBlockStart: "/**",
}
const HASH_STYLE: CommentPatterns = {
  line: "#",
  docLine: null,
  blockStart: null,
  blockEnd: null,
  docBlockStart: "###",
}

const COMMENT_STYLE_BY_EXT: Record<string, CommentPatterns> = {}
for (const ext of "ts tsx js jsx mjs cjs go rs c h cpp hpp cc java kt swift scala dart jsonc css scss less php".split(" ")) {
  COMMENT_STYLE_BY_EXT[ext] = C_STYLE
}
for (const ext of "py pyi rb sh bash zsh yaml yml toml dockerfile makefile gemspec rake".split(" ")) {
  COMMENT_STYLE_BY_EXT[ext] = HASH_STYLE
}

function detectCommentStyle(filePath: string): CommentPatterns | null {
  const ext = filePath.slice(filePath.lastIndexOf(".") + 1).toLowerCase()
  return COMMENT_STYLE_BY_EXT[ext] ?? null
}

// rtk `--level minimal`, adapted to opencode's `N: <content>` line format.
// Strips line/block comments (keeping doc comments) and collapses 3+ blank
// lines to 2, while preserving the `N:` prefix so file:line references the
// model emits stay accurate. If the stripped result still exceeds maxChars,
// falls back to truncateMiddle on the (now denser) text.
function rtkMinimal(text: string, maxChars: number, filePath?: string): string {
  const patterns = filePath ? detectCommentStyle(filePath) : null
  if (!patterns) return truncateMiddle(text, maxChars)

  const kept: string[] = []
  let inBlock = false
  let blankRun = 0

  for (const raw of text.split("\n")) {
    const match = /^(\d+):\s?(.*)$/.exec(raw)
    if (!match) {
      kept.push(raw)
      continue
    }
    const trimmed = match[2].trim()

    // Block comments (C-style): drop the whole span, but keep /** doc */ blocks.
    if (patterns.blockStart && patterns.blockEnd) {
      if (
        !inBlock &&
        trimmed.includes(patterns.blockStart) &&
        !trimmed.startsWith(patterns.docBlockStart ?? "###")
      ) {
        inBlock = true
      }
      if (inBlock) {
        if (trimmed.includes(patterns.blockEnd)) inBlock = false
        continue
      }
    }

    // Line comments: drop, but keep doc comments (///, //!, …).
    if (patterns.line && trimmed.startsWith(patterns.line)) {
      if (patterns.docLine && trimmed.startsWith(patterns.docLine)) kept.push(raw)
      continue
    }

    // Collapse 3+ consecutive blank lines to 2 (rtk behavior).
    if (trimmed === "") {
      blankRun++
      if (blankRun <= 2) kept.push(raw)
      continue
    }

    blankRun = 0
    kept.push(raw)
  }

  const result = kept.join("\n")
  return result.length <= maxChars ? result : truncateMiddle(result, maxChars)
}

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

// ─── Tool-result plumbing ─────────────────────────────────────────────────────

type Sink = "output" | "content"
interface TextPart {
  type: string
  text?: string
}

// opencode delivers built-in results as { output: string } and MCP results as
// { content: TextPart[] } (raw SDK shape since opencode commit 458ec7b37).
// Returns the joined text and which field to write back, or sink=null when
// there is nothing safe to rewrite — empty output, or mixed image/resource
// content that becomes attachments downstream and must not be dropped.
function extractText(output: any): { text: string; sink: Sink | null } {
  if (typeof output?.output === "string") {
    return { text: output.output, sink: "output" }
  }
  if (Array.isArray(output?.content)) {
    const parts = output.content as TextPart[]
    if (parts.every((p) => p.type === "text")) {
      return { text: parts.map((p) => p.text ?? "").join("\n"), sink: "content" }
    }
    logDebug(`skip: mixed content [${parts.map((p) => p.type).join(",")}]`)
  }
  return { text: "", sink: null }
}

function writeSink(output: any, sink: Sink, value: string): void {
  if (sink === "output") output.output = value
  else output.content = [{ type: "text", text: value }]
}

// ─── Plugin ────────────────────────────────────────────────────────────────────

const RtkOpenCodePlugin: Plugin = async ({ $ }) => {
  try {
    await $`which rtk`.quiet()
  } catch {
    console.warn("[rtk] rtk binary not found in PATH — plugin disabled")
    return {}
  }

  return {
    // Rewrite bash/shell commands to their rtk equivalents before they run.
    "tool.execute.before": async (input, output) => {
      const tool = String(input?.tool ?? "").toLowerCase()
      if (tool !== "bash" && tool !== "shell") return

      const args = output?.args
      if (!args || typeof args !== "object") return
      const command = args.command
      if (typeof command !== "string" || !command) return

      try {
        const result = await $`rtk rewrite ${command}`.quiet().nothrow()
        const rewritten = String(result.stdout).trim()
        if (rewritten && rewritten !== command) args.command = rewritten
      } catch {
        // rtk rewrite failed — leave the command unchanged (never block).
      }
    },

    // Compress heavy tool outputs (built-in, MCP, and unlisted) after they run.
    "tool.execute.after": async (input, output) => {
      const tool = String(input?.tool ?? "")
      const strategy = HEAVY_TOOLS[tool.toLowerCase()] ?? DEFAULT_STRATEGY

      const { text, sink } = extractText(output)
      logDebug(
        `${tool} — ${text.length} chars (~${approxTokens(text)} tokens), ` +
          `sink=${sink ?? "none"}, threshold=${CHAR_THRESHOLD} chars (~${TOKEN_THRESHOLD} tokens)`,
      )
      if (!sink || text.length < CHAR_THRESHOLD) return

      const originalTokens = approxTokens(text)
      const filePath = typeof input?.args?.filePath === "string" ? input.args.filePath : undefined
      const compressed = compressOutput(text, strategy, filePath)
      const compressedTokens = approxTokens(compressed)
      const savedPct = Math.round(((originalTokens - compressedTokens) / originalTokens) * 100)
      if (savedPct <= MIN_SAVINGS_PCT) return

      writeSink(
        output,
        sink,
        `[rtk: compressed ${tool} output — ${originalTokens}→${compressedTokens} tokens (${savedPct}% saved)]\n\n${compressed}`,
      )
      logDebug(`  → COMPRESSED ${tool}: ${originalTokens}→${compressedTokens} tokens (${savedPct}% saved), sink=${sink}`)
    },
  }
}

export { RtkOpenCodePlugin }
export default RtkOpenCodePlugin
