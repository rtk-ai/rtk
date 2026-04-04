---
title: Filter Pipeline
description: The 12 filtering strategies RTK uses to compress command output
sidebar:
  order: 2
---

# Filter Pipeline

## Filtering strategies

RTK uses 12 distinct filtering strategies depending on the command type:

| # | Strategy | Reduction | Used by |
|---|----------|-----------|---------|
| 1 | **Stats Extraction** | 90-99% | git status, git log, git diff, pnpm list |
| 2 | **Error Only** | 60-80% | runner (err mode), test failures |
| 3 | **Grouping by Pattern** | 80-90% | lint, tsc, grep (group by file/rule/code) |
| 4 | **Deduplication** | 70-85% | log_cmd (count occurrences) |
| 5 | **Structure Only** | 80-95% | json_cmd (keys + types, strip values) |
| 6 | **Code Filtering** | 0-90% | read, smart (language-aware, 3 levels) |
| 7 | **Failure Focus** | 94-99% | vitest, playwright, runner (test mode) |
| 8 | **Tree Compression** | 50-70% | ls (directory tree with counts) |
| 9 | **Progress Filtering** | 85-95% | wget, pnpm install (strip ANSI bars) |
| 10 | **JSON/Text Dual Mode** | 80%+ | ruff (JSON when available, text fallback) |
| 11 | **State Machine Parsing** | 90%+ | pytest (text state machine: name → result) |
| 12 | **NDJSON Streaming** | 90%+ | go test (line-by-line JSON event parsing) |

## TOML filter pipeline (8 stages)

TOML filters run output through this pipeline in order:

```
1. strip_ansi           — remove terminal color codes
2. replace              — regex substitutions
3. match_output         — short-circuit: if pattern matches, emit message and stop
4. strip/keep_lines     — keep or remove lines by regex
5. truncate_lines_at    — truncate lines longer than N chars
6. tail_lines           — keep only the last N lines
7. max_lines            — keep only the first N lines
8. on_empty             — if output is empty, emit this message
```

Stages not configured in the filter definition are skipped. First match wins in TOML filter lookup (project → user-global → built-in).

## Code filtering levels (src/core/filter.rs)

The `read` and `smart` commands use language-aware filtering with three levels:

- **`none`**: Keep everything (0%)
- **`minimal`**: Strip comments and excessive blank lines (20-40%)
- **`aggressive`**: Keep signatures only, remove function bodies (60-90%)

Supported languages: Rust, Python, JavaScript, TypeScript, Go, C, C++, Java, Ruby, Shell.

## Savings by ecosystem

```
GIT (cmds/git/)         85-99%    status, diff, log, gh, gt
JS/TS (cmds/js/)        70-99%    lint, tsc, next, vitest, playwright, pnpm, prisma
PYTHON (cmds/python/)   70-90%    ruff, pytest, mypy, pip
GO (cmds/go/)           75-90%    go test, golangci-lint
RUBY (cmds/ruby/)       60-90%    rake, rspec, rubocop
DOTNET (cmds/dotnet/)   70-85%    dotnet build/test, binlog
CLOUD (cmds/cloud/)     60-80%    aws, docker, kubectl, curl, wget, psql
SYSTEM (cmds/system/)   50-90%    ls, read, grep, find, json, log, env
RUST (cmds/rust/)       60-99%    cargo test/build/clippy, err
```

## Shared infrastructure (src/core/)

| Module | Responsibility |
|--------|----------------|
| `utils.rs` | `strip_ansi`, `truncate`, `execute_command` |
| `filter.rs` | Language-aware code filtering engine |
| `toml_filter.rs` | TOML DSL filter engine (runtime) |
| `tracking.rs` | SQLite token metrics recording |
| `tee.rs` | Raw output recovery on failure |
| `config.rs` | `~/.config/rtk/config.toml` loading |
| `display_helpers.rs` | Terminal formatting helpers |
| `telemetry.rs` | Anonymous daily ping |
