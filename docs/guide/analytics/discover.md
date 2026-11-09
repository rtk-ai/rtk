---
title: Discover and Session
description: Find missed savings opportunities with rtk discover, and track RTK adoption with rtk session
sidebar:
  order: 2
---

# Discover and Session

## rtk discover — find missed savings

`rtk discover` analyzes your Claude Code and OpenCode command history to identify commands that ran without RTK filtering, and estimates how much bash output RTK would have removed from them.

```bash
rtk discover                    # analyze current project history
rtk discover --all              # all projects
rtk discover --all --since 7    # last 7 days, all projects
rtk discover --agent claude     # Claude Code history only
rtk discover --agent opencode   # OpenCode history only
rtk discover --agent all        # both stores, merged into one report
```

By default `discover` scans every supported agent whose storage exists on your machine (Claude Code and OpenCode). OpenCode history is read from OpenCode's own SQLite database, strictly read-only — RTK never writes to it. Because OpenCode calls never pass through RTK's hook, coverage is not measured for OpenCode sessions; the report says so explicitly instead of attributing a coverage figure to them.

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

Low coverage on a session usually means RTK was disabled (`RTK_DISABLED=1`) or the hook wasn't active for a specific subagent.
