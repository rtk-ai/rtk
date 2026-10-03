import { execFile } from "node:child_process"
import { existsSync } from "node:fs"
import { homedir } from "node:os"
import { delimiter, join } from "node:path"

let cachedRtkPath: string | null = null

export function _resetCachedRtkPath(): void {
  cachedRtkPath = null
}

export function expandHome(filepath: string): string {
  return /^~[/\\]?/.test(filepath)
    ? join(homedir(), filepath.replace(/^~[/\\]?/, ""))
    : filepath
}

/**
 * Resolves the rtk binary from RTK_BIN, standard PATH, or common installation directories.
 */
export function resolveRtkPath(): string | null {
  if (cachedRtkPath && existsSync(cachedRtkPath)) return cachedRtkPath

  const envBin = process.env.RTK_BIN
  if (envBin) {
    const expanded = expandHome(envBin)
    if (existsSync(expanded)) return (cachedRtkPath = expanded)
  }

  const dirs = [
    ...(process.env.PATH ?? "").split(delimiter).filter(Boolean),
    join(homedir(), ".local", "bin"),
    join(homedir(), ".cargo", "bin"),
    "/opt/homebrew/bin",
    "/usr/local/bin",
  ]
  const exts = process.platform === "win32"
    ? (process.env.PATHEXT ?? ".EXE;.CMD;.BAT;.COM").split(";")
    : [""]

  for (const dir of dirs) {
    for (const ext of exts) {
      const fullPath = join(dir, `rtk${ext}`)
      if (existsSync(fullPath)) return (cachedRtkPath = fullPath)
    }
  }

  return (cachedRtkPath = null)
}

/**
 * Invokes `rtk rewrite <command>`.
 * Handles exit code 0 (Allow) and exit code 3 (Ask/Default).
 * Discards partial stdout if the process was terminated, killed by timeout,
 * or exited with non-rewrite error codes (Deny: 2, Defer: 1).
 */
export function runRtkRewrite(
  rtkBin: string,
  command: string,
  timeoutMs = 3000
): Promise<string | null> {
  return new Promise((resolve) => {
    execFile(
      rtkBin,
      ["rewrite", command],
      { encoding: "utf8", timeout: timeoutMs, windowsHide: true },
      (error, stdout) => {
        if (error) {
          if (error.killed || error.signal) return resolve(null)
          const exitCode = (error as unknown as { code?: number | string }).code
          if (exitCode !== 3) return resolve(null)
        }

        const output = String(stdout ?? "").trim()
        resolve(output && output !== command ? output : null)
      }
    )
  })
}

export async function tryRewriteCommand(
  toolName: string,
  command: unknown
): Promise<string | null> {
  const tool = String(toolName ?? "").toLowerCase()
  if ((tool !== "bash" && tool !== "shell") || typeof command !== "string" || !command.trim()) {
    return null
  }

  const rtkBin = resolveRtkPath()
  if (!rtkBin) return null

  try {
    return await runRtkRewrite(rtkBin, command)
  } catch {
    return null
  }
}

async function handleToolHook(tool: unknown, container: any, key: "command") {
  if (!container || typeof container !== "object") return
  const rewritten = await tryRewriteCommand(String(tool ?? ""), container[key])
  if (rewritten) container[key] = rewritten
}

function warnMissingRtk(): boolean {
  const found = Boolean(resolveRtkPath())
  if (!found) console.warn("[rtk] rtk binary not found — plugin disabled")
  return found
}

/**
 * Universal OpenCode plugin for RTK.
 * Exports a plain object with `id` and `setup(ctx)` matching OpenCode 2.x Schema validation,
 * with a `server()` method for OpenCode 1.x (1.18.29+) dual-shape compatibility.
 */
const RtkOpenCodePlugin = {
  id: "rtk",

  // OpenCode 2.x entrypoint
  async setup(ctx: any) {
    if (!warnMissingRtk()) return
    await ctx?.tool?.hook?.("execute.before", (e: any) => handleToolHook(e?.tool, e?.input, "command"))
  },

  // OpenCode 1.x entrypoint (supported via dual-shape in 1.18.29+)
  async server() {
    if (!warnMissingRtk()) return {}
    return {
      "tool.execute.before": (input: any, output: any) => handleToolHook(input?.tool, output?.args, "command"),
    }
  },
}

export default RtkOpenCodePlugin
export { RtkOpenCodePlugin }