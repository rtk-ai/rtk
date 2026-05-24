# Codex CLI Plugin

> Part of [`hooks/`](../README.md) -- see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Package Layout

`rtk init --codex` registers the RTK Codex plugin through a local Codex marketplace instead of creating new loose `RTK.md` or `AGENTS.md` guidance files.

The source package lives at [`rtk-codex/`](rtk-codex/):

- `.codex-plugin/plugin.json` -- Codex plugin metadata, skill path, and hook path
- `skills/rtk/SKILL.md` -- RTK usage, rewrite, opt-out, and validation guidance for Codex
- `hooks/hooks.json` -- `PreToolUse` hook for Bash tool calls
- `hooks/run-rtk-codex-hook.sh` -- thin launcher that delegates stdin to `rtk hook codex`

## Install Paths

- Local: `plugins/rtk-codex/` plus `.agents/plugins/marketplace.json`
- Global: `$CODEX_HOME/plugins/rtk-codex/` or `~/.codex/plugins/rtk-codex/`, plus `~/.agents/plugins/marketplace.json`

Uninstall removes only the RTK plugin package, its marketplace entry, and legacy RTK-managed `RTK.md` / `AGENTS.md` references. Unrelated plugins and marketplace entries are preserved.

## Hook Behavior

The hook launcher resolves `RTK_EXE` first, falls back to `rtk` on `PATH`, forwards the original Codex hook payload to `rtk hook codex`, and fails open when RTK is unavailable.

`rtk hook codex` rewrites only `PreToolUse` payloads for `tool_name = "Bash"` with a string `tool_input.command`. Empty input, malformed JSON, unsupported tools, missing commands, unsupported commands, already-RTK commands, and heredocs exit successfully without hook output.

Codex currently supports rewritten input only with `permissionDecision: "allow"` and `updatedInput.command`. RTK does not emit the unsupported `permissionDecision: "ask"` rewrite shape.

## Activation and Trust

Codex plugin hooks require the Codex `hooks` and `plugin_hooks` features to be active. After installation, restart Codex and use `/hooks` to review and trust the RTK plugin hook when Codex asks for hook trust.

The legacy `rtk-awareness.md` file is retained only as compatibility context for previous instruction-only installs.
