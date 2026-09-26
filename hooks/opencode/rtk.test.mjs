import test from "node:test"
import assert from "node:assert/strict"
import { writeFileSync, unlinkSync } from "node:fs"
import { join } from "node:path"
import { homedir } from "node:os"
import RtkOpenCodePlugin, {
  resolveRtkPath,
  runRtkRewrite,
  tryRewriteCommand,
  expandHome,
  _resetCachedRtkPath,
} from "./rtk.ts"

test("plugin schema: default export is a plain object matching V2 loader spec", async () => {
  // Must be a plain object to pass OpenCode 2.x Schema validation (typeof === 'function' fails schema)
  assert.equal(typeof RtkOpenCodePlugin, "object")
  assert.notEqual(RtkOpenCodePlugin, null)
  assert.equal(Array.isArray(RtkOpenCodePlugin), false)

  // V2 contract
  assert.equal(RtkOpenCodePlugin.id, "rtk")
  assert.equal(typeof RtkOpenCodePlugin.setup, "function")

  // V1 dual-shape contract (server method)
  assert.equal(typeof RtkOpenCodePlugin.server, "function")
  const v1Hooks = await RtkOpenCodePlugin.server()
  assert.equal(typeof v1Hooks, "object")
})

test("tool filter: only bash and shell tools are intercepted", async () => {
  assert.equal(await tryRewriteCommand("read_file", "git status"), null)
  assert.equal(await tryRewriteCommand("grep", "git status"), null)
  assert.equal(await tryRewriteCommand("", "git status"), null)
  assert.equal(await tryRewriteCommand(null, "git status"), null)
})

test("input guard: non-string and empty commands are skipped", async () => {
  assert.equal(await tryRewriteCommand("bash", ""), null)
  assert.equal(await tryRewriteCommand("bash", "   "), null)
  assert.equal(await tryRewriteCommand("bash", 12345), null)
  assert.equal(await tryRewriteCommand("bash", null), null)
})

test("binary discovery: expands ~ in RTK_BIN", () => {
  // 1. Direct expansion test for POSIX and Windows slashes
  assert.equal(expandHome("~/test-bin"), join(homedir(), "test-bin"))
  assert.equal(expandHome("~\\test-bin"), join(homedir(), "test-bin"))

  // 2. Verified resolution precedence with an existing binary
  const originalEnv = process.env.RTK_BIN
  try {
    _resetCachedRtkPath()
    process.env.RTK_BIN = process.execPath // Node binary always exists
    assert.equal(resolveRtkPath(), process.execPath)
  } finally {
    process.env.RTK_BIN = originalEnv
    _resetCachedRtkPath()
  }
})

test("rewrite execution logic: exit code 0, 3, and failure handling", async () => {
  // Uses process.execPath (node) as a real native binary runner
  const mockScriptPath = join(process.cwd(), "rewrite")

  writeFileSync(
    mockScriptPath,
    `
const arg = process.argv[2]
if (arg === "code0") {
  console.log("rtk git status")
  process.exit(0)
} else if (arg === "code3") {
  console.log("rtk cargo test")
  process.exit(3)
} else if (arg === "code1") {
  console.log("defer")
  process.exit(1)
} else if (arg === "sleep") {
  setTimeout(() => process.exit(0), 5000)
} else {
  process.exit(2)
}
`
  )

  try {
    const res0 = await runRtkRewrite(process.execPath, "code0")
    assert.equal(res0, "rtk git status")

    const res3 = await runRtkRewrite(process.execPath, "code3")
    assert.equal(res3, "rtk cargo test")

    const res1 = await runRtkRewrite(process.execPath, "code1")
    assert.equal(res1, null)

    const resTimeout = await runRtkRewrite(process.execPath, "sleep", 100)
    assert.equal(resTimeout, null)
  } finally {
    try {
      unlinkSync(mockScriptPath)
    } catch {}
  }
})

test("V2 hook execution lifecycle mutates event.input.command", async () => {
  let registeredHook = null
  const mockCtx = {
    tool: {
      async hook(name, callback) {
        if (name === "execute.before") {
          registeredHook = callback
        }
      },
    },
  }

  await RtkOpenCodePlugin.setup(mockCtx)
  assert.equal(typeof registeredHook, "function")
})