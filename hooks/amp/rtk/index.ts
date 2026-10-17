import type { PluginAPI } from '@ampcode/plugin'

export const description = 'Transparently compress shell output by delegating command rewriting to RTK.'

export default function (amp: PluginAPI) {
	amp.on('tool.call', async (event) => {
		const shell = amp.helpers.shellCommandFromToolCall(event)
		if (!shell?.command.trim() || process.env.RTK_DISABLED === '1') {
			return { action: 'allow' }
		}
		if (/^\s*rtk(?:\s|$)/.test(shell.command)) {
			return { action: 'allow' }
		}

		const commandKey = event.tool === 'Bash' ? 'cmd' : 'command'
		if (event.input[commandKey] !== shell.command) {
			return { action: 'allow' }
		}

		try {
			// Pass the entire command as data; only Amp's original tool executes it.
			const child = Bun.spawn(['rtk', 'rewrite', '--', shell.command], {
				cwd: shell.dir,
				env: process.env,
				stdin: 'ignore',
				stdout: 'pipe',
				stderr: 'ignore',
				timeout: 2_000,
			})
			const [exitCode, stdout] = await Promise.all([
				child.exited,
				new Response(child.stdout).text(),
			])
			const rewritten = stdout.trim()
			// RTK's exit 3 is a valid rewrite with approval left to the host.
			if ((exitCode === 0 || exitCode === 3) && rewritten && rewritten !== shell.command) {
				return { action: 'modify', input: { ...event.input, [commandKey]: rewritten } }
			}
		} catch {
			// Missing RTK, spawn errors, and rewrite failures leave execution unchanged.
		}
		return { action: 'allow' }
	})
}
