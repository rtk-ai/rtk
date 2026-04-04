---
title: TOML DSL Specification
description: Complete specification for RTK's TOML filter format
sidebar:
  order: 1
---

# TOML DSL Specification

TOML filters are the declarative way to add RTK support for commands with line-by-line text output. They are embedded in the binary at build time and can also be used as project-local or user-global overrides.

## File format

```toml
[filters.my-tool]
description = "Short description of what this filter does"
match_command = "^my-tool\\b"        # regex matched against full command string
strip_ansi = true                     # optional: strip ANSI codes first

strip_lines_matching = [              # optional: drop lines matching any regex
  "^\\s*$",
  "^noise pattern",
]

keep_lines_matching = [               # optional: keep only matching lines
  "^error",
  "^warning",
]

max_lines = 40                        # optional: keep only first N lines after filtering
tail_lines = 20                       # optional: keep only last N lines
truncate_lines_at = 120               # optional: truncate lines longer than N chars
on_empty = "my-tool: nothing to do"  # optional: message when output is empty

[[tests.my-tool]]
name = "descriptive test name"
input = "raw command output here"
expected = "expected filtered output"
```

## Field reference

| Field | Type | Required | Description |
|-------|------|----------|-------------|
| `description` | string | yes | Human-readable description |
| `match_command` | regex | yes | Matched against the full command string |
| `strip_ansi` | bool | no | Strip ANSI escape codes before processing |
| `strip_lines_matching` | regex[] | no | Drop lines matching any of these patterns |
| `keep_lines_matching` | regex[] | no | Keep only lines matching at least one pattern |
| `replace` | array | no | Regex substitutions: `{ pattern, replacement }` |
| `match_output` | array | no | Short-circuit rules: `{ pattern, message }` |
| `truncate_lines_at` | int | no | Truncate lines longer than N characters |
| `max_lines` | int | no | Keep only the first N lines (after other stages) |
| `tail_lines` | int | no | Keep only the last N lines |
| `on_empty` | string | no | Message emitted when pipeline produces empty output |

## Pipeline execution order

When a filter matches, output passes through these stages in order:

1. `strip_ansi` — remove terminal color codes
2. `replace` — regex substitutions
3. `match_output` — short-circuit: if pattern matches, emit message and stop
4. `strip_lines_matching` / `keep_lines_matching`
5. `truncate_lines_at`
6. `tail_lines`
7. `max_lines`
8. `on_empty` — if output is now empty, emit this message

Stages not defined in the filter are skipped.

## Inline tests

Every filter must have at least one `[[tests.<filter-name>]]` entry. Tests run during `cargo test` and `rtk verify`.

```toml
[[tests.my-tool]]
name = "strips progress lines"
input = "Loading...\nDone: 3 issues"
expected = "Done: 3 issues"

[[tests.my-tool]]
name = "empty output fallback"
input = "Loading...\nProgress: 100%"
expected = "my-tool: nothing to do"
```

## Filter lookup priority

```
1. .rtk/filters.toml              (project-local — highest priority)
2. ~/.config/rtk/filters.toml     (user-global)
3. BUILTIN_TOML                   (embedded in binary — lowest priority)
```

First match wins. A project filter with the same name as a built-in triggers a warning:

```
[rtk] warning: filter 'make' is shadowing a built-in filter
```

## Built-in filter compilation

Built-in filters live in `src/filters/*.toml`. At build time, `build.rs`:

1. Lists all `*.toml` files alphabetically
2. Concatenates them into a single TOML blob
3. Validates syntax and checks for duplicate names (build fails on error)
4. Embeds the result in the binary via `include_str!(concat!(env!(OUT_DIR), "/builtin_filters.toml"))`

New built-in filters require a new RTK release. Project and user filters take effect immediately without rebuilding.

## Naming convention

Use the command name as the filter key: `terraform-plan`, `docker-inspect`, `mix-compile`. For subcommands, prefer `cmd-subcommand` over grouping: `docker-ps.toml`, not `docker.toml` with multiple filters.

## Regex syntax

Patterns use Rust's `regex` crate (RE2-compatible). Backslashes must be doubled in TOML strings:

```toml
match_command = "^my-tool\\b"          # matches "my-tool" as a word
strip_lines_matching = ["^\\s*$"]      # matches blank lines
```
