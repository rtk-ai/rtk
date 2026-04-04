---
title: Token Savings Analytics
description: Measure and analyze your RTK token savings with rtk gain
sidebar:
  order: 1
---

# Token Savings Analytics

`rtk gain` shows how many tokens RTK has saved across all your commands, with daily, weekly, and monthly breakdowns.

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
rtk gain --quota -t pro   # quota analysis (pro/5x/20x tiers)

# Export
rtk gain --all --format json > savings.json
rtk gain --all --format csv  > savings.csv
```

## Daily breakdown

```bash
rtk gain --daily
```

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
- **Input**: Estimated tokens from raw command output
- **Output**: Actual tokens after filtering
- **Saved**: Input - Output (tokens that never reached the LLM)
- **Save%**: Saved / Input × 100

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

| Command | Typical savings | Mechanism |
|---------|----------------|-----------|
| `rtk git status` | 77-93% | Compact stat format |
| `rtk eslint` | 84% | Group by rule |
| `rtk vitest run` | 94-99% | Show failures only |
| `rtk find` | 75% | Tree format |
| `rtk pnpm list` | 70-90% | Compact dependencies |
| `rtk grep` | 70% | Truncate + group |

## How token estimation works

RTK estimates tokens using `text.len() / 4` (4 characters per token average). This is accurate to ±10% compared to actual LLM tokenization — sufficient for trend analysis.

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

## Troubleshooting

**No data showing:**
```bash
ls -lh ~/.local/share/rtk/history.db
sqlite3 ~/.local/share/rtk/history.db "SELECT COUNT(*) FROM commands"
rtk git status    # run a tracked command to generate data
```

**Incorrect statistics:** Token estimation is a heuristic. For precise counts, use `tiktoken`:
```bash
pip install tiktoken
rtk git status > output.txt
python -c "
import tiktoken
enc = tiktoken.get_encoding('cl100k_base')
print(len(enc.encode(open('output.txt').read())), 'actual tokens')
"
```
