# RTK Token Savings Audit Guide

Complete guide to analyzing your rtk token savings with temporal breakdowns and data exports.

## Overview

The `rtk gain` command provides comprehensive analytics for tracking your token savings across time periods.

**Database Location**: `~/.local/share/rtk/history.db`
**Retention Policy**: 90 days
**Scope**: Global across all projects, worktrees, and Claude sessions

## Quick Reference

```bash
# Default summary view
rtk gain

# Temporal breakdowns
rtk gain --daily          # All days since tracking started
rtk gain --weekly         # Aggregated by week
rtk gain --monthly        # Aggregated by month
rtk gain --all            # Show all breakdowns at once

# Export formats
rtk gain --all --format json > savings.json
rtk gain --all --format csv > savings.csv

# Combined flags
rtk gain --graph --history --quota    # Classic view with extras
rtk gain --daily --weekly --monthly   # Multiple breakdowns
```

## Command Options

### Temporal Flags

| Flag | Description | Output |
|------|-------------|--------|
| `--daily` | Day-by-day breakdown | All days with full metrics |
| `--weekly` | Week-by-week breakdown | Aggregated by Sunday-Saturday weeks |
| `--monthly` | Month-by-month breakdown | Aggregated by calendar month |
| `--all` | All time breakdowns | Daily + Weekly + Monthly combined |

### Classic Flags (still available)

| Flag | Description |
|------|-------------|
| `--graph` | ASCII graph of last 30 days |
| `--history` | Recent 10 commands |
| `--quota` | Monthly quota analysis (Pro/5x/20x tiers) |
| `--tier <TIER>` | Quota tier: pro, 5x, 20x (default: 20x) |

### Export Formats

| Format | Flag | Use Case |
|--------|------|----------|
| `text` | `--format text` (default) | Terminal display |
| `json` | `--format json` | Programmatic analysis, APIs |
| `csv` | `--format csv` | Excel, data analysis, plotting |

## Output Examples

### Daily Breakdown

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

**Metrics explained:**
- **Cmds**: Number of rtk commands executed
- **Input**: Estimated tokens from raw command output
- **Output**: Actual tokens after rtk filtering
- **Saved**: Input - Output (tokens prevented from reaching LLM)
- **Save%**: Percentage reduction (Saved / Input × 100)

### Weekly Breakdown

```
📊 Weekly Breakdown (1 weeks)
════════════════════════════════════════════════════════════════════════
Week                      Cmds      Input     Output      Saved   Save%
────────────────────────────────────────────────────────────────────────
01-26 → 02-01              196       1.3M      59.2K       1.2M   95.6%
────────────────────────────────────────────────────────────────────────
TOTAL                      196       1.3M      59.2K       1.2M   95.6%
```

**Week definition**: Sunday to Saturday (ISO week starting Sunday at 00:00)

### Monthly Breakdown

```
📆 Monthly Breakdown (1 months)
════════════════════════════════════════════════════════════════
Month         Cmds      Input     Output      Saved   Save%
────────────────────────────────────────────────────────────────
2026-01        196       1.3M      59.2K       1.2M   95.6%
────────────────────────────────────────────────────────────────
TOTAL          196       1.3M      59.2K       1.2M   95.6%
```

**Month format**: YYYY-MM (calendar month)

### JSON Export

```json
{
  "summary": {
    "total_commands": 196,
    "total_input": 1276098,
    "total_output": 59244,
    "total_saved": 1220217,
    "avg_savings_pct": 95.62
  },
  "daily": [
    {
      "date": "2026-01-28",
      "commands": 89,
      "input_tokens": 380894,
      "output_tokens": 26744,
      "saved_tokens": 355779,
      "savings_pct": 93.41
    }
  ],
  "weekly": [...],
  "monthly": [...]
}
```

**Use cases:**
- API integration
- Custom dashboards
- Automated reporting
- Data pipeline ingestion

### CSV Export

```csv
# Daily Data
date,commands,input_tokens,output_tokens,saved_tokens,savings_pct
2026-01-28,89,380894,26744,355779,93.41
2026-01-29,102,894455,32445,863744,96.57

# Weekly Data
week_start,week_end,commands,input_tokens,output_tokens,saved_tokens,savings_pct
2026-01-26,2026-02-01,196,1276098,59244,1220217,95.62

# Monthly Data
month,commands,input_tokens,output_tokens,saved_tokens,savings_pct
2026-01,196,1276098,59244,1220217,95.62
```

**Use cases:**
- Excel analysis
- Python/R data science
- Google Sheets dashboards
- Matplotlib/seaborn plotting

## Analysis Workflows

### Weekly Progress Tracking

```bash
# Generate weekly report every Monday
rtk gain --weekly --format csv > reports/week-$(date +%Y-%W).csv

# Compare this week vs last week
rtk gain --weekly | tail -3
```

### Monthly Cost Analysis

```bash
# Export monthly data for budget review
rtk gain --monthly --format json | jq '.monthly[] |
  {month, saved_tokens, quota_pct: (.saved_tokens / 6000000 * 100)}'
```

### Data Science Analysis

```python
import pandas as pd
import subprocess

# Get CSV data
result = subprocess.run(['rtk', 'gain', '--all', '--format', 'csv'],
                       capture_output=True, text=True)

# Parse daily data
lines = result.stdout.split('\n')
daily_start = lines.index('# Daily Data') + 2
daily_end = lines.index('', daily_start)
daily_df = pd.read_csv(pd.StringIO('\n'.join(lines[daily_start:daily_end])))

# Plot savings trend
daily_df['date'] = pd.to_datetime(daily_df['date'])
daily_df.plot(x='date', y='savings_pct', kind='line')
```

### Excel Analysis

1. Export CSV: `rtk gain --all --format csv > rtk-data.csv`
2. Open in Excel
3. Create pivot tables:
   - Daily trends (line chart)
   - Weekly totals (bar chart)
   - Savings % distribution (histogram)

### Dashboard Creation

```bash
# Generate dashboard data daily via cron
0 0 * * * rtk gain --all --format json > /var/www/dashboard/rtk-stats.json

# Serve with static site
cat > index.html <<'EOF'
<script src="https://cdn.jsdelivr.net/npm/chart.js"></script>
<canvas id="savings"></canvas>
<script>
fetch('rtk-stats.json')
  .then(r => r.json())
  .then(data => {
    new Chart(document.getElementById('savings'), {
      type: 'line',
      data: {
        labels: data.daily.map(d => d.date),
        datasets: [{
          label: 'Daily Savings %',
          data: data.daily.map(d => d.savings_pct)
        }]
      }
    });
  });
</script>
EOF
```

## Understanding Token Savings

### Token Estimation

rtk estimates tokens using `text.len() / 4` (4 characters per token average).

**Accuracy**: ±10% compared to actual LLM tokenization (sufficient for trends).

### Savings Calculation

```
Input Tokens    = estimate_tokens(raw_command_output)
Output Tokens   = estimate_tokens(rtk_filtered_output)
Saved Tokens    = Input - Output
Savings %       = (Saved / Input) × 100
```

### Typical Savings by Command

| Command | Typical Savings | Mechanism |
|---------|----------------|-----------|
| `rtk git status` | 77-93% | Compact stat format |
| `rtk eslint` | 84% | Group by rule |
| `rtk vitest run` | 94-99% | Show failures only |
| `rtk find` | 75% | Tree format |
| `rtk pnpm list` | 70-90% | Compact dependencies |
| `rtk grep` | 70% | Truncate + group |

## Database Management

### Inspect Raw Data

```bash
# Location
ls -lh ~/.local/share/rtk/history.db

# Schema
sqlite3 ~/.local/share/rtk/history.db ".schema"

# Recent records
sqlite3 ~/.local/share/rtk/history.db \
  "SELECT timestamp, rtk_cmd, saved_tokens FROM commands
   ORDER BY timestamp DESC LIMIT 10"

# Total database size
sqlite3 ~/.local/share/rtk/history.db \
  "SELECT COUNT(*),
          SUM(saved_tokens) as total_saved,
          MIN(DATE(timestamp)) as first_record,
          MAX(DATE(timestamp)) as last_record
   FROM commands"
```

### Backup & Restore

```bash
# Backup
cp ~/.local/share/rtk/history.db ~/backups/rtk-history-$(date +%Y%m%d).db

# Restore
cp ~/backups/rtk-history-20260128.db ~/.local/share/rtk/history.db

# Export for migration
sqlite3 ~/.local/share/rtk/history.db .dump > rtk-backup.sql
```

### Cleanup

```bash
# Manual cleanup (older than 90 days)
sqlite3 ~/.local/share/rtk/history.db \
  "DELETE FROM commands WHERE timestamp < datetime('now', '-90 days')"

# Reset all data
rm ~/.local/share/rtk/history.db
# Next rtk command will recreate database
```

## Integration Examples

### GitHub Actions CI/CD

```yaml
# .github/workflows/rtk-stats.yml
name: RTK Stats Report
on:
  schedule:
    - cron: '0 0 * * 1'  # Weekly on Monday
jobs:
  stats:
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v3
      - name: Install rtk
        run: cargo install --path .
      - name: Generate report
        run: |
          rtk gain --weekly --format json > stats/week-$(date +%Y-%W).json
      - name: Commit stats
        run: |
          git add stats/
          git commit -m "Weekly rtk stats"
          git push
```

### Slack Bot

```python
import subprocess
import json
import requests

def send_rtk_stats():
    result = subprocess.run(['rtk', 'gain', '--format', 'json'],
                           capture_output=True, text=True)
    data = json.loads(result.stdout)

    message = f"""
    📊 *RTK Token Savings Report*

    Total Saved: {data['summary']['total_saved']:,} tokens
    Savings Rate: {data['summary']['avg_savings_pct']:.1f}%
    Commands: {data['summary']['total_commands']}
    """

    requests.post(SLACK_WEBHOOK_URL, json={'text': message})
```

## Troubleshooting

### No data showing

```bash
# Check if database exists
ls -lh ~/.local/share/rtk/history.db

# Check record count
sqlite3 ~/.local/share/rtk/history.db "SELECT COUNT(*) FROM commands"

# Run a tracked command to generate data
rtk git status
```

### Export fails

```bash
# Check for pipe errors
rtk gain --format json 2>&1 | tee /tmp/rtk-debug.log | jq .

# Use release build to avoid warnings
cargo build --release
./target/release/rtk gain --format json
```

### Incorrect statistics

Token estimation is a heuristic. For precise measurements:

```bash
# Install tiktoken
pip install tiktoken

# Validate estimation
rtk git status > output.txt
python -c "
import tiktoken
enc = tiktoken.get_encoding('cl100k_base')
text = open('output.txt').read()
print(f'Actual tokens: {len(enc.encode(text))}')
print(f'rtk estimate: {len(text) // 4}')
"
```

## Best Practices

1. **Regular Exports**: `rtk gain --all --format json > monthly-$(date +%Y%m).json`
2. **Trend Analysis**: Compare week-over-week savings to identify optimization opportunities
3. **Command Profiling**: Use `--history` to see which commands save the most
4. **Backup Before Cleanup**: Always backup before manual database operations
5. **CI Integration**: Track savings across team in shared dashboards

## See Also

- [README.md](../README.md) - Full rtk documentation
- [CLAUDE.md](../CLAUDE.md) - Claude Code integration guide
- [ARCHITECTURE.md](../ARCHITECTURE.md) - Technical architecture
