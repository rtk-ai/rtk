# Grok Build CLI Hooks

> Part of [`hooks/`](../README.md) — see also [`src/hooks/`](../../src/hooks/README.md) for installation code

## Specifics

- Native Rust `PreToolUse` processor: `rtk hook grok`
- Transparently rewrites `tool_input.command` / `toolInput.command` with Grok's `updatedInput` response
- Registers a `Bash` matcher in `$GROK_HOME/hooks/rtk-rewrite.json` (global, default `~/.grok`) or `.grok/hooks/rtk-rewrite.json` (project)
- Writes the shared awareness document selected by `awareness.level` to `$GROK_HOME/rules/rtk.md` or `.grok/rules/rtk.md`
- Installed by `rtk init -g --agent grok` (global) or `rtk init --agent grok` (project)
- Uninstalled by adding `--uninstall` to the corresponding command
- `--hook-only` skips the rules file

Grok aliases `Bash` to `run_terminal_command`, so a `Bash` matcher fires for the shell tool. The processor also accepts camelCase Grok payloads (`toolName`, `toolInput`).

Unlike Codex, Grok applies `updatedInput` when `permissionDecision` is omitted. RTK therefore never sends `allow`: Grok's own permission prompt, plan-mode gate, and sandbox still run on the rewritten command.

Malformed JSON, non-shell tools, empty commands, heredocs, substitutions, file redirects, and commands with no RTK filter fail open: the hook exits 0 with empty stdout and Grok runs the original command.

RTK does not parse Grok permission files.

Project-scoped hooks require Grok folder trust (`/hooks-trust` or `--trust`).
