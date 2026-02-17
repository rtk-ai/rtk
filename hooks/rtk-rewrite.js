#!/usr/bin/env node
// RTK auto-rewrite hook for Claude Code PreToolUse:Bash
// Cross-platform version (Node.js/Bun) - Works on Windows, macOS, Linux
// Transparently rewrites raw commands to their rtk equivalents.
// Outputs JSON with updatedInput to modify the command before execution.

const fs = require('fs');

// ============================================================================
// LEXER - Quote-aware tokenizer for shell commands
// ============================================================================

const TokenKind = {
  ARG: 'arg',
  OPERATOR: 'operator',  // &&, ||, ;
  PIPE: 'pipe',          // |
  REDIRECT: 'redirect',  // >, >>, <, 2>, etc.
  SHELLISM: 'shellism',  // *, ?, $(), ``, etc.
};

/**
 * Tokenize a shell command string, respecting quotes.
 * Returns array of { kind, value } tokens.
 */
function tokenize(input) {
  const tokens = [];
  let i = 0;
  let current = '';
  let inSingleQuote = false;
  let inDoubleQuote = false;

  const pushArg = () => {
    if (current) {
      // Check for shellisms in unquoted content
      // Skip if the token is fully quoted (starts and ends with same quote)
      const isFullyQuoted = (current.startsWith('"') && current.endsWith('"')) ||
                            (current.startsWith("'") && current.endsWith("'"));
      if (!isFullyQuoted && /[*?]|\$\(|`/.test(current)) {
        tokens.push({ kind: TokenKind.SHELLISM, value: current });
      } else {
        tokens.push({ kind: TokenKind.ARG, value: current });
      }
      current = '';
    }
  };

  while (i < input.length) {
    const ch = input[i];
    const next = input[i + 1];

    // Handle escape sequences
    if (ch === '\\' && !inSingleQuote) {
      current += ch + (next || '');
      i += 2;
      continue;
    }

    // Handle quotes
    if (ch === "'" && !inDoubleQuote) {
      current += ch;
      inSingleQuote = !inSingleQuote;
      i++;
      continue;
    }

    if (ch === '"' && !inSingleQuote) {
      current += ch;
      inDoubleQuote = !inDoubleQuote;
      i++;
      continue;
    }

    // Inside quotes, just accumulate
    if (inSingleQuote || inDoubleQuote) {
      current += ch;
      i++;
      continue;
    }

    // Whitespace - delimiter
    if (/\s/.test(ch)) {
      pushArg();
      i++;
      continue;
    }

    // Operators: &&, ||, ;
    if (ch === '&' && next === '&') {
      pushArg();
      tokens.push({ kind: TokenKind.OPERATOR, value: '&&' });
      i += 2;
      continue;
    }

    if (ch === '|' && next === '|') {
      pushArg();
      tokens.push({ kind: TokenKind.OPERATOR, value: '||' });
      i += 2;
      continue;
    }

    if (ch === ';') {
      pushArg();
      tokens.push({ kind: TokenKind.OPERATOR, value: ';' });
      i++;
      continue;
    }

    // Pipe: |
    if (ch === '|') {
      pushArg();
      tokens.push({ kind: TokenKind.PIPE, value: '|' });
      i++;
      continue;
    }

    // Redirects: >, >>, <, 2>, 2>&1, etc.
    if (ch === '>' || ch === '<') {
      pushArg();
      let redirect = ch;
      if (next === '>' || next === '&') {
        redirect += next;
        i++;
      }
      tokens.push({ kind: TokenKind.REDIRECT, value: redirect });
      i++;
      continue;
    }

    // Check for 2> redirect
    if (ch === '2' && (next === '>' || next === '&')) {
      pushArg();
      let redirect = '2';
      i++;
      while (i < input.length && /[>&]/.test(input[i])) {
        redirect += input[i];
        i++;
      }
      tokens.push({ kind: TokenKind.REDIRECT, value: redirect });
      continue;
    }

    // Regular character
    current += ch;
    i++;
  }

  pushArg();
  return tokens;
}

/**
 * Check if tokens contain shellisms that need real shell
 */
function needsShell(tokens) {
  return tokens.some(t =>
    t.kind === TokenKind.SHELLISM ||
    t.kind === TokenKind.PIPE ||
    t.kind === TokenKind.REDIRECT
  );
}

/**
 * Parse tokens into command chain.
 * Returns array of { args: string[], operator: string|null }
 */
function parseChain(tokens) {
  const commands = [];
  let currentArgs = [];

  for (const token of tokens) {
    if (token.kind === TokenKind.OPERATOR) {
      if (currentArgs.length > 0) {
        commands.push({ args: currentArgs, operator: token.value });
        currentArgs = [];
      }
    } else if (token.kind === TokenKind.ARG) {
      currentArgs.push(token.value);
    }
    // Skip PIPE, REDIRECT, SHELLISM - handled by needsShell
  }

  // Last command (no trailing operator)
  if (currentArgs.length > 0) {
    commands.push({ args: currentArgs, operator: null });
  }

  return commands;
}

/**
 * Strip quotes from a string for pattern matching
 */
function stripQuotes(s) {
  if ((s.startsWith('"') && s.endsWith('"')) ||
      (s.startsWith("'") && s.endsWith("'"))) {
    return s.slice(1, -1);
  }
  return s;
}

/**
 * Reconstruct a command from args array
 */
function argsToString(args) {
  return args.join(' ');
}

// ============================================================================
// REWRITE RULES
// ============================================================================

/**
 * Check if a command should be rewritten and return the rewritten version.
 * Returns null if no rewrite needed.
 */
function rewriteCommand(args) {
  if (args.length === 0) return null;

  const binary = stripQuotes(args[0]);
  const cmdStr = argsToString(args);

  // Skip if already using rtk
  if (binary === 'rtk') return null;

  // Strip leading env var assignments for pattern matching
  let matchArgs = args;
  let envPrefix = '';
  while (matchArgs.length > 0 && /^[A-Za-z_][A-Za-z0-9_]*=/.test(matchArgs[0])) {
    envPrefix += matchArgs[0] + ' ';
    matchArgs = matchArgs.slice(1);
  }

  if (matchArgs.length === 0) return null;

  const matchBinary = stripQuotes(matchArgs[0]);
  const matchCmdStr = argsToString(matchArgs);

  // --- Git commands ---
  if (matchBinary === 'git' && matchArgs.length > 1) {
    // Find the subcommand (skip flags like -C, -c, --no-pager)
    let subcmdIdx = 1;
    while (subcmdIdx < matchArgs.length) {
      const arg = matchArgs[subcmdIdx];
      if (arg === '-C' || arg === '-c') {
        subcmdIdx += 2; // skip flag and its value
      } else if (arg.startsWith('--')) {
        subcmdIdx++;
      } else {
        break;
      }
    }
    if (subcmdIdx < matchArgs.length) {
      const subcmd = stripQuotes(matchArgs[subcmdIdx]);
      if (/^(status|diff|log|add|commit|push|pull|branch|fetch|stash|show|worktree)$/.test(subcmd)) {
        return `${envPrefix}rtk ${matchCmdStr}`;
      }
    }
  }

  // --- GitHub CLI ---
  if (matchBinary === 'gh' && matchArgs.length > 1) {
    const subcmd = stripQuotes(matchArgs[1]);
    if (/^(pr|issue|run|api|release)$/.test(subcmd)) {
      return `${envPrefix}rtk ${matchCmdStr}`;
    }
  }

  // --- Cargo ---
  if (matchBinary === 'cargo' && matchArgs.length > 1) {
    let subcmdIdx = 1;
    // Skip toolchain specifier like +nightly
    if (matchArgs[1] && matchArgs[1].startsWith('+')) {
      subcmdIdx = 2;
    }
    if (subcmdIdx < matchArgs.length) {
      const subcmd = stripQuotes(matchArgs[subcmdIdx]);
      if (/^(test|build|clippy|check|install|fmt)$/.test(subcmd)) {
        return `${envPrefix}rtk ${matchCmdStr}`;
      }
    }
  }

  // --- File operations ---
  if (matchBinary === 'cat') {
    return `${envPrefix}rtk read ${argsToString(matchArgs.slice(1))}`;
  }
  if (matchBinary === 'rg' || matchBinary === 'grep') {
    return `${envPrefix}rtk grep ${argsToString(matchArgs.slice(1))}`;
  }
  if (matchBinary === 'ls') {
    return `${envPrefix}rtk ls ${argsToString(matchArgs.slice(1))}`.trim();
  }
  if (matchBinary === 'tree') {
    return `${envPrefix}rtk tree ${argsToString(matchArgs.slice(1))}`.trim();
  }
  if (matchBinary === 'find') {
    return `${envPrefix}rtk find ${argsToString(matchArgs.slice(1))}`;
  }
  if (matchBinary === 'diff') {
    return `${envPrefix}rtk diff ${argsToString(matchArgs.slice(1))}`;
  }

  // --- JS/TS tooling ---
  if (matchBinary === 'vitest' ||
      (matchBinary === 'npx' && matchArgs[1] === 'vitest') ||
      (matchBinary === 'pnpm' && matchArgs[1] === 'vitest')) {
    const rest = matchBinary === 'vitest' ? matchArgs.slice(1) : matchArgs.slice(2);
    const restStr = argsToString(rest).replace(/^run\s*/, '');
    return `${envPrefix}rtk vitest run ${restStr}`.trim();
  }
  if (matchBinary === 'pnpm' && matchArgs[1] === 'test') {
    return `${envPrefix}rtk vitest run ${argsToString(matchArgs.slice(2))}`.trim();
  }
  if (matchBinary === 'npm' && matchArgs[1] === 'test') {
    return `${envPrefix}rtk npm test ${argsToString(matchArgs.slice(2))}`.trim();
  }
  if (matchBinary === 'npm' && matchArgs[1] === 'run') {
    return `${envPrefix}rtk npm ${argsToString(matchArgs.slice(2))}`;
  }

  // TypeScript
  if (matchBinary === 'tsc' || matchBinary === 'vue-tsc' ||
      (matchBinary === 'npx' && (matchArgs[1] === 'tsc' || matchArgs[1] === 'vue-tsc')) ||
      (matchBinary === 'pnpm' && matchArgs[1] === 'tsc')) {
    const rest = matchBinary === 'tsc' || matchBinary === 'vue-tsc' ? matchArgs.slice(1) : matchArgs.slice(2);
    return `${envPrefix}rtk tsc ${argsToString(rest)}`.trim();
  }

  // Linting
  if (matchBinary === 'eslint' || (matchBinary === 'npx' && matchArgs[1] === 'eslint')) {
    const rest = matchBinary === 'eslint' ? matchArgs.slice(1) : matchArgs.slice(2);
    return `${envPrefix}rtk lint ${argsToString(rest)}`.trim();
  }
  if (matchBinary === 'pnpm' && matchArgs[1] === 'lint') {
    return `${envPrefix}rtk lint ${argsToString(matchArgs.slice(2))}`.trim();
  }

  // Prettier
  if (matchBinary === 'prettier' || (matchBinary === 'npx' && matchArgs[1] === 'prettier')) {
    const rest = matchBinary === 'prettier' ? matchArgs.slice(1) : matchArgs.slice(2);
    return `${envPrefix}rtk prettier ${argsToString(rest)}`.trim();
  }

  // Playwright
  if (matchBinary === 'playwright' ||
      (matchBinary === 'npx' && matchArgs[1] === 'playwright') ||
      (matchBinary === 'pnpm' && matchArgs[1] === 'playwright')) {
    const rest = matchBinary === 'playwright' ? matchArgs.slice(1) : matchArgs.slice(2);
    return `${envPrefix}rtk playwright ${argsToString(rest)}`.trim();
  }

  // Prisma
  if (matchBinary === 'prisma' || (matchBinary === 'npx' && matchArgs[1] === 'prisma')) {
    const rest = matchBinary === 'prisma' ? matchArgs.slice(1) : matchArgs.slice(2);
    return `${envPrefix}rtk prisma ${argsToString(rest)}`.trim();
  }

  // --- Containers ---
  if (matchBinary === 'docker') {
    if (matchArgs.length > 1) {
      const subcmd = stripQuotes(matchArgs[1]);
      if (subcmd === 'compose' || /^(ps|images|logs|run|build|exec)$/.test(subcmd)) {
        return `${envPrefix}rtk docker ${argsToString(matchArgs.slice(1))}`;
      }
    }
  }
  if (matchBinary === 'kubectl' && matchArgs.length > 1) {
    // Skip context/namespace flags to find subcommand
    let subcmdIdx = 1;
    while (subcmdIdx < matchArgs.length) {
      const arg = matchArgs[subcmdIdx];
      if (arg === '--context' || arg === '--kubeconfig' || arg === '--namespace' || arg === '-n') {
        subcmdIdx += 2;
      } else if (arg.startsWith('--')) {
        subcmdIdx++;
      } else {
        break;
      }
    }
    if (subcmdIdx < matchArgs.length) {
      const subcmd = stripQuotes(matchArgs[subcmdIdx]);
      if (/^(get|logs|describe|apply)$/.test(subcmd)) {
        return `${envPrefix}rtk kubectl ${argsToString(matchArgs.slice(1))}`;
      }
    }
  }

  // --- Network ---
  if (matchBinary === 'curl') {
    return `${envPrefix}rtk curl ${argsToString(matchArgs.slice(1))}`;
  }
  if (matchBinary === 'wget') {
    return `${envPrefix}rtk wget ${argsToString(matchArgs.slice(1))}`;
  }

  // --- pnpm package management ---
  if (matchBinary === 'pnpm' && matchArgs.length > 1) {
    const subcmd = stripQuotes(matchArgs[1]);
    if (/^(list|ls|outdated)$/.test(subcmd)) {
      return `${envPrefix}rtk pnpm ${argsToString(matchArgs.slice(1))}`;
    }
  }

  // --- Python tooling ---
  if (matchBinary === 'pytest') {
    return `${envPrefix}rtk pytest ${argsToString(matchArgs.slice(1))}`.trim();
  }
  if (matchBinary === 'python' && matchArgs[1] === '-m' && matchArgs[2] === 'pytest') {
    return `${envPrefix}rtk pytest ${argsToString(matchArgs.slice(3))}`.trim();
  }
  if (matchBinary === 'ruff' && matchArgs.length > 1) {
    const subcmd = stripQuotes(matchArgs[1]);
    if (/^(check|format)$/.test(subcmd)) {
      return `${envPrefix}rtk ruff ${argsToString(matchArgs.slice(1))}`;
    }
  }
  if (matchBinary === 'pip' && matchArgs.length > 1) {
    const subcmd = stripQuotes(matchArgs[1]);
    if (/^(list|outdated|install|show)$/.test(subcmd)) {
      return `${envPrefix}rtk pip ${argsToString(matchArgs.slice(1))}`;
    }
  }
  if (matchBinary === 'uv' && matchArgs[1] === 'pip' && matchArgs.length > 2) {
    const subcmd = stripQuotes(matchArgs[2]);
    if (/^(list|outdated|install|show)$/.test(subcmd)) {
      return `${envPrefix}rtk pip ${argsToString(matchArgs.slice(2))}`;
    }
  }

  // --- Go tooling ---
  if (matchBinary === 'go' && matchArgs.length > 1) {
    const subcmd = stripQuotes(matchArgs[1]);
    if (/^(test|build|vet)$/.test(subcmd)) {
      return `${envPrefix}rtk go ${argsToString(matchArgs.slice(1))}`;
    }
  }
  if (matchBinary === 'golangci-lint') {
    return `${envPrefix}rtk golangci-lint ${argsToString(matchArgs.slice(1))}`.trim();
  }

  return null;
}

// ============================================================================
// MAIN
// ============================================================================

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

// Skip heredocs entirely
if (cmd.includes('<<')) process.exit(0);

// Tokenize the command
const tokens = tokenize(cmd);

// If command has pipes/redirects/shellisms, pass through unchanged
if (needsShell(tokens)) process.exit(0);

// Parse into command chain
const chain = parseChain(tokens);
if (chain.length === 0) process.exit(0);

// Rewrite each command in the chain
let anyRewritten = false;
const rewrittenParts = [];

for (const { args, operator } of chain) {
  const rewritten = rewriteCommand(args);
  if (rewritten) {
    anyRewritten = true;
    rewrittenParts.push(rewritten);
  } else {
    rewrittenParts.push(argsToString(args));
  }
  if (operator) {
    rewrittenParts.push(` ${operator} `);
  }
}

// If nothing was rewritten, exit silently
if (!anyRewritten) process.exit(0);

// Reconstruct the full command
const rewrittenCmd = rewrittenParts.join('').trim();

// Build the updated tool_input with all original fields preserved
const updatedInput = { ...data.tool_input, command: rewrittenCmd };

// Output the rewrite instruction
console.log(JSON.stringify({
  hookSpecificOutput: {
    hookEventName: "PreToolUse",
    permissionDecision: "allow",
    permissionDecisionReason: "RTK auto-rewrite",
    updatedInput: updatedInput
  }
}));
