import type { Plugin } from "@opencode-ai/plugin"
import { spawnSync } from "node:child_process"
import { existsSync } from "node:fs"

const RTK_CANDIDATES = [
  `${process.env.HOME ?? ""}/.cargo/bin/rtk`,
  "/usr/local/bin/rtk",
  "/opt/homebrew/bin/rtk",
]

function findRtk(): string | null {
  for (const candidate of RTK_CANDIDATES) {
    if (candidate && existsSync(candidate)) {
      return candidate
    }
  }

  return null
}

const RtkRewritePlugin: Plugin = async () => {
  return {
    "tool.execute.before": async (input, output) => {
      if (input.tool === "bash") {
        const original = output.args?.command
        if (typeof original !== "string" || original.length === 0) {
          return
        }

        const rtkBin = findRtk()
        if (!rtkBin) {
          return
        }

        const result = spawnSync(rtkBin, ["rewrite", original], {
          encoding: "utf8",
          timeout: 5000,
        })

        if (result.error || result.signal || result.status !== 0) {
          return
        }

        const rewritten = result.stdout.trimEnd()
        if (!rewritten || rewritten === original) {
          return
        }

        output.args.command = rewritten
      } else {
        return
      }
    },
  }
}

export default RtkRewritePlugin
