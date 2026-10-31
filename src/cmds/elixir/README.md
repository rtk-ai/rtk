# Elixir Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `mix compile` and `mix format` are TOML filters (`src/filters/mix-*.toml`); `mix_cmd.rs` takes only `mix test` and `mix precommit`, which need state across lines. There is no `mix` clap subcommand: `run_fallback` in `lib.rs` asks `mix_cmd::try_run` first, so every other mix task still reaches its TOML filter or runs raw.
- mix runs with stdout and stderr on one pipe. Elixir writes compile warnings to stderr and everything else to stdout, and reading them apart would move a dependency's warnings out of its `==> dep` section.
- The project's own apps come from `mix.exs` (and `apps/*/mix.exs` in an umbrella). Any other app in a `==> app` section or a stacktrace frame is a dependency. Without a `mix.exs` nothing is.
- Cut: dependency compile sections (kept whole when they report a failure), compile progress, progress dots, top-level debug/info/notice log lines, repeats of a warning/error log line (shown once with a count), runs of dependency stack frames (one line naming the deps), and the error and stacktrace of a failure that repeats an earlier one apart from its own test file's lines: it keeps its `code:` line and says `(same error as 11, from test/foo_test.exs:130)`. Every failure's header and location stay.
- What is cut is listed in a `[rtk: hid ...]` line and recoverable with `rtk recall`. Without a recall store only the progress noise goes.
