# Build / Task-Runner Filters

Filters for monorepo task runners that orchestrate underlying tools.

## Modules

- `moon_cmd` — wraps [moon](https://moonrepo.dev) task runner. Strips moon's
  chrome (upgrade banner, `▮▮▮▮` decoration, `(hash)` suffix, `❯❯❯❯` footer)
  and routes each sequential-mode task's body through the matching rtk
  filter for its underlying command via `moon query tasks` → `TaskMap`
  detection. Currently routes `prettier`, `vitest`/`jest`, `tsc`, and
  `eslint`/`biome`. See issue #1877.

## How filtering applies

Only `moon run/ci/check/exec` invocations go through the filter — `moon
query`/`graph`/`--version` etc. pass through raw so we don't pay the
`moon query tasks` overhead (~150ms on large workspaces) for non-task
commands.

## Known limitations (follow-up work)

- **Parallel-mode bodies passthrough.** When moon runs tasks in parallel
  (the common case — dep graph), task bodies are prefixed with
  ` <project>:<task> | ` and currently bypass the per-tool filter to
  avoid breaking tools that need batch context (prettier's filter
  collapses multi-line output into a canonical summary; called per-line
  it would emit that summary repeatedly). A future PR could buffer
  parallel bodies per-task and apply the filter once on task completion.
- **No `bun test` filter.** Bun's test runner emits plain text; the
  existing rtk `vitest_cmd` parser requires JSON. Adding `filter_bun_test_output`
  in `cmds/js/` and routing `bun` in `filter_for_tool` would unlock
  compression for `audit:test`-style tasks.

## Why a separate ecosystem

Task runners are language-agnostic and orchestrate other tools across the
project graph. They need cross-command routing (detect the underlying tool
per task, apply that tool's filter) which the `system/` ecosystem does not
do, and they don't belong to any single language stack (`js/`, `rust/`, ...).

Related TOML-only filters that conceptually belong here but don't need a
Rust module today: `src/filters/turbo.toml`, `nx.toml`, `just.toml`,
`task.toml`, `make.toml`. They live in `src/filters/` because their
filtering needs no detection or routing logic.
