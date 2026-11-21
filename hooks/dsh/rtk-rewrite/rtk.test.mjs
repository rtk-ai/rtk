import test, { after, before } from "node:test"
import assert from "node:assert/strict"
import {
  copyFileSync,
  existsSync,
  mkdirSync,
  mkdtempSync,
  readFileSync,
  rmSync,
  writeFileSync,
} from "node:fs"
import { tmpdir } from "node:os"
import { join } from "node:path"
import RtkDshPlugin, {
  DEFAULT_COMMANDS,
  apply,
  askRtk,
  canSkipRewrite,
  defaultBypassPath,
  defaultLogPath,
  insertedSubcommands,
  isSafeRewrite,
  name,
  tokens,
} from "./lib/index.js"

// ── tokens() ────────────────────────────────────────────────────────────────

test("tokens: splits a plain command on whitespace", () => {
  assert.deepEqual(tokens("git status"), ["git", "status"])
})

test("tokens: keeps quoted arguments as one token (quotes retained)", () => {
  assert.deepEqual(tokens('git commit -m "fix bug"'), [
    "git",
    "commit",
    "-m",
    '"fix bug"',
  ])
  assert.deepEqual(tokens("git commit -m 'fix bug'"), [
    "git",
    "commit",
    "-m",
    "'fix bug'",
  ])
})

test("tokens: operators become their own tokens, with or without surrounding spaces", () => {
  assert.deepEqual(tokens("cargo fmt --all && cargo test"), [
    "cargo",
    "fmt",
    "--all",
    "&&",
    "cargo",
    "test",
  ])
  assert.deepEqual(tokens("a&&b"), ["a", "&&", "b"])
  assert.deepEqual(tokens("a||b"), ["a", "||", "b"])
  assert.deepEqual(tokens("a;b"), ["a", ";", "b"])
  assert.deepEqual(tokens("a&b"), ["a", "&", "b"])
  assert.deepEqual(tokens("a|b"), ["a", "|", "b"])
})

test("tokens: multiple different operators in one command", () => {
  assert.deepEqual(tokens("npm test; npm run lint && echo done | cat"), [
    "npm",
    "test",
    ";",
    "npm",
    "run",
    "lint",
    "&&",
    "echo",
    "done",
    "|",
    "cat",
  ])
})

test("tokens: a literal rtk inside a quoted argument stays one token", () => {
  assert.deepEqual(tokens('git log --grep "rtk"'), [
    "git",
    "log",
    "--grep",
    '"rtk"',
  ])
})

// ── isSafeRewrite() / insertedSubcommands() ──────────────────────────────────

test("isSafeRewrite: accepts a pure rtk-prefix insertion", () => {
  assert.equal(isSafeRewrite("git status", "rtk git status"), true)
  assert.deepEqual(
    insertedSubcommands("git status", "rtk git status"),
    ["git"]
  )
})

test("isSafeRewrite: accepts independent insertions on both sides of an operator", () => {
  const orig = "cargo fmt --all && cargo test"
  const rewritten = "rtk cargo fmt --all && rtk cargo test"
  assert.equal(isSafeRewrite(orig, rewritten), true)
  assert.deepEqual(insertedSubcommands(orig, rewritten), ["cargo", "cargo"])
})

test("isSafeRewrite: accepts rtk.exe as the inserted token (Windows)", () => {
  assert.equal(isSafeRewrite("git status", "rtk.exe git status"), true)
})

test("isSafeRewrite: accepts insertion on only one side of a pipe", () => {
  // the registry's pipeline-safe form: only the final stage is rewritten
  assert.equal(isSafeRewrite("git log | grep foo", "git log | rtk grep foo"), true)
  assert.deepEqual(
    insertedSubcommands("git log | grep foo", "git log | rtk grep foo"),
    ["grep"]
  )
})

test("isSafeRewrite: rejects a changed argument value", () => {
  assert.equal(isSafeRewrite("git status", "rtk git status --extra"), false)
  assert.equal(isSafeRewrite("git status -s", "rtk git status"), false)
})

test("isSafeRewrite: rejects a removed argument", () => {
  assert.equal(isSafeRewrite("git status --short", "rtk git status"), false)
})

test("isSafeRewrite: rejects a non-rtk inserted token", () => {
  assert.equal(isSafeRewrite("git status", "nice rtk git status"), false)
})

test("isSafeRewrite: a literal 'rtk' argument already in the original is not miscounted as an insertion", () => {
  const orig = "git log --grep rtk"
  const rewritten = "rtk git log --grep rtk"
  assert.equal(isSafeRewrite(orig, rewritten), true)
  assert.deepEqual(insertedSubcommands(orig, rewritten), ["git"])
})

test("isSafeRewrite: rejects when the command is already prefixed and nothing changed", () => {
  // no insertion at all -- rewritten === original is handled by the caller,
  // but isSafeRewrite on its own still reports true (zero insertions is a
  // trivial match of "original with zero extra tokens").
  assert.equal(isSafeRewrite("rtk git status", "rtk git status"), true)
  assert.deepEqual(insertedSubcommands("rtk git status", "rtk git status"), [])
})

test("isSafeRewrite: rejects a totally unrelated command", () => {
  assert.equal(isSafeRewrite("git status", "rm -rf /"), false)
})

// ── askRtk() against a mock rtk binary ──────────────────────────────────────

let mockDir = ""
const BIN = process.platform === "win32" ? "mock-rtk.exe" : "mock-rtk"

// Mimics `rtk rewrite -- <command>`'s exit code contract (see
// src/hooks/rewrite_cmd.rs): argv is ["rewrite", "--", <command>].
const MOCK = `
const [sub, dashdash, ...rest] = process.argv.slice(2)
const command = rest.join(" ")
if (sub !== "rewrite" || dashdash !== "--") process.exit(1)

if (command === "git status") {
  process.stdout.write("rtk git status")
  process.exit(0)
} else if (command === "ask me") {
  process.stdout.write("rtk ask me")
  process.exit(3)
} else if (command === "no rewrite") {
  process.exit(1)
} else if (command === "denied") {
  process.exit(2)
} else if (command === "sleep") {
  setTimeout(() => process.exit(0), 5000)
} else {
  process.exit(1)
}
`

const mockBin = () => join(mockDir, BIN)

before(() => {
  mockDir = mkdtempSync(join(tmpdir(), "rtk-dsh-mock-"))
  writeFileSync(join(mockDir, "package.json"), '{"type":"commonjs"}')
  writeFileSync(join(mockDir, "script.cjs"), MOCK)
  // Node's shebang-free #! wrapper trick: copy node itself, name it the mock
  // binary, and have it run script.cjs via argv[1]. Simpler: just invoke node
  // directly as the "bin" via execFile with a wrapper script. We emulate a
  // native rtk binary by writing a tiny shell-less launcher: on POSIX, a
  // shebang script; on Windows, a .cmd shim. Both just exec node on script.cjs.
  if (process.platform === "win32") {
    writeFileSync(
      mockBin(),
      `@echo off\r\nnode "${join(mockDir, "script.cjs")}" %*\r\n`
    )
  } else {
    writeFileSync(
      mockBin(),
      `#!/bin/sh\nexec node "${join(mockDir, "script.cjs")}" "$@"\n`,
      { mode: 0o755 }
    )
  }
})

after(() => {
  rmSync(mockDir, { recursive: true, force: true })
})

test("askRtk: exit 0 returns the rewritten command", async () => {
  assert.equal(await askRtk(mockBin(), "git status", 2000), "rtk git status")
})

test("askRtk: exit 3 (ask) also returns the rewritten command", async () => {
  assert.equal(await askRtk(mockBin(), "ask me", 2000), "rtk ask me")
})

test("askRtk: exit 1 (no rewrite) resolves undefined", async () => {
  assert.equal(await askRtk(mockBin(), "no rewrite", 2000), undefined)
})

test("askRtk: exit 2 (deny) resolves undefined", async () => {
  assert.equal(await askRtk(mockBin(), "denied", 2000), undefined)
})

test("askRtk: a timeout resolves undefined, never throws", async () => {
  assert.equal(await askRtk(mockBin(), "sleep", 100), undefined)
})

test("askRtk: a missing binary resolves undefined, never throws", async () => {
  assert.equal(
    await askRtk(join(mockDir, "does-not-exist"), "git status", 2000),
    undefined
  )
})

// ── defaults ──────────────────────────────────────────────────────────────

test("defaultBypassPath and defaultLogPath live under ~/.dsh/rtk/", () => {
  assert.match(defaultBypassPath(), /\.dsh[\\/]rtk[\\/]off$/)
  assert.match(defaultLogPath(), /\.dsh[\\/]rtk[\\/]rewrites\.jsonl$/)
})

test("DEFAULT_COMMANDS excludes host-sensitive aliases", () => {
  for (const excluded of ["ls", "env", "cat", "echo"]) {
    assert.equal(DEFAULT_COMMANDS.includes(excluded), false, excluded)
  }
  for (const included of ["git", "cargo", "pytest", "find", "tree", "read"]) {
    assert.equal(DEFAULT_COMMANDS.includes(included), true, included)
  }
})

test("canSkipRewrite: skips only known simple standalone commands", () => {
  for (const command of ["echo hello", "pwd", "Get-ChildItem foo", "printf %s hello"]) {
    assert.equal(canSkipRewrite(command), true, command)
  }
  for (const command of [
    "git status", "echo ok && git status", "cd repo && grep foo bar",
    "echo $(git status)", "echo 'hello'", "echo hello | cat",
    "echo hello >out", "echo hello; git status", "unknown-tool arg",
  ]) {
    assert.equal(canSkipRewrite(command), false, command)
  }
})

// ── apply() against a fake cordis context ───────────────────────────────────

const CORDIS_ORIGINAL = Symbol.for("cordis.original")

/** A minimal fake ctx.shell: a tracked-service-proxy standin over a real target. */
function makeFakeShell(executeImpl) {
  const target = {
    execute: executeImpl,
  }
  // Mimic cordis: every *access* to ctx.shell could be a distinct proxy
  // object, but CORDIS_ORIGINAL always unwraps to the same `target`.
  const proxy = new Proxy(target, {
    get(t, prop) {
      if (prop === CORDIS_ORIGINAL) return target
      return t[prop]
    },
    set(t, prop, value) {
      t[prop] = value
      return true
    },
  })
  return { proxy, target }
}

function makeFakeCtx(shellProxy) {
  const disposers = []
  return {
    ctx: {
      shell: shellProxy,
      logger: { warn: () => {} },
      effect: (fn) => {
        const dispose = fn()
        disposers.push(dispose)
        return dispose
      },
    },
    runDisposers: () => disposers.forEach((d) => d()),
  }
}

test("apply: exports name/inject", () => {
  assert.equal(name, "rtk-rewrite")
  assert.equal(RtkDshPlugin.name, "rtk-rewrite")
  assert.deepEqual(RtkDshPlugin.inject, ["shell"])
})

test("apply: wraps execute and rewrites an accepted command", async () => {
  const calls = []
  const { proxy, target } = makeFakeShell(async (spec) => {
    calls.push(spec.command)
    return { ok: true }
  })
  const { ctx } = makeFakeCtx(proxy)

  apply(ctx, { bin: mockBin(), log: false })
  assert.notEqual(target.execute, undefined)

  await target.execute({ command: "git status" })
  assert.deepEqual(calls, ["rtk git status"])
})

test("apply: leaves an unrecognized command's execute call alone (exit 1)", async () => {
  const calls = []
  const { proxy, target } = makeFakeShell(async (spec) => {
    calls.push(spec.command)
    return { ok: true }
  })
  const { ctx } = makeFakeCtx(proxy)

  apply(ctx, { bin: mockBin() })
  await target.execute({ command: "no rewrite" })
  assert.deepEqual(calls, ["no rewrite"])
})

test("apply: simple pass-through commands do not spawn the rewrite binary", async () => {
  const calls = []
  const { proxy, target } = makeFakeShell(async (spec) => {
    calls.push(spec.command)
    return { ok: true }
  })
  const { ctx } = makeFakeCtx(proxy)
  apply(ctx, { bin: mockBin() })
  await target.execute({ command: "echo hello" })
  assert.deepEqual(calls, ["echo hello"])
})

test("apply: a non-string spec.command is passed straight through", async () => {
  const calls = []
  const { proxy, target } = makeFakeShell(async (spec) => {
    calls.push(spec)
    return { ok: true }
  })
  const { ctx } = makeFakeCtx(proxy)

  apply(ctx, { bin: mockBin() })
  await target.execute({ command: undefined, other: 1 })
  assert.deepEqual(calls, [{ command: undefined, other: 1 }])
})

test("apply: the bypass file disables rewriting without touching the command", async () => {
  const bypassDir = mkdtempSync(join(tmpdir(), "rtk-dsh-bypass-"))
  const bypassPath = join(bypassDir, "off")
  writeFileSync(bypassPath, "")

  try {
    const calls = []
    const { proxy, target } = makeFakeShell(async (spec) => {
      calls.push(spec.command)
      return { ok: true }
    })
    const { ctx } = makeFakeCtx(proxy)

    apply(ctx, { bin: mockBin(), bypassPath })
    await target.execute({ command: "git status" })
    assert.deepEqual(calls, ["git status"])
  } finally {
    rmSync(bypassDir, { recursive: true, force: true })
  }
})

test("apply: enabled: false never patches execute at all", async () => {
  const original = async (spec) => ({ saw: spec.command })
  const { proxy, target } = makeFakeShell(original)
  const { ctx } = makeFakeCtx(proxy)

  apply(ctx, { bin: mockBin(), enabled: false })
  assert.equal(target.execute, original)
})

test("apply: restores the original execute when the effect disposer runs", async () => {
  const original = async (spec) => ({ saw: spec.command })
  const { proxy, target } = makeFakeShell(original)
  const { ctx, runDisposers } = makeFakeCtx(proxy)

  apply(ctx, { bin: mockBin() })
  assert.notEqual(target.execute, original)

  runDisposers()
  assert.equal(target.execute, original)
})

test("apply: an unsafe rewrite (rejected by the whitelist) runs the original command", async () => {
  // A config whose whitelist excludes "git" simulates a rewrite whose
  // inserted subcommand this install does not trust.
  const calls = []
  const { proxy, target } = makeFakeShell(async (spec) => {
    calls.push(spec.command)
    return { ok: true }
  })
  const { ctx } = makeFakeCtx(proxy)

  apply(ctx, { bin: mockBin(), commands: new Set(["cargo"]) })
  await target.execute({ command: "git status" })
  assert.deepEqual(calls, ["git status"])
})

test("apply: fails open when ctx.shell has no execute to patch", () => {
  const warnings = []
  const ctx = {
    shell: {},
    logger: { warn: (msg) => warnings.push(msg) },
    effect: () => {},
  }
  apply(ctx, { bin: mockBin() })
  assert.ok(warnings.length > 0)
})

test("apply: logs a JSON line per rewrite when log: true", async () => {
  const logDir = mkdtempSync(join(tmpdir(), "rtk-dsh-log-"))
  const logPath = join(logDir, "rewrites.jsonl")

  try {
    const { proxy, target } = makeFakeShell(async () => ({ ok: true }))
    const { ctx } = makeFakeCtx(proxy)

    apply(ctx, { bin: mockBin(), log: true, logPath })
    await target.execute({ command: "git status" })

    assert.ok(existsSync(logPath))
    const lines = readFileSync(logPath, "utf8").trim().split("\n")
    assert.equal(lines.length, 1)
    const entry = JSON.parse(lines[0])
    assert.equal(entry.original, "git status")
    assert.equal(entry.rewritten, "rtk git status")
    assert.equal(typeof entry.ts, "string")
  } finally {
    rmSync(logDir, { recursive: true, force: true })
  }
})
