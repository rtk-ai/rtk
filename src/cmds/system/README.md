# System and Generic Utilities

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `read.rs` uses `core/filter` for language-aware code stripping (FilterLevel: none/minimal/aggressive)
- `search.rs` backs both `rtk grep` and `rtk rg`: it runs the invoked engine (never substituting one for the other) and groups its output, reading `core/config` for `limits.grep_max_results` and `limits.grep_max_per_file`. Format-altering flags (`-c`, `-l`, `-L`, `-o`, `-Z`) bypass RTK filtering and run raw.
- `local_llm.rs` (`rtk smart`) uses `core/filter` for heuristic file summarization
- `format_cmd.rs` is a cross-ecosystem dispatcher: auto-detects and routes to `prettier_cmd` or `ruff_cmd` (black is handled inline, not as a separate module)

## Windows / no-coreutils fallback

`ls`, `grep`, `wc`, and `tree` shell out to Unix binaries that aren't present on
a stock Windows install. Each of these now has a **native Rust execution path**
used when the underlying binary is unavailable (detected via
`core::utils::tool_exists`; `tree` also always goes native on Windows because
`tree.com` rejects the flags rtk uses):

- `ls.rs` — `run_native` synthesizes `ls -la`-style lines from `std::fs` and
  reuses `compact_ls`. POSIX permission bits don't exist on Windows, so the `-l`
  octal column is approximate there.
- `wc_cmd.rs` — `run_native` counts lines/words/bytes/chars in Rust and reuses
  `filter_wc_output`.
- `tree.rs` — `run_native` does a recursive `std::fs` walk, pruning `NOISE_DIRS`
  unless `-a`.
- `search.rs` — `native_grep` (final fallback after `rg` then `grep`) walks
  files with the `ignore` crate and matches with `regex`, emitting the same
  NUL-separated `path\0line:content` format the parser expects. It respects
  `.gitignore` and skips hidden files, which differs slightly from
  `rg --no-ignore-vcs`.

These native paths reuse the existing compression filters, so token-savings
behavior is identical to the Unix spawn path.

## PowerShell cmdlet rewrites

`powershell_cmd.rs` maps common, **non-piped** PowerShell cmdlets to their rtk
equivalents so the hook saves tokens on Windows:

| Cmdlet (and aliases) | rtk equivalent |
|---|---|
| `Get-Content` / `gc` / `type` | `rtk read` (`-TotalCount`→`--max-lines`, `-Tail`→`--tail-lines`) |
| `Get-ChildItem` / `gci` / `dir` | `rtk ls` (`-Recurse`→`rtk tree`, `-Force`→`-a`) |
| `Select-String` / `sls` | `rtk grep` (`-i` added unless `-CaseSensitive`) |

Piped/compound cmdlet invocations are intentionally left untouched to avoid
breaking PowerShell pipeline semantics.

The `rtk powershell` and `rtk pwsh` wrappers also recognize safe `-Command`
forms and dispatch those same cmdlets through RTK. Scripts, `-File`,
`-EncodedCommand`, pipelines, interpolation, and unsupported parameters run
unchanged through the requested PowerShell executable.

## Cross-command

- `format_cmd` routes to `cmds/js/prettier_cmd` and `cmds/python/ruff_cmd`

