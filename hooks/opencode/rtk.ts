import { execFile, spawn, type ChildProcess } from "node:child_process"
import { existsSync, statSync } from "node:fs"
import { homedir } from "node:os"
import { basename, delimiter, dirname, isAbsolute, join } from "node:path"

// RTK OpenCode plugin — rewrites commands to use rtk for token savings and
// compresses tool output after it runs.
//
// This is a thin compat shim: all rewrite and permission logic lives in
// `rtk hook opencode`, and all output-compression policy lives in the
// resolved pipeline (`rtk hook opencode-after`). It judges a command against
// OpenCode's own permission rules and answers `{}` whenever the rewrite would
// change what those rules decide — OpenCode evaluates the final command
// itself, so a rewrite RTK does return never lifts a deny, silences an ask, or
// blocks an allow (#4195). To add or change rewrite rules, edit the Rust
// registry; to add or change compression rules, edit the pipeline — not this
// file.
//
// This file only carries the transport: it must run wherever OpenCode runs, so
// it uses `node:child_process` and PATH discovery rather than Bun's `$` shell
// helper (`TypeError: $ is not a function` under OpenCode Desktop / Electron)
// and `which` (absent on Windows).

type Answer = { command?: string }

// Hard ceiling on the replacement text, matching the Rust reader's 64 MiB cap.
const MAX_AFTER_BYTES = 64 * 1024 * 1024

let cachedRtkPath: string | null = null
let probedBin: { bin: string; capable: boolean } | null = null
let probedAfterBin: { bin: string; capable: boolean; active: boolean } | null = null
let warnedHookTimeout = false

export function _resetCachedRtkPath(): void {
  cachedRtkPath = null
  probedBin = null
  probedAfterBin = null
  warnedHookTimeout = false
}

/**
 * Default budget for one hook call, in milliseconds.
 *
 * 15 s covers an 8 s classifier with room for process start. The 3 s budget
 * this replaced did not: a classifier rule was killed at 3 s and the tool call
 * silently kept its raw output.
 */
const DEFAULT_HOOK_TIMEOUT_MS = 15000

/**
 * Largest delay a single `setTimeout` can hold: 2^31 - 1 ms (~24.8 days).
 *
 * Node overflows a larger delay and fires it after ~1 ms instead, which would
 * kill every hook call at once — the probe included — and read as "no
 * subcommand". Clamping keeps an over-large value meaning "as long as possible".
 */
const MAX_HOOK_TIMEOUT_MS = 2 ** 31 - 1

/** Grace period between SIGTERM and SIGKILL when a hook call times out. */
const KILL_GRACE_MS = 2000

/**
 * Stop a hook process that outstayed its budget: SIGTERM, then SIGKILL once
 * [`KILL_GRACE_MS`] has passed. A child that ignores the first signal must not
 * outlive the hook that gave up on it. The escalation timer is unref'd and
 * cleared when the child does exit, so it never holds OpenCode open.
 */
function killWithEscalation(child: ChildProcess): void {
  child.kill()
  const escalate = setTimeout(() => child.kill("SIGKILL"), KILL_GRACE_MS)
  escalate.unref?.()
  child.once("close", () => clearTimeout(escalate))
}

/**
 * How long a hook call may take, in milliseconds.
 *
 * `RTK_OPENCODE_HOOK_TIMEOUT_MS` raises it for a pipeline whose rules call a
 * model directly (route workers default to 30 s). A missing, non-numeric or
 * non-positive value falls back to the default rather than disabling the
 * timeout: an unbounded spawn would hang the tool call it is compressing. A
 * value past [`MAX_HOOK_TIMEOUT_MS`] is clamped, not taken at face value.
 */
export function hookTimeoutMs(): number {
  const raw = process.env.RTK_OPENCODE_HOOK_TIMEOUT_MS
  const parsed = raw === undefined || raw === "" ? Number.NaN : Number(raw)
  if (!Number.isFinite(parsed) || parsed <= 0) return DEFAULT_HOOK_TIMEOUT_MS
  return Math.min(Math.floor(parsed), MAX_HOOK_TIMEOUT_MS)
}

/**
 * Warn once per process that a hook call hit its budget.
 *
 * Once, not per call: a slow pipeline rule would otherwise print a line for
 * every tool call. The line names the variable that fixes it, which is the
 * only thing a user can act on.
 */
function warnHookTimeoutOnce(): void {
  if (warnedHookTimeout) return
  warnedHookTimeout = true
  console.warn(
    `[rtk] a hook call timed out after ${hookTimeoutMs()} ms — raise RTK_OPENCODE_HOOK_TIMEOUT_MS for slow pipeline rules (this warning prints once)`
  )
}

function probeDirsForRtk(dirs: string[], exts: string[]): string | null {
  for (const dir of dirs) {
    for (const ext of exts) {
      const fullPath = join(dir, `rtk${ext}`)
      try {
        if (existsSync(fullPath) && !statSync(fullPath).isDirectory()) {
          return fullPath
        }
      } catch {
        continue
      }
    }
  }
  return null
}

/**
 * Whether the shell reaches `bin` as a bare `rtk` once `dirname(bin)` is
 * appended to its PATH: an absolute path (a relative one would be resolved
 * against the shell's working directory) to a file named `rtk`.
 */
function reachableAsRtk(bin: string, exts: string[]): boolean {
  const fold = (s: string) => (process.platform === "win32" ? s.toLowerCase() : s)
  const name = fold(basename(bin))
  if (!isAbsolute(bin) || !exts.some((ext) => name === fold(`rtk${ext}`))) return false
  try {
    return existsSync(bin) && !statSync(bin).isDirectory()
  } catch {
    return false
  }
}

/**
 * Resolves the rtk binary that OpenCode's shell will run.
 *
 * The rewrite stays a bare `rtk …` and the shell hooks only append rtk's
 * directory to the shell's PATH, so an rtk already on PATH is the one every
 * rewritten command runs. It is therefore checked first (PATHEXT on Windows,
 * #1993): probing and judging the command with any other rtk would let the
 * binary that judged it and the one that runs it disagree. Without one,
 * RTK_BIN is used when the shell can reach it as `rtk`, then the standard
 * install directories: ~/.cargo/bin, ~/.local/bin, /opt/homebrew/bin,
 * /usr/local/bin, /usr/bin, and Windows local app data / program files. Those
 * are where an rtk sits when OpenCode is launched from a desktop GUI (macOS
 * Dock, a Linux desktop entry) that does not source the shell profile (#4462).
 *
 * A directory named `rtk` is skipped rather than returned.
 */
export function resolveRtkPath(): string | null {
  if (cachedRtkPath && existsSync(cachedRtkPath)) return cachedRtkPath

  const exts = process.platform === "win32"
    ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM").split(";")
    : [""]
  const rtkBinEnv = process.env.RTK_BIN

  // 1. System PATH
  const pathDirs = (process.env.PATH ?? "").split(delimiter).filter(Boolean)
  const pathMatch = probeDirsForRtk(pathDirs, exts)
  if (pathMatch) {
    if (rtkBinEnv && rtkBinEnv !== pathMatch) {
      console.warn(`[rtk] RTK_BIN=${rtkBinEnv} ignored: ${pathMatch} is on PATH, so it is the rtk OpenCode's shell runs`)
    }
    return (cachedRtkPath = pathMatch)
  }

  // 2. RTK_BIN
  if (rtkBinEnv) {
    if (reachableAsRtk(rtkBinEnv, exts)) return (cachedRtkPath = rtkBinEnv)
    console.warn(`[rtk] RTK_BIN=${rtkBinEnv} ignored: it must be an absolute path to a file named rtk`)
  }

  // 3. Candidate directories outside system PATH (#4462)
  const candidateMatch = probeDirsForRtk(candidateRtkDirs(), exts)
  if (candidateMatch) return (cachedRtkPath = candidateMatch)

  return (cachedRtkPath = null)
}

/**
 * Install directories probed when rtk is on neither `PATH` nor `RTK_BIN`.
 */
export function candidateRtkDirs(): string[] {
  const home = homedir()
  const dirs: string[] = [
    join(home, ".cargo", "bin"),
    join(home, ".local", "bin"), // install.sh's default INSTALL_DIR
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
  ]

  if (process.platform === "win32") {
    if (process.env.LOCALAPPDATA) dirs.push(join(process.env.LOCALAPPDATA, "cargo", "bin"))
    if (process.env.ProgramFiles) dirs.push(join(process.env.ProgramFiles, "rtk"))
    if (process.env["ProgramFiles(x86)"]) dirs.push(join(process.env["ProgramFiles(x86)"], "rtk"))
  }
  return dirs
}

/**
 * Appends the rtk directory to the spawned shell's PATH if not already present.
 *
 * Appending rather than prepending keeps every command that already resolves
 * on PATH resolving to the same binary.
 */
export function appendRtkDirToPath(env: Record<string, any>, rtkDir: string): void {
  // Windows names it `Path` and matches it case-insensitively. Reuse the key the
  // environment already has, so `{ ...process.env, ...env }` carries one PATH
  // rather than two that differ only in case.
  const isPathKey = (k: string) => (process.platform === "win32" ? k.toUpperCase() === "PATH" : k === "PATH")
  const pathKey = Object.keys(env).find(isPathKey) ?? Object.keys(process.env).find(isPathKey) ?? "PATH"
  const currentPath = env[pathKey] ?? process.env[pathKey] ?? ""
  if (currentPath) {
    const parts = currentPath.split(delimiter)
    if (!parts.includes(rtkDir)) {
      env[pathKey] = `${currentPath}${delimiter}${rtkDir}`
    }
  } else {
    env[pathKey] = rtkDir
  }
}

/**
 * Probes for the `rtk hook opencode` subcommand, and nothing else.
 *
 * Not a version number: a develop build reports `rtk 0.49.0` (release-please
 * only bumps Cargo.toml on master) and so does the pre-#4349 release that
 * lacks the subcommand entirely — the two are indistinguishable by banner, and
 * a version floor also disabled the plugin on the very binary that installed
 * it. `hook opencode --help` separates them: exit 0 with the subcommand, exit 2
 * without it, and a broken or wrong-arch binary fails to spawn at all. One
 * call, and it subsumes the "does this binary even run" check.
 *
 * Cached per binary — this runs once per OpenCode session, not per tool call.
 */
export function probeRtkHookOpencode(rtkBin: string): Promise<boolean> {
  if (probedBin?.bin === rtkBin) return Promise.resolve(probedBin.capable)
  return new Promise((resolve) => {
    execFile(
      rtkBin,
      ["hook", "opencode", "--help"],
      { encoding: "utf8", timeout: hookTimeoutMs(), windowsHide: true },
      (error) => {
        if (error) return resolve(false)
        probedBin = { bin: rtkBin, capable: true }
        resolve(true)
      }
    )
  })
}

/**
 * What the after transport can do for this binary: whether it is supported at
 * all, and whether the resolved pipeline actually has a rule to run.
 *
 * `active: false` with `capable: true` means "supported, but nothing to do" —
 * the plugin then registers no after hook, so a tool call spawns no process.
 */
export type AfterProbe = { capable: boolean; active: boolean }

/** Version of the probe envelope; anything else is not an answer. */
const PROBE_ENVELOPE_VERSION = 1

/**
 * Reads `{"v":1,"active":true|false}` from `rtk hook opencode-after --probe`.
 *
 * `null` means the binary did not answer: the flag is unknown (an older
 * binary), or the output is not the probe envelope. The caller then falls back
 * to registering, which is what the plugin did before the probe existed.
 */
function parseProbeEnvelope(stdout: string): boolean | null {
  try {
    const parsed = JSON.parse(stdout)
    if (
      parsed &&
      typeof parsed === "object" &&
      parsed.v === PROBE_ENVELOPE_VERSION &&
      typeof parsed.active === "boolean"
    ) {
      return parsed.active
    }
  } catch {
    // Not JSON: a crash message, or a binary whose stdout is not this
    // envelope at all.
  }
  return null
}

/** Asks the binary's resolved pipeline whether it has an after-tool rule. */
function probeAfterActivity(rtkBin: string): Promise<boolean | null> {
  return new Promise((resolve) => {
    execFile(
      rtkBin,
      ["hook", "opencode-after", "--probe"],
      { encoding: "utf8", timeout: hookTimeoutMs(), windowsHide: true },
      (error, stdout) => {
        // A non-zero exit means the flag or the subcommand is unknown.
        if (error) return resolve(null)
        resolve(parseProbeEnvelope(String(stdout ?? "")))
      }
    )
  })
}

/** Whether `rtk hook opencode-after --help` answers at all. */
function probeAfterCapable(rtkBin: string): Promise<boolean> {
  return new Promise((resolve) => {
    execFile(
      rtkBin,
      ["hook", "opencode-after", "--help"],
      { encoding: "utf8", timeout: hookTimeoutMs(), windowsHide: true },
      (error) => resolve(!error)
    )
  })
}

/**
 * Probes for the `rtk hook opencode-after` subcommand, separately from the
 * before path, and asks whether there is anything for it to do.
 *
 * A binary that predates the after transport still carries `hook opencode`,
 * so reusing that probe would register nothing. This one is independent and
 * cached per binary: when it fails, rewriting keeps working and only output
 * compression stays off.
 *
 * The probe is fail-open by design. An unknown `--probe` flag (an older binary
 * that has the subcommand), a failed probe or a non-JSON answer all resolve to
 * `active: true` — registering is the behaviour every release before this one
 * had, so version skew never turns compression off.
 */
export async function probeRtkHookOpencodeAfter(rtkBin: string): Promise<AfterProbe> {
  if (probedAfterBin?.bin === rtkBin) {
    return { capable: probedAfterBin.capable, active: probedAfterBin.active }
  }

  const active = await probeAfterActivity(rtkBin)
  if (active !== null) {
    // A binary that answers the probe supports the subcommand by definition.
    probedAfterBin = { bin: rtkBin, capable: true, active }
    return { capable: true, active }
  }

  const capable = await probeAfterCapable(rtkBin)
  probedAfterBin = { bin: rtkBin, capable, active: capable }
  return { capable, active: capable }
}

/**
 * Invokes `rtk hook opencode [--agent <name>] <command>` and returns the
 * answered rewrite.
 *
 * `agent` is OpenCode 2.x's `event.agent`; rtk reads `agent.<name>.permission`
 * on top of the root rules, which is what OpenCode itself applies. Without it
 * an agent-scoped ask on `ls *` would be silenced by the rewrite and an
 * agent-scoped deny on `ls -la` would never see the command. OpenCode 1.x sends
 * no agent field, so the V1 path stays root-only.
 *
 * The answer is `{}` whenever the rewrite would change the verdict OpenCode's
 * own permission rules give, and that empty answer means the command runs as
 * typed. Anything unusable — non-zero exit, timeout, non-JSON stdout — also
 * resolves to null, so a broken rtk passes the command through instead of
 * blocking the tool call.
 */
export function runHookOpencode(
  rtkBin: string,
  command: string,
  agent?: string,
  timeoutMs = hookTimeoutMs()
): Promise<string | null> {
  const argv = ["hook", "opencode"]
  if (agent) argv.push("--agent", agent)
  argv.push(command)

  return new Promise((resolve) => {
    let settled = false
    let child: ChildProcess | undefined
    const finish = (value: string | null) => {
      if (settled) return
      settled = true
      clearTimeout(timer)
      resolve(value)
    }
    // The budget is enforced here rather than with execFile's own `timeout`,
    // which sends one SIGTERM and gives up: the same escalation as the after
    // path keeps a wedged rtk from outliving the hook.
    const timer = setTimeout(() => {
      warnHookTimeoutOnce()
      if (child) killWithEscalation(child)
      finish(null)
    }, timeoutMs)

    try {
      child = execFile(
        rtkBin,
        argv,
        { encoding: "utf8", windowsHide: true },
        (error, stdout) => {
          if (error) return finish(null)

          let answer: Answer
          try {
            answer = JSON.parse(String(stdout ?? "").trim() || "{}")
          } catch {
            return finish(null)
          }

          const rewritten = typeof answer?.command === "string" ? answer.command.trim() : ""
          finish(rewritten && rewritten !== command ? rewritten : null)
        }
      )
    } catch {
      // A binary that cannot be spawned at all: same answer as a failed call.
      finish(null)
    }
  })
}

/**
 * Invokes `rtk hook opencode-after` with the tool name, its args and the text
 * the tool produced, and returns the replacement text.
 *
 * The payload goes over stdin rather than argv: tool output is unbounded in
 * argv terms and is exactly the thing being compressed. `spawn` is the only
 * `node:child_process` API with a writable stdin, and it keeps the shell out
 * of the picture the same way `execFile` does on the before path.
 *
 * Anything unusable — spawn failure, non-zero exit, timeout, oversized
 * stdout — resolves to null, so OpenCode keeps the original output.
 */
export function runHookOpencodeAfter(
  rtkBin: string,
  payload: string,
  timeoutMs = hookTimeoutMs()
): Promise<string | null> {
  return new Promise((resolve) => {
    let settled = false
    let stdout = ""
    let timer: ReturnType<typeof setTimeout> | undefined

    const finish = (value: string | null) => {
      if (settled) return
      settled = true
      if (timer) clearTimeout(timer)
      resolve(value)
    }

    const child = (() => {
      try {
        return spawn(rtkBin, ["hook", "opencode-after"], {
          windowsHide: true,
          stdio: ["pipe", "pipe", "ignore"],
        })
      } catch {
        return null
      }
    })()
    if (!child) return finish(null)

    timer = setTimeout(() => {
      warnHookTimeoutOnce()
      killWithEscalation(child)
      finish(null)
    }, timeoutMs)

    child.on("error", () => finish(null))
    child.stdin.on("error", () => {
      // EPIPE when rtk exits before reading the payload; the exit code and
      // stdout decide the answer.
    })
    child.stdout.setEncoding("utf8")
    child.stdout.on("data", (chunk: string) => {
      stdout += chunk
      if (Buffer.byteLength(stdout, "utf8") > MAX_AFTER_BYTES) {
        child.kill()
        finish(null)
      }
    })
    child.on("close", (code) => finish(code === 0 && stdout ? stdout : null))

    child.stdin.end(payload)
  })
}

/**
 * The metadata both OpenCode API shapes carry about a finished tool call.
 * Everything is optional: 1.x has no `status`, and a tool that reports no exit
 * code simply omits `metadata.exit`.
 */
export type ToolMeta = {
  status?: string
  exit?: number
  description?: string
  durationMs?: number
}

/** First number among `values`, ignoring anything that is not a finite number. */
function firstNumber(...values: unknown[]): number | undefined {
  for (const value of values) {
    if (typeof value === "number" && Number.isFinite(value)) return value
  }
  return undefined
}

/**
 * Milliseconds the call took, when the tool's metadata carries them.
 *
 * OpenCode has no timing field on the plugin hooks, so this only ever reports
 * what a tool put in its own `metadata`: `durationMs`/`duration_ms`, or a
 * `{ start, end }` pair. Absent means 0, which is what the Rust side treats as
 * "unknown".
 */
export function durationMsOf(metadata: unknown): number | undefined {
  if (!metadata || typeof metadata !== "object") return undefined
  const meta = metadata as Record<string, any>
  const direct = firstNumber(meta.durationMs, meta.duration_ms)
  if (direct !== undefined) return direct
  const time = meta.time
  if (time && typeof time === "object") {
    const start = firstNumber((time as any).start, (time as any).started)
    const end = firstNumber((time as any).end, (time as any).ended)
    if (start !== undefined && end !== undefined && end >= start) return end - start
  }
  return undefined
}

/**
 * The stdout contract of a current `rtk hook opencode-after`.
 *
 * `v` is the compatibility handle: the replacement text lives in `output`, and
 * `append` carries the note plain text has no room for (the Read→Edit fidelity
 * guard), always present and empty when there is none.
 */
const AFTER_ENVELOPE_VERSION = 1

/**
 * Reads the stdout of `rtk hook opencode-after`.
 *
 * The plugin asks for the envelope (`expect.envelope`, see `tryCompressOutput`),
 * and a current binary answers `{"v":1,"output":"…","append":"…"}`. Anything
 * else — an older binary that printed the replacement text alone, or an
 * envelope version this plugin does not know — is read as the replacement text
 * itself, so version skew leaves the model with rtk's output rather than a JSON
 * blob it cannot use.
 */
export function parseAfterEnvelope(stdout: string): { output: string; append: string } {
  try {
    const parsed = JSON.parse(stdout)
    if (
      parsed &&
      typeof parsed === "object" &&
      parsed.v === AFTER_ENVELOPE_VERSION &&
      typeof parsed.output === "string"
    ) {
      return {
        output: parsed.output,
        append: typeof parsed.append === "string" ? parsed.append : "",
      }
    }
  } catch {
    // Not JSON: a binary from before the envelope, or a crash message.
  }
  return { output: stdout, append: "" }
}

/**
 * Compresses one tool result through the resolved pipeline. Returns null when
 * there is no rule, the pipeline leaves the text unchanged, or the result is
 * not strictly smaller — the caller then keeps the tool's own output.
 *
 * `meta` is the call metadata the host already has (status, exit code,
 * description, duration). It rides along in the same payload so pipeline
 * conditions such as `exit_code` or `duration_ms` see the same facts the Claude
 * path gets from its event.
 */
export async function tryCompressOutput(
  toolName: unknown,
  args: unknown,
  output: unknown,
  meta: ToolMeta = {}
): Promise<string | null> {
  const tool = String(toolName ?? "").toLowerCase()
  if (!tool || typeof output !== "string" || !output) return null

  // Same session-wide switch as the before path. The Rust side checks it too;
  // checking here avoids spawning for every tool call.
  if (process.env.RTK_DISABLED === "1") return null

  const rtkBin = resolveRtkPath()
  if (!rtkBin) return null

  const payload: Record<string, unknown> = {
    tool,
    args: args && typeof args === "object" ? args : {},
    output,
    // The envelope is opt-in: rtk answers plain replacement text unless the
    // host asks, which is what a plugin from before this field understands. An
    // old binary ignores it and keeps answering plain text.
    expect: { envelope: 1 },
  }
  if (typeof meta.status === "string" && meta.status) payload.status = meta.status
  if (typeof meta.exit === "number" && Number.isFinite(meta.exit)) {
    payload.metadata = { exit: meta.exit }
  }
  if (typeof meta.description === "string" && meta.description) {
    payload.description = meta.description
  }
  if (typeof meta.durationMs === "number" && Number.isFinite(meta.durationMs)) {
    payload.duration_ms = meta.durationMs
  }

  try {
    const rewritten = await runHookOpencodeAfter(rtkBin, JSON.stringify(payload))
    if (!rewritten) return null
    const { output: replacement, append } = parseAfterEnvelope(rewritten)
    if (!replacement) return null
    // `append` rides beside the replacement, never in place of it: the note
    // explains a format conversion, it is not the converted text.
    const text = append ? `${replacement}\n${append}` : replacement
    // The Rust side refuses a non-smaller result; compare bytes here as well
    // so a misbehaving binary cannot grow the agent's context — and so the
    // appended note can never tip a marginal saving over the raw size.
    return Buffer.byteLength(text, "utf8") < Buffer.byteLength(output, "utf8") ? text : null
  } catch {
    return null
  }
}

export async function tryRewriteCommand(
  toolName: string,
  command: unknown,
  agent?: string
): Promise<string | null> {
  const tool = String(toolName ?? "").toLowerCase()
  if ((tool !== "bash" && tool !== "shell") || typeof command !== "string" || !command.trim()) {
    return null
  }

  // RTK_DISABLED is two different things and only one of them is checked here.
  // The per-command form (`RTK_DISABLED=1 git status`) is an env prefix inside
  // the command string, and the Rust rewrite engine handles it — this plugin
  // passes the string through untouched, so that one already works. What is
  // checked here is the OpenCode process environment, which switches rewriting
  // off for the whole session. hooks/pi/rtk.ts:124 checks the same var.
  //
  // Per call rather than at setup: the variable is inherited from whatever
  // launched OpenCode, so it is not known until a hook fires.
  if (process.env.RTK_DISABLED === "1") return null

  const rtkBin = resolveRtkPath()
  if (!rtkBin) return null

  try {
    return await runHookOpencode(rtkBin, command, agent)
  } catch {
    return null
  }
}

async function handleToolHook(tool: unknown, container: any, agent?: string) {
  if (!container || typeof container !== "object") return
  const rewritten = await tryRewriteCommand(String(tool ?? ""), container.command, agent)
  if (rewritten) container.command = rewritten
}

/** The text a V2 tool result carries, when every block can be put back. */
function joinedTextContent(content: unknown): string | null {
  if (typeof content === "string") return content
  if (!Array.isArray(content) || content.length === 0) return null
  const texts: string[] = []
  for (const block of content) {
    if (
      !block ||
      typeof block !== "object" ||
      (block as any).type !== "text" ||
      typeof (block as any).text !== "string"
    ) {
      return null
    }
    texts.push((block as any).text)
  }
  return texts.join("\n")
}

/** Rebuild a V2 result's `content` in the shape it arrived in. */
function rebuildContent(content: unknown, text: string): unknown {
  return typeof content === "string" ? text : [{ type: "text", text }]
}

/**
 * OpenCode 2.x `execute.after`: `event.result.content` is the model-visible
 * text. Only completed calls are touched, and only when the result can be put
 * back in its original shape.
 *
 * `status`, `result.metadata.exit` and `input.description` are forwarded so
 * pipeline conditions see the same facts the Claude path reads from its event.
 */
async function handleToolAfter(event: any) {
  try {
    if (!event || event.status !== "completed") return
    const result = event.result
    if (!result || typeof result !== "object") return
    const text = joinedTextContent(result.content)
    if (text === null) return
    const metadata = result.metadata
    const rewritten = await tryCompressOutput(event.tool, event.input, text, {
      status: typeof event.status === "string" ? event.status : undefined,
      exit: firstNumber(
        metadata && typeof metadata === "object" ? (metadata as any).exit : undefined,
        metadata && typeof metadata === "object" ? (metadata as any).exitCode : undefined
      ),
      description: typeof event.input?.description === "string" ? event.input.description : undefined,
      durationMs: durationMsOf(metadata),
    })
    if (rewritten) {
      event.result = { ...result, content: rebuildContent(result.content, rewritten) }
    }
  } catch {
    // Fail open: compression must never fail the tool call.
  }
}

/**
 * OpenCode 1.x `tool.execute.after`: `output.output` is the text the model
 * reads, and `input.args` is the per-tool input the source mapping needs.
 *
 * 1.x has no `status` (the hook only fires on completion); the exit code, the
 * call description and any timing live in `output.metadata` / `output.args`.
 */
async function handleToolAfterV1(input: any, output: any) {
  try {
    if (!output || typeof output !== "object" || typeof output.output !== "string") return
    const metadata = output.metadata
    // `output.args` is the tool's own arguments (the same object the before
    // hook mutates); `input.args` is the fallback when the host omits them.
    // One source, so the command and the description always agree.
    const args = output?.args ?? input?.args
    const rewritten = await tryCompressOutput(input?.tool, args, output.output, {
      exit: firstNumber(
        metadata && typeof metadata === "object" ? (metadata as any).exit : undefined,
        metadata && typeof metadata === "object" ? (metadata as any).exitCode : undefined
      ),
      description: typeof args?.description === "string" ? args.description : undefined,
      durationMs: durationMsOf(metadata),
    })
    if (rewritten) output.output = rewritten
  } catch {
    // Fail open: compression must never fail the tool call.
  }
}

/**
 * Resolves the capabilities the plugin can register.
 *
 * The before subcommand is the baseline: without it the plugin disables
 * itself. The after transport is additive, so a binary that predates it keeps
 * rewriting and only skips output compression. It is also conditional on the
 * pipeline: `after` is false both when the subcommand is missing and when the
 * resolved pipeline has no `hook.after_tool.*` rule, so no process is spawned
 * per tool call for a rule set that cannot match.
 */
async function ensureRtkUsable(): Promise<{ after: boolean; bin: string } | null> {
  const rtkBin = resolveRtkPath()
  if (!rtkBin) {
    console.warn(
      "[rtk] rtk binary not found (checked PATH, RTK_BIN and the standard install directories) — plugin disabled"
    )
    return null
  }
  if (!(await probeRtkHookOpencode(rtkBin))) {
    console.warn(`[rtk] ${rtkBin} has no \`rtk hook opencode\` subcommand — plugin disabled`)
    return null
  }
  const after = await probeRtkHookOpencodeAfter(rtkBin)
  if (!after.capable) {
    console.warn(
      `[rtk] ${rtkBin} has no \`rtk hook opencode-after\` subcommand — output compression disabled`
    )
  }
  return { after: after.capable && after.active, bin: rtkBin }
}

/**
 * OpenCode 2.x and 1.x (>= 1.18.29) plugin for RTK.
 *
 * A plain object, because the 2.x loader schema-validates `default` against
 * `typeof === "object"` — a callable with `id`/`setup` bolted on fails
 * validation, so the V2 branch would never activate.
 * The `server()` method is the documented 1.x dual-shape entrypoint.
 *
 * 1.18.29 is the floor OpenCode's own V2 plugin docs put on the object form:
 * https://opencode.ai/v2/docs/build/plugins#support-v1. Below it
 * `rtk init -g --opencode` installs nothing when it can read
 * `opencode --version`, and names the version it found.
 */
const RtkOpenCodePlugin = {
  id: "rtk",

  // OpenCode 2.x entrypoints: `execute.before` gets
  // `{tool, sessionID, agent, messageID, id, input}`, `execute.after` adds
  // `status` and the tool `result`.
  async setup(ctx: any) {
    const usable = await ensureRtkUsable()
    if (!usable) return
    const rtkDir = dirname(usable.bin)

    await ctx?.shell?.hook?.("create.before", (e: any) => {
      if (!e) return
      if (!e.env) e.env = {}
      appendRtkDirToPath(e.env, rtkDir)
    })

    await ctx?.tool?.hook?.("execute.before", (e: any) =>
      handleToolHook(e?.tool, e?.input, typeof e?.agent === "string" ? e.agent : undefined)
    )
    if (usable.after) {
      await ctx?.tool?.hook?.("execute.after", (e: any) => handleToolAfter(e))
    }
  },

  // OpenCode 1.x entrypoint: `{tool, sessionID, callID}` — no agent field, so
  // an agent-scoped ask stays silenced here.
  async server() {
    const usable = await ensureRtkUsable()
    if (!usable) return {}
    const rtkDir = dirname(usable.bin)

    const hooks: Record<string, (input: any, output: any) => void | Promise<void>> = {
      "shell.env": (input: any, output: any) => {
        if (!output) return
        if (!output.env) output.env = {}
        appendRtkDirToPath(output.env, rtkDir)
      },
      "tool.execute.before": (input: any, output: any) =>
        handleToolHook(input?.tool, output?.args),
    }
    if (usable.after) {
      hooks["tool.execute.after"] = (input: any, output: any) =>
        handleToolAfterV1(input, output)
    }
    return hooks
  },
}

export default RtkOpenCodePlugin
export { RtkOpenCodePlugin }
