# System and Generic Utilities

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `read.rs` uses `core/filter` for language-aware code stripping (FilterLevel: none/minimal/aggressive)
- `grep_cmd.rs` reads `core/config` for `limits.grep_max_results` and `limits.grep_max_per_file`. Format-altering flags (`-c`, `-l`, `-L`, `-o`, `-Z`) bypass RTK filtering and run raw.
- `find_cmd.rs` filters simple RTK/native queries (`-name`, `-iname`, `-type`, `-maxdepth`) and passes unsupported read-only native predicates through to `find` so compound expressions keep native semantics. Side-effecting native actions (`-delete`, `-exec`, `-execdir`, `-ok`, `-okdir`) are left for users to run with `find` directly.
- `local_llm.rs` (`rtk smart`) uses `core/filter` for heuristic file summarization
- `format_cmd.rs` is a cross-ecosystem dispatcher: auto-detects and routes to `prettier_cmd` or `ruff_cmd` (black is handled inline, not as a separate module)

## Cross-command

- `format_cmd` routes to `cmds/js/prettier_cmd` and `cmds/python/ruff_cmd`
