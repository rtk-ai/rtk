---
title: Conditions and Judges
description: Every when key a rule can test, by position and subject — the agent's call context, proxy-safe keys, and AI classifiers (judges) that condition on meaning
sidebar:
  order: 3
---

# Conditions and Judges

## Operators

Keys combine with AND inside one `when`. For OR, add a second rule. Operators are suffixes on the key:

| Suffix | Meaning | Example |
|---|---|---|
| *(none)* | equals | `format = "json"` · `whole_file = true` |
| `_in` · `_not_in` | membership | `basename_in = ["Cargo.lock", "yarn.lock"]` |
| `_starts_with` | prefix | `command_starts_with = "git "` |
| `_matches` | regex | `command_matches = "^git (diff\|show)\\b"` |
| `_gt` `_gte` `_lt` `_lte` | numeric | `tokens_gt = 300` · `file_lines_gte = 350` |

Tool input fields: `input.<field>` with the same suffixes inside a nested table — `input = { limit_gt = 100 }` — and `input_missing = ["limit", "offset"]`.

## Hook conditions

A key not listed for the rule's subject and event is refused on a control rule (`action = …` — E004, the file does not load) and degraded on a transform rule (W015 — the rule can never match, the file still loads). See [Errors and Warnings](./errors.md).

| Subject | `before_tool` and `after_tool` | `after_tool` adds |
|---|---|---|
| all | `tool` (the category, or the raw tool name under `mcp`/`any`), `input.*`, `input_missing`, and the [call context](#call-context): `intent` `goal` `recent_calls` | `format` `bytes` `lines` `tokens` `duration_ms` |
| `read` | `file_path` `extension` `basename` `whole_file` (no `limit`/`offset` given) — and from disk, before the tool runs: `file_exists` `file_bytes` `file_lines` | |
| `write` `edit` | `file_path` `extension` `basename` | |
| `shell` | `command` `command_starts_with` `command_matches` `is_rtk_command` (the command runs through `rtk` — rewritten, a task, or `rtk recall`) | `exit_code` |
| `search` | `input.pattern` `input.path` | |
| `web` | `url` `input.query` | |
| `subagent` | `input.*` | |
| `mcp` | `server` (from `mcp__<server>__<tool>`), `input.*` | |

`file_lines` and `file_bytes` are computed under a cost cap (size first; lines counted only below a threshold) so the hook stays within budget on a huge file.

**Output `format`** (after a tool): `term` · `json` · `web` · `matches` (grep-style `path:line:content`) · `lockfile`.

**MCP output.** An `mcp` rule reads the two response shapes RTK can put back unambiguously: a `result` string, or a `content` array whose every block is text. A response carrying an image, an embedded resource or a mixed array is passed through untouched — there is no way to rewrite part of it and leave the rest coherent.

### Call context

What the agent was doing when it made the call — the same keys for every agent:

| Key | Holds | Where it comes from |
|---|---|---|
| `intent` | the agent's own words about this step (last ~600 chars) | Cursor: `agent_message`. Claude Code, Codex: the latest assistant text or reasoning in the transcript. Otherwise the tool's `description` |
| `goal` | the user's latest prompt (first ~600 chars) | the transcript |
| `recent_calls` | the turn's last 5 tool calls, newest first, `\|`-separated | the transcript |

They are read only when a rule or a judge at that position uses them, and are absent when the agent offers nothing. Claude Code writes its transcript asynchronously, so there `intent` can be the agent's reasoning from one step earlier — `goal` and `recent_calls` are always current.

Test them directly, or let a [judge](#judges--conditions-on-meaning) read them:

```toml
[pipeline]
extends = "rtk:delegate-reads"

[[hook.before_tool.read]]
name   = "big-reads-not-for-editing"
when   = { file_lines_gt = 400, whole_file = true, intent_matches = "(?i)understand|explain|overview" }
action = "route"
task   = "bulk-read"
```

## Proxy conditions

A block already in the conversation is part of the provider's cached prefix; if RTK treated it differently on one turn than on the previous one, the whole prefix would be re-billed. So a proxy rule may only look at things that are identical on every turn:

| Class | Keys | Legal on |
|---|---|---|
| block-intrinsic | `tool` `format` `shape` `tokens` `succeeded` | every part |
| session-stable | `model` `provider` `session` `project` | every part |
| turn-varying | `tokens_in` `message_count` `stream` | `user` and `any` only — rejected on `tool_result`, `tool_use`, `assistant`, `cache` (E010) |

`tool` at the proxy uses the same categories as the hooks (`shell`, `read`, `subagent` …). `shape` is not supplied by the daemon.

## Judges — conditions on meaning

A `[classifier.<name>]` asks a model a typed question and turns the answer into facts a rule conditions on. Three question kinds:

| `question` | Fact(s) produced | Tested with |
|---|---|---|
| `choice` | `class.<name>` (label) · `class.<name>.confidence` | `"class.intent" = "test_failure"`, `_in`, `.confidence_gte` |
| `score` | `score.<name>` (0–1) | `"score.quality_lt" = 0.4` — needs `rubric = [..]`, ordered worst to best |
| `noul` | `noul.<name>` (0–1 truth) | `"noul.is_error_gte" = 0.9` |

```toml
[upstream.jev]
provider     = "typesafe"
model        = "jev-latest"
base_url     = "https://api.typesafe.ai/v1/systemone"
api_key_env  = "TYPESAFE_API_KEY"
timeout_secs = 8

[classifier.intent]
upstream       = "jev"
question       = "choice"
instructions   = "What kind of tool output is this?"
input_chars    = 2000
min_confidence = 0.8

[classifier.intent.labels]
test_failure = "Failing test output: assertions, FAILED markers"
build_log    = "Compiler or build progress output"

[upstream.flash]
provider    = "google"
model       = "gemini-2.5-flash"
api_key_env = "GEMINI_API_KEY"

[task.digest-failures]
kind     = "digest"
upstream = "flash"

[[hook.after_tool.shell]]
name   = "route-test-failures"
when   = { "class.intent" = "test_failure", tokens_gt = 1500 }
action = "route"
task   = "digest-failures"
```

| Key | Meaning |
|---|---|
| `upstream` | an [`[upstream.*]`](./tasks.md#upstreams) with `base_url` and `api_key_env`, speaking the System One wire format (a label + confidence, not generated text) |
| `question` | `choice`, `score` or `noul` |
| `instructions` | what to judge |
| `input_chars` | how much of the block (or the call) is sent, from the head |
| `min_confidence` | below this, no fact is produced — rules cannot fire |
| `labels` / `rubric` | the answers a `choice` / `score` may give |
| `context` | optional, default `false` — after a tool, also send the judge the call that produced the output: the tool, its command or path, and the [call context](#call-context). See below |

How judges behave:

- **Before or after the tool.** After a tool the judge reads the output — and, with `context = true`, the call that produced it first: the tool, its command or path, size facts and the [call context](#call-context). That is what lets it tell output the agent wants to understand from output it needs verbatim. The call is sent in addition to `input_chars` of output, so the judge still sees as much output as before. Before a tool it reads the *call*: the tool, its command or path, size facts (`file_lines`, `file_bytes`), every `input.*` field — for Bash, the agent's own `description` of why it runs the command — the [call context](#call-context) and, for a read, the head of the file. The context is what separates "read to understand" from "read to edit", and it is what lets a judge delegate work before it is done. It costs one judge round trip per call it is asked about (cached afterwards), so put cheap fixed conditions in the same `when` (`file_lines_gt`, `command_matches`) and the judge is only asked about calls worth delegating.
- **Never on `deny`** (E020). Gate denies on fixed conditions.
- **Not under `proxy.request`.** Judges run at hook positions only.
- **Fail-open, always.** Missing key, endpoint down, timeout, or confidence under the floor: the fact is absent, absent facts never match, and the call or output goes through untouched.
- **Lazy, batched, memoized.** Nothing runs unless an enabled rule at that exact position conditions on a classifier; every question bound to one upstream travels in a single request; answers are cached per content, so repeated output classifies once.
- **Auth is declared, not discovered.** The upstream's `api_key_env` names the credential; `rtk pipeline validate`, `rtk trust` and `rtk init` print `auth needed: export VAR=...` for anything unset. See [Tasks and Upstreams](./tasks.md#upstreams) for `api_key_command` and for borrowing the key your agent already uses through the RTK Proxy.

### A judge before the tool

Delegate big reads, heavy commands and documentation fetches only when the judge thinks the agent needs a summary rather than the bytes:

```toml
[upstream.judge]
provider     = "laya"
model        = "laya"
base_url     = "http://127.0.0.1:41733/v1/systemone"
api_key_env  = "RTK_LAYA_TOKEN"
timeout_secs = 4

[upstream.haiku]
provider = "anthropic"
model    = "claude-haiku-4-5"

[classifier.skim]
upstream     = "judge"
question     = "noul"
instructions = "The agent is about to run this tool call. Use its intent, the user goal and its recent calls. Answer true if a short answer a smaller model could extract is enough, false if it needs the exact content."
input_chars  = 1500

[task.bulk-read]
kind     = "summarize"
upstream = "haiku"

[[hook.before_tool.read]]
name   = "judge-big-reads"
when   = { file_lines_gt = 300, whole_file = true, "noul.skim_gte" = 0.8 }
action = "route"
task   = "bulk-read"
```

The full file, which also judges heavy commands and documentation fetches, ships as the `judge-before-tool` example pipeline; `classify-routing` is the after-tool counterpart.

For any AI-driven decision, use a classifier.
