---
title: Claude Code Economics
description: Compare your Claude Code spending against RTK token savings
sidebar:
  order: 3
---

# Claude Code Economics

`rtk cc-economics` compares your Claude Code API spending (via ccusage) against the token savings RTK has generated. It answers: "what is RTK actually saving me in dollars?"

## Requirements

Requires [ccusage](https://github.com/ryoppippi/ccusage) to be installed and have data. ccusage tracks Claude Code API costs from your usage history.

## Usage

```bash
rtk cc-economics              # summary
rtk cc-economics --daily      # day-by-day breakdown
rtk cc-economics --weekly     # week-by-week
rtk cc-economics --monthly    # month-by-month
rtk cc-economics --all        # all breakdowns at once
rtk cc-economics --format json
```

## Example output

```
Claude Code Economics
════════════════════════════════════════
Total API cost      $12.40  (30 days)
RTK tokens saved    1.2M    (30 days)
Estimated savings   $3.20   (26% of bill)

At current savings rate:
  Monthly reduction: ~$3.20/mo
  Annual reduction:  ~$38/yr
```

## How savings are estimated

RTK estimates dollar savings by applying Claude's input token pricing to the tokens it prevented from reaching the LLM:

```
Saved tokens × (input price per token) = estimated dollar savings
```

This is an estimate — actual savings depend on which model was used for each request.

## See also

- [Token Savings Analytics](./gain.md) — the `rtk gain` command for raw token counts
- [Discover Missed Savings](./discover.md) — find commands that ran without RTK
