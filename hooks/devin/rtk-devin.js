#!/usr/bin/env node
// rtk — Devin CLI lifecycle hook adapter
// Injects RTK instructions into context on SessionStart/UserPromptSubmit/PostCompaction.
// The actual command rewriting is done by the PreToolUse hook "rtk hook devin".
//
// Uses a small state file to avoid re-injecting the full instruction block on
// every user prompt, while still re-injecting after context compaction.

const fs = require('fs');
const path = require('path');

const EVENT = process.argv[2] || 'SessionStart';
const STATE_FILE = path.join(__dirname, '.rtk-active');
const TTL_MS = 60 * 60 * 1000; // re-inject at least once per hour

function readStdin() {
  return new Promise((resolve) => {
    let input = '';
    let done = false;
    function finish() {
      if (done) return;
      done = true;
      resolve(input);
    }
    process.stdin.on('data', (chunk) => { input += chunk; });
    process.stdin.on('end', finish);
    process.stdin.on('error', finish);
    setTimeout(finish, 500).unref();
  });
}

function shouldInject() {
  if (EVENT === 'SessionStart' || EVENT === 'PostCompaction') {
    return true;
  }
  try {
    const stat = fs.statSync(STATE_FILE);
    const age = Date.now() - stat.mtimeMs;
    if (age < TTL_MS) {
      return false;
    }
  } catch (e) {
    // State missing or unreadable: inject.
  }
  return true;
}

function markActive() {
  try {
    fs.writeFileSync(STATE_FILE, EVENT);
  } catch (e) {
    // Best-effort: don't block the session.
  }
}

function instructionsPath() {
  return path.join(__dirname, 'rtk-instructions.md');
}

async function main() {
  // Only inject instructions on context-carrying lifecycle events.
  if (!['SessionStart', 'UserPromptSubmit', 'PostCompaction'].includes(EVENT)) {
    return;
  }

  await readStdin(); // consume stdin (e.g. prompt or source), content not needed

  if (!shouldInject()) {
    return;
  }

  let context;
  try {
    context = fs.readFileSync(instructionsPath(), 'utf8');
  } catch (e) {
    // Fail silently so the session is not blocked if the instructions file is missing.
    return;
  }

  markActive();

  const output = {
    hookSpecificOutput: {
      hookEventName: EVENT,
      additionalContext: context,
    },
  };
  process.stdout.write(JSON.stringify(output));
}

main().catch(() => { /* best-effort: never block the session */ });
