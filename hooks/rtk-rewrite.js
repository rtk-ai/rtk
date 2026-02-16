#!/usr/bin/env node
// RTK auto-rewrite hook for Claude Code PreToolUse:Bash
// Cross-platform version (Node.js/Bun) - Works on Windows, macOS, Linux
// Transparently rewrites raw commands to their rtk equivalents.
// Outputs JSON with updatedInput to modify the command before execution.

const fs = require('fs');

// Read stdin
let input = '';
try {
  input = fs.readFileSync(0, 'utf-8');
} catch (e) {
  process.exit(0);
}

let data;
try {
  data = JSON.parse(input);
} catch (e) {
  process.exit(0);
}

const cmd = data?.tool_input?.command;
if (!cmd) process.exit(0);

// Skip if already using rtk
if (/\brtk\s/.test(cmd)) process.exit(0);

// Skip heredocs
if (cmd.includes('<<')) process.exit(0);

// Handle chained commands: cd "..." && git status -> cd "..." && rtk git status
// Extract the last command after && for matching
const chainMatch = cmd.match(/^(.+?\s*&&\s*)(.+)$/);
let prefix = '';
let matchCmd = cmd;

if (chainMatch) {
  prefix = chainMatch[1]; // e.g., 'cd "..." && '
  matchCmd = chainMatch[2]; // e.g., 'git status'
}

// Strip leading env var assignments for pattern matching
// e.g., "TEST_SESSION_ID=2 npx playwright test" → match against "npx playwright test"
const envPrefixMatch = matchCmd.match(/^(?:[A-Za-z_][A-Za-z0-9_]*=[^ ]* +)+/);
const envPrefix = envPrefixMatch ? envPrefixMatch[0] : '';
if (envPrefix) {
  matchCmd = matchCmd.slice(envPrefix.length);
}
const cmdBody = matchCmd;

let rewritten = null;

// --- Git commands ---
if (/^git\s/.test(matchCmd)) {
  const gitSubcmd = matchCmd
    .replace(/^git\s+/, '')
    .replace(/(-C|-c)\s+\S+\s*/g, '')
    .replace(/--[a-z-]+=\S+\s*/g, '')
    .replace(/--(no-pager|no-optional-locks|bare|literal-pathspecs)\s*/g, '')
    .trim();

  if (/^(status|diff|log|add|commit|push|pull|branch|fetch|stash|show)(\s|$)/.test(gitSubcmd)) {
    rewritten = `${prefix}${envPrefix}rtk ${cmdBody}`;
  }
}
// --- GitHub CLI ---
else if (/^gh\s+(pr|issue|run|api|release)(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^gh /, 'rtk gh ')}`;
}
// --- Cargo ---
else if (/^cargo\s/.test(matchCmd)) {
  const cargoSubcmd = matchCmd.replace(/^cargo\s+(\+\S+\s+)?/, '');
  if (/^(test|build|clippy|check|install|fmt)(\s|$)/.test(cargoSubcmd)) {
    rewritten = `${prefix}${envPrefix}rtk ${cmdBody}`;
  }
}
// --- File operations ---
else if (/^cat\s+/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^cat /, 'rtk read ')}`;
}
else if (/^(rg|grep)\s+/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^(rg|grep) /, 'rtk grep ')}`;
}
else if (/^ls(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^ls/, 'rtk ls')}`;
}
else if (/^tree(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^tree/, 'rtk tree')}`;
}
else if (/^find\s+/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^find /, 'rtk find ')}`;
}
else if (/^diff\s+/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^diff /, 'rtk diff ')}`;
}
// --- JS/TS tooling ---
else if (/^(pnpm\s+)?(npx\s+)?vitest(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^(pnpm )?(npx )?vitest( run)?/, 'rtk vitest run')}`;
}
else if (/^pnpm\s+test(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^pnpm test/, 'rtk vitest run')}`;
}
else if (/^npm\s+test(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^npm test/, 'rtk npm test')}`;
}
else if (/^npm\s+run\s+/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^npm run /, 'rtk npm ')}`;
}
else if (/^(npx\s+)?vue-tsc(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^(npx )?vue-tsc/, 'rtk tsc')}`;
}
else if (/^pnpm\s+tsc(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^pnpm tsc/, 'rtk tsc')}`;
}
else if (/^(npx\s+)?tsc(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^(npx )?tsc/, 'rtk tsc')}`;
}
else if (/^pnpm\s+lint(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^pnpm lint/, 'rtk lint')}`;
}
else if (/^(npx\s+)?eslint(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^(npx )?eslint/, 'rtk lint')}`;
}
else if (/^(npx\s+)?prettier(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^(npx )?prettier/, 'rtk prettier')}`;
}
else if (/^(npx\s+)?playwright(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^(npx )?playwright/, 'rtk playwright')}`;
}
else if (/^pnpm\s+playwright(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^pnpm playwright/, 'rtk playwright')}`;
}
else if (/^(npx\s+)?prisma(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^(npx )?prisma/, 'rtk prisma')}`;
}
// --- Containers ---
else if (/^docker\s+compose(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^docker /, 'rtk docker ')}`;
}
else if (/^docker\s/.test(matchCmd)) {
  const dockerSubcmd = matchCmd
    .replace(/^docker\s+/, '')
    .replace(/(-H|--context|--config)\s+\S+\s*/g, '')
    .replace(/--[a-z-]+=\S+\s*/g, '')
    .trim();
  if (/^(ps|images|logs|run|build|exec)(\s|$)/.test(dockerSubcmd)) {
    rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^docker /, 'rtk docker ')}`;
  }
}
else if (/^kubectl\s/.test(matchCmd)) {
  const kubeSubcmd = matchCmd
    .replace(/^kubectl\s+/, '')
    .replace(/(--context|--kubeconfig|--namespace|-n)\s+\S+\s*/g, '')
    .replace(/--[a-z-]+=\S+\s*/g, '')
    .trim();
  if (/^(get|logs|describe|apply)(\s|$)/.test(kubeSubcmd)) {
    rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^kubectl /, 'rtk kubectl ')}`;
  }
}
// --- Network ---
else if (/^curl\s+/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^curl /, 'rtk curl ')}`;
}
else if (/^wget\s+/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^wget /, 'rtk wget ')}`;
}
// --- pnpm package management ---
else if (/^pnpm\s+(list|ls|outdated)(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^pnpm /, 'rtk pnpm ')}`;
}
// --- Python tooling ---
else if (/^pytest(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^pytest/, 'rtk pytest')}`;
}
else if (/^python\s+-m\s+pytest(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^python -m pytest/, 'rtk pytest')}`;
}
else if (/^ruff\s+(check|format)(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^ruff /, 'rtk ruff ')}`;
}
else if (/^pip\s+(list|outdated|install|show)(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^pip /, 'rtk pip ')}`;
}
else if (/^uv\s+pip\s+(list|outdated|install|show)(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^uv pip /, 'rtk pip ')}`;
}
// --- Go tooling ---
else if (/^go\s+test(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^go test/, 'rtk go test')}`;
}
else if (/^go\s+build(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^go build/, 'rtk go build')}`;
}
else if (/^go\s+vet(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^go vet/, 'rtk go vet')}`;
}
else if (/^golangci-lint(\s|$)/.test(matchCmd)) {
  rewritten = `${prefix}${envPrefix}${cmdBody.replace(/^golangci-lint/, 'rtk golangci-lint')}`;
}

// If no rewrite needed, exit silently (approves command as-is)
if (!rewritten) process.exit(0);

// Build the updated tool_input with all original fields preserved
const updatedInput = { ...data.tool_input, command: rewritten };

// Output the rewrite instruction
console.log(JSON.stringify({
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    permissionDecision: "allow",
    permissionDecisionReason: "RTK auto-rewrite",
    updatedInput: updatedInput
  }
}));
