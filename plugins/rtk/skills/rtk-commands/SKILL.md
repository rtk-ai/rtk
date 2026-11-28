---
name: rtk-commands
description: >-
  Work effectively when shell output is condensed by RTK: what each filtered
  command shows, when to use rtk bypass or the recovery path a truncated result
  prints, and how pipelines and the optional rtk-proxy extend compression. Use
  when RTK is active in a session, when filtered output looks incomplete, or
  when choosing a pipeline.
---

# Working with RTK-filtered output

Reference: `README.md` ("Out of the box", "Pipelines", "RTK Proxy"),
`hooks/rtk-awareness.md`.

## Treat filtered output as complete

RTK keeps every signal and drops noise:

| Command | What you read |
|---------|---------------|
| `cargo test`, `npm test`, `pytest`, `go test` | Failures with assertions; passes collapsed to a count |
| `git status` / `diff` / `log` | Compact status, reduced diff context, one line per commit |
| `git add` / `commit` / `push` | One confirmation line |
| `ls`, `tree`, `grep`, `rg` | Counts, matches grouped by file, long lines cut |
| `ruff check`, `eslint`, `cargo clippy` | Grouped by rule and file |
| `docker ps`, `kubectl` | Essential fields |

Run commands normally and batch related ones into a single call. Exit codes
are passed through unchanged; if a filter fails you get raw output.

Explicit detail flags are respected: `cargo test -- --nocapture` or
`ls -la` pass more through.

## When to bypass

Truncated output states its own recovery path; follow it first. Use
`rtk bypass <cmd>` only when the result is unusable: empty when output was
clearly expected, contradicting the exit code, or garbled.

```bash
rtk bypass git log --oneline -20
rtk bypass npm install express
```

Bypassed runs are still tracked in `rtk gain`.

## Pipelines (optional)

A pipeline file adds compression beyond shell output: other tool results
(Claude Code), large reads handed to a cheaper model, terse agent prose, or
API-traffic cleanup via the proxy.

```bash
rtk pipeline add github:user/repo
```

A pipeline can rewrite commands and send content to the models it declares.
Read its file and confirm with the user before installing one.

## rtk-proxy (optional, separate BUSL-1.1 binary)

```bash
rtk claude              # start the daemon and launch Claude through it
rtk llm-proxy status
```

Not the same as `rtk bypass`.
