import { execFile, spawn } from "node:child_process"
import { existsSync, statSync } from "node:fs"
import { delimiter, join } from "node:path"

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
let probedAfterBin: { bin: string; capable: boolean } | null = null

export function _resetCachedRtkPath(): void {
  cachedRtkPath = null
  probedBin = null
  probedAfterBin = null
}

/**
 * Resolves the rtk binary from the system PATH.
 *
 * PATH only, deliberately. OpenCode's bash/shell tool runs commands with
 * OpenCode's own PATH and never sources `.profile`, `.bashrc` or
 * `.bash_profile`, so an rtk found outside PATH cannot be spawned as a bare
 * `rtk …` — the tool comes back `rtk: command not found`. Finding one anyway
 * is worse than not rewriting (#4462 carries that case). A directory on PATH
 * named `rtk` is skipped rather than frozen as the answer: it disables the
 * plugin for the whole session.
 *
 * Windows still probes PATHEXT, which is the #1993 fix: `which` does not exist
 * there.
 */
export function resolveRtkPath(): string | null {
  if (cachedRtkPath && existsSync(cachedRtkPath)) return cachedRtkPath

  const dirs = (process.env.PATH ?? "").split(delimiter).filter(Boolean)
  const exts = process.platform === "win32"
    ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM").split(";")
    : [""]

  for (const dir of dirs) {
    for (const ext of exts) {
      const fullPath = join(dir, `rtk${ext}`)
      if (existsSync(fullPath) && !statSync(fullPath).isDirectory()) {
        return (cachedRtkPath = fullPath)
      }
    }
  }

  return (cachedRtkPath = null)
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
      { encoding: "utf8", timeout: 3000, windowsHide: true },
      (error) => {
        if (error) return resolve(false)
        probedBin = { bin: rtkBin, capable: true }
        resolve(true)
      }
    )
  })
}

/**
 * Probes for the `rtk hook opencode-after` subcommand, separately from the
 * before path.
 *
 * A binary that predates the after transport still carries `hook opencode`,
 * so reusing that probe would register nothing. This one is independent and
 * cached per binary: when it fails, rewriting keeps working and only output
 * compression stays off.
 */
export function probeRtkHookOpencodeAfter(rtkBin: string): Promise<boolean> {
  if (probedAfterBin?.bin === rtkBin) return Promise.resolve(probedAfterBin.capable)
  return new Promise((resolve) => {
    execFile(
      rtkBin,
      ["hook", "opencode-after", "--help"],
      { encoding: "utf8", timeout: 3000, windowsHide: true },
      (error) => {
        if (error) return resolve(false)
        probedAfterBin = { bin: rtkBin, capable: true }
        resolve(true)
      }
    )
  })
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
  timeoutMs = 3000
): Promise<string | null> {
  const argv = ["hook", "opencode"]
  if (agent) argv.push("--agent", agent)
  argv.push(command)

  return new Promise((resolve) => {
    execFile(rtkBin, argv, { encoding: "utf8", timeout: timeoutMs, windowsHide: true }, (error, stdout) => {
      if (error) return resolve(null)

      let answer: Answer
      try {
        answer = JSON.parse(String(stdout ?? "").trim() || "{}")
      } catch {
        return resolve(null)
      }

      const rewritten = typeof answer?.command === "string" ? answer.command.trim() : ""
      resolve(rewritten && rewritten !== command ? rewritten : null)
    })
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
  timeoutMs = 3000
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
      child.kill()
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
 * Compresses one tool result through the resolved pipeline. Returns null when
 * there is no rule, the pipeline leaves the text unchanged, or the result is
 * not strictly smaller — the caller then keeps the tool's own output.
 */
export async function tryCompressOutput(
  toolName: unknown,
  args: unknown,
  output: unknown
): Promise<string | null> {
  const tool = String(toolName ?? "").toLowerCase()
  if (!tool || typeof output !== "string" || !output) return null

  // Same session-wide switch as the before path. The Rust side checks it too;
  // checking here avoids spawning for every tool call.
  if (process.env.RTK_DISABLED === "1") return null

  const rtkBin = resolveRtkPath()
  if (!rtkBin) return null

  const payload = JSON.stringify({
    tool,
    args: args && typeof args === "object" ? args : {},
    output,
  })

  try {
    const rewritten = await runHookOpencodeAfter(rtkBin, payload)
    if (!rewritten) return null
    // The Rust side refuses a non-smaller result; compare bytes here as well
    // so a misbehaving binary cannot grow the agent's context.
    return Buffer.byteLength(rewritten, "utf8") < Buffer.byteLength(output, "utf8")
      ? rewritten
      : null
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
 */
async function handleToolAfter(event: any) {
  try {
    if (!event || event.status !== "completed") return
    const result = event.result
    if (!result || typeof result !== "object") return
    const text = joinedTextContent(result.content)
    if (text === null) return
    const rewritten = await tryCompressOutput(event.tool, event.input, text)
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
 */
async function handleToolAfterV1(input: any, output: any) {
  try {
    if (!output || typeof output !== "object" || typeof output.output !== "string") return
    const rewritten = await tryCompressOutput(input?.tool, input?.args, output.output)
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
 * rewriting and only skips output compression.
 */
async function ensureRtkUsable(): Promise<{ after: boolean } | null> {
  const rtkBin = resolveRtkPath()
  if (!rtkBin) {
    console.warn("[rtk] no rtk on OpenCode's PATH — plugin disabled")
    return null
  }
  if (!(await probeRtkHookOpencode(rtkBin))) {
    console.warn(`[rtk] ${rtkBin} has no \`rtk hook opencode\` subcommand — plugin disabled`)
    return null
  }
  const after = await probeRtkHookOpencodeAfter(rtkBin)
  if (!after) {
    console.warn(
      `[rtk] ${rtkBin} has no \`rtk hook opencode-after\` subcommand — output compression disabled`
    )
  }
  return { after }
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
    const hooks: Record<string, (input: any, output: any) => Promise<void>> = {
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
