# Hook System

> See also [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md) for the full architecture overview | [hooks/](../../hooks/README.md) for deployed hook artifacts

## Scope

The **lifecycle management** layer for LLM agent hooks: install, uninstall, verify integrity, audit usage, and manage trust. This component creates and maintains the hook artifacts that live in `hooks/` (root), but does **not** execute rewrite logic itself — that lives in `discover/registry`.

Owns: `rtk init` installation flows (4 agents via `AgentTarget` enum + 3 special modes: Gemini, Codex, OpenCode), SHA-256 integrity verification, hook version checking, audit log analysis, `rtk rewrite` CLI entry point, and TOML filter trust management.

Does **not** own: the deployed hook scripts themselves (that's `hooks/`), the rewrite pattern registry (that's `discover/`), or command filtering (that's `cmds/`).

Boundary notes:
- `rewrite_cmd.rs` is a thin CLI bridge — it exists to serve hooks (hooks call `rtk rewrite` as a subprocess) and delegates entirely to `discover/registry`.
- `trust.rs` gates project-local TOML filter execution. It lives here because the trust workflow is tied to hook-installed filter discovery, not to the core filter engine.

## Purpose
LLM agent integration layer that installs, validates, and executes command-rewriting hooks for AI coding assistants. Hooks intercept raw CLI commands (e.g., `git status`) and rewrite them to RTK equivalents (e.g., `rtk git status`) so that LLM agents automatically benefit from token savings without explicit user configuration.

## Installation Modes

`rtk init` supports 6 distinct installation flows:

| Mode | Command | Creates | Patches |
|------|---------|---------|---------|
| Default (global) | `rtk init -g` | Hook, SHA-256 hash, RTK.md | settings.json, CLAUDE.md |
| Hook only | `rtk init -g --hook-only` | Hook, SHA-256 hash | settings.json |
| Claude-MD (legacy) | `rtk init --claude-md` | 134-line RTK block | CLAUDE.md |
| Windsurf | `rtk init -g --agent windsurf` | `.windsurfrules` | -- |
| Cline | `rtk init --agent cline` | `.clinerules` | -- |
| Codex | `rtk init --codex` | RTK.md in `$CODEX_HOME` or `~/.codex` | AGENTS.md |
| Cursor | `rtk init -g --agent cursor` | Cursor hook | hooks.json |


## Integrity Verification

The integrity system prevents unauthorized hook modifications:

1. At install: `integrity::store_hash()` computes SHA-256 of the hook file, writes to `~/.claude/hooks/.rtk-hook.sha256` (read-only 0o444)
2. At runtime: `integrity::runtime_check()` re-computes hash and compares; blocks execution if tampered
3. On demand: `rtk verify` prints detailed verification status (PASS/FAIL/WARN/SKIP)

Five integrity states:
- **Verified**: Hash matches stored value
- **Tampered**: Hash mismatch (blocks execution)
- **NoBaseline**: Hook exists but no hash stored (old install)
- **NotInstalled**: No hook, no hash
- **OrphanedHash**: Hash file exists, hook missing

## PatchMode Behavior

Controls how `rtk init` modifies agent settings files:

| Mode | Flag | Behavior |
|------|------|----------|
| Ask (default) | -- | Prompts user `[y/N]`; defaults to No if stdin not terminal |
| Auto | `--auto-patch` | Patches without prompting; for CI/scripted installs |
| Skip | `--no-patch` | Prints manual instructions; user patches manually |

## Atomicity and Safety

All file operations use atomic writes (tempfile + rename) to prevent corruption on crash. Settings files are backed up to `.bak` before modification. All operations are idempotent -- running `rtk init` multiple times is safe.

## Permission Model

RTK enforces a permission precedence that matches Claude Code's least-privilege default:

```
Deny > Ask > Allow (explicit) > Default (ask)
```

Rules are loaded from all Claude Code `settings.json` files (project + global, including `.local` variants). Only `Bash(...)` rules are extracted; other scopes (Read, Write) are ignored.

| Verdict | Trigger | rewrite_cmd exit | Hook behavior |
|---------|---------|-----------------|---------------|
| Deny | `permissions.deny` rule matched | 2 | Passthrough — host tool handles denial |
| Ask | `permissions.ask` rule matched | 3 | Rewrite + let host tool prompt user |
| Allow | `permissions.allow` rule matched | 0 | Rewrite + auto-allow |
| Default | No rule matched | 3 | Rewrite + let host tool prompt user |

### Per-tool support

| Tool | ask support | Behavior on Default |
|------|------------|-------------------|
| Claude Code (rtk-rewrite.sh) | Yes | `permissionDecision: "ask"` — user prompted |
| Copilot VS Code (rtk hook copilot) | Yes | `permissionDecision: "ask"` — user prompted |
| Gemini CLI (rtk hook gemini) | No (allow/deny only) | allow (limitation — no ask mode in Gemini) |
| Copilot CLI (rtk hook copilot) | No updatedInput | deny-with-suggestion (unchanged) |
| Codex | ask parsed but no-op | allow (limitation — fails open) |

### Implementation

- `permissions.rs` — loads deny/ask/allow rules, evaluates precedence, returns `PermissionVerdict`
- `rewrite_cmd.rs` — maps verdict to exit code (consumed by shell hook)
- `hook_cmd.rs` — maps verdict to JSON `permissionDecision` field (Copilot/Gemini)

## Exit Code Contract

Hook processors in `hook_cmd.rs` must return `Ok(())` on every path — success, no-match, parse error, and unexpected input. Returning `Err` propagates to `main()` and exits non-zero, which blocks the agent's command from executing. This violates the non-blocking guarantee documented in `hooks/README.md`.

## Adding New Functionality
To add support for a new AI coding agent: (1) add the hook installation logic to `init.rs` following the existing agent patterns, (2) if the agent requires a custom hook protocol (like Gemini's `BeforeTool`), add a processor function in `hook_cmd.rs`, (3) add the agent's hook file path to `hook_check.rs` for validation, and (4) update `integrity.rs` with the expected hash for the new hook file. Test by running `rtk init` in a fresh environment and verifying the hook rewrites commands correctly in the target agent.
