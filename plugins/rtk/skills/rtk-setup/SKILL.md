---
name: rtk-setup
description: >-
  Install RTK (Rust Token Killer) and hook it into a coding agent with rtk init:
  Claude Code, Codex, Cursor, Copilot, Gemini CLI, OpenCode, Windsurf, Cline and
  others, global vs project installs, dry runs, uninstall, and the Codex
  sandbox history note. Use when setting up RTK, checking it is the right
  binary, or wiring it into a new agent.
---

# Setting up RTK

References: `README.md` ("Install & forget", "Works in your agent"),
`INSTALL.md`, `hooks/<agent>/README.md`.

## Install

```bash
brew install rtk                 # macOS / Linux with Homebrew
winget install rtk-ai.rtk        # Windows
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

Check it is this RTK, not the unrelated crates.io `rtk`: `rtk gain` must
print the savings dashboard.

## Hook it into the agent

`-g` installs globally; without it, into the current project.

| Agent | Command | Mechanism |
|-------|---------|-----------|
| Claude Code | `rtk init -g` | hook |
| Codex | `rtk init -g --codex` | hook |
| Cursor | `rtk init -g --agent cursor` | hook |
| GitHub Copilot | `rtk init -g --copilot` | hook |
| Gemini CLI | `rtk init -g --gemini` | hook |
| OpenCode | `rtk init -g --opencode` | plugin |
| Windsurf | `rtk init -g --agent windsurf` | rules file |
| Cline / Roo Code | `rtk init --agent cline` | rules file |

Hook-based agents get automatic rewriting (`git status` runs as
`rtk git status`). Rules-file agents are only asked to prefer `rtk` commands.
Restart the agent afterwards. Add `--uninstall` to the same command to remove
it.

## Codex specifics

- `rtk hook codex` is a `PreToolUse` processor registered for `Bash` in
  `.codex/hooks.json` (project) or `$CODEX_HOME/hooks.json` (global).
- Unsupported shapes (heredocs, substitutions, redirections, malformed JSON)
  fail open: the original command runs.
- In `workspace-write` sandboxes, history may fail with `SQLITE_CANTOPEN`. Run
  `rtk init --codex` outside the sandbox to print a `writable_roots` snippet,
  and let the user decide whether to add it. RTK never edits sandbox settings.

## Configuration

`config.toml` controls display and tracking, e.g. `pro_hint = false` under
`[display]`. History DB path: `RTK_DB_PATH`, then `tracking.database_path`,
then the platform default.
