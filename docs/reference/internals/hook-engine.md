---
title: Hook Engine
description: How RTK hooks intercept and rewrite agent commands before execution
sidebar:
  order: 4
---

# Hook Engine

## Architecture

RTK hooks are thin delegates: they parse agent-specific JSON, call `rtk rewrite` as a subprocess, and format the agent-specific response. All rewrite logic lives in the Rust binary (`src/discover/registry.rs`).

```
Agent runs "cargo test"
  → Hook intercepts (PreToolUse / plugin event)
  → Reads JSON input, extracts command string
  → Calls `rtk rewrite "cargo test"`
  → Registry matches pattern, returns "rtk cargo test"
  → Hook formats agent-specific response
  → Agent executes "rtk cargo test"
  → Filtered output reaches LLM
```

## Integration tiers

| Tier | Mechanism | Examples |
|------|-----------|---------|
| **Full hook** | Shell script or Rust binary, intercepts via agent hook API | Claude Code, Cursor, Copilot, Gemini |
| **Plugin** | TypeScript/JS in agent's plugin system | OpenCode |
| **Rules file** | Prompt-level instructions | Cline, Windsurf, Codex |

## Rewrite registry

`src/discover/registry.rs` holds 70+ rewrite patterns organized by category:

| Category | Savings |
|----------|---------|
| Test runners (vitest, pytest, cargo test, go test, playwright) | 90-99% |
| Build tools (cargo build, npm, pnpm, dotnet, make) | 70-90% |
| VCS (git status/log/diff/show) | 70-80% |
| Language servers (tsc, mypy) | 80-83% |
| Linters (eslint, ruff, golangci-lint, biome) | 80-85% |
| Package managers (pip, cargo install, pnpm list) | 75-80% |
| File operations (ls, find, grep) | 60-75% |
| Infrastructure (docker, kubectl, aws) | 75-85% |

## Compound command handling

| Operator | Behavior |
|----------|----------|
| `&&`, `\|\|`, `;` | Both sides rewritten independently |
| `\|` (pipe) | Left side only (right side consumes output format) |
| `find`/`fd` in pipes | Never rewritten (incompatible with xargs/wc/grep) |

Example: `cargo fmt --all && cargo test` → `rtk cargo fmt --all && rtk cargo test`

## Exit code contract

**Hooks must never block command execution.** All error paths must exit 0. A hook that exits non-zero prevents the user's command from running.

Failure modes handled gracefully:
- RTK binary not found → warning to stderr, exit 0
- Invalid JSON input → pass through unchanged
- RTK version too old (< 0.23.0) → warning + exit 0
- `rtk rewrite` crashes → hook exits 0

## rtk init

`src/hooks/init.rs` installs hook files for each supported agent:

```bash
rtk init --global              # Claude Code
rtk init --global --cursor     # Cursor
rtk init --global --copilot    # VS Code Copilot
rtk init --global --gemini     # Gemini CLI
rtk init --global --opencode   # OpenCode
rtk init --cline               # Cline (project-local)
rtk init --windsurf            # Windsurf (project-local)
rtk init --codex               # Codex CLI
```

## rtk verify

`src/hooks/verify_cmd.rs` runs inline TOML filter tests and checks hook integrity via SHA-256:

```bash
rtk verify     # all filters pass / fail with diff
rtk init --show    # hook status per agent
```

## Override controls

```bash
RTK_DISABLED=1 git status    # skip rewrite for one command
```

```toml
# ~/.config/rtk/config.toml
[hooks]
exclude_commands = ["git rebase", "docker exec"]
```
