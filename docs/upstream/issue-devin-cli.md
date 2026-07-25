# Upstream issue draft: Add Devin CLI support

## Title
feat(hooks): Add first-class Devin CLI integration

## Description

Devin CLI (https://docs.devin.ai/get-started) is a command-line interface for the Devin autonomous coding agent. It exposes a `PreToolUse` hook system and lifecycle hooks (`SessionStart`, `UserPromptSubmit`, `PostCompaction`) that are a natural fit for RTK's command interception model.

### Goal

Add native Devin CLI support to RTK so that shell commands invoked by Devin CLI are transparently rewritten to `rtk <command>`, producing compact output without per-command prompting.

### Why this matters

- Devin CLI users currently miss out on RTK's token savings because there is no hook integration.
- Devin CLI already has its own `permissions` model (`allow`/`ask`/`deny`). RTK can respect it, so allowed commands stay auto-approved, denied commands stay blocked, and unknown commands are rewritten so Devin CLI prompts on the rewritten command.
- Hook-based integration is the most effective RTK delivery mechanism (100% adoption across all conversations and subagents with zero per-command context overhead).

### Proposed behavior

```bash
rtk init -g --agent devin   # global install
rtk init --agent devin      # project install
rtk init --agent devin --show
rtk init --agent devin --uninstall
```

Installs:
- A `PreToolUse` hook (matcher `^exec$`) into `~/.config/devin/config.json` (global) or `.devin/hooks.v1.json` / `.devin/config.json` (project).
- Lifecycle context hooks that inject `rtk-instructions.md` into Devin CLI context on `SessionStart`, `UserPromptSubmit`, and `PostCompaction`.
- A portable Node lifecycle script (`rtk-devin.js`) and instruction file (`rtk-instructions.md`).

`rtk hook devin` reads the JSON payload from Devin CLI, rewrites supported commands, and emits `permissionDecision` when Devin CLI's own permission settings allow the command.

### Acceptance criteria

- [ ] `rtk init -g --agent devin` installs the global hook and lifecycle instructions.
- [ ] `rtk init --agent devin` installs project-scoped hooks (portable paths using `$DEVIN_PROJECT_DIR`).
- [ ] `rtk hook devin` correctly rewrites `git status`, `cargo test`, `docker ps`, etc. to `rtk <command>`.
- [ ] Allowed commands emit `decision: approve`; denied commands emit `decision: block`.
- [ ] `RTK_DISABLED=1 <cmd>` bypasses rewriting.
- [ ] `rtk verify` checks the integrity of the installed Devin CLI hook files.
- [ ] `rtk init --agent devin --uninstall` removes RTK entries and hook files without touching user hooks.
- [ ] A regression test suite exists for the Devin CLI hook (`hooks/devin/test-rtk-devin.sh`).
- [ ] Documentation is updated (`README.md`, `INSTALL.md`, `docs/guide/getting-started/supported-agents.md`, plus a dedicated `docs/guide/devin-cli.md`).

### Related

This would bring Devin CLI to parity with Claude Code, Cursor, Gemini CLI, Codex, GitHub Copilot, Factory Droid, and the other agents already supported by RTK.
