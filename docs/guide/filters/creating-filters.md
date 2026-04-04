---
title: Creating Filters
description: Write TOML filters to compress output from any command RTK doesn't cover yet
sidebar:
  order: 2
---

# Creating Filters

TOML filters let you add RTK support for any command without writing Rust. They work well for tools with predictable, line-by-line text output where regex filtering achieves 60%+ savings.

## When to use a TOML filter

TOML works well for:
- Install/update logs (brew, composer, poetry) — strip `Using ...` / `Already installed` lines
- System monitoring (df, ps, systemctl) — keep essential rows, drop headers
- Simple linters (shellcheck, yamllint, hadolint) — strip context, keep findings
- Infra tools (terraform plan, helm, rsync) — strip progress, keep summary

For commands with structured JSON output, state machine parsing, or complex routing, a Rust module is needed instead.

## Create a project-local filter

Create `.rtk/filters.toml` in your project root:

```toml
[filters.my-tool]
description = "Strip progress noise from my-tool output"
match_command = "^my-tool\\b"
strip_ansi = true
strip_lines_matching = [
  "^Loading",
  "^Scanning",
  "^\\s*$",
]
max_lines = 50
on_empty = "my-tool: nothing to report"

[[tests.my-tool]]
name = "strips progress lines"
input = "Loading plugins...\nScan complete: 3 issues\nWarning: foo at line 42"
expected = "Scan complete: 3 issues\nWarning: foo at line 42"
```

Verify:

```bash
rtk verify
```

## Filter fields reference

| Field | Type | Description |
|-------|------|-------------|
| `description` | string | Human-readable description |
| `match_command` | regex | Matched against full command string |
| `strip_ansi` | bool | Strip ANSI escape codes first |
| `strip_lines_matching` | regex[] | Drop lines matching any of these |
| `keep_lines_matching` | regex[] | Keep only lines matching at least one |
| `replace` | array | Regex substitutions (`{ pattern, replacement }`) |
| `match_output` | array | Short-circuit rules (`{ pattern, message }`) |
| `truncate_lines_at` | int | Truncate lines longer than N chars |
| `max_lines` | int | Keep only the first N lines |
| `tail_lines` | int | Keep only the last N lines |
| `on_empty` | string | Message when output is empty after filtering |

## Naming convention

Use the command name as the filter key: `terraform-plan`, `docker-inspect`, `mix-compile`. For commands with subcommands, prefer `cmd-subcommand` over grouping multiple filters together.

## Filter lookup order

Project filters override user-global filters, which override built-ins:

```
1. .rtk/filters.toml          (project-local — highest priority)
2. ~/.config/rtk/filters.toml (user-global)
3. BUILTIN_TOML               (embedded in binary — lowest priority)
```

A project filter with the same key as a built-in shadows the built-in with a warning:

```
[rtk] warning: filter 'make' is shadowing a built-in filter
```

## Writing inline tests

Always add at least one `[[tests.my-tool]]` entry. Tests run via `rtk verify` and also during `cargo test` (for built-in filters).

```toml
[[tests.my-tool]]
name = "normal output"
input = "Progress: 10%\nDone. 3 findings."
expected = "Done. 3 findings."

[[tests.my-tool]]
name = "empty output"
input = "Progress: 10%\nProgress: 100%"
expected = "my-tool: nothing to report"
```

## Contributing a filter upstream

To add a filter to RTK's built-in set, see the [Contributing guide](../../reference/contributing/guide.md) for the full checklist (register in `discover/rules.rs`, update filter count in tests, write fixture).
