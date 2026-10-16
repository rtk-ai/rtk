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

Commands RTK passes through without filtering are tracked at 0% reduction.

## Invocation and retrieval accounting

`rtk gain --format json` includes an additive `output_accounting` object.
`rtk gain --history --format json` also exports the latest 100 `commands` and
100 `recall_events` (the totals cover all retained rows, not only those pages).
`--project` scopes both sets by the working directory of each event.

- `invocation_id` is a fresh opaque identifier for an RTK process. A process
  may record multiple command rows, so the command row's `id` remains its key.
- Native Claude hooks carry an opaque `hook_event_id` in a hidden, leading
  `--hook-event-id` option for proven simple RTK rewrites. The option is not
  global: an operand with that spelling is not consumed by RTK. A command's
  hook event joins directly to `hook_decisions.event_id`, exposing the recorded
  `session_id` and `tool_use_id` without time-window matching.
- Compound commands, wrappers, other hook integrations and old history can
  have null hook IDs. Missing associations are never inferred from command text
  or timestamps. Permission decisions are still made on the original command.
- Every payload store gets an independent immutable `capture_id` in
  `recall.db`'s `recall_captures` table. Content deduplication does not deduplicate
  capture events. This metadata is capped at 1,000 rows and pruned when its
  payload disappears; it does not contain another copy of the raw output.
- Every measured `rtk recall` emission gets a fresh event ID. Partial selections,
  `--grep`, empty matches and repeated reads count separately. `emitted_bytes`
  measures the stdout bytes written; `output_tokens` uses RTK's byte-based
  estimator on those bytes. Failed lookups are not successful reads, and failed
  writes only count bytes accepted before the error.
- Legacy hash references do not identify a specific capture. Their
  `source_invocation_id` and `source_capture_id` remain null even if there is
  only one currently retained matching capture. Pruning older records must not
  turn an ambiguous reference into an apparently certain one.

`observed_net_saved_tokens = initial_input_tokens - initial_output_tokens -
retrieved_output_tokens` is signed and can be negative. It combines recorded
command output and retrieval events by **each event's own date and project**;
recalling an old capture today charges today's retrieval output. It is not
source-capture-date accounting, a complete agent-context measurement, a model
bill, or evidence of better task success or latency. Existing summary fields
continue to describe initial command output only.

`measurement_complete` is deliberately false: historical reads, external tee
file reads, stderr, and tracking failures are not measured here. Use
`unattributed_recall_events` and `commands_without_invocation_id` to assess
coverage rather than treating missing data as zero cost. Existing
`rtk gain --recalls` remains a distinct unique-content recall metric.

Tracking schema migrations are additive and leave old associations null.
Command and retrieval history uses the existing 90-day retention window and
`rtk gain --reset` clears both. IDs are local database/export metadata and are
not added to telemetry.

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
