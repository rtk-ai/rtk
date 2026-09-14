---
title: Token Savings Analytics
description: Measure and analyze the bash output reduction RTK achieves with rtk gain
sidebar:
  order: 1
---

# Token Savings Analytics

`rtk gain` shows how much bash output RTK has removed across all your commands, with daily, weekly, and monthly breakdowns.

What `rtk gain` measures is the reduction in **bash output bytes**, converted to estimated tokens. Bash output is one contributor to input tokens, alongside your prompt, the system prompt and conversation history, and input tokens are in turn only part of the bill, which also counts output tokens. See [How RTK Savings Work](../resources/savings-explained.md) for the full picture.

## Quick reference

```bash
# Default summary
rtk gain

# Temporal breakdowns
rtk gain --daily          # all days since tracking started
rtk gain --weekly         # aggregated by week
rtk gain --monthly        # aggregated by month
rtk gain --all            # all breakdowns at once

# Classic flags
rtk gain --graph          # ASCII graph, last 30 days
rtk gain --history        # last 10 commands
rtk gain --quota          # monthly quota savings estimate (default tier: 20x)
rtk gain --quota -t pro   # use pro tier token budget for estimate

# Export
rtk gain --all --format json > savings.json
rtk gain --all --format csv  > savings.csv
```

## Daily breakdown

```bash
rtk gain --daily
```

**Example output** (illustrative numbers from one machine, not typical results — yours depend entirely on which commands you run):

```
📅 Daily Breakdown (3 days)
════════════════════════════════════════════════════════════════
Date            Cmds      Input     Output      Saved   Save%
────────────────────────────────────────────────────────────────
2026-01-28        89     380.9K      26.7K     355.8K   93.4%
2026-01-29       102     894.5K      32.4K     863.7K   96.6%
2026-01-30         5        749         55        694   92.7%
────────────────────────────────────────────────────────────────
TOTAL            196       1.3M      59.2K       1.2M   95.6%
```

- **Cmds**: RTK commands executed
- **Input**: Estimated tokens from raw command output (`bytes / 4`)
- **Output**: Estimated tokens after filtering (`bytes / 4`)
- **Saved**: Input - Output, in estimated tokens
- **Save%**: Saved / Input × 100 — a **bash output byte ratio**, not a share of your bill

## Weekly and monthly breakdowns

```bash
rtk gain --weekly
rtk gain --monthly
```

Same columns as daily, aggregated by Sunday-Saturday week or calendar month.

## Export formats

| Format | Flag | Use case |
|--------|------|----------|
| `text` | default | Terminal display |
| `json` | `--format json` | Programmatic analysis, dashboards |
| `csv` | `--format csv` | Excel, Python/R, Google Sheets |

**JSON structure:**
```json
{
  "summary": {
    "total_commands": 196,
    "total_input": 1276098,
    "total_output": 59244,
    "total_saved": 1220217,
    "avg_savings_pct": 95.62
  },
  "daily": [...],
  "weekly": [...],
  "monthly": [...]
}
```

## Typical savings by command

| Command | Bash output reduction | Mechanism |
|---------|----------------------|-----------|
| `git status` | 77-93% | Compact stat format |
| `eslint` | 84% | Group by rule |
| `jest` | 94-99% | Show failures only |
| `vitest` | 94-99% | Show failures only |
| `find` | 75% | Tree format |
| `pnpm list` | 70-90% | Compact dependencies |
| `grep` | 70% | Truncate + group |

These percentages measure bash output bytes removed, not cost reduction.

## How token estimation works

`rtk gain` estimates tokens as `bytes / 4` (`src/core/tracking.rs:1284`). RTK ships no real tokenizer by design: embedding one would cost startup time and would require a tokenizer per model, or a per-session model lookup, which RTK does not implement. The same estimator is applied to raw and filtered output, so the percentage is reliable; the absolute token counts are approximate and will not match your provider's billing.

```
Input Tokens  = estimate_tokens(raw_command_output)
Output Tokens = estimate_tokens(rtk_filtered_output)
Saved Tokens  = Input - Output
Savings %     = (Saved / Input) × 100
```

## Database

Savings data is stored locally in SQLite:

- **Location**: `~/.local/share/rtk/history.db` (Linux / macOS)
- **Retention**: 90 days (automatic cleanup)
- **Scope**: Global across all projects and Claude sessions

```bash
# Inspect raw data
sqlite3 ~/.local/share/rtk/history.db \
  "SELECT timestamp, rtk_cmd, saved_tokens FROM commands
   ORDER BY timestamp DESC LIMIT 10"

# Backup
cp ~/.local/share/rtk/history.db ~/backups/rtk-history-$(date +%Y%m%d).db

# Reset
rm ~/.local/share/rtk/history.db    # recreated on next command
```

## Analysis workflows

```bash
# Weekly progress: generate a CSV report every Monday
rtk gain --weekly --format csv > reports/week-$(date +%Y-%W).csv

# Monthly budget review
rtk gain --monthly --format json | jq '.monthly[] |
  {month, saved_tokens, quota_pct: (.saved_tokens / 6000000 * 100)}'

# Cron: daily JSON snapshot for a dashboard
0 0 * * * rtk gain --all --format json > /var/www/dashboard/rtk-stats.json
```

**Python/pandas:**
```python
import pandas as pd
import subprocess

result = subprocess.run(['rtk', 'gain', '--all', '--format', 'csv'],
                       capture_output=True, text=True)
lines = result.stdout.split('\n')
daily_start = lines.index('# Daily Data') + 2
daily_end = lines.index('', daily_start)
daily_df = pd.read_csv(pd.StringIO('\n'.join(lines[daily_start:daily_end])))
daily_df['date'] = pd.to_datetime(daily_df['date'])
daily_df.plot(x='date', y='savings_pct', kind='line')
```

**GitHub Actions (weekly stats):**
```yaml
on:
  schedule:
    - cron: '0 0 * * 1'
jobs:
  stats:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - run: cargo install --git https://github.com/rtk-ai/rtk --branch master rtk
      - run: rtk gain --weekly --format json > stats/week-$(date +%Y-%W).json
      - run: git add stats/ && git commit -m "Weekly rtk stats" && git push
```

## Quota estimate

`--quota` expresses the estimated tokens saved as a fraction of a monthly subscription budget. Like every other figure in `rtk gain`, it is derived from the `bytes / 4` estimate of bash output, so treat it as an order of magnitude rather than a billing forecast.

```bash
rtk gain --quota          # uses 20x tier by default
rtk gain --quota -t pro   # Claude Pro plan budget
rtk gain --quota -t 5x    # 5× usage plan budget
rtk gain --quota -t 20x   # 20× usage plan budget
```

The tiers (`pro`, `5x`, `20x`) correspond to Anthropic Claude API subscription levels, each with a different monthly token allocation. RTK uses those allocations as a denominator to express your savings as a percentage of your budget.

:::tip[Find missed savings]
`rtk gain` shows what RTK saved. To find commands that ran *without* RTK and calculate what you lost, see [rtk discover](./discover.md).
:::

## Troubleshooting

**No data showing:**
```bash
ls -lh ~/.local/share/rtk/history.db
sqlite3 ~/.local/share/rtk/history.db "SELECT COUNT(*) FROM commands"
git status    # run any tracked command to generate data
```

**Incorrect statistics:** Token estimation is a heuristic. For precise counts, use `tiktoken`:
```bash
pip install tiktoken
git status > output.txt
python -c "
import tiktoken
enc = tiktoken.get_encoding('cl100k_base')
print(len(enc.encode(open('output.txt').read())), 'actual tokens')
"
```
