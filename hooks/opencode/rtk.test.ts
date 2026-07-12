import { afterAll, describe, expect, mock, test } from "bun:test"
import { mkdtemp, readFile, readdir, rm, symlink, utimes, writeFile } from "node:fs/promises"
import { tmpdir } from "node:os"
import { join } from "node:path"

const cacheDir = await mkdtemp(join(tmpdir(), "rtk-opencode-test-"))
process.env.RTK_CACHE_DIR = cacheDir
process.env.RTK_TRIGGER_TOKENS = "3000"
process.env.RTK_TARGET_TOKENS = "2500"
process.env.RTK_MAX_OUTPUT_CHARS = "10000"
delete process.env.RTK_TOKEN_THRESHOLD

function schema() {
  const value = {
    describe: () => value,
    int: () => value,
    max: () => value,
    min: () => value,
    optional: () => value,
  }
  return value
}

const tool = Object.assign(<T>(definition: T) => definition, {
  schema: { number: schema, string: schema },
})

mock.module("@opencode-ai/plugin", () => ({ tool }))

const { default: plugin } = await import("./rtk.ts")

type ShellResult = Promise<{ exitCode: number; stdout: string; stderr: string }> & {
  quiet(): ShellResult
  nothrow(): ShellResult
}

function fakeShell(options: { available?: boolean; rewrite?: string; rewriteExitCode?: number } = {}) {
  const calls: string[] = []
  const available = options.available ?? true
  const shell = (strings: TemplateStringsArray, ...values: unknown[]) => {
    const command = strings.reduce((result, part, index) => result + part + (values[index] ?? ""), "")
    calls.push(command)
    const checkingWithWhich = command.includes("which rtk")
    const stdout = command.includes("rtk rewrite ") ? (options.rewrite ?? "") : ""
    const promise = (
      checkingWithWhich && !available
        ? Promise.reject(new Error("rtk unavailable"))
        : Promise.resolve({
            exitCode: command.includes("rtk rewrite ") ? (options.rewriteExitCode ?? 0) : available ? 0 : 1,
            stdout,
            stderr: "",
          })
    ) as ShellResult
    promise.quiet = () => promise
    promise.nothrow = () => promise
    return promise
  }
  return { calls, shell }
}

async function load(options: { available?: boolean; rewrite?: string; rewriteExitCode?: number } = {}) {
  const fake = fakeShell(options)
  const originalWhich = Bun.which
  ;(Bun as any).which = () => ((options.available ?? true) ? "/test/bin/rtk" : null)
  try {
    const hooks = await plugin({ $: fake.shell } as never)
    return { ...fake, hooks }
  } finally {
    ;(Bun as any).which = originalWhich
  }
}

function text(size: number, needle = "") {
  const line = "result path/to/file.ts:42 payload dependency status=active\n"
  return (needle + line.repeat(Math.ceil(size / line.length))).slice(0, size)
}

function toolInput(toolName: string, args: Record<string, unknown> = {}) {
  return { tool: toolName, sessionID: "session", callID: `${toolName}-call`, args }
}

afterAll(async () => {
  await rm(cacheDir, { force: true, recursive: true })
})

describe("RTK OpenCode plugin", () => {
  test("keeps output compression active without the rtk binary", async () => {
    const { calls, hooks } = await load({ available: false })
    const output = { title: "", metadata: {}, output: text(50000) }

    await hooks["tool.execute.after"]?.(toolInput("grep"), output)

    expect(output.output).toStartWith("[rtk: compressed ")
    expect(hooks.tool?.rtk_retrieve).toBeDefined()
    expect(calls).toHaveLength(0)
  })

  test("never turns a successful tool into a failure", async () => {
    const { hooks } = await load()
    const output = Object.freeze({ title: "", metadata: Object.freeze({}), output: text(50000) })
    const before = (await readdir(cacheDir)).filter((name) => name.endsWith(".txt")).length

    await expect(hooks["tool.execute.after"]?.(toolInput("frozen_tool"), output as never)).resolves.toBeUndefined()
    const after = (await readdir(cacheDir)).filter((name) => name.endsWith(".txt")).length
    expect(after).toBe(before)
  })

  test("enforces the final budget and records structured metrics", async () => {
    const { hooks } = await load()
    const output: any = { title: "", metadata: {}, output: text(50000) }

    await hooks["tool.execute.after"]?.(toolInput("grep"), output)

    expect(output.output.length).toBeLessThanOrEqual(10000)
    expect(output.metadata.rtk).toMatchObject({
      compressed: true,
      finalChars: output.output.length,
      originalChars: 50000,
      strategy: "truncate-tail",
    })
    expect(output.metadata.rtk.savedPct).toBeGreaterThan(10)
  })

  test("preserves OpenCode's full-output path", async () => {
    const { hooks } = await load()
    const path = `/tmp/${"segment with space/".repeat(20)}full-output.txt`
    const output: any = {
      title: "",
      metadata: { outputPath: path, truncated: true },
      output: text(50000),
    }

    await hooks["tool.execute.after"]?.(toolInput("grep"), output)

    expect(output.output).toContain(path)
    expect(output.metadata.rtk.recovery).toEqual({ outputPath: path })
  })

  test("caches and retrieves content omitted by compression", async () => {
    const { hooks } = await load()
    const original = text(60000, "unique-start\n") + "\nneedle-777\nunique-end"
    const output: any = { title: "", metadata: {}, output: original }

    await hooks["tool.execute.after"]?.(toolInput("custom_tool"), output)

    const id = output.output.match(/rtk_retrieve[^\n]*id="([a-f0-9]{24})"/)?.[1]
    expect(id).toBeDefined()
    const retrieve = hooks.tool?.rtk_retrieve as any
    const slice = await retrieve.execute({ id, limit: 200, offset: original.length - 200 }, {})
    const search = await retrieve.execute({ id, limit: 1000, query: "needle-777" }, {})
    expect(slice).toContain("unique-end")
    expect(search).toContain("needle-777")
  })

  test("does not follow a pre-existing cache symlink", async () => {
    const { hooks } = await load()
    const original = text(60000, "symlink-safe\n")
    const first: any = { title: "", metadata: {}, output: original }
    await hooks["tool.execute.after"]?.(toolInput("symlink_tool"), first)
    const id = first.metadata.rtk.recovery.id
    const cacheFile = join(cacheDir, `${id}.txt`)
    const target = join(cacheDir, "external-target.txt")
    await rm(cacheFile, { force: true })
    await writeFile(target, "sentinel")
    await symlink(target, cacheFile)

    const second: any = { title: "", metadata: {}, output: original }
    await hooks["tool.execute.after"]?.(toolInput("symlink_tool"), second)

    expect(await readFile(target, "utf8")).toBe("sentinel")
    const retrieve = hooks.tool?.rtk_retrieve as any
    expect(await retrieve.execute({ id, limit: 100, offset: 0 }, {})).toContain("symlink-safe")
  })

  test("cleans orphaned atomic-write files", async () => {
    const { hooks } = await load()
    const orphan = join(cacheDir, `.${"a".repeat(24)}-1-1.tmp`)
    await writeFile(orphan, "orphan")
    const old = new Date(Date.now() - 10 * 60 * 1000)
    await utimes(orphan, old, old)
    const output: any = { title: "", metadata: {}, output: text(50000, "cleanup\n") }

    await hooks["tool.execute.after"]?.(toolInput("cleanup_tool"), output)

    await expect(readFile(orphan)).rejects.toThrow()
  })

  test("does not steal an old lock from a live cache writer", async () => {
    const { hooks } = await load()
    const lock = join(cacheDir, ".lock")
    await writeFile(lock, JSON.stringify({ pid: process.pid, token: "live-owner" }))
    const old = new Date(Date.now() - 60 * 1000)
    await utimes(lock, old, old)
    const original = text(50000, "lock-contention\n")
    const output: any = { title: "", metadata: {}, output: original }

    await hooks["tool.execute.after"]?.(toolInput("locked_tool"), output)

    expect(output.output).toBe(original)
    expect(await readFile(lock, "utf8")).toContain("live-owner")
    await rm(lock, { force: true })
  })

  test("compresses MCP text while preserving non-text parts", async () => {
    const { hooks } = await load()
    const image = { type: "image", data: "AAAA", mimeType: "image/png" }
    const output: any = {
      content: [{ type: "text", text: text(30000) }, image, { type: "text", text: text(30000) }],
    }

    await hooks["tool.execute.after"]?.(toolInput("mcp_search"), output)

    expect(output.content).toContain(image)
    expect(output.content.map((part: any) => part.type)).toEqual(["text", "image", "text"])
    expect(output.content[0].text).toStartWith("[rtk: compressed ")
    expect(output.content[2].text).toContain("rtk recovery")
  })

  test("keeps skewed MCP text in its original slots and the recovery id contiguous", async () => {
    const { hooks } = await load()
    const image = { type: "image", data: "AAAA", mimeType: "image/png" }
    const output: any = {
      content: [{ type: "text", text: `PRE_IMAGE_SENTINEL\n${text(20)}` }, image, { type: "text", text: text(59980) }],
    }

    await hooks["tool.execute.after"]?.(toolInput("mcp_skewed"), output)

    expect(output.content[0].text).toContain("PRE_IMAGE_SENTINEL")
    expect(output.content[2].text).toMatch(/rtk_retrieve[^\n]*id="[a-f0-9]{24}"/)
  })

  test("accounts for textual MCP resources without dropping their blobs", async () => {
    const { hooks } = await load()
    const resource = {
      type: "resource",
      resource: { blob: "AAAA", mimeType: "application/octet-stream", text: text(30000), uri: "memo://one" },
    }
    const output: any = { content: [{ type: "text", text: text(30000) }, resource] }

    await hooks["tool.execute.after"]?.(toolInput("mcp_resource"), output)

    expect(output.content.map((part: any) => part.type)).toEqual(["text", "resource"])
    expect(output.content[1].resource.blob).toBe("AAAA")
    expect(
      output.content.map((part: any) => (part.type === "text" ? part.text : part.resource.text)).join("\n\n").length,
    ).toBeLessThanOrEqual(10000)
  })

  test("does not mistake block-comment text inside code for a comment", async () => {
    const { hooks } = await load()
    const source = `1: const marker = "/*";\n${Array.from({ length: 1000 }, (_, i) => `${i + 2}: const value_${i} = ${i};`).join("\n")}`
    const output = { title: "", metadata: {}, output: source }

    await hooks["tool.execute.after"]?.(toolInput("read", { filePath: "/tmp/example.ts" }), output)

    expect(output.output).toContain('1: const marker = "/*";')
  })

  test("strips comments from extensionless files", async () => {
    const { hooks } = await load()
    const source = Array.from({ length: 1200 }, (_, i) =>
      i % 2 === 0 ? `${i + 1}: # removable comment ${i}` : `${i + 1}: RUN echo value_${i}`,
    ).join("\n")
    const output = { title: "", metadata: {}, output: source }

    await hooks["tool.execute.after"]?.(toolInput("read", { filePath: "/tmp/Dockerfile" }), output)

    expect(output.output).not.toContain("# removable comment")
    expect(output.output).toContain("RUN echo value_")
  })

  test("leaves oversized uncached MCP output for OpenCode to persist", async () => {
    const { hooks } = await load()
    const original = text(1024 * 1024 + 1)
    const output: any = { content: [{ type: "text", text: original }] }

    await hooks["tool.execute.after"]?.(toolInput("mcp_huge"), output)

    expect(output.content).toEqual([{ type: "text", text: original }])
  })

  test("still bounds oversized standard output when it cannot be cached", async () => {
    const { hooks } = await load()
    const original = text(1024 * 1024 + 1)
    const output: any = { title: "", metadata: {}, output: original }

    await hooks["tool.execute.after"]?.(toolInput("custom_huge"), output)

    expect(output.output.length).toBeLessThanOrEqual(10000)
    expect(output.output).toContain("recovery unavailable")
    expect(output.metadata.rtk.recovery).toHaveProperty("unavailable")
  })

  test("strips terminal control sequences from oversized standard output", async () => {
    const { hooks } = await load()
    const output: any = { title: "", metadata: {}, output: "\u001b[31merror\u001b[0m\n".repeat(70000) }

    await hooks["tool.execute.after"]?.(toolInput("custom_huge_ansi"), output)

    expect(output.output).not.toContain("\u001b[")
  })

  test("strips terminal control sequences before compression", async () => {
    const { hooks } = await load()
    const output = { title: "", metadata: {}, output: "\u001b[31merror\u001b[0m\n".repeat(4000) }

    await hooks["tool.execute.after"]?.(toolInput("custom_tool"), output)

    expect(output.output).not.toContain("\u001b[")
  })

  test("rewrites shell commands only when the capability is available", async () => {
    const missing = await load({ available: false, rewrite: "rtk git status" })
    const available = await load({ available: true, rewrite: "rtk git status" })
    const untouched = { args: { command: "git status" } }
    const rewritten = { args: { command: "git status" } }

    await missing.hooks["tool.execute.before"]?.(toolInput("bash"), untouched)
    await available.hooks["tool.execute.before"]?.(toolInput("bash"), rewritten)

    expect(untouched.args.command).toBe("git status")
    expect(rewritten.args.command).toBe("rtk git status")
  })

  test("lets rtk rewrite later segments of compound commands", async () => {
    const { hooks } = await load({ rewrite: "rtk git status && rtk cargo test" })
    const output = { args: { command: "rtk git status && cargo test" } }

    await hooks["tool.execute.before"]?.(toolInput("bash"), output)

    expect(output.args.command).toBe("rtk git status && rtk cargo test")
  })

  test("accepts permission-gated rewrites returned with exit code 3", async () => {
    const { hooks } = await load({ rewrite: "rtk git status", rewriteExitCode: 3 })
    const output = { args: { command: "git status" } }

    await hooks["tool.execute.before"]?.(toolInput("bash"), output)

    expect(output.args.command).toBe("rtk git status")
  })

  test("retrieves Unicode safely and reports matches that exceed a small limit", async () => {
    const { hooks } = await load()
    const longNeedle = "Q".repeat(100)
    const original = `${"İ".repeat(2000)}NEEDLE-${longNeedle}-${"😀".repeat(4000)}`
    const output: any = { title: "", metadata: {}, output: original.repeat(3) }

    await hooks["tool.execute.after"]?.(toolInput("unicode_tool"), output)

    const id = output.metadata.rtk.recovery.id
    const retrieve = hooks.tool?.rtk_retrieve as any
    const search = await retrieve.execute({ id, limit: 10, query: "needle" }, {})
    const bounded = await retrieve.execute({ id, limit: 20, query: longNeedle }, {})
    const slice = await retrieve.execute({ id, limit: 1, offset: 2109 }, {})
    expect(search).toContain("match at char")
    expect((bounded.split(/\[match at char \d+\]\n/)[1] ?? "").length).toBeLessThanOrEqual(20)
    expect(slice).not.toMatch(/[\uD800-\uDBFF](?![\uDC00-\uDFFF])|(?<![\uD800-\uDBFF])[\uDC00-\uDFFF]/)
  })
})
