import type { Plugin } from "@opencode-ai/plugin"
import { spawnSync } from "node:child_process"
import { appendFileSync, existsSync, mkdirSync } from "node:fs"
import { dirname, join } from "node:path"

const RTK_CANDIDATES = [
  `${process.env.HOME ?? ""}/.cargo/bin/rtk`,
  "/usr/local/bin/rtk",
  "/opt/homebrew/bin/rtk",
]

const DEBUG_ENABLED = process.env.RTK_OPENCODE_DEBUG === "1"
const DEBUG_FILE = process.env.RTK_OPENCODE_DEBUG_FILE || join(process.env.TMPDIR || "/tmp", "rtk-opencode-debug.log")

type CommandAccessor = {
  field: string
  get: () => string
  set: (value: string) => void
}

function writeDebug(event: string, details: Record<string, string | null> = {}): void {
  if (!DEBUG_ENABLED) {
    return
  }

  try {
    mkdirSync(dirname(DEBUG_FILE), { recursive: true })
    appendFileSync(
      DEBUG_FILE,
      JSON.stringify({
        event,
        ...details,
        ts: new Date().toISOString(),
      }) + "\n",
      "utf8",
    )
  } catch {
    return
  }
}

function findRtk(): string | null {
  for (const candidate of RTK_CANDIDATES) {
    if (candidate && existsSync(candidate)) {
      writeDebug("rtk-candidate", { candidate })
      return candidate
    }
  }

  writeDebug("rtk-candidate", { candidate: null })
  return null
}

function getCommandAccessor(output: { args?: Record<string, unknown> }): CommandAccessor | null {
  if (typeof output.args?.command === "string" && output.args.command.length > 0) {
    return {
      field: "output.args.command",
      get: () => output.args!.command as string,
      set: (value: string) => {
        output.args!.command = value
      },
    }
  }

  if (typeof output.args?.cmd === "string" && output.args.cmd.length > 0) {
    return {
      field: "output.args.cmd",
      get: () => output.args!.cmd as string,
      set: (value: string) => {
        output.args!.cmd = value
      },
    }
  }

  const nestedArgvCommand = output.args?.argv?.command
  if (
    typeof output.args?.argv === "object" &&
    output.args?.argv !== null &&
    typeof nestedArgvCommand === "string" &&
    nestedArgvCommand.length > 0
  ) {
    return {
      field: "output.args.argv.command",
      get: () => (output.args!.argv as { command: string }).command,
      set: (value: string) => {
        ;(output.args!.argv as { command: string }).command = value
      },
    }
  }

  const nestedBashCommand = output.args?.bash?.command
  if (
    typeof output.args?.bash === "object" &&
    output.args?.bash !== null &&
    typeof nestedBashCommand === "string" &&
    nestedBashCommand.length > 0
  ) {
    return {
      field: "output.args.bash.command",
      get: () => (output.args!.bash as { command: string }).command,
      set: (value: string) => {
        ;(output.args!.bash as { command: string }).command = value
      },
    }
  }

  return null
}

function setCommandValue(accessor: CommandAccessor, value: string): boolean {
  if (!value) {
    return false
  }

  accessor.set(value)
  return true
}

export const RtkRewritePlugin: Plugin = async () => {
  writeDebug("plugin-loaded")

  return {
    "tool.execute.before": async (input, output) => {
      writeDebug("incoming-tool", { tool: input.tool })

      if (input.tool === "bash") {
      } else {
        return
      }

      const accessor = getCommandAccessor(output)
      if (!accessor) {
        writeDebug("command-field", { field: "unsupported-command-shape" })
        return
      }

      writeDebug("command-field", { field: accessor.field })

      const original = accessor.get()
      if (!original) {
        writeDebug("rewrite-result", { outcome: "rewrite-noop", reason: "empty-command" })
        return
      }

      const rtkBin = findRtk()
      if (!rtkBin) {
        writeDebug("rewrite-result", { outcome: "rewrite-noop", reason: "rtk-missing" })
        return
      }

      try {
        const result = spawnSync(rtkBin, ["rewrite", original], {
          encoding: "utf8",
          timeout: 5000,
        })

        if (result.error || result.signal || result.status !== 0) {
          writeDebug("rewrite-result", {
            outcome: "rewrite-error",
            reason: result.error?.message || result.signal || String(result.status),
          })
          return
        }

        const rewritten = result.stdout.trimEnd()
        if (!rewritten || rewritten === original) {
          writeDebug("rewrite-result", { outcome: "rewrite-noop", reason: "unchanged" })
          return
        }

        if (!setCommandValue(accessor, rewritten)) {
          writeDebug("rewrite-result", { outcome: "rewrite-error", reason: "set-failed" })
          return
        }

        writeDebug("rewrite-result", { outcome: "rewritten", field: accessor.field })
      } catch (error) {
        writeDebug("rewrite-result", {
          outcome: "rewrite-error",
          reason: error instanceof Error ? error.message : "unknown",
        })
        return
      }
    },
  }
}

export default RtkRewritePlugin
