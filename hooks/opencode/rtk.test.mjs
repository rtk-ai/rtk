import test, { after, before } from "node:test"
import assert from "node:assert/strict"
import { copyFileSync, mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, join } from "node:path"
import RtkOpenCodePlugin, {
  probeRtkHookOpencode,
  resolveRtkPath,
  runHookOpencode,
  tryRewriteCommand,
  _resetCachedRtkPath,
} from "./rtk.ts"

// The plugin resolves rtk off PATH and runs it, so the tests need a real rtk
// to find. node is copied in under the name `rtk`, and because the plugin
// spawns `[<rtk>, "hook", "opencode", …]` — node reading argv[2] as the script
// to run — the mock script is named `hook` and the tests chdir into the mock
// directory so it is found there instead of over whatever sits in the repo.
// Everything below then exercises the shipped code path: PATH discovery, the
// `hook opencode --help` probe, argv construction, and the registered hooks.
const BIN = process.platform === "win32" ? "rtk.EXE" : "rtk"
let mockDir = ""
let emptyDir = ""
let savedPath = ""
let savedCwd = ""

const MOCK = `
const fs = require("fs")
const [sub, ...rest] = process.argv.slice(2)

if (rest[0] === "--help") {
  if (process.env.MOCK_PROBE_LOG) fs.appendFileSync(process.env.MOCK_PROBE_LOG, "probe\\n")
  // pre-#4349 rtk has no such subcommand and answers like clap does: exit 2
  process.exit(process.env.MOCK_NO_SUB ? 2 : 0)
}
if (sub !== "opencode") process.exit(3)

const agent = rest[0] === "--agent" ? rest[1] : undefined
const command = agent ? rest[2] : rest[0]

if (process.env.MOCK_ECHO) {
  console.log(JSON.stringify({ command: JSON.stringify(process.argv.slice(2)) }))
} else if (command === "ls -la") {
  console.log(JSON.stringify({ command: "rtk ls -la" }))
} else if (command === "unchanged") {
  // what the Rust side answers when the rewrite would change the verdict
  console.log("{}")
} else if (command === "garbage") {
  console.log("not json at all")
} else if (command === "empty") {
  process.exit(0)
} else if (command === "fail") {
  process.exit(2)
} else if (command === "sleep") {
  setTimeout(() => process.exit(0), 5000)
}
`

const mockBin = () => join(mockDir, BIN)

// The event shapes OpenCode actually sends, per KuSh's run against 2.0.22 and
// 1.18.34. A plugin that reads the wrong property rewrites nothing, and an
// event object that carries the wrong shape hides that.
const v2Event = (command) => ({
  tool: "shell",
  sessionID: "ses_1",
  agent: "build",
  messageID: "msg_1",
  id: "call_1",
  input: { command },
})
const v1Input = { tool: "bash", sessionID: "ses_1", callID: "call_1" }
const v1Output = (command) => ({ args: { command, description: "list files" } })

async function captureSetup() {
  const names = []
  let hook = null
  await RtkOpenCodePlugin.setup({
    tool: {
      hook(name, callback) {
        names.push(name)
        if (name === "execute.before") hook = callback
      },
    },
  })
  return { names, hook }
}

async function captureServer() {
  const hooks = await RtkOpenCodePlugin.server()
  return { names: Object.keys(hooks), hooks }
}

before(() => {
  savedCwd = process.cwd()
  savedPath = process.env.PATH ?? ""
  mockDir = mkdtempSync(join(tmpdir(), "rtk-opencode-mock-"))
  emptyDir = mkdtempSync(join(tmpdir(), "rtk-opencode-empty-"))
  writeFileSync(join(mockDir, "package.json"), '{"type":"commonjs"}')
  writeFileSync(join(mockDir, "hook"), MOCK)
  copyFileSync(process.execPath, mockBin())
  process.chdir(mockDir)
  process.env.PATH = `${mockDir}${delimiter}${savedPath}`
  _resetCachedRtkPath()
})

after(() => {
  process.chdir(savedCwd)
  process.env.PATH = savedPath
  delete process.env.MOCK_ECHO
  delete process.env.MOCK_NO_SUB
  delete process.env.MOCK_PROBE_LOG
  delete process.env.RTK_DISABLED
  _resetCachedRtkPath()
  rmSync(mockDir, { recursive: true, force: true })
  rmSync(emptyDir, { recursive: true, force: true })
})

test("2.0.22: execute.before mutates event.input.command", async () => {
  assert.equal(RtkOpenCodePlugin.id, "rtk")
  const { names, hook } = await captureSetup()
  assert.deepEqual(names, ["execute.before"])
  assert.equal(typeof hook, "function")

  const rewritten = v2Event("ls -la")
  await hook(rewritten)
  assert.equal(rewritten.input.command, "rtk ls -la")

  // `{}` means "run it as typed". Assigning anyway would blank the command.
  const kept = v2Event("unchanged")
  await hook(kept)
  assert.equal(kept.input.command, "unchanged")
})

test("1.18.34: tool.execute.before mutates output.args.command", async () => {
  const { names, hooks } = await captureServer()
  assert.deepEqual(names, ["tool.execute.before"])
  assert.equal(typeof hooks["tool.execute.before"], "function")

  const rewritten = v1Output("ls -la")
  await hooks["tool.execute.before"](v1Input, rewritten)
  assert.equal(rewritten.args.command, "rtk ls -la")
  assert.equal(rewritten.args.description, "list files")

  const kept = v1Output("unchanged")
  await hooks["tool.execute.before"](v1Input, kept)
  assert.equal(kept.args.command, "unchanged")
})

test("argv is exact and shell-free, and the agent is forwarded", async () => {
  process.env.MOCK_ECHO = "1"
  try {
    const { hook } = await captureSetup()
    const withAgent = v2Event("ls -la && rm -rf /")
    await hook(withAgent)
    // `agent` is OpenCode 2.x's event.agent, which rtk applies as
    // agent.<name>.permission — without it an agent-scoped ask on `ls *` gets
    // silenced and an agent deny on `ls -la` never sees the command.
    assert.equal(withAgent.input.command, '["opencode","--agent","build","ls -la && rm -rf /"]')

    const { hooks } = await captureServer()
    const noAgent = v1Output("ls -la && rm -rf /")
    // 1.x sends no agent field, so that path stays root-only.
    await hooks["tool.execute.before"](v1Input, noAgent)
    assert.equal(noAgent.args.command, '["opencode","ls -la && rm -rf /"]')
  } finally {
    delete process.env.MOCK_ECHO
  }
})

test("an rtk without `hook opencode` registers nothing", async () => {
  const warnings = []
  const realWarn = console.warn
  console.warn = (message) => warnings.push(String(message))
  try {
    // (a) nothing named rtk on PATH. OpenCode's shell tool runs with OpenCode's
    // PATH, so an rtk found elsewhere cannot be spawned as a bare `rtk …` —
    // reporting one would rewrite commands that come back `command not found`
    // (#4462).
    process.env.PATH = emptyDir
    _resetCachedRtkPath()
    assert.equal(resolveRtkPath(), null)
    assert.deepEqual(await captureSetup().then((c) => c.names), [])
    assert.deepEqual((await captureServer()).names, [])

    // (b) an rtk too old for the subcommand: `hook opencode --help` exits 2.
    process.env.PATH = `${mockDir}${delimiter}${savedPath}`
    process.env.MOCK_NO_SUB = "1"
    _resetCachedRtkPath()
    assert.equal(resolveRtkPath(), mockBin())
    assert.equal(await probeRtkHookOpencode(mockBin()), false)
    assert.deepEqual(await captureSetup().then((c) => c.names), [])
    assert.deepEqual((await captureServer()).names, [])
    assert.match(warnings.join("\n"), /no .*hook opencode. *subcommand/)
  } finally {
    delete process.env.MOCK_NO_SUB
    process.env.PATH = `${mockDir}${delimiter}${savedPath}`
    console.warn = realWarn
    _resetCachedRtkPath()
  }
})

test("the `hook opencode --help` probe is asked once per binary", async () => {
  const log = join(mockDir, "probes")
  writeFileSync(log, "")
  process.env.MOCK_PROBE_LOG = log
  _resetCachedRtkPath()
  try {
    assert.equal(await probeRtkHookOpencode(mockBin()), true)
    assert.equal(await probeRtkHookOpencode(mockBin()), true)
    assert.equal(readFileSync(log, "utf8").trim().split("\n").length, 1)
  } finally {
    delete process.env.MOCK_PROBE_LOG
    _resetCachedRtkPath()
  }
})

test("RTK_DISABLED=1 passes commands through with a live rtk behind it", async () => {
  delete process.env.RTK_DISABLED
  const { hooks } = await captureServer()
  const live = v1Output("ls -la")
  await hooks["tool.execute.before"](v1Input, live)
  assert.equal(live.args.command, "rtk ls -la")

  try {
    // Only the exact value "1" disables it. "0" and "true" are not the
    // documented form, and honouring them would silently kill the plugin for
    // anyone who exports RTK_DISABLED=true in .envrc.
    for (const value of ["0", "true"]) {
      process.env.RTK_DISABLED = value
      const probed = v1Output("ls -la")
      await hooks["tool.execute.before"](v1Input, probed)
      assert.equal(probed.args.command, "rtk ls -la")
    }

    process.env.RTK_DISABLED = "1"
    const off = v1Output("ls -la")
    await hooks["tool.execute.before"](v1Input, off)
    assert.equal(off.args.command, "ls -la")
  } finally {
    delete process.env.RTK_DISABLED
  }
})

test("resolveRtkPath skips a directory named rtk", () => {
  const decoy = mkdtempSync(join(tmpdir(), "rtk-opencode-decoy-"))
  mkdirSync(join(decoy, BIN))
  process.env.PATH = `${decoy}${delimiter}${mockDir}${delimiter}${savedPath}`
  _resetCachedRtkPath()
  try {
    // execFile on a directory fails, which would disable the plugin for the
    // whole session.
    assert.equal(resolveRtkPath(), mockBin())
  } finally {
    process.env.PATH = `${mockDir}${delimiter}${savedPath}`
    _resetCachedRtkPath()
    rmSync(decoy, { recursive: true, force: true })
  }
})

test("a non-string command never reaches the binary", async () => {
  let touched = false
  const sneaky = {
    toString() {
      touched = true
      return "ls -la"
    },
  }
  // The guard has to reject on type, before anything stringifies the value.
  assert.equal(await tryRewriteCommand("shell", sneaky), null)
  assert.equal(touched, false)
})

test("tryRewriteCommand guards the tool name and the command shape", async () => {
  assert.equal(await tryRewriteCommand("read", "ls -la"), null)
  assert.equal(await tryRewriteCommand("BASH", 42), null)
  assert.equal(await tryRewriteCommand("shell", "   "), null)
  // lowercased, so the real tool names match
  assert.equal(await tryRewriteCommand("SHELL", "ls -la"), "rtk ls -la")
})

test("runHookOpencode fails open on anything unusable", async () => {
  assert.equal(await runHookOpencode(mockBin(), "garbage"), null)
  assert.equal(await runHookOpencode(mockBin(), "empty"), null)
  assert.equal(await runHookOpencode(mockBin(), "fail"), null)
  assert.equal(await runHookOpencode(mockBin(), "sleep", undefined, 100), null)
})
