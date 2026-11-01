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

Rows are grouped by what works — output compression first, then command rewriting without compression, then partial coverage — and alphabetically within each group. The table is generated from the engine's harness profiles (`engine/src/harness.rs`); a test fails when the two drift, so what you read here is what the binary does.

<!-- harness-table:start -->
| Agent | `rtk init` id | Pipeline rules | Rewrite the command | Block it | Compress tool output | Add context | Session start | Route via proxy |
|---|---|---|---|---|---|---|---|---|
| **Claude Code** | `claude-code` | yes | yes | yes | **yes** | yes | yes | yes |
| **Copilot CLI** | `copilot-cli` | — | yes | yes | **yes** | yes | yes | yes |
| **Hermes** | `hermes` | — | yes | yes | **yes** | yes | yes | yes |
| **Kilo Code** | `kilocode` | — | yes | yes | **yes** | yes | — | yes |
| **Mistral Vibe** | `vibe` | yes | yes | yes | **yes** | yes | — | — |
| **Oh My Pi** | `omp` | — | yes | yes | **yes** | yes | yes | yes |
| **OpenCode** | `opencode` | — | yes | yes | **yes** | yes | — | — |
| **Pi** | `pi` | — | yes | yes | **yes** | yes | yes | — |
| **Cursor** | `cursor` | yes | yes | yes | MCP only | yes | yes | — |
| **Antigravity** | `antigravity` | yes | yes | yes | — | — | yes | — |
| **Codex** | `codex` | yes | yes | yes | — | yes | yes | yes |
| **Copilot (VS Code)** | `vscode-copilot` | — | yes | yes | — | yes | yes | — |
| **Factory Droid** | `droid` | — | yes | yes | — | yes | yes | — |
| **Gemini CLI** | `gemini` | — | yes | yes | — | yes | yes | yes |
| **OpenClaw** | `openclaw` | — | yes | yes | — | — | yes | yes |
| **Trae** | `trae` | — | yes | yes | — | yes | yes | — |
| **Windsurf / Devin** | `windsurf` | — | yes | yes | — | yes | yes | — |
| **Cline** | `cline` | — | — | yes | — | yes | — | yes |
| **Kimi Code** | `kimi` | — | — | yes | — | — | yes | yes |
| **Roo Code** | `roo` | — | — | — | — | — | — | yes |
<!-- harness-table:end -->

**Pipeline rules** says whether the agent's hook evaluates your `pipeline.toml` — `deny`, `route`, judges, read gating, `after_tool` chains — or only RTK's built-in command rewrite. On every other agent the hook is the rewrite alone, and `rtk init` says so.

**Command rewriting — turning `git status` into `rtk git status` — is what most agents allow.** That is RTK's main mechanism, and it is where most of the savings come from.

**Compressing tool output after the fact is allowed by eight.** Everywhere else the model reads the raw result, because the agent provides no way to replace it — the proxy is the route there.

## Footnotes — where the detail matters

- **Copilot CLI**: output replacement relies on Copilot CLI's documented `modifiedResult` field.
- **OpenCode**: built-in tools only. MCP results are rebuilt after the hook runs, and built-in output is already truncated by OpenCode before RTK sees it.
- **Mistral Vibe**: replacement rides on a `deny` decision, reaches only the text form of the result, and the UI shows it as a warning. `rtk init` registers `post_tool` only when the pipeline has `after_tool` rules. MCP tools are spelled `<server>.<tool>`, so a `[[hook.before_tool.mcp]]` rule reaches them in the hook but `rtk init` cannot write a matcher for them.
- **Gemini CLI**: input edits merge rather than replace, and each one makes Gemini append a `[System] Tool input parameters were modified` note to the output — so rewriting costs a few tokens back.
- **Codex**: the rewrite is honoured only alongside an explicit `allow` decision, so Codex never asks on a rewritten command; its own approval runs on the rewritten argv afterwards. Hosted tools such as web search never reach a hook, so a `[[hook.before_tool.web]]` rule is reported as unservable. `PostToolUse` can add context but not replace the result. Project hooks must be trusted once with `/hooks`.
- **Cursor**: `preToolUse` accepts `ask` but does not enforce it. Matchers test the tool *type* (`Shell`, `Read`, `Write`, `Grep`, `Delete`, `Task`, `MCP:<tool>`); file edits are `Write`, and no web tool is documented, so a `web` rule is reported as unservable. An invalid hook response blocks the call, so RTK always answers with valid JSON.
- **Antigravity**: `PreToolUse` rewrites through the documented `overwrite` field (the result then carries a notice naming the changed keys) and can `ask`. `PostToolUse` carries no result and its output is ignored, so output is immutable — the proxy is not available either. `SessionStart` and `PreInvocation` accept injected context.
- **Windsurf**: now **Devin Desktop**. Older Windsurf installs are block-only and cannot rewrite.
- **Cline**: the VS Code extension silently drops input overrides and cannot replace output. Both work only in the SDK/CLI plugin runtime, which is a different install. Hooks are also off by default — enable them in Feature Settings.
- **Rules file only**: for Kilo Code, Cline, Windsurf / Devin and Kimi Code, `rtk init` writes a rules file asking the model to prefer `rtk <cmd>` and installs no hook. Their row shows what the agent's hook API allows; with RTK today, nothing is blocked, rewritten or compressed.

## What this means per pipeline position

| Position | Where it runs |
|---|---|
| `hook.before_tool.shell` | every agent with a hook (rewrite only where **Pipeline rules** is "—") |
| `hook.before_tool.{read,write,edit,search,web,subagent,mcp,any}` | Claude Code, Codex, Cursor, Antigravity, Mistral Vibe |
| `hook.after_tool.*` | Claude Code, Mistral Vibe |
| `proxy.request.*` | any agent you can point at the daemon (the last column above) |
| `[[directive]]` on `awareness_file` | every agent |
| `[[directive]]` on `session_start` | Claude Code, Codex, Cursor, Antigravity |

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
when   = { server = "payments" }
```

The `tool` fact is the *category* — `edit`, `web`, `search` — so one rule covers
agents that spell the tool differently. Under `mcp` and `any` it is the raw tool
name instead. Prefer `server` over `tool_starts_with = "mcp__…"`: the `server`
fact is read from each agent's own MCP spelling (`mcp__payments__charge` on
Claude Code and Codex, `payments.charge` on Mistral Vibe), while a prefix test
only matches the agents that use that prefix.

The `input.*` facts carry each agent's own argument names, with the common ones
normalised so a rule written once reaches every agent: `input.command` for the
shell command (Antigravity's `CommandLine`), `input.file_path` for the file a
read, write or edit touches (Antigravity's `TargetFile` and `AbsolutePath`),
`input.url` for a fetch.

`read` keeps its own evaluator because its conditions (`file_exists`,
`file_bytes`, `file_lines`) are read from disk before the tool runs.

## Agents that cannot be made agnostic

Two are limited by the agent's own hook API:

- **Kimi Code** — the hook API is allow/deny with no mutation anywhere. Its API traffic *is* proxyable, so the proxy layer is the route.
- **Roo Code** — has no hooks. Only the awareness file and an OpenAI-compatible base URL are available.

## Checking your own setup

```bash
rtk pipeline validate --agent all
rtk pipeline explain --agent cursor --tool Shell --command "git status"
```

`validate --agent` lists every rule position and `session_start` directive of your pipeline that the named agents cannot serve, with the reason; `rtk init --agent <id>` prints the same lines when it registers the hooks. `explain` reports the subject your tool maps to, the agent's capabilities for that call, which rules fire, and how far RTK can take the call and the output on that agent. Agent names accept either spelling — `claude` and `claude-code` both work.
