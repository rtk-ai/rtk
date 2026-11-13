---
title: Discover and Session
description: Find missed savings opportunities with rtk discover, and track RTK adoption with rtk session
sidebar:
  order: 2
---

# Discover and Session

## rtk discover — find missed savings

`rtk discover` analyzes your Claude Code command history to identify commands that ran without RTK filtering, and estimates how much bash output RTK would have removed from them.

```bash
rtk discover                    # analyze current project history
rtk discover --all              # all projects
rtk discover --all --since 7    # last 7 days, all projects
```

**Example output** (illustrative values):

```
Missed savings analysis (last 7 days)
────────────────────────────────────
Command              Count   Est. lost
cargo test              12     ~48,000 tokens
git log                  8     ~12,000 tokens
pnpm list                3      ~6,000 tokens
────────────────────────────────────
Total missed:           23     ~66,000 tokens

Run `rtk init --global` to capture these automatically.
```

The `~N tokens` figures are **estimated bash output bytes divided by 4**, not tokens billed by your provider. RTK ships no real tokenizer, and bash output is only one contributor to input tokens. Read them as an order of magnitude of the output volume RTK could have compressed. See [How RTK Savings Work](../resources/savings-explained.md).

If commands appear in the missed list after installing RTK, it usually means the hook isn't active for that agent. See [Troubleshooting](../resources/troubleshooting.md#ai-assistant-not-using-rtk) — "AI assistant not using RTK".

## rtk session — adoption tracking

`rtk session` shows RTK adoption across recent Claude Code sessions: how many shell commands ran through RTK vs. raw.

```bash
rtk session
```

**Example output** (illustrative values):

```
Recent sessions (last 10)
─────────────────────────────────────────────────────
Session                         Total   RTK   Coverage
2026-01-03 10:00  (40 cmds)       40    38      95.0%
2026-01-02 10:00  (40 cmds)       40    40     100.0%
2026-01-01 10:00  (40 cmds)       40    36      90.0%
─────────────────────────────────────────────────────
Average coverage: 95.0%
```

A command counts as RTK when it runs `rtk` (behind `timeout`, `env` or an assignment too, but not `rtk bypass`), or when it has an `rtk` rule and the hook rewrites it where it stands, with your `exclude_commands` and `transparent_prefixes`. A command with no rule that the hook rewrites anyway (`stat f` through a filter) does not count, and neither does one the hook declines: a pipe's raw consumer (the `head` of `git log | head -5`) or raw producer (`cat file | tail -5`). Low coverage on a session usually means RTK was disabled (`RTK_DISABLED=1`), the hook wasn't active for a specific subagent, or the commands were written in a form the hook leaves alone: a line with a substitution (`$(…)`) or a redirect to a file (`cargo test > test.log 2>&1`). `rtk discover` lists the commands on such a line as opportunities (unless the hook's log shows it rewrote the line), and a raw producer too; it leaves out a raw pipe consumer.
