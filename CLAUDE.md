# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**rtk (Rust Token Killer)** is a high-performance CLI proxy that minimizes LLM token consumption by filtering and compressing command outputs. It achieves 60-90% token savings on common development operations through smart filtering, grouping, truncation, and deduplication.

This is a fork with critical fixes for git argument parsing and modern JavaScript stack support (pnpm, vitest, Next.js, TypeScript, Playwright, Prisma).

### ⚠️ Name Collision Warning

**Two different "rtk" projects exist:**
- ✅ **This project**: Rust Token Killer (rtk-ai/rtk)
- ❌ **reachingforthejack/rtk**: Rust Type Kit (DIFFERENT - generates Rust types)

**Verify correct installation:**
```bash
rtk --version  # Should show "rtk X.Y.Z"
rtk gain       # Should show token savings stats (NOT "command not found")
```

If `rtk gain` fails, you have the wrong package installed.

## Development Commands

### Build & Run
```bash
# Development build
cargo build

# Release build (optimized)
cargo build --release

# Run directly
cargo run -- <command>

# Install locally
cargo install --path .
```

### Testing
```bash
# Run all tests
cargo test

# Run specific test
cargo test <test_name>

# Run tests with output
cargo test -- --nocapture

# Run tests in specific module
cargo test <module_name>::
```

### Linting & Quality
```bash
# Check without building
cargo check

# Format code
cargo fmt

# Run clippy lints
cargo clippy

# Check all targets
cargo clippy --all-targets
```

### Package Building
```bash
# Build DEB package (Linux)
cargo install cargo-deb
cargo deb

# Build RPM package (Fedora/RHEL)
cargo install cargo-generate-rpm
cargo build --release
cargo generate-rpm
```

## Architecture

### Core Design Pattern

rtk uses a **command proxy architecture** with specialized modules for each output type:

```
main.rs (CLI entry)
  → Clap command parsing
  → Route to specialized modules
  → tracking.rs (SQLite) records token savings
```

### Key Architectural Components

**1. Command Modules** (src/*_cmd.rs, src/git.rs, src/container.rs)
- Each module handles a specific command type (git, grep, diff, etc.)
- Responsible for executing underlying commands and transforming output
- Implement token-optimized formatting strategies

**2. Core Filtering** (src/filter.rs)
- Language-aware code filtering (Rust, Python, JavaScript, etc.)
- Filter levels: `none`, `minimal`, `aggressive`
- Strips comments, whitespace, and function bodies (aggressive mode)
- Used by `read` and `smart` commands

**3. Token Tracking** (src/tracking.rs)
- SQLite-based persistent storage (~/.local/share/rtk/tracking.db)
- Records: original_cmd, rtk_cmd, input_tokens, output_tokens, savings_pct
- 90-day retention policy with automatic cleanup
- Powers the `rtk gain` analytics command

**4. Configuration System** (src/config.rs, src/init.rs)
- Manages CLAUDE.md initialization (global vs local)
- Reads ~/.config/rtk/config.toml for user preferences
- `rtk init` command bootstraps LLM integration

**5. Shared Utilities** (src/utils.rs)
- Common functions for command modules: truncate, strip_ansi, execute_command
- Package manager auto-detection (pnpm/yarn/npm/npx)
- Consistent error handling and output formatting
- Used by all modern JavaScript/TypeScript tooling commands

### Command Routing Flow

All commands follow this pattern:
```rust
main.rs:Commands enum
  → match statement routes to module
  → module::run() executes logic
  → tracking::track_command() records metrics
  → Result<()> propagates errors
```

### Critical Implementation Details

**Git Argument Handling** (src/git.rs)
- Uses `trailing_var_arg = true` + `allow_hyphen_values = true` to properly handle git flags
- Auto-detects `--merges` flag to avoid conflicting with `--no-merges` injection
- Propagates git exit codes for CI/CD reliability (PR #5 fix)

**Output Filtering Strategy**
- Compact mode: Show only summary/failures
- Full mode: Available with `-v` verbosity flags
- Test output: Show only failures (90% token reduction)
- Git operations: Ultra-compressed confirmations ("ok ✓")

**Language Detection** (src/filter.rs)
- File extension-based with fallback heuristics
- Supports Rust, Python, JS/TS, Java, Go, C/C++, etc.
- Tokenization rules vary by language (comments, strings, blocks)

### Module Responsibilities

| Module | Purpose | Token Strategy |
|--------|---------|----------------|
| git.rs | Git operations | Stat summaries + compact diffs |
| grep_cmd.rs | Code search | Group by file, truncate lines |
| ls.rs | Directory listing | Tree format, aggregate counts |
| read.rs | File reading | Filter-level based stripping |
| runner.rs | Command execution | Stderr only (err), failures only (test) |
| log_cmd.rs | Log parsing | Deduplication with counts |
| json_cmd.rs | JSON inspection | Structure without values |
| lint_cmd.rs | ESLint/Biome linting | Group by rule, file summary (84% reduction) |
| tsc_cmd.rs | TypeScript compiler | Group by file/error code (83% reduction) |
| next_cmd.rs | Next.js build/dev | Route metrics, bundle stats only (87% reduction) |
| prettier_cmd.rs | Format checking | Files needing changes only (70% reduction) |
| playwright_cmd.rs | E2E test results | Failures only, grouped by suite (94% reduction) |
| prisma_cmd.rs | Prisma CLI | Strip ASCII art and verbose output (88% reduction) |
| gh_cmd.rs | GitHub CLI | Compact PR/issue/run views (26-87% reduction) |
| vitest_cmd.rs | Vitest test runner | Failures only with ANSI stripping (99.5% reduction) |
| pnpm_cmd.rs | pnpm package manager | Compact dependency trees (70-90% reduction) |
| utils.rs | Shared utilities | Package manager detection, common formatting |
| discover/ | Claude Code history analysis | Scan JSONL sessions, classify commands, report missed savings |

## Fork-Specific Features

### PR #5: Git Argument Parsing Fix (CRITICAL)
- **Problem**: Git flags like `--oneline`, `--cached` were rejected
- **Solution**: Fixed Clap parsing with proper trailing_var_arg configuration
- **Impact**: All git commands now accept native git flags

### PR #6: pnpm Support
- **New Commands**: `rtk pnpm list`, `rtk pnpm outdated`, `rtk pnpm install`
- **Token Savings**: 70-90% reduction on package manager operations
- **Security**: Package name validation prevents command injection

### PR #9: Modern JavaScript/TypeScript Tooling (2026-01-29)
- **New Commands**: 6 commands for T3 Stack workflows
  - `rtk lint`: ESLint/Biome with grouped rule violations (84% reduction)
  - `rtk tsc`: TypeScript compiler errors grouped by file/code (83% reduction)
  - `rtk next`: Next.js build with route/bundle metrics (87% reduction)
  - `rtk prettier`: Format checker showing files needing changes (70% reduction)
  - `rtk playwright`: E2E test results showing failures only (94% reduction)
  - `rtk prisma`: Prisma CLI without ASCII art (88% reduction)
- **Shared Infrastructure**: utils.rs module for package manager auto-detection
- **Features**: Exit code preservation, error grouping, consistent formatting
- **Testing**: Validated on production T3 Stack project (methode-aristote/app)

## Testing Strategy

### TDD Workflow (mandatory)
All code follows Red-Green-Refactor. See `.claude/skills/rtk-tdd/` for the full workflow and Rust-idiomatic patterns. See `.claude/skills/rtk-tdd/references/testing-patterns.md` for RTK-specific patterns and untested module backlog.

### Test Architecture
- **Unit tests**: Embedded `#[cfg(test)] mod tests` in each module (105+ tests, 25+ files)
- **Smoke tests**: `scripts/test-all.sh` (69 assertions on all commands)
- **Dominant pattern**: raw string input -> filter function -> assert output contains/excludes

### Pre-commit gate
```bash
cargo fmt --all --check && cargo clippy --all-targets && cargo test
```

### Test commands
```bash
cargo test                    # All tests
cargo test filter::tests::    # Module-specific
cargo test -- --nocapture     # With stdout
bash scripts/test-all.sh      # Smoke tests (installed binary required)
```

## Dependencies

Core dependencies (see Cargo.toml):
- **clap**: CLI parsing with derive macros
- **anyhow**: Error handling
- **rusqlite**: SQLite for tracking database
- **regex**: Pattern matching for filtering
- **ignore**: gitignore-aware file traversal
- **colored**: Terminal output formatting
- **serde/serde_json**: Configuration and JSON parsing

## Build Optimizations

Release profile (Cargo.toml:31-36):
- `opt-level = 3`: Maximum optimization
- `lto = true`: Link-time optimization
- `codegen-units = 1`: Single codegen for better optimization
- `strip = true`: Remove debug symbols
- `panic = "abort"`: Smaller binary size

## CI/CD

GitHub Actions workflow (.github/workflows/release.yml):
- Multi-platform builds (macOS, Linux x86_64/ARM64, Windows)
- DEB/RPM package generation
- Automated releases on version tags (v*)
- Checksums for binary verification
