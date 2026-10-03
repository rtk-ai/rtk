# JavaScript / TypeScript / Node

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `utils::package_manager_exec()` auto-detects pnpm/yarn/npm -- JS modules should use this instead of hardcoding a package manager
- `lint_cmd.rs` is a cross-ecosystem router: detects Python projects and delegates to `mypy_cmd` or `ruff_cmd`
- `vitest_cmd.rs` uses the `parser/` module for structured output parsing
- `playwright_cmd.rs` uses the `parser/` module for test result extraction
- `ng_cmd.rs` compacts recognized Angular `ng build` table padding and the redundant initial progress pair. It keeps every bundle, size, build status, output location, and diagnostic; stderr is never capped. The native `ng` on PATH and arguments are preserved, without package-manager fallback. `--help`, `--verbose`, `--watch` (including explicit false values), and other subcommands pass through with inherited stdio. Only confidently parsed local workspaces using standard Angular application builders are captured. Workspaces with `watch: true` anywhere, JSONC/unreadable/unknown configuration, unknown builders, or no local workspace pass through conservatively. This safety check reads the nearest workspace only after build dispatch, never in the rewrite hot path. Only bare `ng build` is auto-rewritten; wrappers, path-based invocations, test, and serve are outside this filter's scope. Real Angular 22.2.1 fixtures live in `tests/fixtures/ng_build/`.

## Cross-command

- `lint_cmd` routes to `cmds/python/mypy_cmd` and `cmds/python/ruff_cmd` for Python projects
- `prettier_cmd` is also called by `cmds/system/format_cmd` as a format dispatcher target
