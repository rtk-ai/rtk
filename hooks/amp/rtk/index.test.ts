import { afterAll, afterEach, beforeAll, beforeEach, describe, expect, test } from 'bun:test'
import { existsSync, mkdtempSync, mkdirSync, rmSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import type { PluginAPI, ToolCallEvent, ToolCallResult } from '@ampcode/plugin'
import plugin from './index'

type Handler = (event: ToolCallEvent) => ToolCallResult | Promise<ToolCallResult>

// Amp supplies this helper in production. This harness exercises the registered
// hook; loading in Amp and running shell_command is the native smoke check.
let hook: Handler
plugin({
	on(_event: string, handler: Handler) {
		hook = handler
	},
	helpers: {
		shellCommandFromToolCall(event: ToolCallEvent) {
			const command = event.tool === 'Bash' ? event.input.cmd : event.input.command
			if (!['Bash', 'shell_command'].includes(event.tool) || typeof command !== 'string') {
				return null
			}
			return { command, dir: event.input.cwd ?? event.input.workdir }
		},
	},
} as unknown as PluginAPI)

function call(input: Record<string, unknown>, tool = 'shell_command') {
	return hook({ tool, input, toolUseID: 'test', thread: { id: 'T-test' } } as ToolCallEvent)
}

const temp = mkdtempSync(join(tmpdir(), 'amp-rtk-test-'))
afterAll(() => rmSync(temp, { recursive: true, force: true }))

describe('rewrite hook', () => {
	const originalEnv = { ...process.env }
	const bin = join(temp, 'bin')
	beforeAll(() => {
		mkdirSync(bin)
		writeFileSync(
			join(bin, 'rtk'),
			`#!/bin/sh
if [ "$RTK_TEST_HANG" = 1 ]; then exec /bin/sleep 10; fi
if [ "$1" != rewrite ] || [ "$2" != -- ] || [ "$#" != 3 ]; then exit 99; fi
if [ "$RTK_TEST_ECHO" = 1 ]; then
  printf 'rtk %s' "$3"
elif [ "$RTK_TEST_CWD" = 1 ]; then
  printf 'rtk %s' "$PWD"
else
  printf '%s' "$RTK_TEST_OUTPUT"
fi
exit "$RTK_TEST_CODE"
`,
			{ mode: 0o755 },
		)
	})
	beforeEach(() => {
		process.env.PATH = bin
		delete process.env.RTK_DISABLED
		process.env.RTK_TEST_OUTPUT = 'rtk git status'
		process.env.RTK_TEST_CODE = '3'
	})
	afterEach(() => {
		for (const key of ['PATH', 'RTK_DISABLED', 'RTK_TEST_OUTPUT', 'RTK_TEST_CODE', 'RTK_TEST_CWD', 'RTK_TEST_ECHO', 'RTK_TEST_HANG']) {
			if (originalEnv[key] === undefined) delete process.env[key]
			else process.env[key] = originalEnv[key]
		}
	})

	test.each(['0', '3'])('accepts RTK exit %s and preserves every other input field', async (code) => {
		process.env.RTK_TEST_CODE = code
		const input = Object.freeze({
			command: 'git status', workdir: temp, timeout_ms: 731,
			env: { MARKER: 'keep' }, future_option: { nested: [1, 2] },
		})
		expect(await call(input)).toEqual({
			action: 'modify', input: { ...input, command: 'rtk git status' },
		})
		expect(input.command).toBe('git status')
	})

	test('uses the Bash cmd field and preserves cwd and timeout', async () => {
		const input = { cmd: 'git status', cwd: temp, timeout: 83, other: 'keep' }
		expect(await call(input, 'Bash')).toEqual({
			action: 'modify', input: { ...input, cmd: 'rtk git status' },
		})
	})

	test('runs the rewriter in the original tool directory', async () => {
		process.env.RTK_TEST_CWD = '1'
		expect(await call({ command: 'git status', workdir: temp })).toEqual({
			action: 'modify', input: { command: `rtk ${temp}`, workdir: temp },
		})
	})

	test.each(['1', '2', '9', '127'])('leaves commands unchanged on RTK exit %s, even with stdout', async (code) => {
		process.env.RTK_TEST_CODE = code
		expect(await call({ command: 'git status' })).toEqual({ action: 'allow' })
	})

	test.each(['', ' \n\t ', 'git status'])('ignores empty or identical rewrite %j', async (output) => {
		process.env.RTK_TEST_OUTPUT = output
		expect(await call({ command: 'git status' })).toEqual({ action: 'allow' })
	})

	test('ignores other tools, malformed commands, and commands already using RTK', async () => {
		for (const command of [undefined, 42, '', ' \n ', 'rtk', ' \trtk git status', 'rtk git status && git diff']) {
			expect(await call({ command })).toEqual({ action: 'allow' })
		}
		expect(await call({ command: 'git status' }, 'unrelated_tool')).toEqual({ action: 'allow' })
	})

	test('passes the entire command as one argument without executing shell syntax', async () => {
		process.env.RTK_TEST_ECHO = '1'
		const marker = join(temp, 'must-not-exist')
		const command = `git log --format='$(touch ${marker})'; touch ${marker}\ngit status`
		expect(await call({ command })).toEqual({
			action: 'modify', input: { command: `rtk ${command}` },
		})
		expect(existsSync(marker)).toBe(false)
	})

	test('leaves commands unchanged when RTK is disabled or missing', async () => {
		process.env.RTK_DISABLED = '1'
		expect(await call({ command: 'git status' })).toEqual({ action: 'allow' })
		delete process.env.RTK_DISABLED
		process.env.PATH = temp
		expect(await call({ command: 'git status' })).toEqual({ action: 'allow' })
	})

	test('leaves commands unchanged when the rewrite times out', async () => {
		process.env.RTK_TEST_HANG = '1'
		const start = performance.now()
		expect(await call({ command: 'git status' })).toEqual({ action: 'allow' })
		expect(performance.now() - start).toBeLessThan(4_000)
	})
})

describe.skipIf(!Bun.which('rtk'))('real RTK', () => {
	const originalEnv = { ...process.env }
	beforeAll(() => {
		process.env.CLAUDE_CONFIG_DIR = join(temp, 'claude')
		process.env.XDG_CONFIG_HOME = join(temp, 'config')
		process.env.RTK_TELEMETRY_DISABLED = '1'
		delete process.env.RTK_DISABLED
	})
	afterAll(() => {
		for (const key of ['CLAUDE_CONFIG_DIR', 'XDG_CONFIG_HOME', 'RTK_TELEMETRY_DISABLED', 'RTK_DISABLED']) {
			if (originalEnv[key] === undefined) delete process.env[key]
			else process.env[key] = originalEnv[key]
		}
	})

	test('rewrites supported commands and passes through unsupported shell syntax', async () => {
		expect(await call({ command: 'git status', workdir: temp })).toEqual({
			action: 'modify', input: { command: 'rtk git status', workdir: temp },
		})
		for (const command of ['printf hello', 'rtk git status', 'git status > status.txt', "cat <<'EOF'\ngit status\nEOF"]) {
			expect(await call({ command, workdir: temp })).toEqual({ action: 'allow' })
		}
	})

	test('delegates quotes, environment prefixes, chains, and multiline commands to RTK', async () => {
		for (const [command, rewritten] of [
			['git log --format="%s" -3', 'rtk git log --format="%s" -3'],
			['FOO=bar git status', 'FOO=bar rtk git status'],
			['git status && git diff --stat', 'rtk git status && rtk git diff --stat'],
			['git status\ngit diff --stat', 'rtk git status\nrtk git diff --stat'],
		]) {
			expect(await call({ command, workdir: temp })).toEqual({
				action: 'modify', input: { command: rewritten, workdir: temp },
			})
		}
	})

	test('executes a rewritten command in a real Git repository with compressed output', async () => {
		const repo = join(temp, 'repo')
		mkdirSync(repo)
		expect(Bun.spawnSync(['git', 'init', '-q', repo]).exitCode).toBe(0)
		for (let i = 0; i < 30; i++) writeFileSync(join(repo, `example-${i}.txt`), 'hello\n')
		const result = await call({ command: 'git status', workdir: repo, timeout_ms: 5000 })
		expect(result.action).toBe('modify')
		if (result.action !== 'modify') throw new Error('RTK did not rewrite git status')
		const raw = Bun.spawnSync(['sh', '-c', 'git status'], { cwd: repo })
		const compressed = Bun.spawnSync(['sh', '-c', String(result.input.command)], { cwd: repo })
		expect(compressed.exitCode).toBe(raw.exitCode)
		expect(compressed.exitCode).toBe(0)
		expect(compressed.stdout.toString()).toContain('example-')
		expect(compressed.stdout.length).toBeLessThan(raw.stdout.length)
		console.log(`git status: ${raw.stdout.length} -> ${compressed.stdout.length} bytes`)

		expect(Bun.spawnSync(['git', 'add', '.'], { cwd: repo }).exitCode).toBe(0)
		writeFileSync(join(repo, 'example-0.txt'), 'changed\n')
		const diff = await call({ command: 'git diff --exit-code', workdir: repo })
		if (diff.action !== 'modify') throw new Error('RTK did not rewrite git diff')
		const compressedDiff = Bun.spawnSync(['sh', '-c', String(diff.input.command)], { cwd: repo })
		const rawDiff = Bun.spawnSync(['git', 'diff', '--exit-code'], { cwd: repo })
		expect(rawDiff.exitCode).toBe(1)
		expect(compressedDiff.exitCode).toBe(rawDiff.exitCode)
		expect(compressedDiff.stdout.toString()).toContain('example-0.txt')
	})
})
