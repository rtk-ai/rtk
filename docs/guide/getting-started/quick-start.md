---
title: Quick Start
description: Get RTK running in 5 minutes and see your first token savings
sidebar:
  order: 2
---

# Quick Start

This guide walks you through your first RTK commands after installation.

## Prerequisites

RTK is installed and verified:

```bash
rtk --version   # rtk x.y.z
rtk gain        # shows token savings dashboard
```

If not, see [Installation](./installation.md).

## Step 1: Initialize for your AI assistant

```bash
# For Claude Code (global — applies to all projects)
rtk init --global

# For a single project only (CLAUDE.md instructions, no hook)
cd /your/project && rtk init
```

`rtk init --global` installs the hook that automatically rewrites commands. Restart your AI assistant after this step.

Using something other than Claude Code? Every agent has its own flag:

```bash
rtk init -g --gemini            # Gemini CLI
rtk init -g --codex             # Codex (OpenAI)
rtk init -g --agent cursor      # Cursor
rtk init -g --agent windsurf    # Windsurf
rtk init --agent cline          # Cline / Roo Code
```

See [Supported agents](./supported-agents.md) for the full list, including per-agent uninstall and the project-local vs global distinction.

### Preview without writing: `--dry-run`

To see exactly what `init` would change before it touches anything, add `--dry-run`:

```bash
rtk init --global --dry-run
```

Every would-be file create/update/patch is printed with a `[dry-run] would ...` prefix, then a `[dry-run] Nothing written.` footer. Nothing on disk is modified, no settings.json is patched, and the telemetry consent prompt is skipped. Combine with `-v` to also print the full content RTK would write:

```bash
rtk init --global --dry-run -v
```

`--dry-run` works for every init flavour (`--agent cursor`, `--gemini`, `--codex`, `--copilot`, `--uninstall`, ...). It cannot be combined with `--show`.

## Step 2: Use your tools normally

Once the hook is installed, nothing changes in how you work. Your AI assistant runs commands as usual — the hook intercepts them transparently and rewrites them before execution.

For example, when Claude Code runs `cargo test`, the hook rewrites it to `rtk cargo test` before it executes. The LLM receives filtered output with only the failures — not 500 lines of passing tests. You never see or type `rtk`.

RTK covers all major ecosystems — Git, Cargo/Rust, JavaScript, Python, Go, Ruby, .NET, Docker/Kubernetes, and more. See [What RTK Optimizes](../resources/what-rtk-covers.md) for the full list.

## Step 3: Check your savings

After a few commands, see how much was saved:

```bash
rtk gain
```

Illustrative output:

```
Total commands : 10
Input tokens   : 50,000
Output tokens  : 5,000
Saved          : 45,000  (90.0%)
```

:::caution[The hook only fires on Bash tool calls]
Built-in agent tools like `Read`, `Grep` and `Glob` do not pass through the Bash hook, so they are not auto-rewritten. To get RTK's compact output for those workflows, either use shell commands (`cat`/`head`/`tail`, `rg`/`grep`, `find`) or call `rtk read`, `rtk grep` and `rtk find` directly.

The full edition can additionally compress an agent's own `Read` and MCP tool output through the PostToolUse hook — see [Pipelines](../pipelines/overview.md).
:::

Simple quoted shell wrappers are rewritten too, without changing the shell they name:

```bash
bash -c "head foo && grep -R bar ."
# → bash -c "rtk read foo --head-lines 10 && rtk grep -R bar ."
```

This covers exact `sh -c`, `bash -c`, `zsh -c` and `fish -c` wrappers around a quoted script. A `fish -c` script is read as fish, so fish's own syntax (`(cmd)` substitution, `and`/`or`/`end`) is left alone rather than rewritten under POSIX rules:

```bash
fish -c 'git status; cargo test'      # → fish -c 'rtk git status; rtk cargo test'
fish -c 'git status; and cargo test'  # unchanged
```

Shell expansion inside an outer double quote, extra shell options, redirects to files and nested wrappers pass through unchanged.

A command string that is *itself* fish — a multi-line `if`/`for`/`switch` … `end` block, or a `; and` / `; or` chain — can be handed back for explicit fish execution instead, because a host evaluating it with a POSIX layer would fail to parse it before RTK ever runs. **This is off by default**; turn it on with `wrap_fish_scripts = true` under `[hooks]`:

```fish
if test -d src
  git status
end
# → rtk run --shell fish -c 'if test -d src\n  git status\nend'
```

**What turning it on costs.** The script reaches Claude Code's approval prompt as a single `rtk run` command, so what the prompt — and every permission rule — sees is that command, not `git status` inside it. RTK therefore always marks a wrap as an explicit *ask*, and withholds it entirely when it can see any `deny` rule, or when a settings file it reads is unparsable or unreadable. But some rule sources never reach the hook at all: `--settings`, `--disallowedTools`, managed settings, and the project directory the agent has `cd`'d away from. A rule in one of those becomes an approval prompt instead of a refusal.

It happens on Claude Code's own hook only — every other host, and `rtk rewrite`, leave fish scripts exactly as they are without the feature. It also needs a resolvable `fish` binary and is disabled on Windows.

## Step 4: Unsupported commands

Commands RTK doesn't recognize run through passthrough — output is unchanged, usage is tracked:

```bash
rtk bypass make install
```

## Next steps

- [What RTK Optimizes](../resources/what-rtk-covers.md) — all supported commands and savings by ecosystem
- [Supported agents](./supported-agents.md) — Claude Code, Cursor, Copilot, and more
- [Configuration](./configuration.md) — customize RTK behavior
- [Token Savings Analytics](../analytics/gain.md) — the full `rtk gain` surface
- [Editions and Licensing](../resources/editions.md) — what each download contains
