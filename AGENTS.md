# AGENTS.md

Router for coding agents (and new human contributors) working on rtk itself. Each row
is one rule, terse on purpose, this file is not a copy of the rules, it just points
you at the authoritative doc before you start coding, so the real thing can't drift
out of sync with a summary.

## Project Reference

**rtk (Rust Token Killer)** is a high-performance CLI proxy that minimizes LLM token consumption by filtering and compressing command outputs. It reduces bash output by 60-90% on common development operations through smart filtering, grouping, truncation, and deduplication. All percentages in this repo measure bash output, not your bill. RTK ships no tokenizer (`src/core/tracking.rs` estimates tokens as `bytes / 4`), so the ratios are reliable but the absolute token counts are approximate.

This is a fork with critical fixes for git argument parsing and modern JavaScript stack support (pnpm, vitest, Next.js, TypeScript, Playwright, Prisma).

### Name Collision Warning

**Two different "rtk" projects exist:**
- This project: Rust Token Killer (rtk-ai/rtk)
- reachingforthejack/rtk: Rust Type Kit (a different project, generates Rust types)

**Verify correct installation:**
```bash
rtk --version  # Should show "rtk 0.28.2" (or newer)
rtk gain       # Should show token savings stats (NOT "command not found")
```
If `rtk gain` fails, you have the wrong package installed.

### Development Commands

> If rtk is installed, prefer `rtk <cmd>` over raw commands for token-optimized output. All commands work with passthrough support even for subcommands rtk doesn't specifically handle.

**Build & Run**
```bash
cargo build                   # raw
rtk cargo build               # preferred (token-optimized)
cargo build --release         # release build (optimized)
cargo run -- <command>        # run directly
cargo install --path .        # install locally
```

**Testing**
```bash
cargo test                    # all tests
rtk cargo test                # preferred (token-optimized)
cargo test <test_name>        # specific test
cargo test <module_name>::    # module tests
cargo test -- --nocapture     # with stdout
bash scripts/test-all.sh      # smoke tests (installed binary required)
```

**Linting & Quality**
```bash
cargo check                   # check without building
cargo fmt                     # format code
cargo clippy --all-targets    # all clippy lints
rtk cargo clippy --all-targets # preferred
```

**Pre-commit gate** (must pass before every commit, see [CONTRIBUTING.md](CONTRIBUTING.md)):
```bash
cargo fmt --all --check && cargo clippy --all-targets && cargo test
```

**Performance verification** (for filter changes):
```bash
hyperfine 'rtk git log -10' --warmup 3          # before
cargo build --release
hyperfine 'target/release/rtk git log -10' --warmup 3  # after (should be <10ms)
```

**Package Building**
```bash
cargo deb                     # DEB package (needs cargo-deb)
cargo generate-rpm            # RPM package (needs cargo-generate-rpm, after release build)
```

### Architecture

rtk uses a **command proxy architecture**: `main.rs` routes CLI commands via a Clap `Commands` enum to specialized filter modules in `src/cmds/*/`, each of which executes the underlying command and compresses its output. Token savings are tracked in SQLite via `src/core/tracking.rs`.

For the full architecture, component details, and module development patterns, see:
- [ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md): system design, module organization, filtering strategies, error handling
- [docs/contributing/TECHNICAL.md](docs/contributing/TECHNICAL.md): end-to-end flow, folder map, hook system, filter pipeline

Module responsibilities are documented in each folder's `README.md` and each file's `//!` doc header. Browse `src/cmds/*/` to discover available filters.

Supported ecosystems: git/gh/gt, cargo, go/golangci-lint, npm/pnpm/npx, ruff/pytest/pip/mypy, rspec/rubocop/rake, dotnet, playwright/vitest/jest, docker/kubectl/aws, gradlew/mvn, php/artisan/phpunit/phpstan/pest.

### Proxy Mode

Executes commands without filtering but still tracks usage for metrics.

Usage: `rtk proxy <command> [args...]`

- **Bypass RTK filtering**: workaround bugs or get full unfiltered output
- **Track usage metrics**: measure which commands agents use most (visible in `rtk gain --history`)
- **Guaranteed compatibility**: always works even if RTK doesn't implement the command

```bash
rtk proxy git log --oneline -20    # Full git log output (no truncation)
rtk proxy npm install express      # Raw npm output (no filtering)
rtk proxy curl https://api.example.com/data  # Any command works
```

All proxy commands appear in `rtk gain --history` with 0% bash output reduction (input = output).

## Contribution Rules

| If you're about to... | Rule | See |
|---|---|---|
| Write any code at all | Correctness > token savings when the user explicitly requests detail; RTK's output must be transparent (never a format the LLM wouldn't expect); every filter falls back to raw output on failure; <10ms startup, no async | [CONTRIBUTING.md § Design Philosophy](CONTRIBUTING.md#design-philosophy) |
| Decide if a feature belongs in RTK | Filtering/compression of CLI output is in scope; general-purpose tooling, config management, and anything not about token savings is not | [CONTRIBUTING.md § What Belongs in RTK?](CONTRIBUTING.md#what-belongs-in-rtk) |
| Add a new command filter | Rust module vs TOML filter decision, the six-phase execution flow, exit-code/tee/truncation contracts every filter must satisfy; route through `core/runner` + `guard::never_worse` and return `Result<i32>`, don't roll your own `.output()`/exit | [src/cmds/README.md § Adding a New Command Filter](src/cmds/README.md#adding-a-new-command-filter) |
| Write or touch Rust code | No `async`, no `unwrap()` in production, `LazyLock` for fixed/reused regex (runtime-dependent patterns stay local), fallback-on-failure, exit code propagation | [docs/contributing/rust-patterns.md](docs/contributing/rust-patterns.md) |
| Add or change a filter's tests | Colocated `#[cfg(test)]` unit tests; inline fixtures vs `include_str!` fixtures in `tests/fixtures/`, when to use which | [docs/contributing/cli-testing.md](docs/contributing/cli-testing.md) |
| Search the codebase | Targeted lookup (symbol/string search, then file-name search) before opening files; avoid shelling out to raw `find`/`grep`/`rg` | [docs/contributing/search-strategy.md](docs/contributing/search-strategy.md) |
| Understand the overall system | Command lifecycle, module map, filtering strategy matrix, token tracking | [docs/contributing/ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md) |
| Name a branch | `type/short-description` convention | [CONTRIBUTING.md § Branch Naming Convention](CONTRIBUTING.md#branch-naming-convention) |
| Commit | Conventional-commit format, one logical change per commit; `cargo fmt --all --check && cargo clippy --all-targets && cargo test` must pass first, zero tolerance on clippy warnings | [CONTRIBUTING.md § Commit Messages & Changelog](CONTRIBUTING.md#commit-messages--changelog) |
| Open a PR | Scope rules, required tests/docs, CLA | [CONTRIBUTING.md § Pull Request Process](CONTRIBUTING.md#pull-request-process) |

Not to be confused with `rtk init --codex`, which writes RTK-awareness into a *user's own*
project `AGENTS.md` so Codex/Kimi route commands through RTK. This file is about
contributing to rtk's own codebase, not RTK's product feature of the same name.

## Claude Code-specific behavior

Claude Code sessions in this repo also follow three session-management protocols that aren't rtk coding rules, so they live in [CLAUDE.md](CLAUDE.md) instead of here: working-directory confirmation before file operations, a rule against open-ended exploration ("rabbit holes"), and a numbered-plan execution protocol. Other agents without an equivalent convention may want to read them for reference; they're not required reading.
