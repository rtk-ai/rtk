# Core Infrastructure

> See also [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md) for the full architecture overview

## Scope

Domain-agnostic building blocks with **no knowledge of any specific command, hook, or agent**. If a module references "git", "cargo", "claude", or any external tool by name, it does not belong here. Core is a leaf in the dependency graph — it is consumed by all other components but imports from none of them.

Owns: configuration loading, token tracking persistence, TOML filter engine, tee output recovery, display formatting, telemetry, and shared utilities.

Does **not** own: command-specific filtering logic (that's `cmds/`), hook lifecycle management (that's `src/hooks/`), or analytics dashboards (that's `analytics/`).

## Purpose
Core infrastructure shared by all RTK command modules. Every filter, tracker, and command handler depends on these modules. No inward dependencies — leaf in the dependency graph (no circular imports possible).

## TOML Filter Pipeline

The TOML DSL applies 8 stages in order:

1. **strip_ansi**: Remove ANSI escape codes if enabled
2. **replace**: Line-by-line regex substitutions (chainable, supports backreferences)
3. **match_output**: Short-circuit rules (if output matches pattern, return message; `unless` field prevents swallowing errors)
4. **strip/keep_lines**: Filter lines by regex (mutually exclusive)
5. **truncate_lines_at**: Truncate each line to N chars (unicode-safe)
6. **head/tail_lines**: Keep first N or last N lines (with omit message)
7. **max_lines**: Absolute line cap applied after head/tail
8. **on_empty**: Return message if result is empty after all stages

Three-tier filter lookup (first match wins):
1. `.rtk/filters.toml` (project-local, requires `rtk trust`)
2. `~/.config/rtk/filters.toml` (user-global)
3. Built-in filters concatenated by `build.rs` at compile time

## Tracking Database Schema

```sql
CREATE TABLE commands (
  id INTEGER PRIMARY KEY,
  timestamp TEXT,              -- UTC ISO8601
  original_cmd TEXT,           -- "ls -la"
  rtk_cmd TEXT,                -- "rtk ls"
  project_path TEXT,           -- cwd (for project-scoped stats)
  input_tokens INTEGER,        -- estimated from raw output
  output_tokens INTEGER,       -- estimated from filtered output
  saved_tokens INTEGER,        -- input - output
  savings_pct REAL,            -- (saved / input) * 100
  exec_time_ms INTEGER         -- elapsed milliseconds
);

CREATE TABLE parse_failures (
  id INTEGER PRIMARY KEY,
  timestamp TEXT,
  raw_command TEXT,
  error_message TEXT,
  fallback_succeeded INTEGER   -- 1=yes, 0=no
);
```

Project-scoped queries use GLOB patterns (not LIKE) to avoid `_`/`%` wildcard issues in paths.

## Config Sections

```toml
[tracking]
enabled = true
history_days = 90
database_path = "/custom/path/to/tracking.db"  # Optional

[display]
colors = true
emoji = true
max_width = 120

[tee]
enabled = true
mode = "failures"  # failures | always | never
max_files = 20
max_file_size = 1048576
directory = "/custom/tee/dir"

[telemetry]
enabled = true

[hooks]
exclude_commands = ["curl", "playwright"]  # Never auto-rewrite these

[limits]
grep_max_results = 200
grep_max_per_file = 25
status_max_files = 15
status_max_untracked = 10
passthrough_max_chars = 2000
```

## Shared Utilities (utils.rs)

Key functions available to all command modules:

| Function | Purpose |
|----------|---------|
| `truncate(s, max)` | Truncate string with `...` suffix |
| `strip_ansi(text)` | Remove ANSI escape/color codes |
| `resolved_command(name)` | Find command in PATH, returns `Command` |
| `tool_exists(name)` | Check if a CLI tool is available |
| `detect_package_manager()` | Detect pnpm/yarn/npm from lockfiles |
| `package_manager_exec(tool)` | Build `Command` using detected package manager |
| `ruby_exec(tool)` | Auto-detect `bundle exec` when `Gemfile` exists |
| `count_tokens(text)` | Estimate tokens: `ceil(chars / 4.0)` |

## Consumer Contracts

Core provides infrastructure that `cmds/` and other components consume. These contracts define expected usage.

### Tracking (`TimedExecution`)

Consumers must call `timer.track()` on **all** code paths — success, failure, and fallback. Calling `std::process::exit()` before `track()` loses metrics. The raw string passed to `track()` should include both stdout and stderr to produce accurate savings percentages.

### Tee (`tee_and_hint`)

Consumers that parse structured output (JSON, NDJSON, state machines) should call `tee::tee_and_hint()` to save raw output for LLM recovery on failure. Tee must be called before `std::process::exit()`.

## Adding New Functionality
Place new infrastructure code here if it meets **all** of these criteria: (1) it has no dependencies on command modules or hooks, (2) it is used by two or more other modules, and (3) it provides a general-purpose utility rather than command-specific logic. Follow the existing pattern of lazy-initialized resources (`lazy_static!` for regex, on-demand config loading) to preserve the <10ms startup target. Add `#[cfg(test)] mod tests` with unit tests in the same file.
