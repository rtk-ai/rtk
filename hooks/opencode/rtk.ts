import type { Plugin } from "@opencode-ai/plugin"
import { fileURLToPath } from "node:url"
import { existsSync } from "node:fs"
import { join } from "node:path"
import { homedir } from "node:os"

const IS_WINDOWS = process.platform === "win32"
const RTK_EXE = IS_WINDOWS ? "rtk.exe" : "rtk"

async function resolveRtkPath(): Promise<string | null> {
  const candidates: string[] = []

  if (process.env.RTK_BIN) {
    candidates.push(process.env.RTK_BIN)
  }

  const home = homedir()
  candidates.push(
    join(home, ".cargo", "bin", RTK_EXE),
    "/opt/homebrew/bin/rtk",
    "/usr/local/bin/rtk",
    "/usr/bin/rtk"
  )

  if (IS_WINDOWS) {
    const localAppData = process.env.LOCALAPPDATA
    const programFiles = process.env.ProgramFiles
    const programFilesX86 = process.env["ProgramFiles(x86)"]
    if (localAppData) candidates.push(join(localAppData, "cargo", "bin", RTK_EXE))
    if (programFiles) candidates.push(join(programFiles, "rtk", RTK_EXE))
    if (programFilesX86) candidates.push(join(programFilesX86, "rtk", RTK_EXE))
  }

  try {
    const { stdout } = await $`which rtk`.quiet().nothrow()
    const p = stdout.trim()
    if (p) candidates.unshift(p)
  } catch {}

  for (const c of candidates) {
    if (existsSync(c)) return c
  }

  try {
    const importPath = fileURLToPath(import.meta.url)
    const pluginDir = importPath.split("/").slice(0, -1).join("/")
    const bundled = join(pluginDir, "..", "..", "..", "target", "release", RTK_EXE)
    if (existsSync(bundled)) return bundled
  } catch {}

  return null
}

type Answer = { command?: string; bin_path?: string }

export const RtkOpenCodePlugin: Plugin = async ({ $ }) => {
  const rtkPath = await resolveRtkPath()
  if (!rtkPath) {
    console.warn("[rtk] rtk binary not found (checked PATH, RTK_BIN, ~/.cargo/bin, Homebrew, /usr/local/bin) — plugin disabled")
    return {}
  }

  return {
    "tool.execute.before": async (input, output) => {
      const tool = String(input?.tool ?? "").toLowerCase()
      if (tool !== "bash" && tool !== "shell") return
      const args = output?.args
      if (!args || typeof args !== "object") return

      const command = (args as Record<string, unknown>).command
      if (typeof command !== "string" || !command) return

      try {
        const result = await $`${rtkPath} hook opencode ${command} --bin ${rtkPath}`.quiet().nothrow()
        const answer = JSON.parse(String(result.stdout).trim() || "{}") as Answer
        if (answer.command && answer.command !== command) {
          ;(args as Record<string, unknown>).command = answer.command
        }
      } catch {
        // rtk hook opencode failed or answered nothing — pass through unchanged
      }
    },
  }
}