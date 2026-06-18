# System and Generic Utilities

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `read.rs` uses `core/filter` for language-aware code stripping (FilterLevel: none/minimal/aggressive)
- `yaml_cmd.rs` (`rtk yaml`) linearizes YAML to dotted-path lines (`path: value`, or paths only with `--keys-only`) so nested structure is greppable on one line. Mirrors `json_cmd.rs`.
- `grep_cmd.rs` reads `core/config` for `limits.grep_max_results` and `limits.grep_max_per_file`. Format-altering flags (`-c`, `-l`, `-L`, `-o`, `-Z`) bypass RTK filtering and run raw. When every path argument is a `.yaml`/`.yml` file (and no match-altering flags are present), it greps the linearized form via `yaml_cmd::filter_yaml_linear` so each hit shows its full dotted path; anything else falls back to raw grep.
- `local_llm.rs` (`rtk smart`) uses `core/filter` for heuristic file summarization
- `format_cmd.rs` is a cross-ecosystem dispatcher: auto-detects and routes to `prettier_cmd` or `ruff_cmd` (black is handled inline, not as a separate module)

## Cross-command

- `format_cmd` routes to `cmds/js/prettier_cmd` and `cmds/python/ruff_cmd`
