# Rust Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `cargo_cmd.rs` uses `restore_double_dash()` fix: Clap strips `--` but cargo needs it for test flags (e.g., `cargo test -- --nocapture`)
- `runner.rs` is a generic two-mode runner (`err` = stderr only, `test` = failures only) used as fallback for commands without a dedicated filter. It executes argv directly by default; `--shell <name> '<script>'` is the explicit escape hatch for shell syntax.
- `runner.rs` is also referenced by other modules outside this directory as a generic command executor

## Cargo diagnostics

`cargo build`, `cargo check`, and `cargo test` stream unmatched text directly, including complete panic backtraces, compiler explanations, and rerun hints. Successful test summaries and compilation progress are still compressed. Once a text diagnostic starts, subsequent lines are retained so diagnostic text cannot be mistaken for progress.

JSON build/check/clippy output renders every compiler diagnostic without error or warning count caps. Records without rendered text retain their original JSON so spans and child diagnostics remain available. This intentionally favors diagnostic completeness over fixed-size failure output; JSON envelopes and artifact records still provide compression. The existing batch JSON/clippy runner combines output streams; text build/check/test preserve diagnostic streams.
