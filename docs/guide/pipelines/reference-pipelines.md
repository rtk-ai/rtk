---
title: Reference Pipelines
description: The six built-in pipelines as literal files — rtk:default, rtk:posthook, rtk:proxy, rtk:delegate-reads, rtk:verbatim-diffs, rtk:terse
sidebar:
  order: 7
---

# Reference Pipelines

Printable with `rtk pipeline show <name>`. Adopt with `extends`; tune with [`[override]`](./writing-pipelines.md#overrides).

## `rtk:default` — RTK exactly as it works today

What every user has after upgrading, with no action. One hook event (`PreToolUse`), the awareness file, your `filters.toml` and `config.toml`. Nothing else is registered or changed.

It also carries one `proxy.request` rule, `normalize`, which does nothing unless you are running the daemon. It strips ANSI codes, progress bars and repeated lines — bytes no reader ever sees — and each of its methods is a pure function of the block's own content, so the request prefix stays byte-identical across turns and the upstream prompt cache keeps hitting.

```toml
schema_version = 1
[pipeline]
name = "rtk:default"

[[hook.before_tool.shell]]
name   = "rtk-rewrite"            # `ls` → `rtk ls` for every command RTK has a filter for; others untouched
action = "rewrite"

[[proxy.request.tool_result]]
name = "normalize"                # inert without a running daemon
use  = ["ansi", "progress", "dedup"]

[[directive]]
use = "rtk-awareness"
level = "default"     # today's [awareness] level in config.toml still applies

[guards]
never_worse = true
min_size = 512
```

How the rewrite flows: your agent calls `Bash "ls -la"` → the hook evaluates `hook.before_tool.shell` → `rtk-rewrite` matches → the call becomes `rtk ls -la` → inside `rtk`, the `ls` filter (built-in, or yours from `filters.toml`) applies. Excluding a command is a `passthrough` rule ahead of `rtk-rewrite`, or `[hooks] exclude_commands` in `config.toml` as before.

**Hooks follow the pipeline.** `rtk init` registers only the events your active pipeline uses, for the tools its rules name. With `rtk:default` that is `PreToolUse` on `Bash`, as today. Enable `rtk:posthook` and `PostToolUse` is registered too. Registration happens at `rtk init`: after adding a rule at a new event or for a new tool (a `hook.before_tool.read` rule, any `hook.after_tool` rule), run `rtk init -g` again. Until you do, `rtk pipeline validate`, `rtk gain` and `rtk init --show` print `hook needed: …`.

## `rtk:posthook` — compress every tool's output

Opt in. Adds `PostToolUse`: after a tool runs, its output is compressed before the agent reads it. Only representation changes; nothing is dropped except by `cap`, which keeps the full output for `rtk recall`.

```toml
[pipeline]
name = "rtk:posthook"
extends = "rtk:default"

[[hook.after_tool.shell]]
name = "term-default"             # floor for output that did not come through rtk
when = { is_rtk_command = false }
use  = ["term"]

[[hook.after_tool.read]]
name = "lockfiles"
when = { basename_in = ["package-lock.json", "Cargo.lock", "yarn.lock", "pnpm-lock.yaml"] }
use  = ["lockfile"]

[[hook.after_tool.read]]
name = "json-reads"
enabled = false
when = { format = "json" }
use  = ["json"]

[[hook.after_tool.read]]
name = "strip-line-numbers"
enabled = false
use  = ["strip-line-numbers"]

[[hook.after_tool.search]]
name = "grep-group"
when = { format = "matches" }     # grep output; glob file lists are not matches and pass untouched
use  = ["matches"]

[[hook.after_tool.web]]
name = "web-default"
use  = ["web"]
```

## `rtk:proxy` — cache alignment in the proxy

Opt in; needs the RTK proxy running (`rtk init` offers to install it). Normalisation
(`ansi`, `progress`, `dedup`) is inherited from `rtk:default`, so what this pipeline adds on top
is the cache breakpoint.

Both are **Anthropic-dialect only**, and `cache-align` does nothing for a client
that already sets its own `cache_control` breakpoints — Claude Code does. An
OpenAI-dialect client is measured but not compressed ([Provider dialects](../proxy/configuration.md#provider-dialects)).

```toml
[pipeline]
name = "rtk:proxy"
extends = "rtk:default"

[[proxy.request.cache]]
name = "align"
use  = ["cache-align"]
```

## `rtk:delegate-reads` — route oversize reads to a cheaper model

```toml
[pipeline]
name = "rtk:delegate-reads"
extends = "rtk:default"

[task.bulk-read]
kind = "summarize"    # no upstream: you supply the model
[task.code-write]
kind = "generate"

[[hook.before_tool.read]]
name   = "shunt-large-read"
when   = { file_lines_gt = 350, whole_file = true }
action = "route"
task   = "bulk-read"
```

Adopt it as in [the third example on the overview](./overview.md#three-files-you-might-write). Tune: `[override.shunt-large-read] when.file_lines_gt = 500`; `[task.bulk-read] temperature = 0.2 instructions = "…"`. `code-write` has no rule — denying a write wastes tokens already generated — the agent learns it from the generated `route-tasks` directive. A `cat big.java` is covered too: it is rewritten to `rtk read big.java`, which evaluates the same rule.

**Routing a command, and routing output** ([how routing works](./writing-pipelines.md#routing-before-a-tool)). The same action covers the other two
anchors. Neither is shipped in a built-in — write them yourself:

```toml
[pipeline]
name = "delegate-commands"
extends = "rtk:posthook"

[upstream.flash]
provider     = "google"
model        = "gemini-2.5-flash"
api_key_env  = "GEMINI_API_KEY"
timeout_secs = 8

[task.check-tests]
kind     = "digest"
upstream = "flash"

[[hook.before_tool.shell]]           # the command never runs in your context
name   = "delegate-tests"
when   = { command_matches = "^(npm|pnpm) test\\b" }
action = "route"
task   = "check-tests"

[[hook.after_tool.web]]              # the fetch already happened; digest it
name   = "digest-pages"
when   = { tokens_gt = 4000 }
action = "route"
task   = "check-tests"
```

The first denies the call and tells the agent to run
`rtk check-tests --question "…" -- npm test`; the exit code still comes from
`npm test`. The second sends the fetched page to the worker and returns its
bullets with a `rtk recall` hint for the original.

Prefer a transform chain where one is enough. `use = ["term"]`
costs no time and no tokens and keeps the whole output recoverable; an
`after_tool` route spends a provider round trip — seconds, on the hook path —
to throw detail away. It earns its keep on output that is both very large and
genuinely disposable, and `timeout_secs` is what bounds the damage when the
worker is slow.

## `rtk:verbatim-diffs` — no line numbers, diffs untouched

```toml
[pipeline]
name = "rtk:verbatim-diffs"
extends = ["rtk:posthook", "rtk:proxy"]

[override.strip-line-numbers]     # for agents whose edit tools do not address lines by number
enabled = true

[[hook.before_tool.shell]]        # diffs must reach the agent byte-for-byte
name   = "keep-diffs"
when   = { command_matches = "^git (diff|show)\\b" }
action = "passthrough"
```

Selective compression with a recovery path and grep grouping come from `rtk:posthook` (`term`, `matches`). It also inherits `rtk:proxy`, which adds the cache breakpoint (see [`rtk:proxy`](#rtkproxy--cache-alignment-in-the-proxy)).

## `rtk:terse` — terse prose

```toml
[pipeline]
name = "rtk:terse"
extends = "rtk:default"

[[directive]]
use = "terse-output"
```

A directive only: it changes what the agent *writes*, never what it reads, so it
composes with anything. For terse prose *and* output compression, name both —
`extends = ["rtk:terse", "rtk:posthook"]`. A directive is also the one thing you
can adopt without a pipeline at all: a file containing nothing but
`[[directive]] use = "terse-output"` is valid and leaves `rtk:default` intact.
