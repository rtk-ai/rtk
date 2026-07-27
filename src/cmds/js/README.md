# JavaScript / TypeScript / Node

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `utils::package_manager_exec()` auto-detects pnpm/yarn/npm -- JS modules should use this instead of hardcoding a package manager
- `lint_cmd.rs` is a cross-ecosystem router: detects Python projects and delegates to `mypy_cmd` or `ruff_cmd`
- `vitest_cmd.rs` uses the `parser/` module for structured output parsing
- `playwright_cmd.rs` uses the `parser/` module for test result extraction

## pnpm script routing

`pnpm_cmd.rs` intercepts script invocations:

- `rtk pnpm run <script>`
- `pnpm <script>` with `:` or `-` in the name (e.g. `pnpm test:unit`, `pnpm lint-fix`) -- rewritten to `rtk pnpm run <script>` by `discover/rules.rs` (pnpm builtins like `self-update` stay passthrough)
- `rtk pnpm --filter/-F <pkg> run <script>` -- filters forwarded to pnpm before `run` (monorepos)

`route_script()` matches static names first (`vitest`, `tsc`, `prettier`), then detects the tool from the script's command string in `package.json`:

| Tool detected | Filter |
|---------------|--------|
| vitest | `VitestStreamFilter` (streaming) |
| playwright | `playwright_cmd` parser |
| eslint / biome | `lint_cmd::filter_generic_lint` |
| tsc / typescript | `tsc_cmd::filter_tsc_output` |
| prettier | `prettier_cmd::filter_prettier_output` |
| jest | `cmds/rust/runner::extract_test_summary` |
| no match | pnpm boilerplate strip only |

The vitest route streams via `run_streamed`: pnpm boilerplate (lifecycle header, `$` script echo, `Done in`, `ELIFECYCLE`), the `RUN v...` banner and passing-file lines are suppressed; failures stay inline (max 10, 30 lines each, then `... (truncated)` / `... and N more failures`) and the run ends with a compact summary:

```text
PASS (120) FAIL (2) | 45 suites (2 failed) | 30s
```

Other routes are buffered (`exec_capture`). On filter error or no route, output falls back to `filter_pnpm_run_output()` (boilerplate strip); an empty successful run prints `ok`.

## Cross-command

- `lint_cmd` routes to `cmds/python/mypy_cmd` and `cmds/python/ruff_cmd` for Python projects
- `prettier_cmd` is also called by `cmds/system/format_cmd` as a format dispatcher target
- `pnpm_cmd` routes generic test scripts (jest) to `cmds/rust/runner::extract_test_summary`
