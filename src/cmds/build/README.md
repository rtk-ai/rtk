# Build / Task-Runner Filters

Filters for monorepo task runners that orchestrate underlying tools.

## Modules

- `moon_cmd` — wraps [moon](https://moonrepo.dev) task runner. Strips moon's
  chrome (banners, hash suffixes, decoration) and routes each task's
  stdout/stderr through the matching rtk filter for its underlying command
  (vitest, tsc, eslint, prettier, cargo test, ...). See issue #1877.

## Why a separate ecosystem

Task runners are language-agnostic and orchestrate other tools across the
project graph. They need cross-command routing (detect the underlying tool
per task, apply that tool's filter) which the `system/` ecosystem does not
do, and they don't belong to any single language stack (`js/`, `rust/`, ...).

Related TOML-only filters that conceptually belong here but don't need a
Rust module today: `src/filters/turbo.toml`, `nx.toml`, `just.toml`,
`task.toml`, `make.toml`. They live in `src/filters/` because their
filtering needs no detection or routing logic.
