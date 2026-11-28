---
title: Agent Coverage
description: Exactly which parts of a pipeline work on each supported agent, and where the agent itself is the limit
sidebar:
  order: 6
---

# Agent Coverage

A `pipeline.toml` is written once. How much of it actually runs depends on what your agent's hook API lets RTK do — and that varies a lot.

Where a column says "—", the agent does not offer the capability.

## The table

Rows are grouped by what works — output compression first, then command rewriting without compression, then partial coverage — and alphabetically within each group.

| Agent | Rewrite the command | Block it | Compress tool output | Add context | Session start | Route via proxy |
|---|---|---|---|---|---|---|
| **Claude Code** | yes | yes | **yes** | yes | yes | yes |
| **Copilot CLI** | yes | yes | **yes** ¹ | yes | yes | yes |
| **Hermes** | yes | yes | **yes** | yes | yes | yes |
| **Kilo Code** ⁸ | yes | yes | **yes** | yes | — | yes |
| **Mistral Vibe** | yes | yes | **yes** ³ | yes | — | — |
| **Oh My Pi** | yes | yes | **yes** | yes | yes | yes |
| **OpenCode** | yes | yes | **yes** ² | yes | — | — |
| **Pi** | yes | yes | **yes** | yes | yes | — |
| **Cursor** | yes | yes | MCP only | yes | yes | — |
| **Codex** | yes ⁵ | yes | — | yes | yes | yes |
| **Copilot (VS Code)** | yes | yes | — | yes | yes | — |
| **Factory Droid** | yes | yes | — | yes | yes | — |
| **Gemini CLI** | yes ⁴ | yes | — | yes | yes | yes |
| **OpenClaw** | yes | yes | — | — | yes | yes |
| **Trae** | yes | yes | — | yes | yes | — |
| **Windsurf / Devin** ⁸ | yes ⁶ | yes | — | yes | yes | — |
| **Antigravity** ⁸ | — | yes | — | — | — | — |
| **Cline** ⁸ | — ⁷ | yes | — | yes | — | yes |
| **Kimi Code** ⁸ | — | yes | — | — | yes | yes |
| **Roo Code** | — | — | — | — | — | yes |

**Command rewriting — turning `git status` into `rtk git status` — is allowed by 16 of 20; `rtk init` installs it on 14.** That is RTK's main mechanism, and it is where most of the savings come from.

**Compressing tool output after the fact is allowed by 8; `rtk init` installs it on 7.** Everywhere else the model reads the raw result, because the agent provides no way to replace it.

## Footnotes — where the detail matters

1. **Copilot CLI**: output replacement relies on Copilot CLI's documented `modifiedResult` field.
2. **OpenCode**: built-in tools only. MCP results are rebuilt after the hook runs, and built-in output is already truncated by OpenCode before RTK sees it.
3. **Mistral Vibe**: replacement rides on a `deny` decision, reaches only the text form of the result, and the UI shows it as a warning.
4. **Gemini CLI**: input edits merge rather than replace, and each one makes Gemini append a `[System] Tool input parameters were modified` note to the output — so rewriting costs a few tokens back.
5. **Codex**: the rewrite is honoured only alongside an explicit `allow` decision. Without it the hook fails and the original command runs unwrapped.
6. **Windsurf**: now **Devin Desktop**. Older Windsurf installs are block-only and cannot rewrite.
7. **Cline**: the VS Code extension silently drops input overrides and cannot replace output. Both work only in the SDK/CLI plugin runtime, which is a different install. Hooks are also off by default — enable them in Feature Settings.
8. **Rules file only**: for Kilo Code, Cline, Windsurf / Devin, Kimi Code and Antigravity, `rtk init` writes a rules file asking the model to prefer `rtk <cmd>` and installs no hook. Their row shows what the agent's hook API allows; with RTK today, nothing is blocked, rewritten or compressed.

## What this means per pipeline position

| Position | Where it runs |
|---|---|
| `hook.before_tool.shell` | every agent with a hook |
| `hook.before_tool.{write,edit,search,web,subagent,mcp,any}` | every agent with a hook |
| `hook.before_tool.read` | Claude Code |
| `hook.after_tool.*` | Claude Code |
| `proxy.request.*` | any agent you can point at the daemon (the last column above) |

A non-shell rule can `deny` or `passthrough`. It cannot rewrite the call — that
is a `shell` capability, because rewriting a command is what RTK does.

Each subject sees the facts its conditions need, plus `input.*` mirroring the
tool's own arguments so a rule can reach a field RTK has no named condition for:

```toml
[[hook.before_tool.edit]]
name    = "no-lockfile-edits"
action  = "deny"
message = "do not hand-edit lockfiles"
when    = { basename = "Cargo.lock" }

[[hook.before_tool.mcp]]
name   = "one-server-only"
action = "deny"
when   = { tool_starts_with = "mcp__payments__" }
```

The `tool` fact is the *category* — `edit`, `web`, `search` — so one rule covers
agents that spell the tool differently. Under `mcp` and `any` it is the raw tool
name instead, which is what lets a rule target a single MCP server.

`read` keeps its own evaluator because its conditions (`file_exists`,
`file_bytes`, `file_lines`) are read from disk before the tool runs.

## Agents that cannot be made agnostic

Three are limited by the agent's own hook API:

- **Antigravity** — hooks can block a call but never rewrite its arguments, `PostToolUse` returns `{}` so output is immutable, and there is no base-URL override. Deny-only.
- **Kimi Code** — the hook API is allow/deny with no mutation anywhere. Its API traffic *is* proxyable, so the proxy layer is the route.
- **Roo Code** — has no hooks. Only the awareness file and an OpenAI-compatible base URL are available.

## Checking your own setup

```bash
rtk pipeline explain --agent cursor --tool Shell --command "git status"
```

`explain` reports the subject your tool maps to, which rules fire, and how far RTK can take the output on that agent. Agent names accept either spelling — `claude` and `claude-code` both work.
