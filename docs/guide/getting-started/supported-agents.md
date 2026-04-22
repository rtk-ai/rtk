---
title: Supported Agents
description: How to integrate RTK with Claude Code, Cursor, Copilot, Cline, Windsurf, Codex, OpenCode, Kilo Code, and Antigravity
sidebar:
  order: 3
---

# Supported Agents

RTK supports all major AI coding agents across 3 integration tiers. Mistral Vibe support is planned.

## How it works

Each agent integration intercepts CLI commands before execution and rewrites them to their RTK equivalent. The agent runs `rtk cargo test` instead of `cargo test`, sees filtered output, and uses up to 90% fewer tokens — without any change to your workflow.

All rewrite logic lives in the RTK binary (`rtk rewrite`). Agent hooks are thin delegates that parse the agent-specific JSON format and call `rtk rewrite` for the actual decision.

```
Agent runs "cargo test"
  -> Hook intercepts (PreToolUse / plugin event)
  -> Calls rtk rewrite "cargo test"
  -> Returns "rtk cargo test"
  -> Agent executes filtered command
  -> LLM sees 90% fewer tokens
```

## Supported agents

| Agent | Integration tier | Can rewrite transparently? |
|-------|-----------------|---------------------------|
| Claude Code | Shell hook (`PreToolUse`) | Yes |
| VS Code Copilot Chat | Shell hook (`PreToolUse`) | Yes |
| GitHub Copilot CLI | Shell hook (deny-with-suggestion) | No (agent retries) |
| Cursor | Shell hook (`preToolUse`) | Yes |
| Gemini CLI | Rust binary (`BeforeTool`) | Yes |
| OpenCode | TypeScript plugin (`tool.execute.before`) | Yes |
| OpenClaw | TypeScript plugin (`before_tool_call`) | Yes |
| Cline / Roo Code | Rules file (prompt-level) | N/A |
| Windsurf | Rules file (prompt-level) | N/A |
| Codex CLI | AGENTS.md instructions | N/A |
| Kilo Code | Rules file (prompt-level) | N/A |
| Google Antigravity | Rules file (prompt-level) | N/A |
| Mistral Vibe | Planned ([#800](https://github.com/rtk-ai/rtk/issues/800)) | Pending upstream |

## Installation by agent

### Claude Code

```bash
rtk init --global    # installs hook + patches settings.json
```

Restart Claude Code. Verify:

```bash
rtk init --show    # shows hook status
```

### Cursor

```bash
rtk init --global --cursor
```

Restart Cursor. The hook uses `preToolUse` with Cursor's `updated_input` format.

### VS Code Copilot Chat

```bash
rtk init --global --copilot
```

### Gemini CLI

```bash
rtk init --global --gemini
```

### OpenCode

```bash
rtk init --global --opencode
```

Creates `~/.config/opencode/plugins/rtk.ts`. Uses the `tool.execute.before` hook.

### OpenClaw

```bash
openclaw plugins install ./openclaw
```

Plugin in the `openclaw/` directory. Uses the `before_tool_call` hook, delegates to `rtk rewrite`.

### Cline / Roo Code

```bash
rtk init --cline    # creates .clinerules in current project
```

Cline reads `.clinerules` as custom instructions. RTK adds guidance telling Cline to prefer `rtk <cmd>` over raw commands.

### Windsurf

```bash
rtk init --windsurf    # creates .windsurfrules in current project
```

### Codex CLI

```bash
rtk init --codex    # creates AGENTS.md or patches existing one
```

### Kilo Code

```bash
rtk init --agent kilocode    # creates .kilocode/rules/rtk-rules.md in current project
```

Kilo Code reads `.kilocode/rules/` as custom instructions. RTK adds guidance telling Kilo Code to prefer `rtk <cmd>` over raw commands.

### Google Antigravity

```bash
rtk init --agent antigravity    # creates .agents/rules/antigravity-rtk-rules.md in current project
```

Antigravity reads `.agents/rules/` as custom instructions. RTK adds guidance telling Antigravity to prefer `rtk <cmd>` over raw commands.

### Mistral Vibe (planned)

Support is blocked on upstream `BeforeToolCallback` ([mistral-vibe#531](https://github.com/mistralai/mistral-vibe/issues/531)). Tracked in [#800](https://github.com/rtk-ai/rtk/issues/800).

## Integration tiers explained

| Tier | Mechanism | How rewrites work |
|------|-----------|------------------|
| **Full hook** | Shell script or Rust binary, intercepts via agent API | Transparent — agent never sees the raw command |
| **Plugin** | TypeScript/JS in agent's plugin system | Transparent — in-place mutation |
| **Rules file** | Prompt-level instructions | Guidance only — agent is told to prefer `rtk <cmd>` |

Rules file integrations (Cline, Windsurf, Codex, Kilo Code, Antigravity) rely on the model following instructions. Full hook integrations (Claude Code, Cursor, Gemini) are guaranteed — the command is rewritten before the agent sees it.

## Windows support

The shell hook (`rtk-rewrite.sh`) requires a Unix shell. On native Windows:

- `rtk init -g` automatically falls back to **CLAUDE.md injection mode** (prompt-level instructions)
- Filters work normally (`rtk cargo test`, `rtk git status`)
- Auto-rewrite does not work — the AI assistant is instructed to use RTK but commands are not intercepted

For full hook support on Windows, use [WSL](https://learn.microsoft.com/en-us/windows/wsl/install). Inside WSL, all agents with shell hook integration (Claude Code, Cursor, Gemini) work identically to Linux.

## Graceful degradation

Hooks never block command execution. If RTK is missing, the hook exits cleanly and the raw command runs unchanged:

- RTK binary not found: warning to stderr, exit 0
- Invalid JSON input: pass through unchanged
- RTK version too old: warning to stderr, exit 0
- Filter logic error: fallback to raw command output

## Override: disable RTK for one command

```bash
RTK_DISABLED=1 git status    # runs raw git status, no rewrite
```

Or exclude commands permanently in `~/.config/rtk/config.toml`:

```toml
[hooks]
exclude_commands = ["git rebase", "git cherry-pick"]
```
