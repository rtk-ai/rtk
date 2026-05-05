# RTK - Rust Token Killer (Codex CLI)

**Usage**: Token-optimized CLI proxy for noisy shell commands.

## Rule

Use `rtk` when filtered output preserves the signal you need. Do not blindly
prefix tiny probes, shell builtins, or commands where exact output matters.

```bash
rtk git status
rtk git diff
rtk git show
rtk gh pr view <num>
rtk grep <pattern> <path>
rtk read <file>
rtk cargo test
rtk pytest
rtk vitest run
```

For noisy repo scripts, use generic wrappers:

```bash
rtk test bun run test
rtk err bun run typecheck
rtk err bun run build
```

## Raw Commands

Use raw commands for exact file snippets, JSON consumed by another command,
small probes like `pwd`/`printf`/`test -d`, interactive servers, secrets,
binary output, or user-requested full output.

## Codex Hook

Codex hooks cannot rewrite Bash input in place yet. If blocked with
`Rerun that as: ...`, run the suggested command.

## Meta Commands

```bash
rtk gain             # Token savings analytics
rtk gain --history   # Recent command savings history
rtk proxy <cmd>      # Track invocation but bypass filtering
rtk --version
which rtk
```
