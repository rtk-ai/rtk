---
title: Devin CLI
description: One-command setup and reference for RTK + Devin CLI integration
sidebar:
  order: 5
---

# Devin CLI Integration

RTK integrates with [Devin CLI](https://docs.devin.ai/get-started) through a `PreToolUse` hook and a small set of lifecycle context hooks. Any allowed `exec` tool call is rewritten to `rtk <command>` before it reaches the shell, giving you compact output without per-command prompting.

## One-command install from a fork

```bash
git clone https://github.com/warelik/rtk.git
cd rtk
bash install-devin.sh
```

`install-devin.sh` does two things:

1. Builds and installs the `rtk` binary with `cargo install --path .`.
2. Registers the global Devin CLI hooks with `rtk init -g --agent devin --auto-patch`.

After it finishes, restart Devin CLI and test with:

```bash
git status
```

## Manual setup

If you already have `rtk` installed from another source:

```bash
# Global (all Devin projects)
rtk init -g --agent devin

# Per-project (commit .devin/ to share with teammates)
rtk init --agent devin
```

## What gets installed

| Scope | Files / entries |
|-------|-----------------|
| Global | `~/.config/devin/config.json` — `PreToolUse` hook + `SessionStart` / `UserPromptSubmit` / `PostCompaction` lifecycle hooks |
| Global | `~/.config/devin/hooks/rtk/rtk-devin.js` — lifecycle hook runner |
| Global | `~/.config/devin/hooks/rtk/rtk-instructions.md` — RTK context injected into Devin |
| Global | `~/Library/Application Support/rtk/filters.toml` (macOS) or `~/.config/rtk/filters.toml` (Linux) — filter template |
| Project | `.devin/hooks.v1.json` or `.devin/config.json` — same hooks, scoped to the project |
| Project | `.devin/hooks/rtk/rtk-devin.js` + `rtk-instructions.md` |

The project-level paths use `$DEVIN_PROJECT_DIR` so they remain portable when committed.

## Verify

```bash
rtk init --agent devin --show   # show detected config state
rtk verify                        # check hook file integrity + filter tests
```

`rtk verify` now checks both the Claude-style native hook and the Devin CLI lifecycle files against the source-of-truth content embedded in the `rtk` binary.

## How the rewrite works

Devin CLI sends a JSON payload to the hook:

```json
{
  "tool_name": "exec",
  "tool_input": { "command": "git status" }
}
```

RTK rewrites it to:

```json
{
  "tool_name": "exec",
  "tool_input": { "command": "rtk git status" }
}
```

For commands that Devin CLI already allows, RTK also emits `permissionDecision: "approve"` so the user is not prompted twice. Blocked commands are blocked; unknown commands are rewritten without auto-approval so Devin CLI can ask on the rewritten command.

## Escape hatches

- `RTK_DISABLED=1 <command>` — bypass RTK rewriting for a single command.
- `rtk proxy <command>` — run the raw command without filtering (still subject to Devin CLI permission checks).
- `rtk <meta>` (`rtk gain`, `rtk --version`, `rtk discover`) — automatically approved.

## Uninstall

```bash
# Remove global Devin CLI hooks
rtk init -g --agent devin --uninstall

# Remove binary (if installed via cargo)
cargo uninstall rtk
```

## Test suite

The Devin CLI hook has a regression suite:

```bash
bash hooks/devin/test-rtk-devin.sh
```

It covers rewrite patterns, environment-prefix handling, meta commands, wrappers, `RTK_DISABLED`, and commands that should not be rewritten.

## Troubleshooting

- `rtk command not found` after `install-devin.sh` — make sure `$HOME/.cargo/bin` is on your PATH.
- `permission denied` on `install-devin.sh` — it should be executable; otherwise run `bash install-devin.sh`.
- Hook not firing — restart Devin CLI after `rtk init`.
- Changed hook files — `rtk verify` will flag tampering or version skew; re-run `rtk init -g --agent devin --auto-patch` to restore.
