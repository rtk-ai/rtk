import { execFile } from "node:child_process"
import { existsSync, statSync } from "node:fs"
import { homedir } from "node:os"
import { delimiter, join } from "node:path"

// RTK OpenCode plugin — rewrites commands to use rtk for token savings.
//
// This is a thin compat shim: all rewrite and permission logic lives in
// `rtk hook opencode`, which is the single source of truth
// (src/discover/registry.rs). It judges the command against OpenCode's own
// permission rules and answers `{}` whenever the rewrite would change what
// those rules decide — OpenCode evaluates the final command itself, so a
// rewrite RTK does return never lifts a deny, silences an ask, or blocks an
// allow (#4195). To add or change rewrite rules, edit the Rust registry — not
// this file.
//
// This file only carries the transport: it must run wherever OpenCode runs, so
// it uses `node:child_process.execFile` and PATH discovery rather than Bun's
// `$` shell helper (`TypeError: $ is not a function` under OpenCode Desktop /
// Electron) and `which` (absent on Windows).

type Answer = { command?: string }

let cachedRtkPath: string | null = null
let cachedRtkOnPath: boolean = false
let probedBin: { bin: string; capable: boolean } | null = null

export function _resetCachedRtkPath(): void {
  cachedRtkPath = null
  cachedRtkOnPath = false
  probedBin = null
}

export function isRtkOnPath(): boolean {
  if (!cachedRtkPath) resolveRtkPath()
  return cachedRtkOnPath
}

/**
 * Resolves the rtk binary.
 *
 * Checks system PATH first. If not found on PATH (common when OpenCode is
 * launched from a desktop GUI like the macOS Dock or Linux desktop entry
 * where shell profiles are not sourced), probes candidate install locations:
 * RTK_BIN, ~/.cargo/bin, Homebrew (/opt/homebrew/bin), /usr/local/bin,
 * and Windows local app data / program files (#4462).
 *
 * Windows still probes PATHEXT, which is the #1993 fix: `which` does not exist
 * there.
 */
export function resolveRtkPath(): string | null {
  if (cachedRtkPath && existsSync(cachedRtkPath)) return cachedRtkPath

  const exts = process.platform === "win32"
    ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM").split(";")
    : [""]

  // 1. Try system PATH
  const dirs = (process.env.PATH ?? "").split(delimiter).filter(Boolean)
  for (const dir of dirs) {
    for (const ext of exts) {
      const fullPath = join(dir, `rtk${ext}`)
      if (existsSync(fullPath) && !statSync(fullPath).isDirectory()) {
        cachedRtkOnPath = true
        return (cachedRtkPath = fullPath)
      }
    }
  }

  // 2. Candidate paths outside system PATH (#4462)
  if (process.env.RTK_BIN) {
    if (existsSync(process.env.RTK_BIN) && !statSync(process.env.RTK_BIN).isDirectory()) {
      cachedRtkOnPath = false
      return (cachedRtkPath = process.env.RTK_BIN)
    }
  }

  const candidateDirs: string[] = [
    join(homedir(), ".cargo", "bin"),
    "/opt/homebrew/bin",
    "/usr/local/bin",
    "/usr/bin",
  ]

  if (process.platform === "win32") {
    if (process.env.LOCALAPPDATA) candidateDirs.push(join(process.env.LOCALAPPDATA, "cargo", "bin"))
    if (process.env.ProgramFiles) candidateDirs.push(join(process.env.ProgramFiles, "rtk"))
    if (process.env["ProgramFiles(x86)"]) candidateDirs.push(join(process.env["ProgramFiles(x86)"], "rtk"))
  }

  for (const dir of candidateDirs) {
    for (const ext of exts) {
      const fullPath = join(dir, `rtk${ext}`)
      if (existsSync(fullPath) && !statSync(fullPath).isDirectory()) {
        cachedRtkOnPath = false
        return (cachedRtkPath = fullPath)
      }
    }
  }

  cachedRtkOnPath = false
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
    let rewritten = await runHookOpencode(rtkBin, command, agent)
    if (!rewritten) return null

    // If rtk was found outside OpenCode's PATH (e.g. ~/.cargo/bin on macOS GUI launch),
    // a bare `rtk …` would fail in the spawned shell with `rtk: command not found` (#4462).
    // Carry the resolved binary path so the rewritten command can execute cleanly.
    if (!cachedRtkOnPath) {
      const quoted = rtkBin.includes(" ") ? `"${rtkBin}"` : rtkBin
      if (rewritten === "rtk") {
        rewritten = quoted
      } else if (rewritten.startsWith("rtk ")) {
        rewritten = `${quoted} ${rewritten.slice(4)}`
      }
    }

    return rewritten
  } catch {
    return null
  }
}

async function handleToolHook(tool: unknown, container: any, agent?: string) {
  if (!container || typeof container !== "object") return
  const rewritten = await tryRewriteCommand(String(tool ?? ""), container.command, agent)
  if (rewritten) container.command = rewritten
}

/**
 * Registers hooks only against an rtk that carries `rtk hook opencode`.
 */
async function ensureRtkUsable(): Promise<boolean> {
  const rtkBin = resolveRtkPath()
  if (!rtkBin) {
    console.warn("[rtk] rtk binary not found (checked PATH, RTK_BIN, ~/.cargo/bin, Homebrew) — plugin disabled")
    return false
  }
  if (!(await probeRtkHookOpencode(rtkBin))) {
    console.warn(`[rtk] ${rtkBin} has no \`rtk hook opencode\` subcommand — plugin disabled`)
    return false
  }
  return true
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

  // OpenCode 2.x entrypoint: `{tool, sessionID, agent, messageID, id, input}`.
  async setup(ctx: any) {
    if (!(await ensureRtkUsable())) return
    await ctx?.tool?.hook?.("execute.before", (e: any) =>
      handleToolHook(e?.tool, e?.input, typeof e?.agent === "string" ? e.agent : undefined)
    )
  },

  // OpenCode 1.x entrypoint: `{tool, sessionID, callID}` — no agent field, so
  // an agent-scoped ask stays silenced here.
  async server() {
    if (!(await ensureRtkUsable())) return {}
    return {
      "tool.execute.before": (input: any, output: any) =>
        handleToolHook(input?.tool, output?.args),
    }
  },
}

export default RtkOpenCodePlugin
export { RtkOpenCodePlugin }
