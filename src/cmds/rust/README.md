# Rust Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `cargo_cmd.rs` uses `restore_double_dash()` fix: Clap strips `--` but cargo needs it for test flags (e.g., `cargo test -- --nocapture`)
- `runner.rs` is a generic two-mode runner (`err` = stderr only, `test` = failures only) used as fallback for commands without a dedicated filter. It executes argv directly by default; `--shell <name> '<script>'` is the explicit escape hatch for shell syntax.
- `runner.rs` is also referenced by other modules outside this directory as a generic command executor
