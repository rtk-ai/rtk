# Jujutsu (jj)

> Part of [`src/cmds/`](../README.md)

Rust filter: argv injection (`builtin_log_oneline`, limits, `--color never`, diff/show summaries) and post-filters for `log`, `status`, `diff`, `show`, `op`. Passthrough via `run_other` / `--no-compact`.

Narrow TOML fallback: [`src/filters/jj.toml`](../../filters/jj.toml) (non-Clap `jj` paths only). Closes gap after #271 (TOML-only in v0.30.0).

Fixtures: `tests/fixtures/jj/` (`harness@rtk.test`).