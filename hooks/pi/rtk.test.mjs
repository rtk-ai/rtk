import test from "node:test"
import assert from "node:assert/strict"
import registerRtkExtension from "./rtk.ts"

async function initialize(rewrite) {
  let handler
  const calls = []
  const pi = {
    async exec(command, args) {
      calls.push({ command, args })
      if (args[0] === "--version") {
        return { code: 0, stdout: "rtk 1.0.1", killed: false }
      }
      return { code: 3, stdout: rewrite(args[1]), killed: false }
    },
    on(eventName, callback) {
      if (eventName === "tool_call") handler = callback
    },
  }
  await registerRtkExtension(pi)
  assert.equal(typeof handler, "function")
  return { handler, calls }
}

function bashCall(command) {
  return { toolName: "bash", input: { command } }
}

test("a chain starting with rtk still rewrites unwrapped later commands (#4606)", async () => {
  const { handler, calls } = await initialize((cmd) =>
    cmd === "rtk git status && git diff --stat"
      ? "rtk git status && rtk git diff --stat"
      : cmd,
  )
  const event = bashCall("rtk git status && git diff --stat")
  await handler(event, {})
  assert.equal(event.input.command, "rtk git status && rtk git diff --stat")
  assert.deepEqual(calls.slice(1), [
    { command: "rtk", args: ["rewrite", "rtk git status && git diff --stat"] },
  ])
})

test("an already-wrapped standalone command stays unchanged, including advisory output", async () => {
  const { handler, calls } = await initialize((cmd) => cmd)
  const event = bashCall("rtk git status")
  await handler(event, {})
  assert.equal(event.input.command, "rtk git status")
  assert.deepEqual(calls.slice(1), [
    { command: "rtk", args: ["rewrite", "rtk git status"] },
  ])
})

test("an unwrapped command is rewritten through the existing callback", async () => {
  const { handler } = await initialize((cmd) => "rtk " + cmd)
  const event = bashCall("git status")
  await handler(event, {})
  assert.equal(event.input.command, "rtk git status")
})

test("non-bash tool calls never invoke rewrite", async () => {
  const { handler, calls } = await initialize((cmd) => "rtk " + cmd)
  const event = { toolName: "read", input: { command: "git status" } }
  await handler(event, {})
  assert.equal(event.input.command, "git status")
  assert.equal(calls.length, 1)
})
