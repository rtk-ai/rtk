# RTK Plugin for Hermes

Initial Hermes support: transparently rewrites shell commands executed via Hermes's `terminal` tool to their RTK equivalents, achieving 60-90% LLM token savings.

This is the Hermes equivalent of the Claude Code hooks in `hooks/claude/rtk-rewrite.sh` and the OpenClaw plugin in `openclaw/`.

## Status

Hermes support is currently **manual/plugin-based**:
- the plugin files in `hermes/` are ready to use
- `rtk init` does **not** install Hermes yet
- if Hermes changes its plugin API, this integration should be updated like any other RTK hook/plugin

## How it works

The plugin registers a `pre_tool_call` hook that intercepts `terminal` tool calls. When the agent runs a command like `git status`, the plugin delegates to `rtk rewrite`, which returns the optimized command (for example `rtk git status`). The compressed output enters the agent's context window, saving tokens.

All rewrite logic lives in RTK itself (`rtk rewrite`). This plugin is a thin delegate — when new filters are added to RTK, the plugin picks them up automatically with zero changes.

```
Agent runs: terminal(command="cargo test --nocapture")
  -> Plugin intercepts pre_tool_call hook
  -> Calls `rtk rewrite "cargo test --nocapture"`
  -> Mutates args["command"] = "rtk cargo test --nocapture"
  -> Agent executes the rewritten command
  -> Filtered output reaches LLM (~90% fewer tokens)
```

## Installation

### Prerequisites

RTK must be installed and available in `$PATH`:

```bash
brew install rtk
# or
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Install the plugin manually

Hermes discovers user plugins from `$HERMES_HOME/plugins/`.

- Default Hermes home: `~/.hermes`
- Profile mode example: `~/.hermes/profiles/zero`
- Effective plugin target: `$HERMES_HOME/plugins/rtk-rewrite/`

```bash
PLUGIN_HOME="${HERMES_HOME:-$HOME/.hermes}"
mkdir -p "$PLUGIN_HOME/plugins/rtk-rewrite"
cp hermes/__init__.py hermes/plugin.yaml "$PLUGIN_HOME/plugins/rtk-rewrite/"
```

Then restart Hermes (or the Hermes gateway, if you run one) so the plugin is reloaded.

## Architecture

Hermes uses a Python plugin system with lifecycle hooks. Plugins live in `$HERMES_HOME/plugins/<name>/` and must contain:

- `plugin.yaml` — manifest (name, version, hooks declared)
- `__init__.py` — module with a `register(ctx)` function

The `register(ctx)` function receives a `PluginContext` that provides:
- `ctx.register_hook(name, callback)` — subscribe to lifecycle events
- `ctx.register_tool(...)` — add new tools to the agent

This plugin uses the `pre_tool_call` hook, which fires before every tool execution with `tool_name`, `args` (mutable dict), and `task_id`. Mutating `args["command"]` in-place rewrites the command before the terminal executes it.

## Rewrite behavior

The plugin accepts RTK rewrite exit codes `0` and `3` as successful rewrite paths. Current RTK builds may return exit code `3` while still emitting a valid rewritten command on stdout.

The plugin also skips:
- commands already starting with `rtk `
- commands containing `RTK_DISABLED=1`
- empty or non-string commands
- non-`terminal` tool calls

## What gets rewritten

Everything that `rtk rewrite` supports (30+ commands). See the [full command list](https://github.com/rtk-ai/rtk#commands).

## What's NOT rewritten

Handled by the plugin and/or `rtk rewrite` guards:
- commands already using `rtk`
- commands explicitly disabled with `RTK_DISABLED=1`
- piped / compound commands according to RTK's compound-command logic
- heredocs (`<<`)
- commands without an RTK filter

## Measured savings

| Command | Token savings |
|---------|--------------|
| `git log --stat` | 87% |
| `ls -la` | 78% |
| `git status` | 66% |
| `grep` (single file) | 52% |
| `find -name` | 48% |

## Graceful degradation

The plugin is non-blocking — it never prevents a command from executing:

- RTK binary not found: warning logged, plugin disabled (no hook registered)
- `rtk rewrite` times out (>2s): command passes through unchanged
- `rtk rewrite` crashes: command passes through unchanged
- `rtk rewrite` returns exit code 1 (no equivalent): command passes through unchanged
- non-terminal tool calls: hook returns immediately (no-op)

## Testing

```bash
cd /path/to/rtk
uv run --with pytest python -m pytest hermes/test_rtk_plugin.py -v
```

Tests cover: binary detection, rewrite logic, hook behavior, edge cases, error handling, and full integration flow.

## License

MIT — same as RTK.
