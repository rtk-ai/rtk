---
title: Writing a Pipeline
description: Where pipeline files live, the rule tree (site, event, subject), rule anatomy, actions, routing to a cheaper model, and overrides
sidebar:
  order: 2
---

# Writing a Pipeline

## The file

### Where it lives, what wins

| File | Scope | Notes |
|---|---|---|
| `rtk:default`, `rtk:posthook`, `rtk:proxy`, `rtk:delegate-reads`, `rtk:verbatim-diffs`, `rtk:terse` | built in | read-only; compiled into the binary — see [Reference Pipelines](./reference-pipelines.md) |
| `~/.config/rtk/pipeline.toml` | you | loads first |
| `~/.config/rtk/pipelines/*.toml` | you | every file loads, alphabetically — name them `10-…`, `20-…` if order matters |
| `.rtk/pipeline.toml` | this project | **trust-gated** — `rtk init` in the project shows what it contains and asks; a changed file asks again |
| `.rtk/pipelines/*.toml` | this project | every file loads, alphabetically; each trust-gated |
| `~/.config/rtk/filters.toml`, `.rtk/filters.toml` | you · project | your custom command filters, unchanged — see [Methods](./methods.md#your-own-command-filters) |

Paths use the Linux spelling. The user directory is the platform config dir: `~/.config/rtk/` on Linux, `~/Library/Application Support/rtk/` on macOS, `%APPDATA%\rtk\` on Windows.

**Load order:** built-ins → your `pipeline.toml` → your `pipelines/` → the project's `pipeline.toml` → the project's `pipelines/`. Later wins. One file is the simple case; a directory is how you keep several pipelines and turn them on by dropping them in.

**Across files:** definitions (`[task]`, `[upstream]`, `[method]`) merge by key; `[override]` applies later-wins per field; a rule or directive name that appears in two loaded files is an error (E013) naming both paths — nothing shadows silently, and `rtk pipeline show` prints the source file of every line. Files do not extend each other: they are all loaded, so to reuse a piece, give it its own file.

`proxy.request` rules belong in your user files: one daemon serves every project, so it never reads a project's `.rtk/` pipeline (W017).

`config.toml` is unchanged and separate: preferences (`[tracking]`, `[display]`, `[hooks]`, `[retriever]`, `[telemetry]`, `[limits]`, `[awareness]`) stay there. The pipeline holds behaviour; it never replaces an existing RTK file.

### Layout

```text
schema_version = 1
[pipeline]                         # name, extends, enabled

[[hook.before_tool.<subject>]]     # rules — before a tool runs
[[hook.after_tool.<subject>]]      # rules — after a tool ran
[[proxy.request.<part>]]           # rules — the request, about to be sent

[[directive]]                      # what RTK tells the agent
[method.<name>]                    # chains, options for built-in methods
[classifier.<name>]                # AI judges rules can condition on
[task.<name>]                      # delegation commands
[upstream.<name>]                  # model endpoints
[override.<name>]                  # patch an inherited rule or directive
[guards]                           # global safety limits
```

A minimal file uses one or two of these.

### `[pipeline]`

| Key | Value |
|---|---|
| `name` | optional for your files; `rtk:*` for built-ins |
| `extends` | a built-in name or a list of them: `"rtk:default"`, `["rtk:delegate-reads", "rtk:terse"]`. Left to right; a shared ancestor is inherited once; the same rule name from two parents is an error unless the rules are identical. Built-ins only — local files compose by being loaded, not by extending each other. Omit for a standalone file. |
| `enabled` | `false` turns the whole file off without moving it. Default `true`. |

## The tree — where a rule lives

A rule's position says three things, in this order:

| Segment | Answers | Values |
|---|---|---|
| **site** | which RTK mechanism runs it | `hook` · `proxy` |
| **event** | when | `before_tool` · `after_tool` · `request` |
| **subject** | on what | a tool category (hook) · a part of the request (proxy) |

The position fixes what the rule can see and do. `file_lines` exists before a tool runs (the file is on disk); `format` exists after (there is output). A rule that names a condition its position cannot provide does not load — see [Conditions and Judges](./conditions.md).

### Hook subjects

You write the category; RTK maps your agent's tool names to it.

| Subject | Meaning | Claude Code tools it covers |
|---|---|---|
| `read` | read a file | Read, NotebookRead, TaskOutput |
| `write` | create or overwrite a file | Write |
| `edit` | modify a file | Edit, MultiEdit, NotebookEdit |
| `shell` | run a shell command | Bash, PowerShell, Monitor |
| `search` | search files or content | Grep, Glob, LSP |
| `web` | fetch or search the web | WebFetch, WebSearch |
| `subagent` | spawn or message a sub-agent | Task, Agent, SendMessage, ListAgents, Workflow |
| `mcp` | any MCP tool | `mcp__*`, ToolSearch, WaitForMcpServers, ListMcpResourcesTool, ReadMcpResourceTool |
| `any` | fallback — evaluated only if no rule under the specific subject matched | all |

Other agents are mapped the same way; `rtk pipeline explain` shows which category a call was assigned, and [Agent Coverage](../resources/agent-coverage.md) lists what each agent supports.

### Proxy parts

| Part | What the rule sees |
|---|---|
| `tool_result` | one tool-result block in the conversation |
| `tool_use` | one tool-call block (the agent's request, including a written file's body) |
| `assistant` | one assistant message |
| `cache` | cache breakpoint placement |
| `any` | any block; fallback |

What the proxy actually runs at these parts is on [What the Proxy Compresses](../proxy/compression.md).

### The token layers, for reference

If you think in the layers Claude Code's `/context` shows, this is where each is configured. Layers are how `rtk gain --by-layer` reports; they are never written in the file.

| Layer | Where |
|---|---|
| Tool results | `hook.after_tool.{shell,search,web,subagent,mcp}` (`rtk:posthook`) · `proxy.request.tool_result` |
| Read | `hook.before_tool.read` · `hook.after_tool.read` |
| Write | `proxy.request.tool_use` · the `code-write` task |
| History | `proxy.request.tool_result` · `proxy.request.assistant` |
| Cache | `proxy.request.cache` |
| Agent message | directive `terse-output` |
| Skills / MCP · User prompt | directives |
| Inference | `route` to a task before any tool, or after any tool with extractable output, optionally gated on a judge |

## Rules

### Anatomy

```toml
[[hook.after_tool.shell]]
name     = "term-default"
when     = { is_rtk_command = false }
use      = ["term"]
continue = false
enabled  = true
shadow   = false
```

| Key | Required | Notes |
|---|---|---|
| `name` | yes* | unique across the resolved pipeline. *Defaults to the first `use` entry when that is unambiguous; a derived name that collides with another rule is a warning — set it explicitly |
| `when` | no | conditions, all must hold; omitted = matches everything |
| `use` / `action` | exactly one | `use` is a chain of [methods](./methods.md); `action` is control flow (below) |
| `task` · `upstream` · `message` | with `route` / `deny` | see [Actions](#actions) |
| `mode` | no | `route` before a tool only: `suggest` (default) · `swap` · `answer` — see [Route modes](#route-modes) |
| `continue` | no | default `false` — first match wins; `true` keeps evaluating later rules |
| `enabled` | no | default `true`; a disabled rule is skipped but still shown by `rtk pipeline show` |
| `shadow` | no | transforms only; decide and record, emit nothing — see [Shadow mode](./commands.md#shadow-mode) |

### Order

Within one `[[site.event.subject]]` list: the project's rules first, then yours, then inherited rules — within a scope, files in load order, then declaration order. **First match wins** unless `continue = true`. The `any` list is evaluated only after the specific subject's list matched nothing.

`rtk pipeline validate` warns when a rule can never fire — for instance a rule after one with no `when` and no `continue` in the same list (W001).

### Actions

What a rule may do depends on its position:

| Position | `use = [...]` | `action =` | Extra keys |
|---|---|---|---|
| `hook.before_tool.shell` | — | `rewrite` · `deny` · `passthrough` · `route` | `task` with route; `message` optional |
| `hook.before_tool.read` | — | `deny` · `passthrough` · `route` | `task` with route; `message` optional |
| `hook.before_tool.write` | — | `deny` (warned: the body is already generated; a deny only buys a retry) · `passthrough` · `route` | `task` with route |
| `hook.before_tool.edit` | — | `deny` · `passthrough` | |
| `hook.before_tool.{search,web,subagent,mcp,any}` | — | `deny` · `passthrough` · `route` | `task` with route |
| `hook.after_tool.{shell,search,web,mcp,subagent,any}` | transform chain | `passthrough` · `route` | `task` with route |
| `hook.after_tool.read` | transform chain | `passthrough` · `route` (W021: breaks a later Edit) | `task` with route |
| `hook.after_tool.{write,edit}` | — (no output to extract, W013) | `passthrough` | |
| `proxy.request.{tool_result,tool_use,assistant,any}` | transform chain — cache-stable methods only | — | |
| `proxy.request.cache` | `["cache-align"]` | — | |

**`rewrite`** — prefix the command with `rtk` when RTK has a filter for it (`ls` → `rtk ls`); leave it untouched otherwise. It never takes a target or options. Its settings (`[hooks] exclude_commands`, `transparent_prefixes`) stay in `config.toml`.

**`deny`** — refuse the call. `message` is shown to the agent; `${file_path}`, `${file_lines}`, `${command}` interpolate. A deny can only depend on fixed conditions, never on a judge (E020).

**`passthrough`** — stop here, do nothing. Before a tool: exempt the command from `rewrite`. After: skip compression for this output.

**`route` + `task`** — hand the work to a cheaper model ([Tasks and Upstreams](./tasks.md)). Where the rule sits decides how.

### Routing before a tool

The call has not run, and no agent lets a hook return a result, so RTK denies the call with a generated message naming the task — *"Read of src/Service.java (4,120 lines) is blocked. Run: `rtk bulk-read --question "<what you need to know>" src/Service.java`"*, or for a command *"`npm test` is handled by a worker model. Run: `rtk check-tests --question "<what you need to know>" -- npm test`"* — and the agent runs it. The agent supplies the question because only the agent knows why it wanted the file or the command. Override the text with `message`. A routed command still exits with the command's own code.

What each call is handed over as, and the task kind that takes it (a mismatch is W023):

| Before | Handed over as | Task kind |
|---|---|---|
| `shell` | the command: `-- <command>` | `digest` |
| `read` | the file path | `summarize` or `digest` |
| `web` | `-- curl -sL <url>` | `digest` |
| `search` | `-- rg -n '<pattern>' <path>` | `digest` |
| `write` | `--spec … --reference … --target <file>` | `generate` |
| `subagent` | its `description` as the question, over files the agent names | `summarize` |
| `mcp`, `any` | a question only | a plugin kind |

Gate it on a [judge](./conditions.md#judges--conditions-on-meaning) to delegate only the calls worth it.

### Route modes

By default a route before a tool *suggests* the task, and the agent spends a turn running it. `mode` lets the rule deliver the cheaper answer in the same turn instead:

| `mode` | What happens | Extra turn | Works on |
|---|---|---|---|
| `suggest` *(default)* | the call is refused with a message naming the task; the agent decides to run it | yes | every subject, every agent |
| `swap` | the command is replaced by the task wrapping it — `rtk <task> --question "…" -- sh -c '<command>'` — and runs as the agent's own call | no | `shell` |
| `answer` | RTK runs the task itself and gives the agent the answer in place of the call | no | `read`, `search`, `web` |

```toml
[upstream.haiku]
provider = "anthropic"
model    = "claude-haiku-4-5"

[task.bulk-read]
kind     = "summarize"
upstream = "haiku"
context  = true

[task.check-build]
kind     = "digest"
upstream = "haiku"
context  = true

[[hook.before_tool.read]]
name   = "answer-big-reads"
when   = { file_lines_gt = 400, whole_file = true }
action = "route"
task   = "bulk-read"
mode   = "answer"

[[hook.before_tool.shell]]
name   = "swap-builds"
when   = { command_matches = "^mvn " }
action = "route"
task   = "check-build"
mode   = "swap"
```

With `swap` and `answer` there is no agent writing the question, so RTK asks one. With [`context = true`](./tasks.md#tasks) on the task, it is built from what the agent is doing — the user's request, the agent's current step and its recent calls. Without it, a generic question asks for what matters, keeping exact names, numbers and error lines.

- **`swap`** needs nothing from the hook: the agent's own call runs the task, and the command keeps its exit code. If the worker fails, the command's own output is shown. Agents that cannot replace a command show the swapped command as a suggestion instead.
- **`answer`** waits for the worker inside the hook, bounded by the upstream's `timeout_secs` — set it low. If anything fails (no key, timeout, empty answer), the original call runs as if the rule had not matched. After an answered read the agent can still read exact lines with `offset` and `limit`, since a ranged read is not `whole_file`.
- Any other combination is refused (E005): `swap` is for commands only, and `answer` on a shell command is what `swap` does without blocking the hook.

### Routing after a tool

On any subject with extractable output the output already exists, so there is nothing to ask the agent to re-run: RTK sends the output to the task and puts the answer in its place, with the original kept for `rtk recall`. The task must be a `digest` instance. Set [`context = true`](./tasks.md#tasks) on it so the worker knows what the agent was looking for; otherwise it answers a generic question.

This is the one rule that makes a provider call on the hook path — bound it with the upstream's `timeout_secs` (default 30 s; set it lower for the hook path). Every failure — no upstream, refused, timed out, empty answer — passes the original output through untouched.

`route` after a `read` is allowed but warned (W021): a later `Edit` needs the exact bytes, so keep it to files the agent only reads. After `write` and `edit` there is no output to replace (W013).

## Overrides

Everything you inherit has a name. `[override.<name>]` patches one inherited rule or directive in place; fields you do not mention are kept.

```toml
[pipeline]
extends = ["rtk:delegate-reads", "rtk:posthook"]

[override.shunt-large-read]
when.file_lines_gt = 500

[override.strip-line-numbers]
enabled = true

[override.term-default]
use = ["ansi", "dedup"]

[override.rtk-awareness]
level = "full"
```

| Field | Semantics |
|---|---|
| `enabled` `action` `task` `upstream` `message` `continue` `shadow` and module options | replaced |
| `when.<key> = v` | patches one key; the rest of `when` is kept |
| `when = { … }` | replaces the whole condition — `validate` warns and lists the keys you dropped (W003) |
| `use` | replaced whole |
| `name`, position in the tree | not overridable — that is a new rule |

Overriding a name that does not exist (E011), or declaring a new rule with an inherited name (E012), does not load. Project overrides beat user overrides beat built-ins. `rtk pipeline show` prints where each value came from.

**Add vs change:** to change how an inherited rule behaves, override it. To add behaviour, write a new rule with a new name — it is evaluated before the inherited ones and can shadow them for what it matches.
