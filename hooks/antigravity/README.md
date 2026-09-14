# Google Antigravity Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native programmatic `PreToolUse` lifecycle hook (`rtk hook antigravity`) providing transparent command rewriting (<1.5ms) via `overwrite.CommandLine`
- Supported across all Antigravity surfaces: Antigravity CLI (`agy`), Antigravity IDE, and Antigravity 2.0
- Uses Antigravity's modular Plugin architecture:
  - Local workspace: `.agents/plugins/rtk/`
  - Global scope: `~/.gemini/config/plugins/rtk/`
- Full lifecycle support:
  - Setup: `rtk init --agent antigravity` (or `rtk init -g --agent antigravity`)
  - Preview: `rtk init --agent antigravity --dry-run`
  - Uninstallation: `rtk init --agent antigravity --uninstall` (or `rtk init -g --agent antigravity --uninstall`)

## Permission Evaluation Note

Antigravity evaluates its tool permissions *after* lifecycle hooks rewrite commands. If you maintain strict command allowlists, ensure permitted commands account for `rtk` (e.g. `command(rtk git status)` or `command(rtk *)`).
