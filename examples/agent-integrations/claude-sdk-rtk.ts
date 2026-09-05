import { spawn } from "node:child_process";

type RtkResult = { stdout: string; stderr: string; exitCode: number };

/**
 * Execute a typed, already-classified RTK route in the same process environment
 * as the SDK worker. The SDK's tool callback should call this only for a
 * supported human-readable route; exact/machine routes must stay native.
 */
export function runRtk(
  args: string[],
  options: { cwd: string; maxTokens?: number },
): Promise<RtkResult> {
  return new Promise((resolve, reject) => {
    const child = spawn("rtk", args, {
      cwd: options.cwd,
      shell: false,
      env: {
        ...process.env,
        RTK_MAX_OUTPUT_TOKENS: String(options.maxTokens ?? 2048),
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    let stdout = "";
    let stderr = "";
    child.stdout.setEncoding("utf8").on("data", (chunk) => (stdout += chunk));
    child.stderr.setEncoding("utf8").on("data", (chunk) => (stderr += chunk));
    child.on("error", reject);
    child.on("close", (exitCode) =>
      resolve({ stdout, stderr, exitCode: exitCode ?? 1 }),
    );
  });
}

// SDK wiring sketch (keep the SDK's own tool schema and approval callbacks):
// const result = await runRtk(["git", "status"], { cwd: process.cwd() });
// return { content: [{ type: "text", text: result.stdout }], isError: result.exitCode !== 0 };
