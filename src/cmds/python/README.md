# Python Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `pytest_cmd.rs` uses a state machine text parser (no JSON available from pytest)
- `ruff_cmd.rs` uses JSON for check mode (`--output-format=json`) and text filtering for format mode
- `sqlfluff_cmd.rs` injects `--format json` for lint mode and groups violations by rule and file; other subcommands pass through capped
- Ruff and SQLFluff list identical descriptions once beside a visible top rule.
  Locations retain the rule code; differing messages and rules outside the
  summary cap keep their descriptions beside each location.
- `pip_cmd.rs` auto-detects `uv` as a pip alternative and routes accordingly
- `uv_cmd.rs` preserves `uv run` environment semantics: the program's own output passes through on success (bounded, with a tee hint), diagnostics are extracted on failure
- Literal `python -m pytest` invocations (including versioned, relative and
  absolute interpreter paths) are rewritten to
  `rtk run --filter pytest -- <original invocation>`. The direct-command runner
  keeps the interpreter, arguments, working directory, environment and stdin;
  it applies the existing pytest stdout filter without injecting pytest flags.
  Reports the filter cannot recognize pass through. Existing output bounds and
  stderr handling still apply, so this is not byte-for-byte output passthrough.
- Interpreter flags before `-m` and dynamic interpreter names are outside this
  rewrite's scope. Bare `pytest` retains its existing `rtk pytest` behavior.
- `python3 -m mypy` is rewritten by the hook registry to `rtk mypy`.

## Cross-command

- `ruff_cmd` is called by `cmds/js/lint_cmd` and `cmds/system/format_cmd` for Python projects
- `mypy_cmd` is called by `cmds/js/lint_cmd` when detecting Python type checking
