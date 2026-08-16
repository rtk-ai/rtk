/**
 * Prime Agent extension for RTK command rewriting.
 *
 * Prime Agent exposes shell execution through its `ipython` tool. This
 * extension rewrites commands in `%%bash` cells using `rtk rewrite` before
 * the cell is executed, preserving commands for which RTK has no proxy.
 */
import type { ExtensionAPI } from "@earendil-works/pi-coding-agent";

const BASH_CELL = /^(%%bash(?:[ \t]+[^\r\n]*)?)(\r?\n)([\s\S]*)$/;
// `rtk rewrite` uses status 3 to indicate that a rewrite was produced.
const REWRITE_APPLIED = 3;

async function rewriteLine(pi: ExtensionAPI, line: string): Promise<string> {
  const indent = line.match(/^\s*/)?.[0] ?? "";
  const command = line.slice(indent.length);
  if (!command || command.startsWith("#") || command.endsWith("\\")) return line;
  // Shell syntax that is not a standalone command should be left unchanged.
  if (/^(if|then|else|elif|fi|for|while|do|done|case|esac|function)\b/.test(command)) return line;
  try {
    const result = await pi.exec("rtk", ["rewrite", command], { timeout: 5000 });
    const rewritten = result.stdout.trim();
    return (result.code === 0 || result.code === REWRITE_APPLIED) && rewritten
      ? indent + rewritten
      : line;
  } catch {
    // RTK is an optimization; fail open if it is unavailable.
    return line;
  }
}

export default function (pi: ExtensionAPI) {
  pi.on("tool_call", async (event) => {
    if (event.toolName !== "ipython" || typeof event.input?.code !== "string") return;
    const original = event.input.code;
    const match = original.match(BASH_CELL);
    if (!match) return;

    const [, header, newline, body] = match;
    // Preserve empty cells exactly as supplied.
    if (!body.trim()) return;

    const lines = body.split(/\r?\n/);
    const rewritten = await Promise.all(lines.map((line) => rewriteLine(pi, line)));
    const rewrittenBody = rewritten.join("\n");

    // Never replace a non-empty cell with an empty body. RTK is optional, so
    // retain the original input if rewriting produces an invalid result.
    event.input.code = rewrittenBody.trim()
      ? `${header}${newline}${rewrittenBody}`
      : original;
  });
}
