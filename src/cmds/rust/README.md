# Rust Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `cargo_cmd.rs::CargoInvocation::parse` locates the Cargo subcommand behind rustup selectors and known global options, returning a `CargoInvocation` with `CommonOptions` alongside command-specific facts in `CargoCommand` (including `NoCommand` for top-level options). Runners interpret these facts; the parser does not choose filters. Classification and execution share this parser.
- `cargo_cmd.rs` preserves the full argument list for execution, tracking, and verbose output, including global options, built-in aliases, and `--` test separators in their original order. Unknown options and special operations pass through to Cargo.
- `runner.rs` is a generic two-mode runner (`err` = stderr only, `test` = failures only) used as fallback for commands without a dedicated filter. It executes argv directly by default; `--shell <name> '<script>'` is the explicit escape hatch for shell syntax.
- `runner.rs` is also referenced by other modules outside this directory as a generic command executor
