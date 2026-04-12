# System and Generic Utilities

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `read.rs` uses `core/filter` for language-aware code stripping (FilterLevel: none/minimal/aggressive)
- `grep_cmd.rs` reads `core/config` for `limits.grep_max_results` and `limits.grep_max_per_file`
- `local_llm.rs` (`rtk smart`) uses `core/filter` for heuristic file summarization
- `format_cmd.rs` is a cross-ecosystem dispatcher: auto-detects and routes to `prettier_cmd` or `ruff_cmd` (black is handled inline, not as a separate module)
- `direnv_cmd.rs` handles `direnv exec` passthrough with shared TOML lookup and dual-stream (`stdout` + `stderr`) filtering for matched guard rules

## Cross-command

- `format_cmd` routes to `cmds/js/prettier_cmd` and `cmds/python/ruff_cmd`
- `direnv_cmd` uses `core/toml_filter` for standard project/user/built-in precedence while keeping `direnv` as a first-class Rust command
