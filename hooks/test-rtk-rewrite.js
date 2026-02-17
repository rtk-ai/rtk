#!/usr/bin/env node
// Tests for rtk-rewrite.js lexer and rewrite logic
// Note: Uses execSync with hardcoded test inputs (no user input)

const fs = require('fs');
const { execSync } = require('child_process');
const path = require('path');

const hookPath = path.join(__dirname, 'rtk-rewrite.js');

function runHook(command) {
  const input = JSON.stringify({ tool_input: { command } });
  try {
    const result = execSync(`node "${hookPath}"`, {
      input,
      encoding: 'utf-8',
      timeout: 5000
    });
    if (!result.trim()) return null; // No rewrite
    return JSON.parse(result).hookSpecificOutput.updatedInput.command;
  } catch (e) {
    if (e.status === 0) return null; // Clean exit, no rewrite
    throw e;
  }
}

let passed = 0;
let failed = 0;

function test(name, input, expected) {
  const result = runHook(input);
  const ok = result === expected;
  if (ok) {
    passed++;
    console.log(`✓ ${name}`);
  } else {
    failed++;
    console.log(`✗ ${name}`);
    console.log(`  Input:    ${input}`);
    console.log(`  Expected: ${expected}`);
    console.log(`  Got:      ${result}`);
  }
}

console.log('\n=== LEXER TESTS: Quote Awareness ===\n');

// Critical test: quotes with && inside should NOT split
test(
  'git commit with && in message - should NOT split',
  'git commit -m "Fix && cleanup"',
  'rtk git commit -m "Fix && cleanup"'
);

test(
  'git commit with || in message - should NOT split',
  'git commit -m "Option A || Option B"',
  'rtk git commit -m "Option A || Option B"'
);

test(
  'single quotes with operators inside',
  "git commit -m 'Fix; cleanup'",
  "rtk git commit -m 'Fix; cleanup'"
);

console.log('\n=== CHAIN PARSING TESTS ===\n');

test(
  'simple && chain: cd && git status',
  'cd /tmp && git status',
  'cd /tmp && rtk git status'
);

test(
  '|| chain: cmd1 || git status',
  'false || git status',
  'false || rtk git status'
);

test(
  '; chain: cmd1 ; git status',
  'echo hello ; git status',
  'echo hello ; rtk git status'
);

test(
  'triple chain: cd && git add && git commit',
  'cd /tmp && git add . && git commit -m "test"',
  'cd /tmp && rtk git add . && rtk git commit -m "test"'
);

test(
  'mixed operators: cmd && cmd || cmd',
  'cd /tmp && git status || git log',
  'cd /tmp && rtk git status || rtk git log'
);

console.log('\n=== SHELLISM DETECTION (should pass through unchanged) ===\n');

test(
  'glob pattern - should NOT rewrite',
  'ls *.js',
  null
);

test(
  'pipe - should NOT rewrite',
  'git status | grep modified',
  null
);

test(
  'redirect - should NOT rewrite',
  'git log > output.txt',
  null
);

test(
  'subshell $() - should NOT rewrite',
  'echo $(git status)',
  null
);

test(
  'backticks - should NOT rewrite',
  'echo `git status`',
  null
);

console.log('\n=== BASIC COMMAND REWRITES ===\n');

test('git status', 'git status', 'rtk git status');
test('git diff', 'git diff', 'rtk git diff');
test('git log', 'git log --oneline', 'rtk git log --oneline');
test('git add', 'git add .', 'rtk git add .');
test('git commit', 'git commit -m "test"', 'rtk git commit -m "test"');
test('git push', 'git push origin main', 'rtk git push origin main');
test('git pull', 'git pull', 'rtk git pull');
test('git branch', 'git branch -a', 'rtk git branch -a');
test('git stash', 'git stash', 'rtk git stash');
test('git show', 'git show HEAD', 'rtk git show HEAD');

test('gh pr view', 'gh pr view 123', 'rtk gh pr view 123');
test('gh issue list', 'gh issue list', 'rtk gh issue list');
test('gh run list', 'gh run list', 'rtk gh run list');

test('cargo test', 'cargo test', 'rtk cargo test');
test('cargo build', 'cargo build --release', 'rtk cargo build --release');
test('cargo clippy', 'cargo clippy', 'rtk cargo clippy');

test('cat file', 'cat README.md', 'rtk read README.md');
test('grep pattern', 'grep -r "TODO" .', 'rtk grep -r "TODO" .');
test('ls', 'ls -la', 'rtk ls -la');
test('find', 'find . -name "*.js"', 'rtk find . -name "*.js"');

test('vitest', 'vitest run', 'rtk vitest run');
test('npx vitest', 'npx vitest run', 'rtk vitest run');
test('pnpm test', 'pnpm test', 'rtk vitest run');

test('tsc', 'tsc --noEmit', 'rtk tsc --noEmit');
test('eslint', 'eslint src/', 'rtk lint src/');
test('prettier', 'prettier --check .', 'rtk prettier --check .');

test('docker ps', 'docker ps', 'rtk docker ps');
test('kubectl get', 'kubectl get pods', 'rtk kubectl get pods');

test('curl', 'curl https://api.example.com', 'rtk curl https://api.example.com');

test('pytest', 'pytest tests/', 'rtk pytest tests/');
test('go test', 'go test ./...', 'rtk go test ./...');

console.log('\n=== ENV VAR PREFIX HANDLING ===\n');

test(
  'env var prefix: TEST_VAR=1 npx vitest',
  'TEST_VAR=1 npx vitest run',
  'TEST_VAR=1 rtk vitest run'
);

test(
  'multiple env vars',
  'FOO=1 BAR=2 git status',
  'FOO=1 BAR=2 rtk git status'
);

console.log('\n=== SKIP CASES ===\n');

test('already using rtk', 'rtk git status', null);
test('heredoc', 'cat <<EOF\nhello\nEOF', null);
test('unknown command', 'someunknowncmd --flag', null);

console.log('\n=== RESULTS ===\n');
console.log(`Passed: ${passed}`);
console.log(`Failed: ${failed}`);
console.log(`Total:  ${passed + failed}`);

process.exit(failed > 0 ? 1 : 0);
