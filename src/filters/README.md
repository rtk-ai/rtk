# Built-in Filters

Each `.toml` file in this directory defines one filter and its inline tests.
Files are concatenated alphabetically by `build.rs` into a single TOML blob embedded in the binary.

Filtering is powered by the [tokf-filter](https://crates.io/crates/tokf-filter) engine.
RTK handles the TOML registry, command matching, and `head/tail/max_lines` omission markers.

## Adding a filter

1. Copy any existing `.toml` file and rename it (e.g. `my-tool.toml`)
2. Update the three required fields: `description`, `match_command`, and at least one action field
3. Add `[[tests.my-tool]]` entries to verify the filter behaves correctly
4. Run `cargo test` — the build step validates TOML syntax and runs inline tests

## File format

```toml
[filters.my-tool]
description = "Short description of what this filter does"
match_command = "^my-tool\\b"          # regex matched against the full command string
strip_ansi = true                       # optional: strip ANSI escape codes first
strip_lines_matching = [               # optional: drop lines matching any of these regexes
  "^\\s*$",
  "^noise pattern",
]
max_lines = 40                          # optional: keep only the first N lines after filtering
on_empty = "my-tool: ok"               # optional: message to emit when output is empty after filtering

[[tests.my-tool]]
name = "descriptive test name"
input = "raw command output here"
expected = "expected filtered output"
```

## Available filter fields

| Field | Type | Description |
|-------|------|-------------|
| `description` | string | Human-readable description |
| `match_command` | regex | Matches the command string (e.g. `"^docker\\s+inspect"`) |
| `strip_ansi` | bool | Strip ANSI escape codes before processing |
| `strip_lines_matching` | regex[] | Drop lines matching any regex (alias: `skip`) |
| `keep_lines_matching` | regex[] | Keep only lines matching at least one regex (alias: `keep`) |
| `replace` | array | Regex substitutions (`{ pattern, replacement }`) — all occurrences per line |
| `match_output` | array | Short-circuit rules (`{ pattern, message, unless }`) |
| `truncate_lines_at` | int | Truncate lines longer than N characters (uses `…` ellipsis) |
| `head_lines` | int | Keep only the first N lines (with `"... (N lines omitted)"` marker) |
| `tail_lines` | int | Keep only the last N lines (with omission marker) |
| `max_lines` | int | Absolute line cap after head/tail (with `"... (N lines truncated)"` marker) |
| `on_empty` | string | Fallback message when filtered output is empty |

## Pipeline order

1. `match_output` — short-circuit if pattern matches (ANSI-stripped when `strip_ansi = true`)
2. `replace` — regex substitutions, all occurrences per line, rules chained sequentially
3. `strip_ansi` — remove ANSI escape codes
4. `skip`/`keep` — filter lines by regex
5. `truncate_lines_at` — truncate long lines
6. `head_lines`/`tail_lines` — keep first/last N lines (with omission markers)
7. `max_lines` — absolute line cap
8. `on_empty` — message if result is empty

## Naming convention

Use the command name as the filename: `terraform-plan.toml`, `docker-inspect.toml`, `mix-compile.toml`.
For commands with subcommands, prefer `<cmd>-<subcommand>.toml` over grouping multiple filters in one file.
