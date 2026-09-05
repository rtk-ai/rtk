# Scala Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `bloop_cmd.rs` uses a `BloopCommands` sub-enum in main.rs (same pattern as cargo/go), with `run_test` / `run_compile` / `run_run` and an `Other` passthrough
- Bloop output has no `[info]`/`[error]`/`[success]` prefixes like the sbt launcher — diagnostics are `[E] [En] file:line:col` blocks and tests end in per-suite `N tests, N passed[, N failed]` tallies, so it needs its own filters
- `filter_bloop_test` summarizes passing suites to a tally and emits failures only, parsing munit / ScalaTest / specs2 / ZIO Test detail; failures capped at `CAP_LIST` with a `… +N more` tail
- `filter_bloop_compile` collapses each `[E]` diagnostic to one line and summarizes `Compiling`/`Compiled` source-and-time tallies; errors capped at `CAP_ERRORS`, warnings at `CAP_WARNINGS`
- uses `restore_double_dash()` (Clap strips `--`, bloop needs it for forwarded test args) — same fix as cargo
