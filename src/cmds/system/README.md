# System and Generic Utilities

## Specifics

- `read.rs` uses `core/filter` for language-aware code stripping (FilterLevel: none/minimal/aggressive)
- `grep_cmd.rs` reads `core/config` for `limits.grep_max_results` and `limits.grep_max_per_file`
- `local_llm.rs` (`rtk smart`) uses `core/filter` for heuristic file summarization
- `format_cmd.rs` is a cross-ecosystem dispatcher: auto-detects and routes to `prettier_cmd`, `ruff_cmd`, or `black`

## Cross-command

- `format_cmd` routes to `cmds/js/prettier_cmd` and `cmds/python/ruff_cmd`
