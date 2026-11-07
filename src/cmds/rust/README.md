# Rust Ecosystem

> Part of [`src/cmds/`](../README.md) — see also [docs/contributing/TECHNICAL.md](../../../docs/contributing/TECHNICAL.md)

## Specifics

- `cargo_cmd.rs::parse` locates the Cargo subcommand behind rustup selectors and known global options, returning a typed `CargoInvocation` with a `CargoCommand`. Classification and execution share this parser.
- `cargo_cmd.rs` receives raw arguments, preserving global options, built-in aliases, and `--` test separators in their original order. Unknown options and special operations pass through to Cargo.
- `runner.rs` is a generic two-mode runner (`err` = stderr only, `test` = failures only) used as fallback for commands without a dedicated filter. It executes argv directly by default; `--shell <name> '<script>'` is the explicit escape hatch for shell syntax.
- `runner.rs` is also referenced by other modules outside this directory as a generic command executor

## Cargo argument rules

The shared parser only selects a filter; Cargo validates the original arguments.
Unknown options, unsupported commands, and script mode pass through unchanged.

| Behavior | Authoritative documentation |
| --- | --- |
| `+toolchain` is recognized only immediately after `cargo`, before its options. | [Rustup toolchain shorthand](https://rust-lang.github.io/rustup/overrides.html#toolchain-override-shorthand) |
| Cargo accepts options before its command; `--config`, `--color`, `-C`, and `-Z` consume values. | [Cargo options](https://doc.rust-lang.org/cargo/commands/cargo.html#options) |
| Repeated `--config` values merge from left to right, so forwarding preserves their order. | [Configuration overrides](https://doc.rust-lang.org/cargo/reference/config.html#command-line-overrides) |
| `-C` changes directory, must precede the command, and requires nightly with `-Z unstable-options`. Cargo enforces these constraints. | [Common options](https://doc.rust-lang.org/cargo/commands/cargo.html#common-options) |
| `-Zscript` enables direct Rust source-file execution; RTK leaves this mode native. | [Cargo script mode](https://doc.rust-lang.org/cargo/reference/unstable.html#script) |
| `b`, `c`, and `t` select the build, check, and test filters; execution retains the alias spelling. | [Built-in aliases](https://doc.rust-lang.org/cargo/reference/config.html#alias) |
| `cargo test -- …` forwards arguments to libtest, including its help request. Nextest supports an emulated test argument syntax. | [Cargo test](https://doc.rust-lang.org/cargo/commands/cargo-test.html#description), [Nextest arguments](https://nexte.st/docs/running/) |
| `cargo install --version` takes a crate version, so RTK must not interpret it as a request for Cargo's version page. | [Install options](https://doc.rust-lang.org/cargo/commands/cargo-install.html#install-options) |
