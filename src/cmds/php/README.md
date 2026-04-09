# PHP

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `phpstan_cmd.rs` — PHPStan static analysis with JSON injection (`--error-format=json`); groups errors by file, sorted by error count descending, shows up to 10 files × 5 messages each (75%+ reduction). Falls back to text parsing when the user specifies a custom format flag.
  - Detects `vendor/bin/phpstan` automatically (Laravel/Composer projects)
  - JSON path: injects `--error-format=json`, parses `totals` + `files` structure
  - Text path: scans for `[OK]` / error count summary lines
  - Fallback: `fallback_tail()` on JSON parse failure — never blocks execution
