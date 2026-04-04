---
title: Proxy Architecture
description: ADR — why RTK uses a CLI proxy pattern instead of shell aliases or wrappers
sidebar:
  order: 2
---

# ADR: Proxy Architecture

## Decision

RTK is a CLI proxy: a single binary that intercepts commands, executes them as subprocesses, filters the output, and exits. Users invoke `rtk git status` instead of `git status`.

Hooks extend this by making the interception transparent — the AI agent's command is rewritten before execution, so neither the agent nor the user types `rtk`.

## Alternatives considered

### Shell aliases

```bash
alias git='rtk git'
alias cargo='rtk cargo'
```

**Rejected because:**
- Requires shell configuration per-user, per-machine, per-shell
- Doesn't work for non-interactive contexts (scripts, CI, AI agents)
- Can't be installed programmatically without modifying shell dotfiles
- Breaks if the user has other aliases or functions with the same name

### Shell function wrappers

Similar to aliases but more fragile. Same problems.

### `LD_PRELOAD` / dynamic linking interception

**Rejected because:**
- Platform-specific (Linux only with glibc)
- Security restrictions (macOS SIP, container environments)
- Complex to implement, maintain, and debug

### Hook-only approach (no explicit `rtk` prefix)

Make RTK entirely invisible — install hooks and never expose `rtk <cmd>` as a user-facing interface.

**Rejected because:**
- Users need a way to invoke RTK explicitly for debugging (`rtk git status -vvv`)
- `rtk gain` and `rtk discover` need a namespace
- Transparent hooks are additive, not a replacement for the explicit interface

## Why the proxy pattern works

**Single binary, no configuration:** `rtk git status` works identically on macOS, Linux, and Windows. No dotfiles. No shell-specific setup.

**Explicit and debuggable:** `-v`/`-vv`/`-vvv` flags expose what RTK is doing at each phase. `RTK_DISABLED=1` bypasses it for one command.

**Exit code preservation:** RTK propagates the underlying tool's exit code. CI pipelines that check `$?` work correctly.

**Fail-safe:** If RTK's filter fails, it falls back to raw output. The user always gets a result.

**Hook interception as an enhancement:** The hook layer adds transparency on top of the proxy pattern — it rewrites `git status` to `rtk git status` before the agent sees it. But the proxy interface remains available for direct use, debugging, and tools that can't be hooked.

## Consequences

- Every supported command needs a module in `src/cmds/`
- Unsupported commands pass through transparently (no breakage)
- The binary grows as new commands are added (~5MB currently, well within the `<5MB` soft target after stripping)
- Adding a new command = adding a module + registering in `main.rs` + adding rewrite pattern in `discover/registry.rs`
