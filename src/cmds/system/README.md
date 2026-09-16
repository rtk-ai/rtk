# System and Generic Utilities

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `read.rs` uses `core/filter` for language-aware code stripping (FilterLevel: none/minimal/aggressive)
- `search.rs` backs both `rtk grep` and `rtk rg`: it runs the invoked engine (never substituting one for the other) and groups its output, reading `core/config` for `limits.grep_max_results` and `limits.grep_max_per_file`. Format-altering flags (`-c`, `-l`, `-L`, `-o`, `-Z`) bypass RTK filtering and run raw.
- `ctest_cmd.rs` takes the run total from the first result line (covering `--stop-on-failure`, disabled tests, and forwarded suites) and validates both result lines and the summary against it, deduplicates retries by test number+name, folds wrapped result lines until their terminator, falls back to the raw `FAILED:` trailer for unparsed failures, keeps the error trailer behind an empty run, labels and caps failure details with tee recovery, and attributes diagnostics safely under `-j`; explicit verbose/show-only/help/version flags and dashboard modes bypass filtering, except `-T Test`, which prints ordinary test output and stays filtered.
- `local_llm.rs` (`rtk smart`) uses `core/filter` for heuristic file summarization
- `format_cmd.rs` is a cross-ecosystem dispatcher: auto-detects and routes to `prettier_cmd` or `ruff_cmd` (black is handled inline, not as a separate module)

## Cross-command

- `format_cmd` routes to `cmds/js/prettier_cmd` and `cmds/python/ruff_cmd`
