# RTK Plugin for OpenClaw

Transparently rewrites shell commands executed via OpenClaw's `exec` tool to their RTK equivalents, achieving 60-90% LLM token savings.

This is the OpenClaw equivalent of the Claude Code hooks in `hooks/rtk-rewrite.sh`.

## How it works

The plugin registers a `before_tool_call` hook that intercepts `exec` tool calls. When the agent runs a command like `git status`, the plugin rewrites it to `rtk git status` before execution. The compressed output enters the agent's context window, saving tokens.

## Installation

### Prerequisites

RTK must be installed and available in `$PATH`:

```bash
brew install rtk-ai/tap/rtk
# or
curl -fsSL https://raw.githubusercontent.com/rtk-ai/rtk/refs/heads/master/install.sh | sh
```

### Install the plugin

```bash
# Copy the plugin to OpenClaw's extensions directory
mkdir -p ~/.openclaw/extensions/rtk-rewrite
cp openclaw/index.ts openclaw/openclaw.plugin.json ~/.openclaw/extensions/rtk-rewrite/

# Enable in OpenClaw config
openclaw config set plugins.entries.rtk-rewrite.enabled true

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

## What gets rewritten

| Command | Rewritten to |
|---------|-------------|
| `git status/diff/log/...` | `rtk git status/diff/log/...` |
| `gh pr/issue/run` | `rtk gh pr/issue/run` |
| `grep/rg` | `rtk grep` |
| `find` | `rtk find` |
| `ls` | `rtk ls` |
| `tsc/eslint/prettier` | `rtk tsc/lint/prettier` |
| `vitest/pytest/go test` | `rtk vitest/pytest/go test` |
| `docker ps/images/logs` | `rtk docker ps/images/logs` |
| `kubectl get/logs` | `rtk kubectl get/logs` |

## What's NOT rewritten (guards)

- Commands already using `rtk`
- Piped commands (`|`, `&&`, `;`)
- Heredocs (`<<`)
- Commands not in the rewrite table (e.g., `cat`, `echo`, `curl`)

## Measured savings

| Command | Token savings |
|---------|--------------|
| `git log --stat` | 87% |
| `ls -la` | 78% |
| `git status` | 66% |
| `grep` (single file) | 52% |
| `find -name` | 48% |

## How it compares to Claude Code hooks

| Feature | CC Hook (`hooks/rtk-rewrite.sh`) | OpenClaw Plugin |
|---------|----------------------------------|-----------------|
| Hook type | Shell script (PreToolUse) | TypeScript (before_tool_call) |
| Rewrite approach | Bash regex | JS regex |
| Installation | `rtk init --global` | Copy to extensions dir |
| Configuration | `.claude/settings.json` | `openclaw.json` |
| Scope | Claude Code sessions | All OpenClaw agents |

## License

MIT — same as RTK.
