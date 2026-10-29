# RTK Tracking

RTK records how much **bash output** each filtered command removed, so `rtk gain` can report your savings.

Everything the tracker stores is measured on bash output bytes, converted to estimated tokens (`bytes / 4`). Bash output is one contributor to input tokens, alongside your prompt, the system prompt and conversation history, and input tokens are in turn only part of the bill, which also counts output tokens. `savings_pct` is therefore a **bash output byte ratio** — not a cost figure and not a share of your token bill.

See [How RTK Savings Work](../guide/resources/savings-explained.md) for the full breakdown.

## Where the data lives

Tracking data is stored locally in a SQLite database named `history.db`:

| Platform | Default path |
|----------|--------------|
| Linux | `~/.local/share/rtk/history.db` |
| macOS | `~/Library/Application Support/rtk/history.db` |
| Windows | `%LOCALAPPDATA%\rtk\history.db` |

To use a different location, either set the `RTK_DB_PATH` environment variable (highest priority) or set it in `config.toml`:

```toml
[tracking]
database_path = "/custom/path/history.db"
```

## What is stored

For each command RTK runs, the database records:

| Field | Description |
|-------|-------------|
| `timestamp` | When the command ran (UTC) |
| `original_cmd` | The command as typed (e.g. `git log --oneline -5`) |
| `rtk_cmd` | The RTK command that ran (e.g. `rtk git log --oneline -5`) |
| `project_path` | The working directory, used by `rtk gain --project` |
| `input_tokens` | Estimated tokens in the raw output (`bytes / 4`) |
| `output_tokens` | Estimated tokens after filtering (`bytes / 4`) |
| `saved_tokens` | `input_tokens - output_tokens` |
| `savings_pct` | `saved / input × 100`, a bash output byte ratio |
| `exec_time_ms` | Execution time in milliseconds |

The same database also holds filter parse failures and hook rewrite decisions, which feed `rtk gain` and `rtk session`.

Commands RTK passes through without filtering are tracked at 0% reduction. They still count toward
`commands` and the token totals, but they are left out of `total_time_ms` and `avg_time_ms` (in the
summary, the daily/weekly/monthly breakdowns, and the JSON/CSV exports): only commands with captured
input (`input_tokens > 0`) are timed. A passthrough row filtered nothing, and its duration is the
wrapped process's lifetime — for an interactive TUI such as `rtk claude`, the whole session.

## Privacy

- The tracking database is **local only** and never leaves your machine.
- It is separate from telemetry. Telemetry is off by default and sends only anonymous aggregates if you opt in — see [Telemetry & Privacy](../guide/resources/telemetry.md).

## Retention

Records older than **90 days** are deleted automatically when new records are written.

## Viewing your data

```bash
rtk gain                  # summary
rtk gain --project        # current project only
rtk gain --daily          # day by day
rtk gain --weekly         # week by week
rtk gain --monthly        # month by month
rtk gain --history        # last 10 commands
```

See [Token Savings Analytics](../guide/analytics/gain.md) for the full `rtk gain` surface.

## Exporting

```bash
rtk gain --all --format json > savings.json
rtk gain --all --format csv  > savings.csv
```

Example JSON entry for one day (illustrative values):

```json
{
  "date": "2026-01-01",
  "commands": 100,
  "input_tokens": 500000,
  "output_tokens": 100000,
  "saved_tokens": 400000,
  "savings_pct": 80.0,
  "total_time_ms": 20000,
  "avg_time_ms": 200
}
```

Weekly entries carry `week_start` and `week_end` instead of `date`; monthly entries carry `month` (`YYYY-MM`).

You can also query the database directly:

```bash
sqlite3 ~/.local/share/rtk/history.db \
  "SELECT timestamp, rtk_cmd, saved_tokens FROM commands
   ORDER BY timestamp DESC LIMIT 10"
```

## Backup

```bash
cp ~/.local/share/rtk/history.db ~/backups/rtk-history-$(date +%Y%m%d).db
```

## Reset

```bash
rtk gain --reset          # asks for confirmation
rtk gain --reset --yes    # skip the prompt (scripts)
```

Alternatively, delete the database file; it is recreated on the next command:

```bash
rm ~/.local/share/rtk/history.db
```

## Troubleshooting

### Database locked error

If you see "database is locked" errors:
- Make sure no other tool holds the database open for writing
- Check file permissions on `history.db`
- As a last resort, delete it: `rm ~/.local/share/rtk/history.db && rtk gain`

### Token counts don't match my provider

Token estimation uses `~4 bytes = 1 token`. The percentage is reliable because the same estimator is applied to raw and filtered output, but absolute counts are approximate. For exact counts, run the raw and filtered output through your model's own tokenizer.

## See Also

- [Token Savings Analytics](../guide/analytics/gain.md)
- [Audit Guide](AUDIT_GUIDE.md)
- [README.md](../../README.md)
