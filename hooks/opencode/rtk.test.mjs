import test, { after, before } from "node:test"
import assert from "node:assert/strict"
import { execSync } from "node:child_process"
import { copyFileSync, existsSync, mkdirSync, mkdtempSync, readFileSync, rmSync, symlinkSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, dirname, join } from "node:path"
import RtkOpenCodePlugin, {
  candidateRtkDirs,
  probeRtkHookOpencode,
  resolveRtkPath,
  runHookOpencode,
  tryRewriteCommand,
  _resetCachedRtkPath,
} from "./rtk.ts"

// The plugin resolves rtk off PATH or candidate directories and runs it, so
// the tests need a real rtk to find. Node is symlinked/copied in under the name
// `rtk`, and because the plugin spawns `[<rtk>, "hook", "opencode", …]` — node
// reading argv[2] as the script to run — the mock script is named `hook` and the
// tests chdir into the mock directory so it is found there instead of over
// whatever sits in the repo.
// Everything below then exercises the shipped code path: PATH discovery, the
// `hook opencode --help` probe, argv construction, and the registered hooks.
const BIN = process.platform === "win32" ? "rtk.EXE" : "rtk"
let mockDir = ""
let emptyDir = ""
let savedPath = ""
let savedHome = ""
let savedUserProfile = ""
let savedCwd = ""

function installMockBin(dest) {
  try {
    symlinkSync(process.execPath, dest)
  } catch {
    copyFileSync(process.execPath, dest)
  }
}

const MOCK = `
const fs = require("fs")
const [sub, ...rest] = process.argv.slice(2)

if (rest[0] === "--help") {
  // marks a run from a shell command, so a test can count which rtk ran
  console.log("rtk-mock")
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
} else if (command === "cd sub && ls -la") {
  console.log(JSON.stringify({ command: "cd sub && rtk ls -la" }))
} else if (command === "ls -la && git status") {
  console.log(JSON.stringify({ command: "rtk ls -la && rtk git status" }))
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
  const toolHooks = []
  const shellHooks = []
  let toolHook = null
  let shellHook = null
  await RtkOpenCodePlugin.setup({
    tool: {
      hook(name, callback) {
        toolHooks.push(name)
        if (name === "execute.before") toolHook = callback
      },
    },
    shell: {
      hook(name, callback) {
        shellHooks.push(name)
        if (name === "create.before") shellHook = callback
      },
    },
  })
  return { names: toolHooks, hook: toolHook, toolHooks, shellHooks, shellHook }
}

async function captureServer() {
  const hooks = await RtkOpenCodePlugin.server()
  return { names: Object.keys(hooks), hooks }
}

before(() => {
  savedCwd = process.cwd()
  savedPath = process.env.PATH ?? ""
  savedHome = process.env.HOME ?? ""
  savedUserProfile = process.env.USERPROFILE ?? ""
  mockDir = mkdtempSync(join(tmpdir(), "rtk-opencode-mock-"))
  emptyDir = mkdtempSync(join(tmpdir(), "rtk-opencode-empty-"))
  writeFileSync(join(mockDir, "package.json"), '{"type":"commonjs"}')
  writeFileSync(join(mockDir, "hook"), MOCK)
  installMockBin(mockBin())
  process.chdir(mockDir)
  process.env.PATH = `${mockDir}${delimiter}${savedPath}`
  process.env.HOME = emptyDir
  process.env.USERPROFILE = emptyDir
  delete process.env.RTK_BIN
  _resetCachedRtkPath()
})

after(() => {
  process.chdir(savedCwd)
  process.env.PATH = savedPath
  process.env.HOME = savedHome
  process.env.USERPROFILE = savedUserProfile
  delete process.env.MOCK_ECHO
  delete process.env.MOCK_NO_SUB
  delete process.env.MOCK_PROBE_LOG
  delete process.env.RTK_DISABLED
  delete process.env.RTK_BIN
  _resetCachedRtkPath()
  rmSync(mockDir, { recursive: true, force: true })
  rmSync(emptyDir, { recursive: true, force: true })
})

test("2.0.22: execute.before mutates event.input.command and create.before appends rtk dir to PATH", async () => {
  assert.equal(RtkOpenCodePlugin.id, "rtk")
  const { names, hook, shellHooks, shellHook } = await captureSetup()
  assert.deepEqual(names, ["execute.before"])
  assert.equal(typeof hook, "function")
  assert.deepEqual(shellHooks, ["create.before"])
  assert.equal(typeof shellHook, "function")

  const shellEvent = { env: { PATH: "/usr/bin" } }
  shellHook(shellEvent)
  assert.equal(shellEvent.env.PATH, `/usr/bin${delimiter}${mockDir}`)

  const rewritten = v2Event("ls -la")
  await hook(rewritten)
  assert.equal(rewritten.input.command, "rtk ls -la")

  // `{}` means "run it as typed". Assigning anyway would blank the command.
  const kept = v2Event("unchanged")
  await hook(kept)
  assert.equal(kept.input.command, "unchanged")
})

test("1.18.34: tool.execute.before mutates output.args.command and shell.env appends rtk dir to PATH", async () => {
  const { names, hooks } = await captureServer()
  assert.ok(names.includes("tool.execute.before"))
  assert.ok(names.includes("shell.env"))
  assert.equal(typeof hooks["tool.execute.before"], "function")
  assert.equal(typeof hooks["shell.env"], "function")

  const shellOut = { env: { PATH: "/usr/bin" } }
  hooks["shell.env"]({}, shellOut)
  assert.equal(shellOut.env.PATH, `/usr/bin${delimiter}${mockDir}`)

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

test("an rtk without `hook opencode` registers nothing", async (t) => {
  const warnings = []
  const realWarn = console.warn
  console.warn = (message) => warnings.push(String(message))
  try {
    // (a) nothing named rtk on PATH, RTK_BIN, or candidate directories.
    // Isolated HOME ensures developer's ~/.cargo/bin is not picked up.
    process.env.PATH = emptyDir
    process.env.HOME = emptyDir
    process.env.USERPROFILE = emptyDir
    delete process.env.RTK_BIN
    _resetCachedRtkPath()
    // The system-wide candidates (Homebrew, /usr/local/bin, …) cannot be
    // redirected; a machine with rtk installed there has nothing to assert here.
    const systemRtk = candidateRtkDirs().map((d) => join(d, BIN)).find((p) => existsSync(p))
    if (systemRtk) {
      t.diagnostic(`(a) skipped: ${systemRtk} exists`)
    } else {
      assert.equal(resolveRtkPath(), null)
      assert.deepEqual(await captureSetup().then((c) => c.names), [])
      assert.deepEqual(await captureSetup().then((c) => c.shellHooks), [])
      assert.deepEqual((await captureServer()).names, [])
    }

    // (b) an rtk too old for the subcommand: `hook opencode --help` exits 2.
    process.env.PATH = `${mockDir}${delimiter}${savedPath}`
    process.env.MOCK_NO_SUB = "1"
    _resetCachedRtkPath()
    assert.equal(resolveRtkPath(), mockBin())
    assert.equal(await probeRtkHookOpencode(mockBin()), false)
    assert.deepEqual(await captureSetup().then((c) => c.names), [])
    assert.deepEqual(await captureSetup().then((c) => c.shellHooks), [])
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

test("candidate probing and shell PATH injection for rtk outside PATH (#4462)", async () => {
  const customHome = mkdtempSync(join(tmpdir(), "rtk-opencode-home-"))
  const cargoBinDir = join(customHome, ".cargo", "bin")
  mkdirSync(cargoBinDir, { recursive: true })
  writeFileSync(join(cargoBinDir, "package.json"), '{"type":"commonjs"}')
  writeFileSync(join(cargoBinDir, "hook"), MOCK)
  const cargoRtk = join(cargoBinDir, BIN)
  installMockBin(cargoRtk)

  const customDir = mkdtempSync(join(tmpdir(), "rtk-opencode-custom-"))
  writeFileSync(join(customDir, "package.json"), '{"type":"commonjs"}')
  writeFileSync(join(customDir, "hook"), MOCK)
  const envRtk = join(customDir, BIN)
  installMockBin(envRtk)

  try {
    // 1. rtk only in $HOME/.cargo/bin (PATH empty):
    // Bare command, chained command, and directory cd all come back as rtk output
    process.env.PATH = emptyDir
    process.env.HOME = customHome
    process.env.USERPROFILE = customHome
    delete process.env.RTK_BIN
    _resetCachedRtkPath()

    assert.equal(resolveRtkPath(), cargoRtk)
    assert.equal(await tryRewriteCommand("shell", "ls -la"), "rtk ls -la")
    assert.equal(await tryRewriteCommand("shell", "ls -la && git status"), "rtk ls -la && rtk git status")
    assert.equal(await tryRewriteCommand("shell", "cd sub && ls -la"), "cd sub && rtk ls -la")

    // The shell hooks append the resolved directory to the shell's PATH
    const v2Shell = { env: { PATH: "/bin" } }
    const { shellHook } = await captureSetup()
    shellHook(v2Shell)
    assert.equal(v2Shell.env.PATH, `/bin${delimiter}${cargoBinDir}`)

    const v1Shell = { env: { PATH: "/bin" } }
    const { hooks } = await captureServer()
    hooks["shell.env"]({}, v1Shell)
    assert.equal(v1Shell.env.PATH, `/bin${delimiter}${cargoBinDir}`)

    // 2. rtk only at RTK_BIN:
    process.env.PATH = emptyDir
    process.env.HOME = emptyDir
    process.env.USERPROFILE = emptyDir
    process.env.RTK_BIN = envRtk
    _resetCachedRtkPath()

    assert.equal(resolveRtkPath(), envRtk)
    assert.equal(await tryRewriteCommand("shell", "ls -la"), "rtk ls -la")
    assert.equal(await tryRewriteCommand("shell", "ls -la && git status"), "rtk ls -la && rtk git status")

    const envShell = { env: { PATH: "/usr/bin" } }
    const { shellHook: envShellHook } = await captureSetup()
    envShellHook(envShell)
    assert.equal(envShell.env.PATH, `/usr/bin${delimiter}${customDir}`)

    // 3. RTK_BIN takes precedence over PATH
    process.env.PATH = mockDir
    _resetCachedRtkPath()
    assert.equal(resolveRtkPath(), envRtk)

    // 4. rtk on PATH:
    delete process.env.RTK_BIN
    _resetCachedRtkPath()
    assert.equal(resolveRtkPath(), mockBin())
    assert.equal(await tryRewriteCommand("shell", "ls -la"), "rtk ls -la")

    // If directory already in PATH, appendRtkDirToPath does not duplicate it
    const alreadyOnPath = { env: { PATH: `/bin${delimiter}${mockDir}` } }
    const { shellHook: pathShellHook } = await captureSetup()
    pathShellHook(alreadyOnPath)
    assert.equal(alreadyOnPath.env.PATH, `/bin${delimiter}${mockDir}`)

    // 5. A directory at RTK_BIN is skipped
    const decoyDir = mkdtempSync(join(tmpdir(), "rtk-opencode-dir-"))
    process.env.RTK_BIN = decoyDir
    _resetCachedRtkPath()
    assert.equal(resolveRtkPath(), mockBin())
    rmSync(decoyDir, { recursive: true, force: true })
  } finally {
    process.env.PATH = `${mockDir}${delimiter}${savedPath}`
    process.env.HOME = emptyDir
    process.env.USERPROFILE = emptyDir
    delete process.env.RTK_BIN
    _resetCachedRtkPath()
    rmSync(customHome, { recursive: true, force: true })
    rmSync(customDir, { recursive: true, force: true })
  }
})

// A mock rtk that `cd`, `&&` and `;` can reach: the binary plus the `hook`
// script it runs, as in `before()`.
function installRtk(dir) {
  mkdirSync(dir, { recursive: true })
  writeFileSync(join(dir, "package.json"), '{"type":"commonjs"}')
  writeFileSync(join(dir, "hook"), MOCK)
  installMockBin(join(dir, BIN))
  return join(dir, BIN)
}

// The PATH a launcher that lacks rtk's directory hands OpenCode: the system
// dirs on POSIX (for `env`), nothing rtk-related on Windows, where cmd's
// builtins need no PATH.
const launcherPath = () => (process.platform === "win32" ? emptyDir : "/usr/bin:/bin")
// Windows hands a GUI-launched process `Path`, not `PATH`.
const PATH_KEY = process.platform === "win32" ? "Path" : "PATH"
function setLauncherPath() {
  delete process.env.PATH
  process.env[PATH_KEY] = launcherPath()
}

// Where `rtk hook opencode` puts rtk in a rewrite: not only at the start
// (the rows in the #4507 review). `env` stands for wrappers like `timeout`
// that exec rtk off the PATH they inherit. Each run of the mock prints one
// `rtk-mock`.
const H = "rtk hook opencode --help"
const SHAPES =
  process.platform === "win32"
    ? [[H, 1], [`cd . && ${H}`, 1], [`${H} && ${H}`, 2], [`${H} & ${H}`, 2]]
    : [[H, 1], [`cd . && ${H}`, 1], [`FOO=1 ${H}`, 1], [`${H} && ${H}`, 2], [`${H}; ${H}`, 2], [`env ${H}`, 1]]

// What OpenCode 1.x does with the `shell.env` output: spread it over its own
// environment and spawn the command through a shell. `cwd` holds the mock's
// `hook` script but no rtk, because cmd.exe looks in the current directory
// before PATH.
function runInShell(command, shellEnv, cwd) {
  try {
    return execSync(command, {
      cwd,
      env: { ...process.env, ...shellEnv },
      encoding: "utf8",
      stdio: ["ignore", "pipe", "pipe"],
      windowsHide: true,
    })
  } catch (error) {
    return String(error.stdout ?? "")
  }
}

test("rtk outside PATH: every rtk of a rewrite runs in the shell the hooks set up (#4462)", async () => {
  const home = mkdtempSync(join(tmpdir(), "rtk-opencode-home-"))
  const elsewhere = mkdtempSync(join(tmpdir(), "rtk-opencode-custom-"))
  const shellCwd = mkdtempSync(join(tmpdir(), "rtk-opencode-cwd-"))
  writeFileSync(join(shellCwd, "package.json"), '{"type":"commonjs"}')
  writeFileSync(join(shellCwd, "hook"), MOCK)
  const installs = [
    ["~/.cargo/bin", () => installRtk(join(home, ".cargo", "bin"))],
    ["~/.local/bin", () => installRtk(join(home, ".local", "bin"))],
    ["RTK_BIN", () => (process.env.RTK_BIN = installRtk(elsewhere))],
  ]
  try {
    process.env.HOME = home
    process.env.USERPROFILE = home
    for (const [where, install] of installs) {
      rmSync(join(home, ".cargo"), { recursive: true, force: true })
      rmSync(join(home, ".local"), { recursive: true, force: true })
      delete process.env.RTK_BIN
      const bin = install()
      setLauncherPath()
      _resetCachedRtkPath()
      assert.equal(resolveRtkPath(), bin, where)

      // OpenCode hands both hooks an env without PATH: the hook has to carry
      // OpenCode's own PATH over, not replace it with rtk's directory alone,
      // and under the key OpenCode's environment already uses, so spreading
      // one over the other leaves a single PATH.
      const { shellHook } = await captureSetup()
      const v2 = { env: {} }
      shellHook(v2)
      const { hooks } = await captureServer()
      const v1 = { env: {} }
      hooks["shell.env"]({ cwd: mockDir }, v1)
      assert.deepEqual(v1.env, v2.env, where)
      assert.deepEqual(v2.env, { [PATH_KEY]: `${launcherPath()}${delimiter}${dirname(bin)}` }, where)

      for (const [command, runs] of SHAPES) {
        const out = runInShell(command, v2.env, shellCwd)
        assert.equal(out.split("rtk-mock").length - 1, runs, `${where}: ${command}\n${out}`)
      }
    }
  } finally {
    process.env.PATH = `${mockDir}${delimiter}${savedPath}`
    process.env.HOME = emptyDir
    process.env.USERPROFILE = emptyDir
    delete process.env.RTK_BIN
    _resetCachedRtkPath()
    rmSync(home, { recursive: true, force: true })
    rmSync(elsewhere, { recursive: true, force: true })
    rmSync(shellCwd, { recursive: true, force: true })
  }
})

test("OpenCode judges rtk's answer as rtk judged it, wherever rtk lives (#4195, #4462)", async () => {
  // `rtk hook opencode` only answers a rewrite whose verdict under OpenCode's
  // rules (`rtk *` included) matches the typed command's. OpenCode then
  // evaluates whatever string the hook leaves, so that string has to be rtk's
  // answer byte for byte, the same one an rtk on PATH gets, and never an
  // absolute path that a rule naming `rtk` no longer matches.
  const home = mkdtempSync(join(tmpdir(), "rtk-opencode-home-"))
  const elsewhere = mkdtempSync(join(tmpdir(), "rtk-opencode-custom-"))
  const commands = ["ls -la", "cd sub && ls -la", "ls -la && git status", "unchanged"]
  const expected = ["rtk ls -la", "cd sub && rtk ls -la", "rtk ls -la && rtk git status", "unchanged"]
  const installs = [
    ["PATH", () => (process.env.PATH = mockDir)],
    ["~/.cargo/bin", () => installRtk(join(home, ".cargo", "bin"))],
    ["RTK_BIN", () => (process.env.RTK_BIN = installRtk(elsewhere))],
  ]
  try {
    process.env.HOME = home
    process.env.USERPROFILE = home
    for (const [where, install] of installs) {
      rmSync(join(home, ".cargo"), { recursive: true, force: true })
      delete process.env.RTK_BIN
      process.env.PATH = emptyDir
      install()
      _resetCachedRtkPath()

      const { hook } = await captureSetup()
      const { hooks } = await captureServer()
      for (const [i, command] of commands.entries()) {
        const v2 = v2Event(command)
        await hook(v2)
        assert.equal(v2.input.command, expected[i], `${where}, 2.x: ${command}`)
        const v1 = v1Output(command)
        await hooks["tool.execute.before"](v1Input, v1)
        assert.equal(v1.args.command, expected[i], `${where}, 1.x: ${command}`)
      }
    }
  } finally {
    process.env.PATH = `${mockDir}${delimiter}${savedPath}`
    process.env.HOME = emptyDir
    process.env.USERPROFILE = emptyDir
    delete process.env.RTK_BIN
    _resetCachedRtkPath()
    rmSync(home, { recursive: true, force: true })
    rmSync(elsewhere, { recursive: true, force: true })
  }
})
