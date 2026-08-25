# Antigravity CLI (`agy`) Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- High-performance native binary hook (`rtk hook agy`) using Antigravity CLI's `PreToolUse` lifecycle hook system
- Ingested as a standard AGY plugin in `.agents/plugins/rtk-agy/` (local workspace) or `~/.gemini/config/plugins/rtk-agy/` (global machine per AGY plugin specifications)
- Low-overhead transparent command rewriting (<1.5ms) via `overwrite.CommandLine`
- Installed via `rtk init --agent agy` (or `rtk init -g --agent agy` for global scope)
- Uninstalled via `rtk init --agent agy --uninstall` (or `rtk init -g --agent agy --uninstall`)
