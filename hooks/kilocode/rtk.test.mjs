import test, { afterEach, beforeEach, mock } from "node:test"
import assert from "node:assert/strict"
import childProcess from "node:child_process"
import { execFileSync } from "node:child_process"
import { syncBuiltinESMExports } from "node:module"
import { mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs"
import { tmpdir } from "node:os"
import { delimiter, dirname, join, relative, sep } from "node:path"
import plugin from "./rtk.ts"

const ENV_KEYS = ["HOME", "USERPROFILE", "PATH", "RTK_KILO_BIN", "RTK_KILO_BASH_PLUGIN"]
const realExecFile = childProcess.execFile
const MOCK = `#!/bin/sh
case "$1" in
  --version) printf 'rtk test\n' ;;
  rewrite)
    case "$2" in
      'git status') printf 'rtk git status\n' ;;
      'git status && git diff') printf 'rtk git status && rtk git diff\n' ;;
      'git status | git diff') printf 'rtk git status | rtk git diff\n' ;;
      *) exit 1 ;;
    esac ;;
  git) printf 'mock:%s\n' "$*" ;;
  *) exit 1 ;;
esac
`

let root
let binary
let savedEnv

beforeEach(() => {
  savedEnv = Object.fromEntries(ENV_KEYS.map((key) => [key, process.env[key]]))
  root = mkdtempSync(join(tmpdir(), "rtk-kilo-env-"))
  binary = join(root, ".local", "bin", "rtk")
  mkdirSync(dirname(binary), { recursive: true })
  mkdirSync(join(root, "empty"))
  writeFileSync(binary, MOCK, { mode: 0o755 })
  process.env.HOME = root
  process.env.USERPROFILE = root
  process.env.PATH = join(root, "empty")
  delete process.env.RTK_KILO_BIN
  delete process.env.RTK_KILO_BASH_PLUGIN
  // Probe only the fixture, never a real RTK in one of the fixed install paths.
  mock.method(childProcess, "execFile", (file, args, options, callback) => {
    if (!file.startsWith(root + sep)) {
      queueMicrotask(() => callback(Object.assign(new Error("not a fixture"), { code: "ENOENT" }), ""))
      return
    }
    return realExecFile(file, args, options, callback)
  })
  syncBuiltinESMExports()
})

afterEach(() => {
  mock.restoreAll()
  syncBuiltinESMExports()
  for (const [key, value] of Object.entries(savedEnv)) {
    if (value === undefined) delete process.env[key]
    else process.env[key] = value
  }
  rmSync(root, { recursive: true, force: true })
})

const posix = { skip: process.platform === "win32" }

async function shellEnv(hooks, env = {}) {
  assert.equal(typeof hooks["shell.env"], "function")
  const output = { env }
  await hooks["shell.env"]({ cwd: root }, output)
  return output.env
}

test("an RTK outside PATH executes rewritten commands, chains and pipelines", posix, async () => {
  const inheritedPath = process.env.PATH
  const hooks = await plugin.server()
  const env = await shellEnv(hooks, { KEEP_ME: "unchanged" })
  assert.deepEqual(
    env.PATH.split(delimiter),
    [inheritedPath, dirname(binary)],
    "append the executable directory after the inherited PATH",
  )
  assert.equal(env.KEEP_ME, "unchanged")
  assert.equal(process.env.PATH, inheritedPath, "do not mutate the host environment")

  for (const [command, rewritten] of [
    ["git status", "rtk git status"],
    ["git status && git diff", "rtk git status && rtk git diff"],
    ["git status | git diff", "rtk git status | rtk git diff"],
  ]) {
    const output = { args: { command, description: "keep description" } }
    await hooks["tool.execute.before"]({ tool: "bash" }, output)
    assert.equal(output.args.command, rewritten, "leave shell syntax untouched")
    assert.equal(output.args.description, "keep description")
    const stdout = execFileSync("/bin/sh", ["-c", output.args.command], {
      cwd: root,
      env: { ...process.env, ...env },
      encoding: "utf8",
    })
    assert.match(stdout, /mock:git (status|diff)/)
  }
})

test("the resolved override directory is appended without shell-quoting or reordering PATH", posix, async () => {
  const override = join(root, "RTK's bin $(not-a-command)", "rtk")
  mkdirSync(dirname(override))
  writeFileSync(override, MOCK, { mode: 0o755 })
  process.env.RTK_KILO_BIN = override
  const hooks = await plugin.server()
  const otherBin = join(root, "other-bin")

  const env = await shellEnv(hooks, { PATH: otherBin })
  assert.deepEqual(env.PATH.split(delimiter), [otherBin, dirname(override)])
  assert.equal(
    execFileSync("/bin/sh", ["-c", "rtk git status"], {
      env: { ...process.env, ...env },
      encoding: "utf8",
    }),
    "mock:git status\n",
  )

  // Already listed, before or after other entries: the user's order is left untouched.
  for (const entries of [[dirname(override), otherBin], [otherBin, dirname(override)]]) {
    const PATH = entries.join(delimiter)
    assert.deepEqual(await shellEnv(hooks, { PATH }), { PATH })
  }
  assert.deepEqual(await shellEnv(hooks, { ...env }), env, "repeated calls do not duplicate the directory")
})

test("other executables in rtk's directory never shadow earlier PATH entries", posix, async () => {
  const venv = join(root, "venv", "bin")
  mkdirSync(venv, { recursive: true })
  for (const [dir, label] of [[venv, "venv"], [dirname(binary), "shared"]]) {
    writeFileSync(join(dir, "pytool"), `#!/bin/sh\nprintf '${label}\\n'\n`, { mode: 0o755 })
  }
  const hooks = await plugin.server()
  const run = (env, command) =>
    execFileSync("/bin/sh", ["-c", command], { env: { ...process.env, ...env }, encoding: "utf8" })

  // rtk's directory is already listed, after the virtual environment.
  const listed = await shellEnv(hooks, { PATH: [venv, dirname(binary), join(root, "empty")].join(delimiter) })
  assert.equal(run(listed, "pytool"), "venv\n")
  assert.equal(run(listed, "rtk git status"), "mock:git status\n")

  // rtk's directory is missing: it is appended and still shadows nothing.
  const appended = await shellEnv(hooks, { PATH: [venv, join(root, "empty")].join(delimiter) })
  assert.equal(run(appended, "pytool"), "venv\n")
  assert.equal(run(appended, "rtk git status"), "mock:git status\n")
})

test("relative executable overrides are resolved before the tool changes directory", posix, async () => {
  process.env.RTK_KILO_BIN = relative(process.cwd(), binary)
  const hooks = await plugin.server()
  const env = await shellEnv(hooks)
  assert.deepEqual(env.PATH.split(delimiter), [join(root, "empty"), dirname(binary)])
})

test("an explicitly empty PATH does not gain a current-directory entry", posix, async () => {
  const hooks = await plugin.server()
  const env = await shellEnv(hooks, { PATH: "" })
  assert.equal(env.PATH, dirname(binary))
})

test("directories with PATH separators and renamed executable overrides fail open", posix, async () => {
  for (const override of [join(root, `unsafe${delimiter}bin`, "rtk"), join(root, "rtk-renamed")]) {
    mkdirSync(dirname(override), { recursive: true })
    writeFileSync(override, MOCK, { mode: 0o755 })
    process.env.RTK_KILO_BIN = override
    assert.deepEqual(await plugin.server(), {})
  }
})

test("Windows environment overrides produce one canonical PATH", posix, async () => {
  const originalPlatform = Object.getOwnPropertyDescriptor(process, "platform")
  writeFileSync(binary + ".exe", MOCK, { mode: 0o755 })
  try {
    Object.defineProperty(process, "platform", { value: "win32" })
    const hooks = await plugin.server()
    const env = await shellEnv(hooks, { Path: "mixed-case", PATH: "effective", KEEP_ME: "ok" })
    assert.deepEqual(env, { PATH: ["effective", dirname(binary)].join(delimiter), KEEP_ME: "ok" })
    const mixedOnly = await shellEnv(hooks, { Path: "custom" })
    assert.deepEqual(mixedOnly, { PATH: ["custom", dirname(binary)].join(delimiter) })
    const listed = { Path: [dirname(binary), "custom"].join(delimiter) }
    assert.deepEqual(await shellEnv(hooks, { ...listed }), listed, "a listed directory leaves the env untouched")
  } finally {
    Object.defineProperty(process, "platform", originalPlatform)
  }
})

test("PATH-only discovery preserves the executable in a tool-specific PATH", posix, async () => {
  process.env.PATH = dirname(binary)
  process.env.HOME = join(root, "other-home")
  process.env.USERPROFILE = process.env.HOME
  const hooks = await plugin.server()
  const toolPath = join(root, "empty")
  const env = await shellEnv(hooks, { PATH: toolPath, KEEP_ME: "unchanged" })
  assert.deepEqual(env, {
    PATH: [toolPath, dirname(binary)].join(delimiter),
    KEEP_ME: "unchanged",
  })
})

test("a relative PATH entry resolves before the tool changes directory", posix, async () => {
  process.env.PATH = relative(process.cwd(), dirname(binary))
  process.env.HOME = join(root, "other-home")
  process.env.USERPROFILE = process.env.HOME
  const hooks = await plugin.server()
  const env = await shellEnv(hooks)
  assert.deepEqual(env.PATH.split(delimiter), [process.env.PATH, dirname(binary)])
  assert.equal(
    execFileSync("/bin/sh", ["-c", "rtk git status"], {
      cwd: root,
      env: { ...process.env, ...env },
      encoding: "utf8",
    }),
    "mock:git status\n",
  )
})

test("machine-readable commands, unrelated tools and unhandled commands stay unchanged", posix, async () => {
  const hooks = await plugin.server()
  for (const [tool, command] of [
    ["bash", "git status --porcelain"],
    ["read", "git status"],
    ["shell", "printf '%s' 'rtk literal'"],
  ]) {
    const output = { args: { command } }
    await hooks["tool.execute.before"]({ tool }, output)
    assert.equal(output.args.command, command)
  }
})

test("disabled or missing RTK registers neither rewriting nor environment hooks", posix, async () => {
  process.env.RTK_KILO_BASH_PLUGIN = "0"
  assert.deepEqual(await plugin.server(), {})
  delete process.env.RTK_KILO_BASH_PLUGIN
  rmSync(binary)
  assert.deepEqual(await plugin.server(), {})
})
