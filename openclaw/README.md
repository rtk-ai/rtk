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

### Build

OpenClaw package installs require compiled JavaScript, and the plugin manifest must point at it:

```bash
cd openclaw
npm install
npm run build   # emits dist/index.js
```

### Install the plugin

```bash
openclaw plugins install ./openclaw --accept-capabilities

# Restart the gateway so the hook is registered
openclaw gateway restart
```

### Activation is required

`openclaw.plugin.json` sets `activation.onStartup: true`. Without explicit activation metadata
OpenClaw does not import the plugin into the Gateway at startup, so the `before_tool_call` hook is
never registered — even while `openclaw plugins list` and `openclaw plugins doctor` both report the
plugin as enabled and healthy. Verify the hook is live by checking the Gateway's startup plugin list
(`openclaw health`, or the Gateway log line `http server listening (N plugins: ...)`), not the plugin
list alone.

### Allow rules for headless agents

`rtk rewrite` derives its allow/ask/deny verdict from Claude Code `Bash()` permission rules
(`~/.claude/settings.json`). With no matching allow rule every rewritable command returns "ask",
which this plugin turns into an approval request that is denied on timeout — so an unattended agent
stalls on nearly every command. Give the service user allow rules for the families you want
auto-rewritten:

```json
{ "permissions": { "allow": ["Bash(git:*)", "Bash(ls:*)", "Bash(rg:*)"] } }
```

Both `Bash(git:*)` and `Bash(git *)` work as prefix patterns. Deny rules take precedence and cause
the plugin to block the tool call.

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
