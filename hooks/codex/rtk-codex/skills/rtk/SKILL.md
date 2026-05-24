---
name: rtk
description: Use RTK from Codex to rewrite supported shell commands for token-optimized command output and validate token savings.
---

# RTK

Use RTK when running shell commands whose raw output can be large. RTK wraps common developer commands, preserves command behavior, and returns compact output for Codex.

## When to use

- Prefer `rtk <command>` for supported build, test, Git, GitHub, package-manager, file-search, log, JSON, and infrastructure commands.
- The RTK Codex plugin can rewrite supported `Bash` tool calls before execution. For example, `git status` can become `rtk git status` automatically after plugin hooks are enabled and trusted.
- Use `rtk gain` to inspect token savings and hook adoption.
- Use `rtk hook check <command>` to preview how RTK would rewrite a command.

## Opt out

- Prefix a command with `RTK_DISABLED=1` to run it without RTK rewriting.
- Configure `[hooks].exclude_commands` in the RTK config for commands that should never be rewritten.
- Use `rtk proxy <command>` when you need raw command behavior while still invoking RTK explicitly.

## Validation

- After installing or updating the plugin, restart Codex and open `/hooks` to review and trust the RTK plugin hook if Codex requests trust.
- Run a simple command such as `git status`; when the hook is active, Codex should execute the RTK-prefixed command.
- Run `rtk gain` after several commands to confirm savings are being tracked.
