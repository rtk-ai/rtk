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

**Example output** (sample numbers, not typical results):

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

If commands appear in the missed list after installing RTK, it usually means the hook isn't active for that agent. See [Troubleshooting](../resources/troubleshooting.md) — "Agent not using RTK".

## rtk session — adoption tracking

`rtk session` shows RTK adoption across recent Claude Code sessions: how many shell commands ran through RTK vs. raw.

```bash
rtk session
```

**Example output:**

```
Recent sessions (last 10)
─────────────────────────────────────────────────────
Session                         Total   RTK   Coverage
2026-04-06 14:32  (45 cmds)       45    43      95.6%
2026-04-05 09:14  (38 cmds)       38    38     100.0%
2026-04-04 16:50  (52 cmds)       52    49      94.2%
─────────────────────────────────────────────────────
Average coverage: 96.6%
```

Low coverage on a session usually means RTK was disabled (`RTK_DISABLED=1`) or the hook wasn't active for a specific subagent.
