import { execFile } from "node:child_process";
import { promisify } from "node:util";
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";
import { isToolCallEventType } from "@earendil-works/pi-coding-agent";

const execFileAsync = promisify(execFile);
const REWRITE_TIMEOUT_MS = 2_000;
const MAX_REWRITE_OUTPUT_BYTES = 1024 * 1024;

async function rewriteCommand(command: string, signal?: AbortSignal): Promise<string> {
  try {
    const { stdout } = await execFileAsync("rtk", ["rewrite", command], {
      timeout: REWRITE_TIMEOUT_MS,
      maxBuffer: MAX_REWRITE_OUTPUT_BYTES,
      signal,
    });
    const rewritten = stdout.trim();
    return rewritten || command;
  } catch (error) {
    // `rtk rewrite` can return 3 when the underlying permission verdict is "ask".
    // Pi does not use RTK's permission verdicts, but stdout still contains the safe rewrite.
    const maybeError = error as { code?: unknown; stdout?: unknown };
    if (maybeError.code === 3 && typeof maybeError.stdout === "string") {
      const rewritten = maybeError.stdout.trim();
      if (rewritten) return rewritten;
    }

    // Fail open: a missing RTK binary, timeout, bad input, or rewrite error must never
    // prevent Pi from running the user's original bash command.
    return command;
  }
}

export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async (event, ctx) => {
    try {
      if (!isToolCallEventType("bash", event)) return;

      const command = event.input.command;
      if (typeof command !== "string" || command.trim() === "") return;

      const rewritten = await rewriteCommand(command, ctx.signal);
      if (rewritten && rewritten !== command) {
        event.input.command = rewritten;
      }
    } catch {
      // Fail open for unexpected extension/runtime errors too. Pi should run the
      // original command rather than block the bash tool because RTK integration failed.
      return;
    }
  });
}
