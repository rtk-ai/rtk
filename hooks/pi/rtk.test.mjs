// Run with Node 22.18+ (native TypeScript stripping): node --test hooks/pi/rtk.test.mjs
import assert from "node:assert/strict"
import { test } from "node:test"
import register from "./rtk.ts"

async function harness(t) {
  const handlers = new Map()
  const calls = []
  const warnings = t.mock.method(console, "warn", () => {})
  await register({
    on: (name, handler) => handlers.set(name, handler),
    exec: async (program, args, options) => {
      calls.push({ program, args, options })
      if (args[0] === "--version") return { code: 0, stdout: "rtk 0.49.0" }
      // Match Node's argv rejection without starting a subprocess.
      if (args.some(arg => arg.includes("\0"))) throw new TypeError("ERR_INVALID_ARG_VALUE")
      return { code: 3, stdout: "rtk git status\n" }
    },
  })
  calls.length = 0 // The version probe is independent of command rewriting.
  return { handler: handlers.get("tool_call"), calls, warnings }
}

test("literal NUL commands pass through silently without an exec attempt", async t => {
  const { handler, calls, warnings } = await harness(t)
  for (const command of ["\0git status", "printf 'a\0b'", "git status\0"]) {
    const event = { toolName: "bash", input: { command } }
    assert.equal(await handler(event, {}), undefined)
    assert.equal(event.input.command, command)
  }
  assert.deepEqual(calls, [])
  assert.equal(warnings.mock.callCount(), 0)
})

test("textual escapes still delegate with the timeout and abort signal", async t => {
  const { handler, calls, warnings } = await harness(t)
  const signal = new AbortController().signal
  const command = String.raw`printf '\0'; git status`
  const event = { toolName: "bash", input: { command } }
  assert.equal(await handler(event, { signal }), undefined)
  assert.equal(event.input.command, "rtk git status")
  assert.deepEqual(calls, [{ program: "rtk", args: ["rewrite", command], options: { timeout: 2000, signal } }])
  assert.equal(warnings.mock.callCount(), 0)
})
