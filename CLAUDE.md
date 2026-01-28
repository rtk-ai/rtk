# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

**rtk (Rust Token Killer)** is a high-performance CLI proxy that minimizes LLM token consumption by filtering and compressing command outputs. It achieves 60-90% token savings on common development operations through smart filtering, grouping, truncation, and deduplication.

This is a fork with critical fixes for git argument parsing and modern JavaScript stack support (pnpm).

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

## Fork-Specific Features

### PR #5: Git Argument Parsing Fix (CRITICAL)
- **Problem**: Git flags like `--oneline`, `--cached` were rejected
- **Solution**: Fixed Clap parsing with proper trailing_var_arg configuration
- **Impact**: All git commands now accept native git flags

### PR #6: pnpm Support
- **New Commands**: `rtk pnpm list`, `rtk pnpm outdated`, `rtk pnpm install`
- **Token Savings**: 70-90% reduction on package manager operations
- **Security**: Package name validation prevents command injection

## Testing Strategy

Tests are embedded in modules using `#[cfg(test)] mod tests`:
- Unit tests validate filtering logic (filter.rs, grep_cmd.rs, etc.)
- Integration tests verify command output transformations (git.rs, runner.rs)
- Security tests ensure proper command sanitization (pnpm validation)

Run module-specific tests:
```bash
cargo test filter::tests::
cargo test git::tests::
cargo test runner::tests::
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
