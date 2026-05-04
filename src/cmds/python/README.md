# Python Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `pytest_cmd.rs` uses a state machine text parser (no JSON available from pytest)
- `ruff_cmd.rs` uses JSON for check mode (`--output-format=json`) and text filtering for format mode
- `pip_cmd.rs` auto-detects `uv` as a pip alternative and routes accordingly
- `dbt_cmd.rs` injects `--log-format json` for run/test/build/seed/snapshot/source-freshness and parses the NDJSON event stream (LogModelResult, LogTestResult, CommandCompleted, etc.) emitted on stderr. compile/parse/list/show/clean/debug/deps use a light banner-stripping text filter. At verbosity ≥ 3, falls through to pure passthrough.
- `python -m pytest` and `python3 -m mypy` are rewritten by the hook registry to `rtk pytest` / `rtk mypy`

## Cross-command

- `ruff_cmd` is called by `cmds/js/lint_cmd` and `cmds/system/format_cmd` for Python projects
- `mypy_cmd` is called by `cmds/js/lint_cmd` when detecting Python type checking
