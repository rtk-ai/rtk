import { describe, it, beforeEach, afterEach } from "node:test"
import assert from "node:assert/strict"
import { writeFileSync, chmodSync, mkdtempSync, rmSync } from "node:fs"
import { join } from "node:path"
import { tmpdir } from "node:os"

// Minimal mock types matching the OpenCode plugin interface
type PluginInput = { tool: string; sessionID?: string; callID?: string }
type PluginOutput = { args: Record<string, unknown> }
type Hooks = { "tool.execute.before"?: (input: PluginInput, output: PluginOutput) => Promise<void> }

const { RtkOpenCodePlugin } = await import("../rtk.ts")

// Create a mock $ that simulates Bun.$ behavior.
// Bun.$ flattens arrays into space-separated args in template literals.
function mock$(rewrites: Record<string, string>) {
  return (strings: TemplateStringsArray, ...values: any[]) => {
    const parts: string[] = []
    for (let i = 0; i < strings.length; i++) {
      parts.push(strings[i])
      if (i < values.length) {
        const v = values[i]
        parts.push(Array.isArray(v) ? v.join(" ") : String(v))
      }
    }
    const cmd = parts.join("").trim()

    const result = (stdout: string) => ({
      stdout: Buffer.from(stdout),
      exitCode: 0,
    })

    return {
      quiet: () => {
        if (cmd.includes("which rtk")) return Promise.resolve(result("/usr/bin/rtk"))
        const prefix = "rtk rewrite "
        if (cmd.startsWith(prefix)) {
          const original = cmd.slice(prefix.length)
          const rewritten = rewrites[original] ?? original
          return { nothrow: () => Promise.resolve(result(rewritten)) }
        }
        return Promise.resolve(result(""))
      },
    }
  }
}

// Create a fake rtk binary that simulates `rtk rewrite` behavior.
function createFakeRtk(rewrites: Record<string, string>): { dir: string; cleanup: () => void } {
  const dir = mkdtempSync(join(tmpdir(), "rtk-test-"))
  const cases = Object.entries(rewrites)
    .map(([k, v]) => `  "${k}") echo "${v}" ;;`)
    .join("\n")
  const script = `#!/bin/sh
if [ "$1" = "rewrite" ]; then
  shift
  CMD="$*"
  case "$CMD" in
${cases}
  *) echo "$CMD" ;;
  esac
  exit 0
fi
exit 1
`
  const bin = join(dir, "rtk")
  writeFileSync(bin, script)
  chmodSync(bin, 0o755)
  return { dir, cleanup: () => rmSync(dir, { recursive: true, force: true }) }
}

describe("RtkOpenCodePlugin", () => {
  describe("initialization with $ (Bun path)", () => {
    it("enables when $ finds rtk", async () => {
      const $ = mock$({})
      const hooks = (await RtkOpenCodePlugin({ $, project: {}, directory: "/tmp", worktree: "/tmp" })) as Hooks
      assert.ok(hooks["tool.execute.before"])
    })

    it("disables when $ cannot find rtk", async () => {
      const $ = (_strings: TemplateStringsArray, ..._args: any[]) => ({
        quiet: () => Promise.reject(new Error("command not found: rtk")),
      })
      const hooks = (await RtkOpenCodePlugin({ $, project: {}, directory: "/tmp", worktree: "/tmp" })) as Hooks
      assert.equal(hooks["tool.execute.before"], undefined)
    })
  })

  describe("initialization with $ = undefined (Node.js/Desktop path)", () => {
    it("enables when fake rtk is in PATH", async () => {
      const fake = createFakeRtk({})
      const origPath = process.env.PATH
      try {
        process.env.PATH = `${fake.dir}:${origPath}`
        const hooks = (await RtkOpenCodePlugin({ $: undefined, project: {}, directory: "/tmp", worktree: "/tmp" })) as Hooks
        assert.ok(hooks["tool.execute.before"])
      } finally {
        process.env.PATH = origPath
        fake.cleanup()
      }
    })

    it("disables when rtk is not in PATH", async () => {
      const origPath = process.env.PATH
      try {
        process.env.PATH = "/nonexistent"
        const hooks = (await RtkOpenCodePlugin({ $: undefined, project: {}, directory: "/tmp", worktree: "/tmp" })) as Hooks
        assert.equal(hooks["tool.execute.before"], undefined)
      } finally {
        process.env.PATH = origPath
      }
    })
  })

  describe("tool.execute.before hook via $ (Bun path)", () => {
    let hook: (input: PluginInput, output: PluginOutput) => Promise<void>

    beforeEach(async () => {
      const $ = mock$({
        "git status": "rtk git status",
        "git log -n 5": "rtk git log -n 5",
        "ls .": "rtk ls .",
      })
      const hooks = (await RtkOpenCodePlugin({ $, project: {}, directory: "/tmp", worktree: "/tmp" })) as Hooks
      hook = hooks["tool.execute.before"]!
    })

    it("rewrites bash tool commands", async () => {
      const output = { args: { command: "git status" } }
      await hook({ tool: "bash" }, output)
      assert.equal(output.args.command, "rtk git status")
    })

    it("rewrites shell tool commands", async () => {
      const output = { args: { command: "git log -n 5" } }
      await hook({ tool: "shell" }, output)
      assert.equal(output.args.command, "rtk git log -n 5")
    })

    it("is case-insensitive on tool name", async () => {
      const output = { args: { command: "ls ." } }
      await hook({ tool: "Bash" }, output)
      assert.equal(output.args.command, "rtk ls .")
    })

    it("ignores non-bash tools", async () => {
      const output = { args: { command: "git status" } }
      await hook({ tool: "read" }, output)
      assert.equal(output.args.command, "git status")
    })

    it("passes through commands with no rewrite", async () => {
      const output = { args: { command: "echo hello" } }
      await hook({ tool: "bash" }, output)
      assert.equal(output.args.command, "echo hello")
    })

    it("handles missing command gracefully", async () => {
      const output = { args: {} } as any
      await hook({ tool: "bash" }, output)
    })

    it("handles undefined args gracefully", async () => {
      const output = { args: undefined } as any
      await hook({ tool: "bash" }, output)
    })

    it("handles non-string command gracefully", async () => {
      const output = { args: { command: 123 } } as any
      await hook({ tool: "bash" }, output)
      assert.equal(output.args.command, 123)
    })
  })

  describe("tool.execute.before hook via child_process (Desktop path)", () => {
    let hook: (input: PluginInput, output: PluginOutput) => Promise<void>
    let fake: { dir: string; cleanup: () => void }
    let origPath: string | undefined

    beforeEach(async () => {
      fake = createFakeRtk({ "git status": "rtk git status", "cargo test": "rtk cargo test" })
      origPath = process.env.PATH
      process.env.PATH = `${fake.dir}:${origPath}`
      const hooks = (await RtkOpenCodePlugin({ $: undefined, project: {}, directory: "/tmp", worktree: "/tmp" })) as Hooks
      hook = hooks["tool.execute.before"]!
    })

    afterEach(() => {
      process.env.PATH = origPath
      fake.cleanup()
    })

    it("rewrites commands via child_process", async () => {
      const output = { args: { command: "git status" } }
      await hook({ tool: "bash" }, output)
      assert.equal(output.args.command, "rtk git status")
    })

    it("passes through unknown commands", async () => {
      const output = { args: { command: "echo hello" } }
      await hook({ tool: "bash" }, output)
      assert.equal(output.args.command, "echo hello")
    })
  })
})
