---
name: rtk-gain
description: >-
  Measure what RTK saves and find missed savings with rtk gain, rtk discover
  and rtk session, including JSON export and how to read the numbers. Use when
  the user asks how much RTK is saving, wants a savings report, or wants to
  find commands that are not yet being compressed.
---

# Measuring RTK savings

Reference: `README.md` ("Measure it: rtk gain"),
`docs/guide/resources/savings-explained.md`.

## Commands

```bash
rtk gain                        # summary: today and since install
rtk gain --graph                # ASCII graph, last 30 days
rtk gain --history              # recent commands
rtk gain --daily                # day-by-day
rtk gain --all --format json    # export for dashboards

rtk discover                    # commands that could have been compressed
rtk discover --all --since 7    # every project, last 7 days

rtk session                     # RTK adoption across recent agent sessions
```

## Reading the numbers

- Savings apply to **command output** the agent reads, which is only part of
  input tokens, which are only part of cost. A 90% output cut is not a 90%
  cost cut; say so when reporting.
- Tokens are estimated as `bytes / 4`: percentages are reliable, absolute
  counts approximate.
- Everything is computed locally from RTK's SQLite history.

## Typical report

1. `rtk gain` for the headline.
2. `rtk gain --daily` for the trend.
3. `rtk discover --since 7` to list the biggest uncompressed commands and
   suggest either running them via a hooked agent or adding a filter.

If `rtk gain` reports "command not found" or no dashboard, the wrong `rtk`
is installed (see `rtk-setup`).
