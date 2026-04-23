# JavaScript / TypeScript / Node

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `utils::package_manager_exec()` auto-detects pnpm/yarn/npm -- JS modules should use this instead of hardcoding a package manager
- `lint_cmd.rs` is a cross-ecosystem router: detects Python projects and delegates to `mypy_cmd` or `ruff_cmd`
- `vitest_cmd.rs` uses the `parser/` module for structured output parsing
- `playwright_cmd.rs` uses the `parser/` module for test result extraction

## Cross-command

- `lint_cmd` routes to `cmds/python/mypy_cmd` and `cmds/python/ruff_cmd` for Python projects
- `prettier_cmd` is also called by `cmds/system/format_cmd` as a format dispatcher target
