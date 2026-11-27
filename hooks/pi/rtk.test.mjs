import test from "node:test"
import assert from "node:assert/strict"
import registerRtkPiExtension from "./rtk.ts"

function createMockPi(execImpl) {
  const handlers = new Map()
  const statusHistory = []
  const execCalls = []

  const sessionCtx = {
    ui: {
      setStatus(key, text) {
        statusHistory.push({ key, text })
      },
    },
  }

  const pi = {
    on(event, handler) {
      handlers.set(event, handler)
    },
    async exec(cmd, args, options) {
      execCalls.push({ cmd, args, options })
      return execImpl(cmd, args, options, execCalls.length)
    },
  }

  return {
    pi,
    handlers,
    statusHistory,
    execCalls,
    sessionCtx,
    emitSessionStart() {
      const handler = handlers.get("session_start")
      if (handler) handler({}, sessionCtx)
    },
    async emitBashToolCall(command, ctx = sessionCtx) {
      const handler = handlers.get("tool_call")
      assert.ok(handler, "expected tool_call handler to be registered")
      const event = {
        toolName: "bash",
        input: { command },
      }
      await handler(event, ctx)
      return event.input.command
    },
  }
}

test("recovers on eligible bash tool_call after transient startup exit 127 and clears status", async () => {
  let rtkAvailable = false
  const harness = createMockPi(async (_cmd, args) => {
    if (!rtkAvailable) {
      return { code: 127, stdout: "", stderr: "rtk: not found", killed: false }
    }
    if (args[0] === "--version") {
      return { code: 0, stdout: "rtk 0.23.0\n", stderr: "", killed: false }
    }
    if (args[0] === "rewrite") {
      return { code: 0, stdout: "rtk git status\n", stderr: "", killed: false }
    }
    throw new Error(`unexpected args: ${args.join(" ")}`)
  })

  await registerRtkPiExtension(harness.pi)
  harness.emitSessionStart()

  assert.equal(harness.statusHistory.length, 1)
  assert.equal(
    harness.statusHistory[0].text,
    "RTK disabled: rtk binary not found in PATH"
  )

  rtkAvailable = true
  const rewritten = await harness.emitBashToolCall("git status")
  assert.equal(rewritten, "rtk git status")
  assert.equal(harness.statusHistory.at(-1)?.text, undefined)

  // Once recovered, subsequent calls only invoke `rtk rewrite` without re-probing `--version`.
  const callsBeforeSecond = harness.execCalls.length
  const second = await harness.emitBashToolCall("git status")
  assert.equal(second, "rtk git status")
  assert.equal(harness.execCalls.length - callsBeforeSecond, 1)
  assert.deepEqual(harness.execCalls.at(-1)?.args, ["rewrite", "git status"])
})

test("catches thrown startup probe error and recovers on subsequent bash tool_call", async () => {
  let shouldThrow = true
  const harness = createMockPi(async (_cmd, args) => {
    if (shouldThrow) {
      throw new Error("spawn rtk ENOENT")
    }
    if (args[0] === "--version") {
      return { code: 0, stdout: "rtk 0.24.1\n", stderr: "", killed: false }
    }
    if (args[0] === "rewrite") {
      return { code: 0, stdout: "rtk cargo test\n", stderr: "", killed: false }
    }
    throw new Error(`unexpected args: ${args.join(" ")}`)
  })

  await assert.doesNotReject(() => registerRtkPiExtension(harness.pi))
  harness.emitSessionStart()
  assert.equal(
    harness.statusHistory.at(-1)?.text,
    "RTK disabled: rtk binary not found in PATH"
  )

  shouldThrow = false
  const cmd = await harness.emitBashToolCall("cargo test")
  assert.equal(cmd, "rtk cargo test")
  assert.equal(harness.statusHistory.at(-1)?.text, undefined)
})

test("confirms recovery when rtk rewrite returns exit code 1 (no rewrite needed)", async () => {
  let rtkAvailable = false
  const harness = createMockPi(async (_cmd, args) => {
    if (!rtkAvailable) {
      return { code: 127, stdout: "", stderr: "not found", killed: false }
    }
    if (args[0] === "--version") {
      return { code: 0, stdout: "rtk 0.23.0\n", stderr: "", killed: false }
    }
    if (args[0] === "rewrite" && args[1] === "echo hello") {
      return { code: 1, stdout: "", stderr: "", killed: false }
    }
    if (args[0] === "rewrite" && args[1] === "git status") {
      return { code: 0, stdout: "rtk git status\n", stderr: "", killed: false }
    }
    throw new Error(`unexpected args: ${args.join(" ")}`)
  })

  await registerRtkPiExtension(harness.pi)
  harness.emitSessionStart()

  rtkAvailable = true
  const unchanged = await harness.emitBashToolCall("echo hello")
  assert.equal(unchanged, "echo hello")
  assert.equal(harness.statusHistory.at(-1)?.text, undefined)

  // Next call should already be in ready state and not re-probe `--version`.
  const callsBefore = harness.execCalls.length
  const rewritten = await harness.emitBashToolCall("git status")
  assert.equal(rewritten, "rtk git status")
  assert.equal(harness.execCalls.length - callsBefore, 1)
  assert.deepEqual(harness.execCalls.at(-1)?.args, ["rewrite", "git status"])
})

test("does not confirm recovery if version probe succeeds but rewrite check fails", async (t) => {
  let now = 1_000_000
  t.mock.method(Date, "now", () => now)

  let phase = "startup_fail"
  const harness = createMockPi(async (_cmd, args) => {
    if (phase === "startup_fail") {
      return { code: 127, stdout: "", stderr: "not found", killed: false }
    }
    if (args[0] === "--version") {
      return { code: 0, stdout: "rtk 0.23.0\n", stderr: "", killed: false }
    }
    if (args[0] === "rewrite") {
      if (phase === "rewrite_fail") {
        return { code: 127, stdout: "", stderr: "failed", killed: false }
      }
      return { code: 0, stdout: "rtk git status\n", stderr: "", killed: false }
    }
    throw new Error(`unexpected args: ${args.join(" ")}`)
  })

  await registerRtkPiExtension(harness.pi)
  harness.emitSessionStart()

  // Retry 1: version probe succeeds, but rewrite fails -> stays degraded (status not cleared).
  phase = "rewrite_fail"
  const firstAttempt = await harness.emitBashToolCall("git status")
  assert.equal(firstAttempt, "git status")
  assert.equal(
    harness.statusHistory.at(-1)?.text,
    "RTK disabled: rtk binary not found in PATH"
  )

  // Retry 2 after cooldown: both version probe and rewrite succeed -> confirms recovery.
  now += 10_000
  phase = "ready"
  const secondAttempt = await harness.emitBashToolCall("git status")
  assert.equal(secondAttempt, "rtk git status")
  assert.equal(harness.statusHistory.at(-1)?.text, undefined)
})

test("enforces 10s cooldown between retries and stops permanently after 3 failed retries", async (t) => {
  let now = 1_000_000
  t.mock.method(Date, "now", () => now)

  const harness = createMockPi(async () => ({
    code: 127,
    stdout: "",
    stderr: "rtk: not found",
    killed: false,
  }))

  await registerRtkPiExtension(harness.pi)
  harness.emitSessionStart()
  assert.equal(harness.execCalls.length, 1)

  // Non-eligible calls (already rtk, empty, or RTK_DISABLED=1) do not consume retries.
  await harness.emitBashToolCall("rtk git status")
  await harness.emitBashToolCall("   ")
  process.env.RTK_DISABLED = "1"
  try {
    await harness.emitBashToolCall("git status")
  } finally {
    delete process.env.RTK_DISABLED
  }
  assert.equal(harness.execCalls.length, 1)

  // Retry 1 runs immediately on the first eligible bash call.
  await harness.emitBashToolCall("git status")
  assert.equal(harness.execCalls.length, 2)

  // Call within cooldown (< 10_000ms) is skipped without invoking pi.exec.
  now += 5_000
  await harness.emitBashToolCall("git status")
  assert.equal(harness.execCalls.length, 2)

  // Retry 2 after 10s cooldown.
  now += 5_000
  await harness.emitBashToolCall("git status")
  assert.equal(harness.execCalls.length, 3)

  // Retry 3 after another 10s cooldown exhausts the retry budget.
  now += 10_000
  await harness.emitBashToolCall("git status")
  assert.equal(harness.execCalls.length, 4)
  assert.match(
    harness.statusHistory.at(-1)?.text ?? "",
    /check PATH and restart or reload the extension/
  )

  // Further calls after cooldown make no more subprocess attempts.
  now += 60_000
  await harness.emitBashToolCall("git status")
  assert.equal(harness.execCalls.length, 4)
})

test("permanently disables at startup when rtk version is older than 0.23.0", async () => {
  const harness = createMockPi(async () => ({
    code: 0,
    stdout: "rtk 0.22.9\n",
    stderr: "",
    killed: false,
  }))

  await registerRtkPiExtension(harness.pi)
  harness.emitSessionStart()

  assert.equal(harness.handlers.has("tool_call"), false)
  assert.equal(
    harness.statusHistory.at(-1)?.text,
    "RTK disabled: rtk 0.22.9 is too old (need >= 0.23.0)"
  )
})
