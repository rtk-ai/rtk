# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**rtk (Rust Token Killer)** is a high-performance CLI proxy that minimizes LLM token consumption by filtering and compressing command outputs. It reduces bash output on common development operations (each filter must cut at least 20%; most cut far more) through smart filtering, grouping, truncation, and deduplication. All percentages in this repo measure bash output, not your bill. RTK ships no tokenizer (`src/core/tracking.rs` estimates tokens as `bytes / 4`), so the ratios are reliable but the absolute token counts are approximate.

### Name Collision Warning

**Two different "rtk" projects exist:**
- This project: Rust Token Killer (rtk-ai/rtk)
- reachingforthejack/rtk: Rust Type Kit (DIFFERENT - generates Rust types)

**Verify correct installation:**
```bash
rtk --version  # Any current version; the check that matters is the next line
rtk gain       # Should show token savings stats (NOT "command not found")
```

If `rtk gain` fails, you have the wrong package installed.

## Development Commands

> **Note**: If rtk is installed, prefer `rtk <cmd>` over raw commands for token-optimized output.
> All commands work with passthrough support even for subcommands rtk doesn't specifically handle.

### Build & Run
```bash
cargo build                   # raw
rtk cargo build               # preferred (token-optimized)
cargo build --release         # release build (optimized)
cargo run -- <command>        # run directly
cargo install --path .        # install locally
```

### Testing
```bash
cargo test                    # all tests
rtk cargo test                # preferred (token-optimized)
cargo test <test_name>        # specific test
cargo test <module_name>::    # module tests
cargo test -- --nocapture     # with stdout
bash scripts/test-all.sh      # smoke tests (installed binary required)
```

### Linting & Quality
```bash
cargo check                   # check without building
cargo fmt                     # format code
cargo clippy --all-targets    # all clippy lints
rtk cargo clippy --all-targets # preferred
```

### Pre-commit Gate
```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
cargo fmt --manifest-path proxy/Cargo.toml --all   # proxy/ is its own workspace — root fmt does not cover it
```

### Package Building
```bash
cargo deb                     # DEB package (needs cargo-deb)
cargo generate-rpm            # RPM package (needs cargo-generate-rpm, after release build)
```

## Architecture

rtk uses a **command proxy architecture**: `main.rs` routes CLI commands via a Clap `Commands` enum to specialized filter modules in `src/cmds/*/`, each of which executes the underlying command and compresses its output. Token savings are tracked in SQLite via `src/core/tracking.rs`.

For the full architecture, component details, and module development patterns, see:
- [ARCHITECTURE.md](docs/contributing/ARCHITECTURE.md) — System design, module organization, filtering strategies, error handling
- [docs/contributing/TECHNICAL.md](docs/contributing/TECHNICAL.md) — End-to-end flow, folder map, hook system, filter pipeline

Module responsibilities are documented in each folder's `README.md` and each file's `//!` doc header. Browse `src/cmds/*/` to discover available filters.

Supported ecosystems: git/gh/gt, cargo, go/golangci-lint, npm/pnpm/npx, ruff/pytest/pip/mypy, rspec/rubocop/rake, dotnet, playwright/vitest/jest, docker/kubectl/aws, gradlew/mvn, php/artisan/phpunit/phpstan/pest.

### Bypass Mode

**Purpose**: Execute commands without filtering but track usage for metrics.

**Usage**: `rtk bypass <command> [args...]`

**Benefits**:
- **Bypass RTK filtering**: Workaround bugs or get full unfiltered output
- **Track usage metrics**: Measure which commands Claude uses most (visible in `rtk gain --history`)
- **Guaranteed compatibility**: Always works even if RTK doesn't implement the command

**Examples**:
```bash
rtk bypass git log --oneline -20    # Full git log output (no truncation)
rtk bypass npm install express      # Raw npm output (no filtering)
rtk bypass curl https://api.example.com/data  # Any command works
```

All bypassed commands appear in `rtk gain --history` with 0% bash output reduction (input = output).

> **Not the same thing as `rtk llm-proxy`.** `rtk bypass <cmd>` runs one local
> command unfiltered (`rtk proxy` is a deprecated alias for it). `rtk llm-proxy` manages the `rtk-proxy` daemon in
> [`proxy/`](proxy/README.md), which intercepts your agent's *provider API*
> traffic and strips bytes no reader sees (ANSI, progress redraws, duplicate
> lines). CCR compression is implemented but off by default.

### LLM Proxy (`proxy/`)

Its own workspace root, **not** a member of the `rtk` workspace and **not** a
dependency of the `rtk` crate: the CLI locates and spawns the `rtk-proxy` binary
and reads its SQLite usage DB for `rtk gain`. This
keeps tokio/axum out of a CLI with a <10ms startup budget, and keeps the two
licenses (rtk is Apache-2.0, `proxy/` is BUSL-1.1) in separate artifacts.

```bash
# `proxy/` is NOT a workspace member (keeps tokio/axum out of the CLI's
# graph). Cargo still scans every manifest in the repo, so installs must
# name the package: `cargo install --git <url> rtk`. Build and test the
# daemon from its own manifest:
cargo build --release                                   # rtk (+ engine)
cargo build --release --manifest-path proxy/Cargo.toml   # the daemon
cargo test --workspace                                  # rtk + engine
cargo test  --manifest-path proxy/Cargo.toml            # the daemon
cargo clippy --workspace --all-targets
cargo clippy --manifest-path proxy/Cargo.toml --all-targets
```

`proxy/` pins `rusqlite 0.34` to match what `icm-store` links — `libsqlite3-sys`
declares `links = "sqlite3"`, so two versions cannot coexist in one graph. That
pin is why the root crate tracks the same version; do not bump one alone.

## Coding Rules

Rust patterns, error handling, and anti-patterns are defined in `.claude/rules/rust-patterns.md` (auto-loaded into context). Key points:

- **anyhow::Result** everywhere, always `.context("description")?`
- **No unwrap()** in production code
- **`LazyLock` statics** for all regex (never compile on every function call)
- **Fallback pattern**: if filter fails, execute raw command unchanged
- **No async**: single-threaded by design (startup <10ms)
- **Exit code propagation**: `std::process::exit(code)` on child failure

Testing strategy and performance targets are defined in `.claude/rules/cli-testing.md` (auto-loaded). Key targets: <10ms on the hook path, ≥20% reduction in bash output bytes per filter (the CONTRIBUTING.md floor). Binary size is tracked there against a measured breakdown rather than a fixed ceiling.

For contribution workflow and design philosophy, see [CONTRIBUTING.md](CONTRIBUTING.md). For the step-by-step filter implementation checklist, see [src/cmds/README.md](src/cmds/README.md#adding-a-new-command-filter).

## Build Verification (Mandatory)

**CRITICAL**: After ANY Rust file edits, ALWAYS run the full quality check pipeline before committing:

```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
cargo fmt --manifest-path proxy/Cargo.toml --all   # proxy/ is its own workspace — root fmt does not cover it
```

**Rules**:
- Never commit code that hasn't passed all 3 checks
- Fix ALL clippy warnings before moving on (zero tolerance)
- If build fails, fix it immediately before continuing to next task

**Performance verification** (for filter changes):
```bash
hyperfine 'rtk git log -10' --warmup 3          # before
cargo build --release
hyperfine 'target/release/rtk git log -10' --warmup 3  # after (should be <10ms)
```

## Working Directory Confirmation

**ALWAYS confirm working directory before starting any work**:

```bash
pwd  # Verify you're in the rtk project root
git branch  # Verify correct branch (main, feature/*, etc.)
```

**Never assume** which project to work in. Always verify before file operations.

## Avoiding Rabbit Holes

**Stay focused on the task**. Do not make excessive operations to verify external APIs, documentation, or edge cases unless explicitly asked.

**Rule**: If verification requires more than 3-4 exploratory commands, STOP and ask the user whether to continue or trust available info.

**Examples of rabbit holes to avoid**:
- Excessive regex pattern testing (trust snapshot tests, don't manually verify 20 edge cases)
- Deep diving into external command documentation (use fixtures, don't research git/cargo internals)
- Over-testing cross-platform behavior (test macOS + Linux, trust CI for Windows)
- Verifying API signatures across multiple crate versions (use docs.rs if needed, don't clone repos)

**When to stop and ask**:
- "Should I research X external API behavior?" → ASK if it requires >3 commands
- "Should I test Y edge case?" → ASK if not mentioned in requirements
- "Should I verify Z across N platforms?" → ASK if N > 2

## Plan Execution Protocol

When user provides a numbered plan (QW1-QW4, Phase 1-5, sprint tasks, etc.):

1. **Execute sequentially**: Follow plan order unless explicitly told otherwise
2. **Commit after each logical step**: One commit per completed phase/task
3. **Never skip or reorder**: If a step is blocked, report it and ask before proceeding
4. **Track progress**: Use task list (TaskCreate/TaskUpdate) for plans with 3+ steps
5. **Validate assumptions**: Before starting, verify all referenced file paths exist and working directory is correct
