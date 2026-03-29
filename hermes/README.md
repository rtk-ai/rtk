# RTK Plugin for Hermes

Hermes-specific RTK integration that intercepts `terminal` tool calls and rewrites eligible commands via `rtk rewrite` before execution.

The goal is to reduce LLM token usage on common shell commands while preserving safe fallback behavior when RTK does not apply.

## What it does

When Hermes is about to run a `terminal` tool call, this plugin can rewrite commands such as:

- `git status` -> `rtk git status`
- `ls -la` -> `rtk ls -la`
- `curl -fsSL https://example.com` -> `rtk curl -fsSL https://example.com`

All rewrite logic lives in the RTK binary itself. This plugin is only a thin Hermes adapter.

## Files

- `plugin.yaml` — Hermes plugin manifest
- `__init__.py` — plugin implementation
- `README.md` — this file

## Requirements

- Hermes plugin system enabled and loading plugins from `~/.hermes/plugins/`
- `rtk` installed and available in `PATH`
- RTK version `>= 0.23.0`

Check RTK:

```bash
which rtk
rtk --version
```

## Installation

```bash
mkdir -p ~/.hermes/plugins/rtk-rewrite
cp hermes/__init__.py hermes/plugin.yaml hermes/README.md ~/.hermes/plugins/rtk-rewrite/
```

Then restart Hermes.

## How it works

The plugin registers a Hermes `pre_tool_call` hook.

On each tool call:
1. It ignores everything except the `terminal` tool.
2. It applies a few Hermes-side skip rules.
3. It runs `rtk rewrite "<command>"`.
4. If RTK returns a rewritten command with exit code `0`, the plugin replaces `args["command"]` in place.
5. If RTK does not rewrite, times out, or fails, the original command is left unchanged.

## RTK exit-code handling

The plugin currently interprets RTK like this:

- `0` — rewrite applied
- `1` — no RTK equivalent; pass through unchanged
- `2` — RTK deny rule matched; leave unchanged
- `3` — RTK ask rule matched; leave unchanged and log the situation

Why exit code `3` is not auto-rewritten:
Hermes `pre_tool_call` can mutate tool arguments, but it does not currently provide a clean native way for this plugin to trigger an interactive user-confirmation flow. Because of that, the plugin takes the conservative route and preserves the original command.

## Hermes-specific safety behavior

By default, the plugin skips rewriting when:

- the command already starts with `rtk`
- the command is a compound shell expression containing:
  - `|`
  - `||`
  - `&&`
  - `;`
  - `<<`
- the `terminal` call has `background=true`
- the `terminal` call has `pty=true`

These defaults reduce the chance of surprising behavior in interactive or complex shell scenarios.

## Configuration

This plugin is configured with environment variables.

### Enable or disable the plugin

Default: enabled

```bash
export HERMES_RTK_ENABLED=1
```

Disable:

```bash
export HERMES_RTK_ENABLED=0
```

### Verbose logging

Default: off

```bash
export HERMES_RTK_VERBOSE=1
```

When enabled, the plugin logs rewrite decisions and skip reasons.

### Rewrite timeout

Default: `2000` milliseconds

```bash
export HERMES_RTK_TIMEOUT_MS=2000
```

### Background-call rewriting

Default: skipped

```bash
export HERMES_RTK_SKIP_BACKGROUND=1
```

Allow rewriting background terminal calls:

```bash
export HERMES_RTK_SKIP_BACKGROUND=0
```

### PTY-call rewriting

Default: skipped

```bash
export HERMES_RTK_SKIP_PTY=1
```

Allow rewriting PTY terminal calls:

```bash
export HERMES_RTK_SKIP_PTY=0
```

### Cache size

Default: `256`

The plugin caches rewrite decisions in memory to avoid repeated `rtk rewrite` subprocess calls for the same command strings.

```bash
export HERMES_RTK_CACHE_SIZE=256
```

Disable cache:

```bash
export HERMES_RTK_CACHE_SIZE=0
```

## Operational notes

- This plugin only affects the Hermes `terminal` tool.
- It does not affect `execute_code` or other tools.
- It is a Hermes adaptation, not the upstream OpenClaw plugin.
- If RTK is missing or too old, the plugin disables itself safely.
- If `rtk rewrite` hangs or errors, the original command is used.
