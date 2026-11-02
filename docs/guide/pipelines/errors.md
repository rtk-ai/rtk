---
title: Errors and Warnings
description: Every pipeline diagnostic code — what fails closed, what degrades — plus the engine's limits
sidebar:
  order: 9
---

# Errors and Warnings

`rtk pipeline validate` checks every file and prints each problem with its file, line, rule and a hint:

```
~/.config/rtk/pipeline.toml:41  [[hook.after_tool.shell]] "my-term"
  E003  condition `file_lines` is not available after the tool ran
        hint: `file_lines` is a before_tool condition; for output size use `lines`
```

Validation never runs inside an agent session. If a file fails to load while the agent is working, RTK drops that file only and records the problem for `rtk pipeline status` and `rtk gain` — see [Pipeline Commands](./commands.md#validating-your-own-pipelines).

## Errors fail closed, warnings load

Errors stop that file from loading; the other files stay active (W025 names the dropped file, and a file that depends on it — a rule routing to a task it defined — is dropped with it). Only an error that cannot be tied to a file falls back to `rtk:default`. Warnings load.

**Transform rules fail open** (an unknown method or condition is a warning — W014/W015 — and only that rule goes inert); **control rules fail closed** (a rule with `action = …`, and every task kind, must validate completely).

| Code | Error |
|---|---|
| E001 | unknown section or position (`[[hook.during_tool.shell]]`) |
| E002 | unknown key on a rule, directive, task, upstream or override |
| E003 | condition not available at this event (`file_lines` after the tool) |
| E004 | condition not available for this subject (`command` on `read`) — **control rules only**; on a transform rule this is W015 |
| E005 | action not legal at this position (`rewrite` outside `before_tool.shell`, `deny` after a tool, `route` before an edit), or a [route `mode`](./writing-pipelines.md#route-modes) it cannot use (`swap` outside `shell`, `answer` outside `read`/`search`/`web`, `mode` without `route`) |
| E006 | a rule has both `use` and `action`, or neither |
| E007 | unknown method, chain or directive — for methods, **control rules only**; on a transform rule this is W014 |
| E008 | chain type mismatch (`["toon", "cap"]`) or a terminal method not last |
| E010 | method under `proxy.*` is not cache-stable, or a turn-varying condition on a cached part |
| E011 | `[override]` target does not exist (did-you-mean listed) |
| E012 | new rule declared with an inherited name |
| E013 | the same rule or directive name in two loaded files (both paths listed) |
| E014 | task: unknown `kind`, unknown `upstream`, name collides with an `rtk` command |
| E015 | upstream: literal key (`api_key = "…"`); use `api_key_env` |
| E016 | `[method.<name>]` carries filter keys (`match_command`, `strip_lines_matching`, …) — filters live in `filters.toml`; or a built-in method name carries `chain` or options (built-in methods take none) |
| E017 | `extends`: two parents define the same rule name differently |
| E018 | unknown subject or tool category |
| E019 | classifier problem: unknown classifier or label in a condition, question/key mismatch, missing or unknown upstream, bad `question` or `min_confidence` |
| E020 | `deny` gated on a classifier — the protection would vanish whenever the judge is unavailable |

| Code | Warning |
|---|---|
| W001 | rule is unreachable (a catch-all precedes it) |
| W002 | derived `name` collides — set it explicitly |
| W003 | `when = { … }` in an override dropped keys: … |
| W004 | `deny` under `write` — the body is already generated |
| W005 | `proxy.*` rules present but the proxy is not running |
| W006 | `rtk-rewrite` disabled — RTK's primary saving is off |
| W007 | `route` to a task with no upstream — inert |
| W008 | rule at a `proxy.request` position that is not active (`user`, `system`, `tools`, `model`) — the rule is inert |
| W009 | a key, condition or method this edition does not support — the rule is inert; the file still loads |
| W010 | an unsupported key is present (`[guards] max_recovery_rate`) — ignored; the file still loads |
| W011 | a proxy-only method (`cache-align`) named outside `proxy.*` — it is skipped there; looks through `[method.*]` chains |
| W012 | every method in a rule is skipped at that position — the rule matches and does nothing |
| W013 | `use` chain or `route` at `hook.after_tool.{write,edit}` — no output to extract, the rule is inert |
| W014 | unknown method or chain on a transform rule — skipped at runtime, the rest of the chain still runs; the file still loads (newer rtk or a plugin you don't have?) |
| W015 | unknown condition on a transform rule — the rule can never match on this rtk; the file still loads |
| W016 | method skipped under `proxy.request` — it needs a recall copy the daemon cannot store, or sits at a part the daemon does not transform; the file still loads |
| W017 | `proxy.request` rule in a project file — the daemon reads only user pipelines; move it |
| W018 | `shadow` on a `proxy.request` rule — not recorded; the rule is skipped |
| W019 | directive `channel` other than `awareness_file` — the directive is skipped |
| W021 | `route` after a `read` — the summary replaces the bytes a later Edit needs |
| W022 | slow method after a tool — every matching call waits for it |
| W023 | the routed task's kind cannot take this call (see [Routing before a tool](./writing-pipelines.md#routing-before-a-tool)) |
| W024 | a rule field has the wrong type (`enabled = "false"`, `action = 5`) — the rule is disabled; the rest of the file still loads |
| W025 | a pipeline file has errors and is not loaded; the other files stay active (the file's own errors are listed with it) |

## Limits

- **No file contains a command to run.** Rewrite is fixed to the `rtk` prefix; tasks call a model, not a shell. Cloning a repository cannot make RTK execute anything.
- **No file names an agent.** Subjects are categories; RTK maps them at `rtk init`.
- `proxy.*` rules need the RTK Proxy in the path. Without it they are inert and `validate` says so (W005).
- A `route` rule needs its task bound to an upstream.
- There are two scopes: you and the project.
