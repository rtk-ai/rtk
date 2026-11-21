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
summary, the daily/weekly/monthly breakdowns, and the JSON/CSV exports). Timing classification uses
the stored label: canonical passthrough, fallback, bypass, proxy, and the unambiguous legacy
`rtk ... (passthrough)` labels are untimed, while ordinary filtered labels remain timed even when the
filter emits zero output. Releases up to v1.1.0 stored two passthroughs under an ordinary label,
`rtk curl ...` for a binary response and `rtk git show ...` for raw blob bytes. Those rows cannot be
told apart from filtered runs, so they count as timed, including the ones v1.1.0 left out because
they had no captured output.
A passthrough row's duration is the wrapped process's lifetime, which can be the whole session for an
interactive TUI such as `rtk claude`.

## Invocation and retrieval accounting

`rtk gain --history --format json` exports an `output_accounting` object and
the latest 100 `commands` and 100 `recall_events`. The totals cover all retained
rows in their respective cohorts, not just those pages. `--project` scopes both
sets by the event's own working directory.

- `invocation_id` is a local opaque identifier for an RTK process. A process
  may record multiple command rows, so the command row's `id` remains its key.
  Old rows keep a null `invocation_id`.
- Each measured `rtk recall` emission has its own event ID and an `invoking_id`.
  Partial selections, `--grep`, empty matches and repeated reads count separately.
  `emitted_bytes` counts bytes the stdout writer accepted, and `output_tokens`
  estimates tokens from those bytes. A failed lookup records no emission.
  A write failure records only bytes accepted before the error, if any.
- Hash references don't identify a specific capture. Their
  `source_invocation_id` and `source_capture_id` are null.

`initial_input_tokens` and `initial_output_tokens` use the same headline cap as
`summary`; `initial_oversized_excluded` reports the omitted row count. The
retrieval totals count all measured recall events for their own dates and
projects, without imposing the initial-command cap. The two cohorts shouldn't
be subtracted to infer net savings. They do not measure model context or billing.

`measurement_complete` is false: historical reads, external tee reads, stderr
and tracking failures are not measured here. `rtk gain --recalls` remains a
separate unique-content metric. `retention_days` reports the configured tracking
retention, and `rtk gain --reset` clears both histories. IDs are not sent in
telemetry.
