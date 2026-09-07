# RTK Plugin for OpenClaw

Transparently rewrites shell commands executed via OpenClaw's `exec` tool to their RTK equivalents, cutting up to 90% of the bash output that reaches the LLM context.

This is the OpenClaw equivalent of the Claude Code hooks in `hooks/rtk-rewrite.sh`.

## How it works

The plugin registers a `before_tool_call` hook that intercepts `exec` tool calls. When the agent runs a command like `git status`, the plugin delegates to `rtk rewrite` which returns the optimized command (e.g. `rtk git status`). The compressed output enters the agent's context window, saving tokens.

All rewrite logic lives in RTK itself (`rtk rewrite`). This plugin is a thin delegate -- when new filters are added to RTK, the plugin picks them up automatically with zero changes.

## Installation

### Prerequisites

RTK must be installed and available in `$PATH`:

```bash
brew install rtk
# or
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Install the plugin

```bash
# Copy the plugin to OpenClaw's extensions directory
mkdir -p ~/.openclaw/extensions/rtk-rewrite
cp openclaw/index.ts openclaw/openclaw.plugin.json ~/.openclaw/extensions/rtk-rewrite/

# Restart the gateway
openclaw gateway restart
```

### Or install via OpenClaw CLI

```bash
openclaw plugins install ./openclaw
```

## Configuration

In `openclaw.json`:

```json5
{
  plugins: {
    entries: {
      "rtk-rewrite": {
        enabled: true,
        config: {
          enabled: true,    // Toggle rewriting on/off
          verbose: false     // Log rewrites to console
        }
      }
    }
  }
}
```

## Permissions

OpenClaw's own exec policy is the sole authority over whether a rewritten command runs.

The plugin calls `rtk rewrite --host openclaw`. That flag tells RTK to decide only whether a command *can be rewritten* and to leave whether it *may run* to `tools.exec.mode`, `security`, and `ask`, which OpenClaw applies after the `before_tool_call` hook returns. RTK reads no rule file of its own for this host.

Without the flag, RTK evaluates every command against Claude Code's four settings files (`.claude/settings.json`, `.claude/settings.local.json`, and the two under `~/.claude/`) and returns "ask" for anything they do not explicitly allow. The plugin turned that into a blocking approval prompt that denied on timeout, so a host running with `tools.exec.mode=full` still stopped on every rewritable command. See [#3908](https://github.com/rtk-ai/rtk/issues/3908).

Two consequences of the change:

- Deny rules in `.claude/settings.json` no longer apply to OpenClaw. If you were relying on them to gate OpenClaw, move the rules into OpenClaw's own exec configuration.
- The plugin no longer raises an approval prompt of its own. Approval prompts you still see come from OpenClaw.

RTK never rewrites a command containing a command substitution (`` ` ``, `$(...)`) or a redirect to a file, on any host. Those pass through unchanged.

### Writing exec rules

The plugin replaces `params.command` in `before_tool_call`, and OpenClaw folds hook adjustments into the parameters it passes to the exec tool. The tool therefore receives `rtk git push`, not `git push`. Write exec allow/deny rules against the `rtk` form. This was already true before the permission change.

### rtk version requirement

`rtk rewrite --host` needs an rtk build that carries [#3908](https://github.com/rtk-ai/rtk/issues/3908). On an older rtk, `--host` is absorbed into the command string RTK is asked to rewrite, nothing matches, and rtk exits 1: rewriting silently stops and no command is blocked or denied. If token savings disappear after installing this plugin, upgrade rtk.

## What gets rewritten

Everything that `rtk rewrite` supports (30+ commands). See the [full command list](https://github.com/rtk-ai/rtk#commands).

## What's NOT rewritten

Handled by `rtk rewrite` guards:
- Commands already using `rtk`
- Piped commands (`|`, `&&`, `;`)
- Heredocs (`<<`)
- Commands without an RTK filter

## Measured savings

| Command | Output reduction |
|---------|--------------|
| `git log --stat` | 87% |
| `ls -la` | 78% |
| `git status` | 66% |
| `grep` (single file) | 52% |
| `find -name` | 48% |

## License

Apache 2.0 -- same as RTK.
