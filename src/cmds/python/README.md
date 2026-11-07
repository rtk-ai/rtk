# Python Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `pytest_cmd.rs` uses a state machine text parser (no JSON available from pytest)
  - Failure windows count every diagnostic `E` line, including blank separators
    and diff headings beginning with `=`. Up to four nonblank lines are shown
    (the first two and last two for longer reports); the elision marker counts
    all remaining diagnostic lines, including omitted blank separators.
  - Blank lines separate traceback diagnostics from `--showlocals` output, so a
    local named `E` does not consume the error budget even when its alignment
    matches an indented diff heading.
- `ruff_cmd.rs` uses JSON for check mode (`--output-format=json`) and text filtering for format mode
- `sqlfluff_cmd.rs` injects `--format json` for lint mode and groups violations by rule and file; other subcommands pass through capped
- Ruff and SQLFluff list identical descriptions once beside a visible top rule.
  Locations retain the rule code; differing messages and rules outside the
  summary cap keep their descriptions beside each location.
- `pip_cmd.rs` auto-detects `uv` as a pip alternative and routes accordingly
- `uv_cmd.rs` preserves `uv run` environment semantics: the program's own output passes through on success (bounded, with a tee hint), diagnostics are extracted on failure
- `python -m pytest` and `python3 -m mypy` are rewritten by the hook registry to `rtk pytest` / `rtk mypy`

## Cross-command

- `ruff_cmd` is called by `cmds/js/lint_cmd` and `cmds/system/format_cmd` for Python projects
- `mypy_cmd` is called by `cmds/js/lint_cmd` when detecting Python type checking
