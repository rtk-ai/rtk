#!/usr/bin/env node
const { execFileSync } = require("node:child_process");

function readInput() {
  const chunks = [];
  process.stdin.on("data", (chunk) => chunks.push(chunk));
  process.stdin.on("end", () => {
    const input = Buffer.concat(chunks).toString("utf8");
    processInput(input);
  });
  process.stdin.resume();
}

function processInput(inputRaw) {
  let input;
  try {
    input = JSON.parse(inputRaw);
  } catch {
    return;
  }

  const command = input?.tool_input?.command;
  if (!command) {
    return;
  }

  let rewritten;
  try {
    rewritten = execFileSync("rtk", ["rewrite", command], {
      encoding: "utf8",
      stdio: ["ignore", "pipe", "ignore"],
      timeout: 5000,
    }).trim();
  } catch {
    return;
  }

  if (!rewritten || rewritten === command) {
    return;
  }

  const updatedInput = { ...input.tool_input, command: rewritten };
  process.stdout.write(
    JSON.stringify({
      hookSpecificOutput: {
        hookEventName: "PreToolUse",
        permissionDecision: "allow",
        permissionDecisionReason: "RTK auto-rewrite",
        updatedInput,
      },
    }),
  );
}

readInput();
