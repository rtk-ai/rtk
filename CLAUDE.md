# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**rtk (Rust Token Killer)** is a high-performance CLI proxy that minimizes LLM token consumption by filtering and compressing command outputs. It achieves 60-90% token savings on common development operations through smart filtering, grouping, truncation, and deduplication.

This is a fork with critical fixes for git argument parsing and modern JavaScript stack support (pnpm, vitest, Next.js, TypeScript, Playwright, Prisma).

### Name Collision Warning

**Two different "rtk" projects exist:**
- This project: Rust Token Killer (rtk-ai/rtk)
- reachingforthejack/rtk: Rust Type Kit (DIFFERENT - generates Rust types)

**Verify correct installation:**
```bash
rtk --version  # Should show "rtk 0.28.2" (or newer)
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
```

### Package Building
```bash
cargo deb                     # DEB package (needs cargo-deb)
cargo generate-rpm            # RPM package (needs cargo-generate-rpm, after release build)
```

## Architecture

rtk uses a **command proxy architecture**: `main.rs` routes CLI commands via a Clap `Commands` enum to specialized filter modules in `src/cmds/*/`, each of which executes the underlying command and compresses its output. Token savings are tracked in SQLite via `src/core/tracking.rs`.

For the full architecture, component details, and module development patterns, see:
- [ARCHITECTURE.md](ARCHITECTURE.md) — System design, module organization, filtering strategies, error handling
- [docs/TECHNICAL.md](docs/TECHNICAL.md) — End-to-end flow, folder map, hook system, filter pipeline

Module responsibilities are documented in each folder's `README.md` and each file's `//!` doc header. Browse `src/cmds/*/` to discover available filters.

Supported ecosystems: git/gh/gt, cargo, go/golangci-lint, npm/pnpm/npx, ruff/pytest/pip/mypy, rspec/rubocop/rake, dotnet, playwright/vitest/jest, docker/kubectl/aws.

### Proxy Mode

**Purpose**: Execute commands without filtering but track usage for metrics.

**Usage**: `rtk proxy <command> [args...]`

**Benefits**:
- **Bypass RTK filtering**: Workaround bugs or get full unfiltered output
- **Track usage metrics**: Measure which commands Claude uses most (visible in `rtk gain --history`)
- **Guaranteed compatibility**: Always works even if RTK doesn't implement the command

**Examples**:
```bash
rtk proxy git log --oneline -20    # Full git log output (no truncation)
rtk proxy npm install express      # Raw npm output (no filtering)
rtk proxy curl https://api.example.com/data  # Any command works
```

All proxy commands appear in `rtk gain --history` with 0% savings (input = output).

## Coding Rules

Rust patterns, error handling, and anti-patterns are defined in `.claude/rules/rust-patterns.md` (auto-loaded into context). Key points:

- **anyhow::Result** everywhere, always `.context("description")?`
- **No unwrap()** in production code
- **lazy_static!** for all regex (never compile inside a function)
- **Fallback pattern**: if filter fails, execute raw command unchanged
- **No async**: single-threaded by design (startup <10ms)
- **Exit code propagation**: `std::process::exit(code)` on child failure

Testing strategy and performance targets are defined in `.claude/rules/cli-testing.md` (auto-loaded). Key targets: <10ms startup, <5MB memory, 60-90% token savings.

For contribution workflow and design philosophy, see [CONTRIBUTING.md](CONTRIBUTING.md). For the step-by-step filter implementation checklist, see [src/cmds/README.md](src/cmds/README.md#adding-a-new-command-filter).

## Build Verification (Mandatory)

**CRITICAL**: After ANY Rust file edits, ALWAYS run the full quality check pipeline before committing:

```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
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
