# System and Generic Utilities

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `read.rs` uses `core/filter` for language-aware code stripping (FilterLevel: none/minimal/aggressive)
- `grep_cmd.rs` reads `core/config` for `limits.grep_max_results` and `limits.grep_max_per_file`. Format-altering flags (`-c`, `-l`, `-L`, `-o`, `-Z`) bypass the group-by-file filter and run raw, but path-shaped passthrough output (notably the `-l`/`-L`/`--files` file lists) still gets a shared directory prefix folded into a one-line header via `fold_path_prefix` (lossless, `never_worse`-guarded).
- `local_llm.rs` (`rtk smart`) uses `core/filter` for heuristic file summarization
- `format_cmd.rs` is a cross-ecosystem dispatcher: auto-detects and routes to `prettier_cmd` or `ruff_cmd` (black is handled inline, not as a separate module)

## Cross-command

- `format_cmd` routes to `cmds/js/prettier_cmd` and `cmds/python/ruff_cmd`
