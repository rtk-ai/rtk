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
rtk gain --recalls        # recall efficiency per filter (calibration)

# Live dashboard
rtk gain                  # auto-launches the dashboard when the RTK Proxy runs
rtk gain --watch          # force the dashboard
rtk gain --text           # force the static view

# Attribution (where the savings came from)
rtk gain --by-method      # per compression method
rtk gain --by-rule        # per pipeline rule
rtk gain --by-layer       # per token layer (read, tool_result, history, ...)
rtk gain --by-pipeline    # per pipeline
rtk gain --pipeline NAME  # one pipeline, sliced by where its savings land
rtk gain --shadow         # what shadow rules would have saved

# Export
rtk gain --all --format json > savings.json
rtk gain --all --format csv  > savings.csv

# Reset
rtk gain --reset          # zero the savings and recall stats (asks first)
```

## Daily breakdown

```bash
rtk gain --daily
```

**Example output** (illustrative values — yours depend entirely on which commands you run):

```
📅 Daily Breakdown (3 days)
════════════════════════════════════════════════════════════════
Date            Cmds      Input     Output      Saved   Save%
────────────────────────────────────────────────────────────────
2026-01-01       100     500.0K     100.0K     400.0K   80.0%
2026-01-02       100     500.0K     100.0K     400.0K   80.0%
2026-01-03       100     500.0K     100.0K     400.0K   80.0%
────────────────────────────────────────────────────────────────
TOTAL            300       1.5M     300.0K       1.2M   80.0%
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
    "total_commands": 300,
    "total_input": 1500000,
    "total_output": 300000,
    "total_saved": 1200000,
    "avg_savings_pct": 80.0
  },
  "daily": [...],
  "weekly": [...],
  "monthly": [...]
}
```

## How token estimation works

`rtk gain` estimates tokens as `bytes / 4`; RTK ships no tokenizer. The same estimator is applied to raw and filtered output, so the percentage is reliable; the absolute token counts are approximate and will not match your provider's billing.

```
Input Tokens  = estimate_tokens(raw_command_output)
Output Tokens = estimate_tokens(rtk_filtered_output)
Saved Tokens  = Input - Output
Savings %     = (Saved / Input) × 100
```

## Database

Savings data is stored locally in SQLite:

- **Location**: `~/.local/share/rtk/history.db` on Linux, `~/Library/Application Support/rtk/history.db` on macOS (override with `RTK_DB_PATH`) — paths below use the Linux location
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

Prefer `--reset` over deleting the file: it zeroes both the token savings and the
[recall stats](#recall-efficiency), which live separately, and leaves the database intact.

```bash
rtk gain --reset          # asks for confirmation
rtk gain --reset --yes    # skip the prompt (scripts)
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

## Live dashboard

`--watch` replaces the static report with a full-screen live dashboard covering **both** savings layers at once: the agent hooks trimming command output, and the [RTK Proxy](../proxy/overview.md) compressing billed input tokens.

```bash
rtk gain --watch
```

### Auto-launch

A bare `rtk gain` launches the dashboard **automatically** once the RTK Proxy is running. All four conditions must hold:

1. You passed no flag asking for a specific static view — no `--daily`, `--weekly`, `--monthly`, `--all`, `--history`, `--graph`, `--quota`, and `--format` left at `text`.
2. You did not pass `--text`.
3. Standard output is an interactive terminal — piping or redirecting opts out, so scripts and cron jobs are unaffected.
4. The RTK Proxy is running and reachable.

Without the proxy running, `rtk gain` behaves exactly as it always has.

`--text` is the escape hatch — it forces the static view with the proxy up:

```bash
rtk gain --text
```

`--watch` and `--text` are mutually exclusive. Asking for `--watch` without a terminal falls back to the text view with a warning rather than failing.

## Attribution

The default report tells you *how much* was saved. The attribution flags tell you *where it came from*, which helps you tune a pipeline. Each covers both the hook and proxy sites; proxy figures appear only if you run the [RTK Proxy](../proxy/overview.md).

```bash
rtk gain --by-method      # which compression method did the work
rtk gain --by-rule        # which pipeline rule fired
rtk gain --by-layer       # which token layer (read, tool_result, history, ...)
rtk gain --by-pipeline    # which pipeline
```

These combine — passing several renders several sections in one report.

### One pipeline in detail

`--pipeline <NAME>` slices a single pipeline by where its savings actually land: which tool outputs the hooks trimmed, and which methods compressed billed input.

```bash
rtk gain --pipeline my-pipeline
```

### Shadow rules

Shadow rules are recorded but never emitted — they let you measure a rule's effect before trusting it with real output. `--shadow` reports what they *would* have saved:

```bash
rtk gain --shadow
```

A shadow rule showing strong savings is a candidate to promote; one showing nothing is a rule you can delete.

:::note[Full edition only]
Rule, pipeline and shadow attribution describe the compression engine, which the Apache-2.0 edition does not include. See [Editions and Licensing](../resources/editions.md).
:::

## Recall efficiency

`--recalls` measures how often your AI assistant goes back for output a filter elided — the signal that a filter's cap is too aggressive for your workflow. Every time a filter stores elided output (an *elision*) and every time the assistant retrieves it (a *recall*), RTK counts it per filter:

```bash
rtk gain --recalls
```

Illustrative output:

```
Recall efficiency  (current mode: sqlite)

SQLITE (exact — reads go through rtk recall)
FILTER                   ELISIONS RECALLED   RATE
vitest                         50       20    40%
docker-images                 100        2     2%

TEE (approximate — bash-observed reads only)
FILTER                   ELISIONS RECALLED   RATE
cargo_test                     40        4   ≥10%
```

How to read it:

- **RATE** is the share of elided outputs the assistant went back for. Re-reading the same entry counts once — the rate measures *entries consulted*, not read commands.
- A **high rate** means that filter regularly hides output the assistant needs: every recall is an extra API round-trip you paid for. Consider raising that filter's cap.
- A **low rate** means the filter's cap is well calibrated — the elided output was noise.

The two sections are never merged because the data quality differs:

- **SQLITE (exact)**: reads go through `rtk recall <hash>`, the only access path, so the count is exact.
- **TEE (approximate)**: in legacy [tee mode](../getting-started/configuration.md#recall-system), reads are shell commands (`tail`, `cat`, `grep`, …) observed by the rewrite hook before execution. Editor or assistant file-tool reads are invisible, denied commands are not counted, and the per-file dedup window is finite — so the `≥` rate is approximate, not exact. A high rate is still a reliable signal that the cap is too aggressive.

Stats survive output-entry eviction and retention cleanup. Separately, statistics are capped at 500 `(filter, mode)` rows across both modes so dynamic command names cannot grow the database indefinitely. When a new row exceeds this limit, RTK drops the row with the fewest elisions plus recalls (oldest insertion breaks ties), keeping the newly added row. Existing oversized stores are trimmed when opened.

Counters for retained rows continue accumulating, but an evicted row's history is discarded, not added to another filter. If that filter appears again its counters restart, so these are bounded calibration statistics, not lifetime totals. A recall of an older output can therefore have no retained matching elision.

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
