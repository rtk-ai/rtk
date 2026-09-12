import { tool, type Plugin } from "@opencode-ai/plugin"
import { createHash, randomUUID } from "node:crypto"
import { appendFileSync } from "node:fs"
import { chmod, lstat, mkdir, open, readFile, readdir, rename, rm, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { basename, join } from "node:path"
import { stripVTControlCharacters } from "node:util"

// RTK OpenCode plugin — two independent responsibilities:
//   1. Rewrite bash/shell commands to their `rtk` equivalents (tool.execute.before)
//   2. Compress heavy tool outputs before they reach the LLM (tool.execute.after)
//
// Command-rewrite logic lives in `rtk rewrite` (src/discover/registry.rs), the
// single source of truth and requires rtk >= 0.42.0. Output compression is
// self-contained and remains active when the binary is unavailable.

// ─── Tunables ─────────────────────────────────────────────────────────────────

/** Chars-per-token heuristic used by OpenCode for approximate accounting. */
const CHARS_PER_TOKEN = 4

function positiveInteger(name: string, fallback: number, minimum: number): number {
  const value = Number(process.env[name])
  return Number.isSafeInteger(value) && value >= minimum ? value : fallback
}

/** Outputs below this approximate token count are left untouched. */
const LEGACY_TRIGGER_TOKENS = positiveInteger("RTK_TOKEN_THRESHOLD", 3000, 256)
const TRIGGER_TOKENS = positiveInteger("RTK_TRIGGER_TOKENS", LEGACY_TRIGGER_TOKENS, 256)

/** Approximate token budget for the complete compressed result. */
const TARGET_TOKENS = positiveInteger("RTK_TARGET_TOKENS", 2500, 128)

/** Hard character ceiling for the complete compressed result. */
const MAX_OUTPUT_CHARS = positiveInteger("RTK_MAX_OUTPUT_CHARS", 32000, 512)

/** Compression is only applied when it saves strictly more than this. */
const MIN_SAVINGS_PCT = 10

/** truncate-middle keeps this fraction from the head, the rest from the tail. */
const HEAD_RATIO = 0.6

const CHAR_THRESHOLD = TRIGGER_TOKENS * CHARS_PER_TOKEN
const TARGET_OUTPUT_CHARS = Math.max(
  256,
  Math.min(MAX_OUTPUT_CHARS, TARGET_TOKENS * CHARS_PER_TOKEN, CHAR_THRESHOLD - 1),
)

// Full outputs are cached only when doing so is bounded and recoverable. Larger
// raw MCP results are left untouched so OpenCode can persist them itself.
const CACHE_ENTRY_MAX_BYTES = 1024 * 1024
const CACHE_TOTAL_MAX_BYTES = 16 * 1024 * 1024
const CACHE_TTL_MS = 24 * 60 * 60 * 1000
const CACHE_TEMP_TTL_MS = 5 * 60 * 1000
const CACHE_LOCK_STALE_MS = 30 * 1000
const CACHE_DIR_OVERRIDE = process.env.RTK_CACHE_DIR?.trim()
const CACHE_DIR = CACHE_DIR_OVERRIDE || join(tmpdir(), "rtk-opencode-cache")
const CACHE_ID_PATTERN = /^[a-f0-9]{24}$/
const RETRIEVAL_DEFAULT_CHARS = 6000
const RETRIEVAL_MAX_CHARS = 8000

// ─── Debug logging (opt-in) ───────────────────────────────────────────────────

// Set RTK_DEBUG=1 to trace every tool call (size, sink, threshold, result) to
// the platform temp directory. Off by default, so there is no I/O on the hot path.
const RTK_DEBUG = process.env.RTK_DEBUG === "1"
const DEBUG_LOG_PATH = join(tmpdir(), "rtk-plugin.log")

function logDebug(message: string): void {
  if (!RTK_DEBUG) return
  try {
    appendFileSync(DEBUG_LOG_PATH, `[${new Date().toISOString()}] ${message}\n`)
  } catch {}
}

// ─── Tool → strategy registry ──────────────────────────────────────────────────

type Strategy = "truncate-middle" | "truncate-tail" | "rtk-minimal"

// Per-tool overrides for OpenCode's built-in tools, mapped to how we shrink
// their output:
//   truncate-middle — keep head + tail, drop the middle (structured dumps)
//   truncate-tail   — keep head, drop tail (docs/logs: first lines matter most)
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
  return `[… ~${tokensFromChars(removedChars)} tokens truncated by rtk — ${removedChars} chars removed from ${where} …]`
}

// ─── Compression strategies ─────────────────────────────────────────────────────

function head(text: string, end: number): string {
  if (end > 0 && end < text.length && /[\uD800-\uDBFF]/.test(text[end - 1]) && /[\uDC00-\uDFFF]/.test(text[end])) {
    end--
  }
  return text.slice(0, end)
}

function tail(text: string, start: number): string {
  if (
    start > 0 &&
    start < text.length &&
    /[\uDC00-\uDFFF]/.test(text[start]) &&
    /[\uD800-\uDBFF]/.test(text[start - 1])
  ) {
    start++
  }
  return text.slice(start)
}

function safeSlice(text: string, start: number, end: number): string {
  if (
    start > 0 &&
    start < text.length &&
    /[\uDC00-\uDFFF]/.test(text[start]) &&
    /[\uD800-\uDBFF]/.test(text[start - 1])
  ) {
    start++
  }
  if (end > 0 && end < text.length && /[\uD800-\uDBFF]/.test(text[end - 1]) && /[\uDC00-\uDFFF]/.test(text[end])) {
    end--
  }
  return text.slice(start, end)
}

function contentBudget(textLength: number, maxChars: number, where: "middle" | "end"): number {
  let kept = Math.max(0, maxChars - 96)
  const separators = where === "middle" ? 4 : 2
  for (let attempt = 0; attempt < 3; attempt++) {
    kept = Math.max(0, maxChars - truncationMarker(textLength - kept, where).length - separators)
  }
  return kept
}

// Keep HEAD_RATIO from the head and the remainder from the tail; the head
// usually carries the most structure (file headers, symbol definitions).
function truncateMiddle(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text
  const kept = contentBudget(text.length, maxChars, "middle")
  const headSize = Math.floor(kept * HEAD_RATIO)
  const tailSize = kept - headSize
  const leading = head(text, headSize)
  const trailing = tail(text, text.length - tailSize)
  const removed = text.length - leading.length - trailing.length
  return `${leading}\n\n${truncationMarker(removed, "middle")}\n\n${trailing}`
}

// Keep the head, drop the tail — best for docs/logs where the start matters most.
function truncateTail(text: string, maxChars: number): string {
  if (text.length <= maxChars) return text
  const kept = contentBudget(text.length, maxChars, "end")
  const leading = head(text, kept)
  const removed = text.length - leading.length
  return `${leading}\n\n${truncationMarker(removed, "end")}`
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
for (const ext of "ts tsx js jsx mjs cjs go rs c h cpp hpp cc java kt swift scala dart jsonc css scss less php".split(
  " ",
)) {
  COMMENT_STYLE_BY_EXT[ext] = C_STYLE
}
for (const ext of "py pyi rb sh bash zsh yaml yml toml dockerfile makefile gemspec rake".split(" ")) {
  COMMENT_STYLE_BY_EXT[ext] = HASH_STYLE
}

function detectCommentStyle(filePath: string): CommentPatterns | null {
  const name = basename(filePath).toLowerCase()
  const dot = name.lastIndexOf(".")
  const ext = dot > 0 ? name.slice(dot + 1) : name
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
      if (!inBlock && trimmed.startsWith(patterns.blockStart) && !trimmed.startsWith(patterns.docBlockStart ?? "###")) {
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

function compressOutput(text: string, strategy: Strategy, maxChars: number, filePath?: string): string {
  switch (strategy) {
    case "truncate-middle":
      return truncateMiddle(text, maxChars)
    case "truncate-tail":
      return truncateTail(text, maxChars)
    case "rtk-minimal":
      return rtkMinimal(text, maxChars, filePath)
  }
}

// ─── Tool-result plumbing ─────────────────────────────────────────────────────

type Sink = "output" | "content"
interface TextPart {
  type: string
  text?: string
  [key: string]: unknown
}

interface TextSource {
  sink: Sink
  length: number
  separatorChars: number
  segments: string[]
  write(values: string[]): void
}

function isTextPart(value: unknown): value is TextPart {
  return typeof value === "object" && value !== null && "type" in value && value.type === "text"
}

function modelText(value: unknown): string | undefined {
  if (isTextPart(value)) return value.text ?? ""
  if (typeof value !== "object" || value === null || !("type" in value) || value.type !== "resource") return undefined
  if (!("resource" in value) || typeof value.resource !== "object" || value.resource === null) return undefined
  return "text" in value.resource && typeof value.resource.text === "string" ? value.resource.text : undefined
}

function replaceModelText(value: unknown, text: string): unknown {
  if (isTextPart(value)) return { ...value, text }
  const part = value as { resource: Record<string, unknown> }
  return { ...(value as Record<string, unknown>), resource: { ...part.resource, text } }
}

// OpenCode delivers built-in results as { output: string } and raw MCP results
// as { content: ContentPart[] }. For MCP, defer joining until after size checks
// and preserve every non-text part when replacing the text payload.
function extractText(output: any): TextSource | null {
  if (typeof output?.output === "string") {
    return {
      sink: "output",
      length: output.output.length,
      separatorChars: 0,
      segments: [output.output],
      write: (values) => {
        output.output = values[0] ?? ""
      },
    }
  }
  if (Array.isArray(output?.content)) {
    const parts = output.content as unknown[]
    const slots = parts.flatMap((part, index) => {
      const text = modelText(part)
      return text === undefined ? [] : [{ index, text }]
    })
    if (slots.length === 0) return null
    const separatorChars = 2 * (slots.length - 1)
    const length = slots.reduce((total, slot) => total + slot.text.length, separatorChars)
    return {
      sink: "content",
      length,
      separatorChars,
      segments: slots.map((slot) => slot.text),
      write: (values) => {
        const replacements = new Map<number, string>()
        for (const [position, slot] of slots.entries()) {
          replacements.set(slot.index, values[position] ?? "")
        }
        output.content = parts.map((part, index) =>
          replacements.has(index) ? replaceModelText(part, replacements.get(index) ?? "") : part,
        )
      },
    }
  }
  return null
}

type Recovery = { id: string } | { outputPath: string } | { unavailable: string }

function cacheID(sessionID: string, callID: string, text: string): string {
  return createHash("sha256")
    .update(sessionID)
    .update("\0")
    .update(callID)
    .update("\0")
    .update(text)
    .digest("hex")
    .slice(0, 24)
}

function cachePath(id: string): string {
  return join(CACHE_DIR, `${id}.txt`)
}

async function ensureCacheDirectory(): Promise<void> {
  await mkdir(CACHE_DIR, { mode: 0o700, recursive: true })
  const directory = await lstat(CACHE_DIR)
  if (!directory.isDirectory() || directory.isSymbolicLink()) throw new Error("RTK cache path is not a real directory")
  if (!CACHE_DIR_OVERRIDE && process.getuid && (directory.uid !== process.getuid() || (directory.mode & 0o077) !== 0)) {
    throw new Error("RTK cache directory is not private to the current user")
  }
}

async function lockIsActive(path: string): Promise<boolean> {
  try {
    const owner = JSON.parse(await readFile(path, "utf8")) as { pid?: unknown }
    if (typeof owner.pid === "number" && Number.isSafeInteger(owner.pid)) {
      try {
        process.kill(owner.pid, 0)
        return true
      } catch (error) {
        if ((error as NodeJS.ErrnoException).code === "EPERM") return true
        if ((error as NodeJS.ErrnoException).code === "ESRCH") return false
      }
    }
  } catch {}
  const info = await lstat(path).catch(() => undefined)
  return Boolean(info && Date.now() - info.mtimeMs <= CACHE_LOCK_STALE_MS)
}

async function withCacheLock<T>(operation: () => Promise<T>): Promise<T> {
  await ensureCacheDirectory()
  const path = join(CACHE_DIR, ".lock")
  const token = randomUUID()
  for (let attempt = 0; attempt < 20; attempt++) {
    let handle
    try {
      handle = await open(path, "wx", 0o600)
      await handle.writeFile(JSON.stringify({ pid: process.pid, token }))
    } catch (error) {
      if (handle) {
        await handle.close().catch(() => undefined)
        await rm(path, { force: true }).catch(() => undefined)
      }
      if ((error as NodeJS.ErrnoException).code !== "EEXIST") throw error
      if (!(await lockIsActive(path))) {
        await rm(path, { force: true })
        continue
      }
      await new Promise((resolve) => setTimeout(resolve, 10))
      continue
    }
    try {
      return await operation()
    } finally {
      await handle.close().catch(() => undefined)
      const owner = await readFile(path, "utf8")
        .then((value) => JSON.parse(value) as { token?: unknown })
        .catch(() => undefined)
      if (owner?.token === token) await rm(path, { force: true }).catch(() => undefined)
    }
  }
  throw new Error("Timed out waiting for the RTK cache lock")
}

async function cleanupCache(): Promise<number> {
  const now = Date.now()
  let total = 0
  await ensureCacheDirectory()
  for (const name of await readdir(CACHE_DIR)) {
    const temporary = /^\.[a-f0-9]{24}-\d+-\d+\.tmp$/.test(name)
    if (!temporary && !/^[a-f0-9]{24}\.txt$/.test(name)) continue
    const path = join(CACHE_DIR, name)
    let info
    try {
      info = await lstat(path)
    } catch {
      continue
    }
    if (!info.isFile()) continue
    if (now - info.mtimeMs > (temporary ? CACHE_TEMP_TTL_MS : CACHE_TTL_MS)) {
      await rm(path, { force: true })
      continue
    }
    total += info.size
  }
  return total
}

let cacheWrites: Promise<void> = Promise.resolve()

type CacheWriteResult = "created" | "existing" | false

function storeCachedOutput(id: string, text: string): Promise<CacheWriteResult> {
  const size = Buffer.byteLength(text, "utf8")
  if (size > CACHE_ENTRY_MAX_BYTES) return Promise.resolve(false)

  const operation = cacheWrites.then(async () => {
    const path = cachePath(id)
    const temporary = join(CACHE_DIR, `.${id}-${process.pid}-${Date.now()}.tmp`)
    try {
      return await withCacheLock(async () => {
        const total = await cleanupCache()
        const existingInfo = await lstat(path).catch(() => undefined)
        if (existingInfo?.isFile() && !existingInfo.isSymbolicLink()) {
          return (await readFile(path, "utf8").catch(() => undefined)) === text ? ("existing" as const) : false
        }
        if (total + size > CACHE_TOTAL_MAX_BYTES) return false
        await writeFile(temporary, text, { encoding: "utf8", flag: "wx", mode: 0o600 })
        await rename(temporary, path)
        await chmod(path, 0o600).catch(() => undefined)
        return "created" as const
      })
    } catch (error) {
      await rm(temporary, { force: true }).catch(() => undefined)
      logDebug(`cache write failed: ${error instanceof Error ? error.message : String(error)}`)
      return false
    }
  })
  cacheWrites = operation.then(
    () => undefined,
    () => undefined,
  )
  return operation
}

async function readCachedOutput(id: string): Promise<string> {
  if (!CACHE_ID_PATTERN.test(id)) throw new Error("Invalid RTK cache id")
  const path = cachePath(id)
  const info = await lstat(path)
  if (!info.isFile() || info.isSymbolicLink()) throw new Error("RTK cache entry is not a regular file")
  if (Date.now() - info.mtimeMs > CACHE_TTL_MS) {
    await rm(path, { force: true })
    throw new Error("RTK cached output has expired")
  }
  return readFile(path, "utf8")
}

function singleLine(value: string, maxLength = 240): string {
  return value
    .replace(/[\r\n\t]+/g, " ")
    .replace(/\s+/g, " ")
    .trim()
    .slice(0, maxLength)
}

function recoveryFooter(recovery: Recovery): string {
  if ("outputPath" in recovery) {
    return `[rtk recovery: full output saved by OpenCode at ${JSON.stringify(recovery.outputPath)}]`
  }
  if ("id" in recovery) return `[rtk recovery: use rtk_retrieve with id="${recovery.id}"]`
  return `[rtk recovery unavailable: ${recovery.unavailable}]`
}

function assemblePayload(
  toolName: string,
  originalChars: number,
  bodies: string[],
  recovery: Recovery,
  extraChars: number,
) {
  const originalTokens = tokensFromChars(originalChars)
  let finalTokens = 0
  let savedPct = 0
  let values = bodies
  for (let attempt = 0; attempt < 4; attempt++) {
    const header = `[rtk: compressed ${toolName} output | ~${originalTokens}->~${finalTokens} tokens (${savedPct}% saved)]`
    values = [...bodies]
    values[0] = `${header}\n\n${values[0] ?? ""}`
    const last = values.length - 1
    values[last] = `${values[last] ?? ""}\n\n${recoveryFooter(recovery)}`
    const finalChars = values.reduce((total, value) => total + value.length, extraChars)
    finalTokens = tokensFromChars(finalChars)
    savedPct = Math.round((1 - finalChars / originalChars) * 100)
  }
  return {
    finalChars: values.reduce((total, value) => total + value.length, extraChars),
    finalTokens,
    savedPct,
    values,
  }
}

function segmentBudgets(segments: string[], total: number): number[] {
  if (segments.length === 0) return []
  const base = Math.min(256, Math.floor(total / segments.length))
  const budgets = segments.map(() => base)
  const distributable = Math.max(0, total - base * segments.length)
  let allocated = 0
  const weight = Math.max(
    1,
    segments.reduce((sum, segment) => sum + segment.length, 0),
  )
  for (let index = 0; index < segments.length; index++) {
    const share =
      index === segments.length - 1
        ? distributable - allocated
        : Math.floor((distributable * segments[index].length) / weight)
    budgets[index] += share
    allocated += share
  }
  return budgets
}

function buildPayload(input: {
  filePath?: string
  originalChars: number
  recovery: Recovery
  separatorChars: number
  segments: string[]
  strategy: Strategy
  toolName: string
}) {
  let bodyBudget = Math.max(0, TARGET_OUTPUT_CHARS - input.separatorChars)
  let result = assemblePayload(
    input.toolName,
    input.originalChars,
    input.segments.map(() => ""),
    input.recovery,
    input.separatorChars,
  )
  for (let attempt = 0; attempt < 6; attempt++) {
    const budgets = segmentBudgets(input.segments, bodyBudget)
    const bodies = input.segments.map((segment, index) =>
      compressOutput(segment, input.strategy, budgets[index] ?? 0, input.filePath),
    )
    result = assemblePayload(input.toolName, input.originalChars, bodies, input.recovery, input.separatorChars)
    const excess = result.finalChars - TARGET_OUTPUT_CHARS
    if (excess <= 0) break
    bodyBudget = Math.max(0, bodyBudget - excess)
  }
  return result
}

function outputPath(output: any): string | undefined {
  const value = output?.metadata?.outputPath
  return typeof value === "string" && value.trim() ? value : undefined
}

function setCompressionMetadata(
  output: any,
  metadata: {
    finalChars: number
    finalTokens: number
    originalChars: number
    originalTokens: number
    recovery: Recovery
    savedPct: number
    strategy: Strategy
  },
): void {
  const current = typeof output.metadata === "object" && output.metadata !== null ? output.metadata : {}
  output.metadata = { ...current, rtk: { compressed: true, ...metadata } }
}

async function compressToolOutput(input: any, output: any): Promise<void> {
  const source = extractText(output)
  const toolName = singleLine(String(input?.tool ?? "tool"), 64) || "tool"
  logDebug(
    `${toolName} — ${source?.length ?? 0} chars (~${tokensFromChars(source?.length ?? 0)} tokens), ` +
      `sink=${source?.sink ?? "none"}, threshold=${CHAR_THRESHOLD} chars (~${TRIGGER_TOKENS} tokens)`,
  )
  if (!source || source.length < CHAR_THRESHOLD) return

  const savedPath = outputPath(output)
  // Avoid joining a huge segmented MCP response. Leaving it unchanged lets
  // OpenCode apply its own bounded preview and durable full-output storage.
  if (source.sink === "content" && source.length > CACHE_ENTRY_MAX_BYTES) {
    logDebug(`skip: ${source.length} chars exceeds bounded MCP materialization limit`)
    return
  }

  const original = source.segments.join("\n\n")
  const originalBytes = Buffer.byteLength(original, "utf8")
  if (source.sink === "content" && originalBytes > CACHE_ENTRY_MAX_BYTES) {
    logDebug(`skip: UTF-8 output is too large to cache safely`)
    return
  }

  const strategy = HEAVY_TOOLS[String(input?.tool ?? "").toLowerCase()] ?? DEFAULT_STRATEGY
  const normalized = source.segments.map((segment) => stripVTControlCharacters(segment).replace(/\r\n?/g, "\n"))
  const recovery: Recovery = savedPath
    ? { outputPath: savedPath }
    : originalBytes <= CACHE_ENTRY_MAX_BYTES
      ? { id: cacheID(String(input?.sessionID ?? ""), String(input?.callID ?? ""), original) }
      : {
          unavailable: `output exceeded the ${CACHE_ENTRY_MAX_BYTES}-byte cache limit; rerun the tool with narrower scope`,
        }
  const filePath = typeof input?.args?.filePath === "string" ? input.args.filePath : undefined
  const compressed = buildPayload({
    filePath,
    originalChars: original.length,
    recovery,
    separatorChars: source.separatorChars,
    segments: normalized,
    strategy,
    toolName,
  })
  const exactSavedPct = (1 - compressed.finalChars / original.length) * 100
  if (compressed.finalChars > TARGET_OUTPUT_CHARS || exactSavedPct <= MIN_SAVINGS_PCT) return

  const cacheResult = "id" in recovery ? await storeCachedOutput(recovery.id, original) : false
  if ("id" in recovery && !cacheResult) return

  try {
    source.write(compressed.values)
  } catch (error) {
    if ("id" in recovery && cacheResult === "created") await rm(cachePath(recovery.id), { force: true })
    throw error
  }
  try {
    setCompressionMetadata(output, {
      finalChars: compressed.finalChars,
      finalTokens: compressed.finalTokens,
      originalChars: original.length,
      originalTokens: approxTokens(original),
      recovery,
      savedPct: Math.round(exactSavedPct * 10) / 10,
      strategy,
    })
  } catch (error) {
    logDebug(`metadata update failed: ${error instanceof Error ? error.message : String(error)}`)
  }
  logDebug(
    `compressed ${toolName}: ${original.length}->${compressed.finalChars} chars ` +
      `(${exactSavedPct.toFixed(1)}% saved), sink=${source.sink}`,
  )
}

function searchCachedText(text: string, query: string, limit: number): string {
  const needle = query.trim()
  if (!needle) return "Search query must not be empty."
  const pattern = new RegExp(needle.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"), "giu")
  const snippets: string[] = []
  let contentUsed = 0
  let found = false
  for (const match of text.matchAll(pattern)) {
    found = true
    if (snippets.length >= 20) break
    const index = match.index
    const label = `[match at char ${index}]\n`
    const remaining = limit - contentUsed
    if (remaining <= 0) {
      if (snippets.length === 0) snippets.push(`${label}[increase limit to include matching content]`)
      break
    }
    const start =
      remaining >= match[0].length ? Math.max(0, index - Math.floor((remaining - match[0].length) / 2)) : index
    const content = safeSlice(text, start, Math.min(text.length, start + remaining))
    snippets.push(`${label}${content}`)
    contentUsed += content.length
  }
  if (snippets.length > 0) return snippets.join("\n\n")
  return found
    ? `[match found for ${JSON.stringify(needle)}, but the requested limit is too small]`
    : `No matches found for ${JSON.stringify(needle)}.`
}

// ─── Plugin ────────────────────────────────────────────────────────────────────

const retrieveTool = tool({
  description:
    "Retrieve content omitted by RTK output compression. Use query to search, or offset and limit to read a bounded slice.",
  args: {
    id: tool.schema.string().min(1).max(64).describe("Cache id shown in the RTK recovery footer"),
    limit: tool.schema
      .number()
      .int()
      .min(1)
      .max(RETRIEVAL_MAX_CHARS)
      .optional()
      .describe("Maximum cached-content characters to return, excluding retrieval metadata"),
    offset: tool.schema.number().int().min(0).optional().describe("Character offset for direct retrieval"),
    query: tool.schema.string().min(1).max(256).optional().describe("Case-insensitive substring to search for"),
  },
  async execute(args) {
    try {
      const text = await readCachedOutput(args.id)
      const limit = Math.min(args.limit ?? RETRIEVAL_DEFAULT_CHARS, RETRIEVAL_MAX_CHARS)
      if (args.query) {
        return `[rtk retrieval ${args.id}: search ${JSON.stringify(args.query)}]\n\n${searchCachedText(text, args.query, limit)}`
      }
      const offset = Math.min(args.offset ?? 0, text.length)
      const end = Math.min(offset + limit, text.length)
      return `[rtk retrieval ${args.id}: chars ${offset}-${end} of ${text.length}]\n\n${safeSlice(text, offset, end)}`
    } catch (error) {
      return `[rtk retrieval failed: ${error instanceof Error ? error.message : String(error)}]`
    }
  },
})

const RtkOpenCodePlugin: Plugin = async ({ $ }) => {
  const canRewrite = Bun.which("rtk") !== null

  if (!canRewrite) console.warn("[rtk] command rewriting unavailable; output compression remains active")

  return {
    tool: { rtk_retrieve: retrieveTool },

    // Rewrite bash/shell commands to their rtk equivalents before they run.
    "tool.execute.before": async (input, output) => {
      const tool = String(input?.tool ?? "").toLowerCase()
      if (!canRewrite || (tool !== "bash" && tool !== "shell")) return

      const args = output?.args
      if (!args || typeof args !== "object") return
      const command = args.command
      if (typeof command !== "string" || !command) return

      try {
        const result = await $`rtk rewrite ${command}`.quiet().nothrow()
        if (result.exitCode !== 0 && result.exitCode !== 3) return
        const rewritten = String(result.stdout).trim()
        if (rewritten && rewritten !== command) args.command = rewritten
      } catch (error) {
        logDebug(`rewrite failed: ${error instanceof Error ? error.message : String(error)}`)
      }
    },

    // Compress heavy tool outputs (built-in, MCP, and unlisted) after they run.
    "tool.execute.after": async (input, output) => {
      try {
        await compressToolOutput(input, output)
      } catch (error) {
        // OpenCode propagates hook errors into tool execution. Compression must
        // therefore fail open and leave every successful tool result untouched.
        logDebug(`compression failed: ${error instanceof Error ? error.message : String(error)}`)
      }
    },
  }
}

export { RtkOpenCodePlugin }
export default RtkOpenCodePlugin
