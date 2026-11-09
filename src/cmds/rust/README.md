# Rust Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `cargo_cmd.rs::CargoInvocation::parse` uses `Tokenizer::parse_while` for both the prefix and selected command's options. Each command's options embed `CommonOptions`; its parser handles local options before delegating shared rules, excluding root-only actions. Runners interpret the resulting facts in `CargoCommand` (including `NoCommand(CommonOptions)` for top-level requests); the parser does not choose filters. Classification and execution share this parser.
- `cargo_cmd.rs` preserves the full argument list for execution, tracking, and verbose output, including global options, built-in aliases, and `--` test separators in their original order. Unknown options and special operations pass through to Cargo.
- `runner.rs` is a generic two-mode runner (`err` = stderr only, `test` = failures only) used as fallback for commands without a dedicated filter. It executes argv directly by default; `--shell <name> '<script>'` is the explicit escape hatch for shell syntax.
- `runner.rs` is also referenced by other modules outside this directory as a generic command executor
