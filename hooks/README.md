# LLM Agent Hooks

## Scope

**Deployed hook artifacts** — the actual files installed on user machines by `rtk init`. These are shell scripts, TypeScript plugins, and rules files that run outside the Rust binary. They are **thin delegates**: parse agent-specific JSON, call `rtk rewrite` as a subprocess, format agent-specific response. Zero filtering logic lives here.

Owns: per-agent hook scripts and configuration files for 7 supported agents (Claude Code, Copilot, Cursor, Cline, Windsurf, Codex, OpenCode).

Does **not** own: hook installation/uninstallation (that's `src/hooks/init.rs`), the rewrite pattern registry (that's `discover/registry`), or integrity verification (that's `src/hooks/integrity.rs`).

Relationship to `src/hooks/`: that component **creates** these files; this directory **contains** them.

## Purpose

LLM agent integrations that intercept CLI commands and route them through RTK for token optimization. Each hook transparently rewrites raw commands (e.g., `git status`) to their RTK equivalents (e.g., `rtk git status`), delivering 60-90% token savings without requiring the agent or user to change their workflow.

## How It Works

```
Agent runs command (e.g., "cargo test --nocapture")
  -> Hook intercepts (PreToolUse / plugin event)
  -> Reads JSON input, extracts command string
  -> Calls `rtk rewrite "cargo test --nocapture"`
  -> Registry matches pattern, returns "rtk cargo test --nocapture"
  -> Hook sends response in agent-specific JSON format
  -> Agent executes "rtk cargo test --nocapture" instead
  -> Filtered output reaches LLM (~90% fewer tokens)
```

All rewrite logic lives in the Rust binary (`src/discover/registry.rs`). Hook scripts are **thin delegates** that handle agent-specific JSON formats and call `rtk rewrite` for the actual decision. This ensures a single source of truth for all 70+ rewrite patterns.

## Directory Structure

```
hooks/
  claude/       Claude Code (shell hook + settings.json)
  copilot/      GitHub Copilot (VS Code Chat + Copilot CLI)
  cursor/       Cursor IDE (shell hook)
  cline/        Cline / Roo Code VS Code extension (rules file)
  windsurf/     Windsurf / Cascade IDE (rules file)
  codex/        OpenAI Codex CLI (awareness file)
  opencode/     OpenCode (TypeScript plugin)
```

## Supported Agents

| Agent | Mechanism | Hook Type | Can Modify Command? |
|-------|-----------|-----------|---------------------|
| Claude Code | Shell hook (`PreToolUse`) | Transparent rewrite | Yes (`updatedInput`) |
| VS Code Copilot Chat | Rust binary (`rtk hook copilot`) | Transparent rewrite | Yes (`updatedInput`) |
| GitHub Copilot CLI | Rust binary (`rtk hook copilot`) | Deny-with-suggestion | No (agent retries) |
| Cursor | Shell hook (`preToolUse`) | Transparent rewrite | Yes (`updated_input`) |
| Gemini CLI | Rust binary (`rtk hook gemini`) | Transparent rewrite | Yes (`hookSpecificOutput`) |
| Cline / Roo Code | Custom instructions (rules file) | Prompt-level guidance | N/A |
| Windsurf | Custom instructions (rules file) | Prompt-level guidance | N/A |
| Codex CLI | AGENTS.md / instructions | Prompt-level guidance | N/A |
| OpenCode | TypeScript plugin (`tool.execute.before`) | In-place mutation | Yes |

## JSON Formats by Agent

### Claude Code (Shell Hook)

**Input** (stdin):
```json
{
  "tool_name": "Bash",
  "tool_input": { "command": "git status" }
}
```

**Output** (stdout, when rewritten):
```json
{
  "hookSpecificOutput": {
    "hookEventName": "PreToolUse",
    "permissionDecision": "allow",
    "permissionDecisionReason": "RTK auto-rewrite",
    "updatedInput": { "command": "rtk git status" }
  }
}
```

### Cursor (Shell Hook)

**Input**: Same as Claude Code.

**Output** (stdout, when rewritten):
```json
{
  "permission": "allow",
  "updated_input": { "command": "rtk git status" }
}
```

Returns `{}` when no rewrite (Cursor requires JSON for all paths).

### Copilot CLI (Rust Binary)

**Input** (stdin, camelCase, `toolArgs` is JSON-stringified):
```json
{
  "toolName": "bash",
  "toolArgs": "{\"command\": \"git status\"}"
}
```

**Output** (no `updatedInput` support -- uses deny-with-suggestion):
```json
{
  "permissionDecision": "deny",
  "permissionDecisionReason": "Token savings: use `rtk git status` instead"
}
```

### VS Code Copilot Chat (Rust Binary)

**Input** (stdin, snake_case):
```json
{
  "tool_name": "Bash",
  "tool_input": { "command": "git status" }
}
```

**Output**: Same as Claude Code format (with `updatedInput`).

### Gemini CLI (Rust Binary)

**Input** (stdin):
```json
{
  "tool_name": "run_shell_command",
  "tool_input": { "command": "git status" }
}
```

**Output** (when rewritten):
```json
{
  "decision": "allow",
  "hookSpecificOutput": {
    "tool_input": { "command": "rtk git status" }
  }
}
```

**No rewrite**: `{"decision": "allow"}`

### OpenCode (TypeScript Plugin)

Mutates `args.command` in-place via the zx library:
```typescript
const result = await $`rtk rewrite ${command}`.quiet().nothrow()
const rewritten = String(result.stdout).trim()
if (rewritten && rewritten !== command) {
  (args as Record<string, unknown>).command = rewritten
}
```

## Command Rewrite Registry

The registry (`src/discover/registry.rs`) handles 70+ command patterns across these categories:

| Category | Examples | Savings |
|----------|----------|---------|
| Test Runners | vitest, pytest, cargo test, go test, playwright | 90-99% |
| Build Tools | cargo build, npm, pnpm, dotnet, make | 70-90% |
| VCS | git status/log/diff/show | 70-80% |
| Language Servers | tsc, mypy | 80-83% |
| Linters | eslint, ruff, golangci-lint, biome | 80-85% |
| Package Managers | pip, cargo install, pnpm list | 75-80% |
| File Operations | ls, find, grep, cat, head, tail | 60-75% |
| Infrastructure | docker, kubectl, aws, terraform | 75-85% |

### Compound Command Handling

The registry handles `&&`, `||`, `;`, `|`, and `&` operators:

- **Pipe** (`|`): Only the left side is rewritten (right side consumes output format)
- **And/Or/Semicolon** (`&&`, `||`, `;`): Both sides rewritten independently
- **find/fd in pipes**: Never rewritten (output format incompatible with xargs/wc/grep)

Example: `cargo fmt --all && cargo test` becomes `rtk cargo fmt --all && rtk cargo test`

### Override Controls

- **`RTK_DISABLED=1`**: Per-command override (`RTK_DISABLED=1 git status` runs raw)
- **`exclude_commands`**: In `~/.config/rtk/config.toml`, list commands to never rewrite
- **Already-RTK**: `rtk git status` passes through unchanged (no `rtk rtk git`)

## Exit Code Contract

Hooks must **never block command execution**. All error paths (missing binary, bad JSON, rewrite failure) must exit 0 so the agent's command runs unmodified. A hook that exits non-zero prevents the user's command from executing.

When there is no rewrite to apply, the hook must produce no output (or `{}` for Cursor, which requires JSON on all paths).

### Gaps (to be fixed)

- `hook_cmd.rs::run_gemini()` — exits 1 on invalid JSON input instead of exit 0

## Graceful Degradation

Hooks are **non-blocking** -- they never prevent a command from executing:

- jq not installed: warning to stderr, exit 0 (command runs raw)
- rtk binary not found: warning to stderr, exit 0
- rtk version too old (< 0.23.0): warning to stderr, exit 0
- Invalid JSON input: pass through unchanged
- `rtk rewrite` crashes: hook exits 0 (subprocess error ignored)
- Filter logic error: fallback to raw command output

