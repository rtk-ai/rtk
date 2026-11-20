import type { Plugin, PluginModule } from "@kilocode/plugin"
import { execFile } from "node:child_process"
import { basename, delimiter, dirname, isAbsolute, join, resolve } from "node:path"
import { appendFileSync } from "node:fs"
import { tmpdir } from "node:os"

const MACHINE_READABLE_FLAGS =
  /(?:^|\s)(?:--(?:json|porcelain|output|format|template)(?:=[^\s]*)?|-[zZ0])(?=\s|$)/

// `rtk rewrite` can transform these modes, but their callers expect exact output.

const DEBUG = process.env.RTK_KILO_BASH_DEBUG === "1"
const TAG = "[rtk]"
const PROCESS_TIMEOUT_MS = 5000

type ProcessResult = {
  exitCode: number | null
  stdout: string
}

function runRtk(rtkBin: string, args: string[]): Promise<ProcessResult> {
  return new Promise((resolve) => {
    execFile(
      rtkBin,
      args,
      {
        encoding: "utf8",
        maxBuffer: 64 * 1024,
        timeout: PROCESS_TIMEOUT_MS,
        windowsHide: true,
      },
      (error, stdout) => {
        resolve({
          exitCode:
            typeof (error as NodeJS.ErrnoException | null)?.code === "number"
              ? (error as NodeJS.ErrnoException & { code: number }).code
              : error
                ? null
                : 0,
          stdout: String(stdout ?? ""),
        })
      },
    )
  })
}

async function findRtk(): Promise<string | null> {
  const home =
    process.platform === "win32"
      ? (process.env.USERPROFILE ?? process.env.HOME ?? "")
      : (process.env.HOME ?? process.env.USERPROFILE ?? "")
  const configuredBin = process.env.RTK_KILO_BIN?.trim()

  const candidates = [
    ...(configuredBin ? [configuredBin] : []),
    ...(process.platform === "win32"
      ? [
          ...(home
            ? [
                join(home, ".local/bin/rtk.exe"),
                join(home, ".cargo/bin/rtk.exe"),
                join(home, "scoop/shims/rtk.exe"),
              ]
            : []),
          "rtk.exe",
        ]
      : [
          ...(home
            ? [join(home, ".local/bin/rtk"), join(home, ".cargo/bin/rtk")]
            : []),
          "/home/linuxbrew/.linuxbrew/bin/rtk",
          "/opt/homebrew/bin/rtk",
          "/usr/local/bin/rtk",
          "/usr/bin/rtk",
          "rtk",
        ]),
  ]

  for (const candidate of new Set(candidates)) {
    const explicitPath = isAbsolute(candidate) || candidate.includes("/") ||
      (process.platform === "win32" && candidate.includes("\\"))
    const paths = explicitPath
      ? [resolve(candidate)]
      : (process.env.PATH ?? "").split(delimiter).filter(Boolean).map((dir) => resolve(dir, candidate))
    for (const path of new Set(paths)) {
      const check = await runRtk(path, ["--version"])
      if (check.exitCode === 0) return path
    }
  }
  return null
}

async function rtkRewrite(rtkBin: string, command: string): Promise<string | null> {
  const result = await runRtk(rtkBin, ["rewrite", command])
  if (result.exitCode !== 0 && result.exitCode !== 3) return null

  const rewritten = result.stdout.trim()
  if (rewritten && rewritten !== command) {
    return rewritten
  }
  return null
}

function writeDebug(logPath: string, message: string): void {
  if (!DEBUG) return
  try {
    appendFileSync(logPath, `${message}\n`)
  } catch (error) {
    console.warn(`${TAG} could not write debug log: ${String(error)}`)
  }
}

const rtkPlugin: Plugin = async () => {
  if (process.env.RTK_KILO_BASH_PLUGIN === "0") return {}

  const rtkBin = await findRtk()
  if (!rtkBin) {
    if (DEBUG) console.log(`${TAG} RTK not found — plugin disabled (${process.platform})`)
    return {}
  }
  const binDir = dirname(rtkBin)
  const validName = process.platform === "win32"
    ? basename(rtkBin).toLowerCase() === "rtk.exe"
    : basename(rtkBin) === "rtk"
  if (binDir.includes(delimiter) || !validName) {
    if (DEBUG) console.log(`${TAG} RTK path cannot supply bare rtk commands — plugin disabled`)
    return {}
  }

  // Per-process filename: a fixed name in a world-writable tmpdir could be
  // pre-planted as a symlink by another local user when debug mode is enabled.
  const logPath = join(
    tmpdir(),
    `rtk-plugin-debug-${process.pid}.log`,
  )

  if (DEBUG) console.log(`${TAG} RTK found at ${rtkBin} — plugin active`)

  return {
    "shell.env": async (_input, output) => {
      const pathKeys = process.platform === "win32"
        ? Object.keys(output.env).filter((key) => key.toUpperCase() === "PATH").sort()
        : ["PATH"]
      const currentPath = output.env[pathKeys[0] ?? "PATH"] ?? process.env.PATH ?? ""
      const entries = currentPath ? currentPath.split(delimiter) : []
      // Rewritten commands use bare `rtk`, including inside chains and pipelines,
      // so its directory only has to be reachable. Never reorder PATH: moving a
      // shared directory (/usr/bin, ~/.local/bin) to the front would let its other
      // executables shadow virtual environments and version-manager shims.
      if (entries.includes(binDir)) return
      // Windows treats PATH keys case-insensitively; emit just one override.
      for (const key of pathKeys) delete output.env[key]
      output.env.PATH = [...entries, binDir].join(delimiter)
    },
    "tool.execute.before": async (input, output) => {
      const tool = String(input?.tool ?? "").toLowerCase()
      writeDebug(
        logPath,
        `direct-hook: tool=${tool} argsKeys=${JSON.stringify(Object.keys(output?.args ?? {}))}`,
      )
      if (tool !== "bash" && tool !== "shell") return

      const args = output?.args
      if (!args || typeof args !== "object") return

      const commandArgs = args as Record<string, unknown>
      const command = commandArgs.command
      if (typeof command !== "string" || !command) return
      if (MACHINE_READABLE_FLAGS.test(command)) return

      const rewritten = await rtkRewrite(rtkBin, command)
      if (rewritten) {
        commandArgs.command = rewritten
      }
    },
  }
}

const plugin: PluginModule = { id: "rtk", server: rtkPlugin }

export default plugin
