# Core Infrastructure

## Scope

Domain-agnostic building blocks with **no knowledge of any specific command, hook, or agent**. If a module references "git", "cargo", "claude", or any external tool by name, it does not belong here. Core is a leaf in the dependency graph — it is consumed by all other components but imports from none of them.

Owns: configuration loading, token tracking persistence, TOML filter engine, tee output recovery, display formatting, telemetry, and shared utilities.

Does **not** own: command-specific filtering logic (that's `cmds/`), hook lifecycle management (that's `src/hooks/`), or analytics dashboards (that's `analytics/`).

## Purpose
Core infrastructure shared by all RTK command modules. These are the foundational building blocks that every filter, tracker, and command handler depends on. This module group has no inward dependencies -- it is a leaf in the dependency graph, ensuring clean layering across the codebase.

## Files
| File | Responsibility |
|------|---------------|
| tracking.rs | SQLite-based persistent token metrics (1429 lines); records input/output token counts per command with project path; 90-day retention cleanup; token estimation via `ceil(chars / 4.0)` heuristic; `TimedExecution` API for timer-based tracking; imported by 46 files (highest dependency count); DB location: `RTK_DB_PATH` env > `config.toml` > `~/.local/share/rtk/tracking.db` |
| utils.rs | Shared utilities used across 41 files: `strip_ansi`, `truncate`, `execute_command`, `resolved_command`, `package_manager_exec`; package manager auto-detection (pnpm/yarn/npm/npx); consistent error handling and output formatting |
| config.rs | Configuration system reading `~/.config/rtk/config.toml`; sections: `[tracking]` (DB path, retention), `[display]` (colors, emoji, max_width), `[tee]` (recovery), `[telemetry]`, `[hooks]` (exclude_commands), `[limits]` (grep/status thresholds); on-demand loading (no startup I/O) |
| filter.rs | Language-aware code filtering engine with `FilterLevel` enum (`none`, `minimal`, `aggressive`); strips comments, whitespace, and function bodies based on level; supports Rust, Python, JS/TS, Java, Go, C/C++ via file extension detection with fallback heuristics |
| toml_filter.rs | TOML DSL filter engine (1686 lines); `TomlFilterRegistry` singleton via `lazy_static!`; three-tier lookup: project-local (trust-gated), user-global, built-in; 8-stage filter pipeline (see below); built-in filters compiled from `src/filters/*.toml` via `build.rs` |
| tee.rs | Raw output recovery on command failure (400 lines); saves unfiltered output to `~/.local/share/rtk/tee/{epoch}_{slug}.log`; prints one-line hint for LLM re-read; 20-file rotation, 1MB cap, min 500 chars; configurable via `[tee]` config section or `RTK_TEE`/`RTK_TEE_DIR` env vars; tee errors never affect command output or exit code |
| display_helpers.rs | Token display formatting helpers for consistent human-readable output of savings percentages, token counts, and comparison tables |
| telemetry.rs | Fire-and-forget usage telemetry (248 lines); non-blocking background thread; once per 23 hours via marker file; sends device hash (SHA-256 of hostname:username), version, OS, top commands, savings stats; 2-second timeout; disabled via `RTK_TELEMETRY_DISABLED=1` or `[telemetry] enabled = false` |

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
3. Built-in filters concatenated by `build.rs` at compile time (57+ filters)

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

## Consumer Contracts

Core provides infrastructure that `cmds/` and other components consume. These contracts define expected usage.

### Tracking (`TimedExecution`)

Consumers must call `timer.track()` on **all** code paths — success, failure, and fallback. Calling `std::process::exit()` before `track()` loses metrics. The raw string passed to `track()` should include both stdout and stderr to produce accurate savings percentages.

### Tee (`tee_and_hint`)

Consumers that parse structured output (JSON, NDJSON, state machines) should call `tee::tee_and_hint()` to save raw output for LLM recovery on failure. Tee must be called before `std::process::exit()`.

### Gaps (to be fixed)

- `ls.rs`, `tree.rs` — exit before `track()` on error path (lost metrics)
- `container.rs` — inconsistent tracking across subcommands
- 26/38 command modules missing tee integration — see `src/cmds/README.md` for the full list

## Adding New Functionality
Place new infrastructure code here if it meets **all** of these criteria: (1) it has no dependencies on command modules or hooks, (2) it is used by two or more other modules, and (3) it provides a general-purpose utility rather than command-specific logic. Follow the existing pattern of lazy-initialized resources (`lazy_static!` for regex, on-demand config loading) to preserve the <10ms startup target. Add `#[cfg(test)] mod tests` with unit tests in the same file.
