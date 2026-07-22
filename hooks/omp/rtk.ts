// RTK OMP extension — rewrites bash commands to use rtk for token savings.
// Requires: rtk >= 0.23.0 in PATH.
//
// This is a thin delegating extension: all rewrite logic lives in `rtk rewrite`,
// which is the single source of truth (src/discover/registry.rs).
// To add or change rewrite rules, edit the Rust registry — not this file.
//
// Same architecture as the Pi extension. Uses OMP's `tool_call` event to
// intercept bash commands BEFORE execution, calling `rtk rewrite` to obtain
// the token-optimized equivalent, then mutating `event.input.command` in-place.
//
// Exit code contract for `rtk rewrite`:
//   0 + stdout  Rewrite found → mutate command
//   1           No RTK equivalent → pass through unchanged
//   3 + stdout  Rewrite (advisory) → mutate command

// Minimal local interfaces — no import dependency, works across OMP versions.
interface ExecResult {
	stdout: string;
	stderr: string;
	code: number;
	killed: boolean;
}
interface ToolCallEvent {
	type: "tool_call";
	toolName: string;
	toolCallId: string;
	input: Record<string, unknown>;
}
interface ExtensionContext {
	cwd: string;
	signal?: AbortSignal;
	ui?: {
		setStatus(key: string, text: string | undefined): void;
	};
}
interface ExtensionAPI {
	setLabel(label: string): void;
	on(
		event: "tool_call",
		handler: (
			event: ToolCallEvent,
			ctx: ExtensionContext,
		) =>
			| Promise<{ block?: boolean; reason?: string } | void>
			| { block?: boolean; reason?: string }
			| void,
	): void;
	on(
		event: "session_start",
		handler: (
			event: unknown,
			ctx: ExtensionContext,
		) => Promise<void> | void,
	): void;
	exec(
		command: string,
		args: string[],
		options?: { cwd?: string; timeout?: number; signal?: AbortSignal },
	): Promise<ExecResult>;
	logger: { warn(msg: string): void; error(msg: string): void };
}

const REWRITE_TIMEOUT_MS = 2_000;
const MIN_SUPPORTED_RTK_MINOR = 23;

// Parse "X.Y.Z" semver, return [major, minor, patch] or null.
function parseSemver(raw: string): [number, number, number] | null {
	const m = raw.trim().match(/(\d+)\.(\d+)\.(\d+)/);
	if (!m) return null;
	return [parseInt(m[1], 10), parseInt(m[2], 10), parseInt(m[3], 10)];
}

// Calls `rtk rewrite`; returns the rewritten command or null (pass through).
async function rewriteCommand(
	pi: ExtensionAPI,
	cmd: string,
	signal?: AbortSignal,
): Promise<string | null> {
	const result = await pi.exec("rtk", ["rewrite", cmd], {
		timeout: REWRITE_TIMEOUT_MS,
		signal,
	});
	if (result.killed) return null;
	if (result.code !== 0 && result.code !== 3) return null;
	return result.stdout.trim() || null;
}

export default async function (pi: ExtensionAPI): Promise<void> {
	pi.setLabel("RTK");

	// Probe rtk version at load time; disables extension if missing or too old.
	const ver = await pi.exec("rtk", ["--version"], { timeout: REWRITE_TIMEOUT_MS });
	if (ver.code !== 0) {
		pi.logger.warn("[rtk] rtk binary not found in PATH — extension disabled");
		pi.on("session_start", (_event, ctx) => {
			ctx?.ui?.setStatus("rtk", "RTK extension disabled: rtk binary not found in PATH.");
		});
		return;
	}

	const parsed = parseSemver(ver.stdout.replace(/^rtk\s+/, ""));
	if (parsed) {
		const [major, minor] = parsed;
		if (major === 0 && minor < MIN_SUPPORTED_RTK_MINOR) {
			pi.logger.warn(
				`[rtk] rtk ${ver.stdout.trim()} is too old (need >= 0.23.0) — extension disabled`,
			);
			return;
		}
	}

	pi.on("tool_call", async (event, ctx) => {
		try {
			if (event.toolName !== "bash") return;

			const command = event.input?.command;
			if (typeof command !== "string" || command.trim() === "") return;

			// Skip already-rewritten or disabled.
			if (command.trimStart().startsWith("rtk ")) return;
			if (process.env.RTK_DISABLED === "1") return;

			// Delegate to RTK.
			const rewritten = await rewriteCommand(pi, command, ctx?.signal);
			if (rewritten && rewritten !== command) {
				event.input.command = rewritten;
			}
		} catch (err) {
			// Fail open: never block execution on an unexpected error.
			pi.logger.warn(
				`[rtk] unexpected error in tool_call handler; passing through command: ${err instanceof Error ? err.message : String(err)}`,
			);
		}

		// Return undefined = transparent passthrough (no block).
	});
}
