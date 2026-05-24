# Codex CLI Plugin

> Part of [`hooks/`](../README.md) -- see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Package Layout

`rtk init --codex` registers the RTK Codex plugin through a local Codex marketplace instead of creating new loose `RTK.md` or `AGENTS.md` guidance files.

The source package lives at [`rtk-codex/`](rtk-codex/):

- `.codex-plugin/plugin.json` -- Codex plugin metadata, skill path, and hook path
- `skills/rtk/SKILL.md` -- RTK usage, rewrite, opt-out, and validation guidance for Codex
- `hooks/hooks.json` -- `PreToolUse` hook for Bash tool calls that invokes `rtk hook codex`

## Install Paths

- Local: `plugins/rtk-codex/` plus `.agents/plugins/marketplace.json`
- Global: `$CODEX_HOME/plugins/rtk-codex/` or `~/.codex/plugins/rtk-codex/`, plus `~/.agents/plugins/marketplace.json`

Uninstall removes only the RTK plugin package, its marketplace entry, and legacy RTK-managed `RTK.md` / `AGENTS.md` references. Unrelated plugins and marketplace entries are preserved.

## Hook Behavior

The hook config invokes the native RTK hook processor directly. On Linux and macOS it uses `rtk hook codex`; on native Windows it uses `commandWindows` with `rtk.exe hook codex`, so Windows does not need Bash, a `.sh` launcher, POSIX executable bits, or POSIX environment expansion.

If `RTK_EXE` is set when `rtk init --codex` runs, RTK writes that executable into the installed hook command. Otherwise, the installed hook resolves `rtk` or `rtk.exe` from `PATH`. Without the old shell launcher, RTK cannot emit a pre-launch advisory if the configured executable is missing, so users should rerun `rtk init --codex` after moving the RTK binary or changing `RTK_EXE`.

`rtk hook codex` rewrites only `PreToolUse` payloads for `tool_name = "Bash"` with a string `tool_input.command`. Empty input, malformed JSON, unsupported tools, missing commands, unsupported commands, already-RTK commands, and heredocs exit successfully without hook output.

Codex currently supports rewritten input only with `permissionDecision: "allow"` and `updatedInput.command`. RTK does not emit the unsupported `permissionDecision: "ask"` rewrite shape.

## Activation and Trust

Codex plugin hooks require the Codex `hooks` and `plugin_hooks` features to be active. After installation, restart Codex and use `/hooks` to review and trust the RTK plugin hook when Codex asks for hook trust.

The legacy `rtk-awareness.md` file is retained only as compatibility context for previous instruction-only installs.

For a user-facing setup and verification walkthrough, see [`../../docs/usage/CODEX_PRETOOLUSE_ADAPTER.md`](../../docs/usage/CODEX_PRETOOLUSE_ADAPTER.md).
