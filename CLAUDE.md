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
rtk --version  # Should show "rtk 0.18.0" (or newer)
rtk gain       # Should show token savings stats (NOT "command not found")
```

If `rtk gain` fails, you have the wrong package installed.

## Development Commands

> **Note**: If rtk is installed, prefer `rtk <cmd>` over raw commands for token-optimized output.
> All commands work with passthrough support even for subcommands rtk doesn't specifically handle.

### Build & Run
```bash
# Development build
cargo build                   # raw
rtk cargo build               # preferred (token-optimized)

# Release build (optimized)
cargo build --release
rtk cargo build --release

# Run directly
cargo run -- <command>

# Install locally
cargo install --path .
```

### Testing
```bash
# Run all tests
cargo test                    # raw
rtk cargo test                # preferred (token-optimized)

# Run specific test
cargo test <test_name>
rtk cargo test <test_name>

# Run tests with output
cargo test -- --nocapture
rtk cargo test -- --nocapture

# Run tests in specific module
cargo test <module_name>::
rtk cargo test <module_name>::
```

### Linting & Quality
```bash
# Check without building
cargo check                   # raw
rtk cargo check               # preferred (token-optimized)

# Format code
cargo fmt                     # passthrough (0% savings, but works)

# Run clippy lints
cargo clippy                  # raw
rtk cargo clippy              # preferred (token-optimized)

# Check all targets
cargo clippy --all-targets
rtk cargo clippy --all-targets
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
- Each module handles a specific command type (git, grep, etc.)
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
- **Configurable database path**: Via `RTK_DB_PATH` env var or `config.toml`
  - Priority: env var > config file > default location

**4. Configuration System** (src/config.rs, src/init.rs)
- Manages CLAUDE.md initialization (global vs local)
- Reads ~/.config/rtk/config.toml for user preferences
- `rtk init` command bootstraps LLM integration
- **New**: `tracking.database_path` field for custom DB location

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

### Proxy Mode

**Purpose**: Execute commands without filtering but track usage for metrics.

**Usage**: `rtk proxy <command> [args...]`

**Benefits**:
- **Bypass RTK filtering**: Workaround bugs or get full unfiltered output
- **Track usage metrics**: Measure which commands Claude uses most (visible in `rtk gain --history`)
- **Guaranteed compatibility**: Always works even if RTK doesn't implement the command
- **Prototyping**: Test new commands before implementing optimized filtering

**Examples**:
```bash
# Full git log output (no truncation)
rtk proxy git log --oneline -20

# Raw npm output (no filtering)
rtk proxy npm install express

# Any command works
rtk proxy curl https://api.example.com/data

# Tracking shows 0% savings (expected)
rtk gain --history | grep proxy
```

**Tracking**: All proxy commands appear in `rtk gain --history` with 0% savings (input = output) but preserve usage statistics.

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
| ruff_cmd.rs | Ruff linter/formatter | JSON for check, text for format (80%+ reduction) |
| pytest_cmd.rs | Pytest test runner | State machine text parser (90%+ reduction) |
| pip_cmd.rs | pip/uv package manager | JSON parsing, auto-detect uv (70-85% reduction) |
| go_cmd.rs | Go commands | NDJSON for test, text for build/vet (80-90% reduction) |
| golangci_cmd.rs | golangci-lint | JSON parsing, group by rule (85% reduction) |
| utils.rs | Shared utilities | Package manager detection, common formatting |
| discover/ | Claude Code history analysis | Scan JSONL sessions, classify commands, report missed savings |

## Performance Constraints

RTK has **strict performance targets** to maintain zero-overhead CLI experience:

| Metric | Target | Verification Method |
|--------|--------|---------------------|
| **Startup time** | <10ms | `hyperfine 'rtk git status' 'git status'` |
| **Memory overhead** | <5MB resident | `/usr/bin/time -l rtk git status` (macOS) |
| **Token savings** | 60-90% | Verify in tests with `count_tokens()` assertions |
| **Binary size** | <5MB stripped | `ls -lh target/release/rtk` |

**Performance regressions are release blockers** - always benchmark before/after changes:

```bash
# Before changes
hyperfine 'rtk git log -10' --warmup 3 > /tmp/before.txt

# After changes
cargo build --release
hyperfine 'target/release/rtk git log -10' --warmup 3 > /tmp/after.txt

# Compare (should be <10ms)
diff /tmp/before.txt /tmp/after.txt
```

**Why <10ms matters**: Claude Code users expect CLI tools to be instant. Any perceptible delay (>10ms) breaks the developer flow. RTK achieves this through:
- **Zero async overhead**: Single-threaded, no tokio runtime
- **Lazy regex compilation**: Compile once with `lazy_static!`, reuse forever
- **Minimal allocations**: Borrow over clone, in-place filtering
- **No user config**: Zero file I/O on startup (config loaded on-demand)

## Error Handling

RTK follows Rust best practices for error handling:

**Rules**:
- **anyhow::Result** for CLI binary (RTK is an application, not a library)
- **ALWAYS** use `.context("description")` with `?` operator
- **NO unwrap()** in production code (tests only - use `expect("explanation")` if needed)
- **Graceful degradation**: If filter fails, fallback to raw command execution

**Example**:

```rust
use anyhow::{Context, Result};

pub fn filter_git_log(input: &str) -> Result<String> {
    let lines: Vec<_> = input
        .lines()
        .filter(|line| !line.is_empty())
        .collect();

    // ✅ RIGHT: Context on error
    let hash = extract_hash(lines[0])
        .context("Failed to extract commit hash from git log")?;

    // ❌ WRONG: No context
    let hash = extract_hash(lines[0])?;

    // ❌ WRONG: Panic in production
    let hash = extract_hash(lines[0]).unwrap();

    Ok(format!("Commit: {}", hash))
}
```

**Fallback pattern** (critical for all filters):

```rust
// ✅ RIGHT: Fallback to raw command if filter fails
pub fn execute_with_filter(cmd: &str, args: &[&str]) -> Result<()> {
    match get_filter(cmd) {
        Some(filter) => match filter.apply(cmd, args) {
            Ok(output) => println!("{}", output),
            Err(e) => {
                eprintln!("Filter failed: {}, falling back to raw", e);
                execute_raw(cmd, args)?;
            }
        },
        None => execute_raw(cmd, args)?,
    }
    Ok(())
}

// ❌ WRONG: Panic if no filter
pub fn execute_with_filter(cmd: &str, args: &[&str]) -> Result<()> {
    let filter = get_filter(cmd).expect("Filter must exist");
    filter.apply(cmd, args)?;
    Ok(())
}
```

## Common Pitfalls

**Don't add async dependencies** (kills startup time)
- RTK is single-threaded by design
- Adding tokio/async-std adds ~5-10ms startup overhead
- Use blocking I/O with fallback to raw command

**Don't recompile regex at runtime** (kills performance)
- ❌ WRONG: `let re = Regex::new(r"pattern").unwrap();` inside function
- ✅ RIGHT: `lazy_static! { static ref RE: Regex = Regex::new(r"pattern").unwrap(); }`

**Don't panic on filter failure** (breaks user workflow)
- Always fallback to raw command execution
- Log error to stderr, execute original command unchanged

**Don't assume command output format** (breaks across versions)
- Test with real fixtures from multiple versions
- Use flexible regex patterns that tolerate format changes

**Don't skip cross-platform testing** (macOS ≠ Linux ≠ Windows)
- Shell escaping differs: bash/zsh vs PowerShell
- Path separators differ: `/` vs `\`
- Line endings differ: LF vs CRLF

**Don't break pipe compatibility** (users expect Unix behavior)
- `rtk git status | grep modified` must work
- Preserve stdout/stderr separation
- Respect exit codes (0 = success, non-zero = failure)

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

### Python & Go Support (2026-02-12)
- **Python Commands**: 3 commands for Python development workflows
  - `rtk ruff check/format`: Ruff linter/formatter with JSON (check) and text (format) parsing (80%+ reduction)
  - `rtk pytest`: Pytest test runner with state machine text parser (90%+ reduction)
  - `rtk pip list/outdated/install`: pip package manager with auto-detect uv (70-85% reduction)
- **Go Commands**: 4 commands via sub-enum for Go ecosystem
  - `rtk go test`: NDJSON line-by-line parser for interleaved events (90%+ reduction)
  - `rtk go build`: Text filter showing errors only (80% reduction)
  - `rtk go vet`: Text filter for issues (75% reduction)
  - `rtk golangci-lint`: JSON parsing grouped by rule (85% reduction)
- **Architecture**: Standalone Python commands (mirror lint/prettier), Go sub-enum (mirror git/cargo)
- **Patterns**: JSON for structured output (ruff check, golangci-lint, pip), NDJSON streaming (go test), text state machine (pytest), text filters (go build/vet, ruff format)

## Testing Strategy

### TDD Workflow (mandatory)
All code follows Red-Green-Refactor. See `.claude/skills/rtk-tdd/` for the full workflow and Rust-idiomatic patterns. See `.claude/skills/rtk-tdd/references/testing-patterns.md` for RTK-specific patterns and untested module backlog.

### Test Architecture
- **Unit tests**: Embedded `#[cfg(test)] mod tests` in each module (105+ tests, 25+ files)
- **Smoke tests**: `scripts/test-all.sh` (69 assertions on all commands)
- **Dominant pattern**: raw string input -> filter function -> assert output contains/excludes

### Pre-commit gate
```bash
cargo fmt --all --check && rtk cargo clippy --all-targets && rtk cargo test
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

## Build Verification (Mandatory)

**CRITICAL**: After ANY Rust file edits, ALWAYS run the full quality check pipeline before committing:

```bash
cargo fmt --all && cargo clippy --all-targets && cargo test --all
```

**Rules**:
- Never commit code that hasn't passed all 3 checks
- Fix ALL clippy warnings before moving on (zero tolerance)
- If build fails, fix it immediately before continuing to next task
- Pre-commit hook will auto-enforce this (see `.claude/hooks/bash/pre-commit-format.sh`)

**Why**: RTK is a production CLI tool used by developers in their workflows. Bugs break developer productivity. Quality gates prevent regressions and maintain user trust.

**Performance verification** (for filter changes):

```bash
# Benchmark before/after
hyperfine 'rtk git log -10' --warmup 3
cargo build --release
hyperfine 'target/release/rtk git log -10' --warmup 3

# Memory profiling
/usr/bin/time -l target/release/rtk git status  # macOS
/usr/bin/time -v target/release/rtk git status  # Linux
```

## Testing Policy

**Manual testing is REQUIRED** for filter changes and new commands:

- **For new filters**: Test with real command (`rtk <cmd>`), verify output matches expectations
  - Example: `rtk git log -10` → inspect output, verify condensed correctly
  - Example: `rtk cargo test` → verify only failures shown, not full output

- **For hook changes**: Test in real Claude Code session, verify command rewriting works
  - Create test Claude Code session
  - Type raw command (e.g., `git status`)
  - Verify hook rewrites to `rtk git status`

- **For performance**: Run `hyperfine` comparison (before/after), verify <10ms startup
  - Benchmark baseline: `hyperfine 'rtk git status' --warmup 3`
  - Make changes, rebuild
  - Benchmark again: `hyperfine 'target/release/rtk git status' --warmup 3`
  - Compare results: startup time should be <10ms

- **For cross-platform**: Test on macOS + Linux (Docker) + Windows (CI), verify shell escaping
  - macOS (zsh): Test locally
  - Linux (bash): Use Docker `docker run --rm -v $(pwd):/rtk -w /rtk rust:latest cargo test`
  - Windows (PowerShell): Trust CI/CD pipeline or test manually if available

**Anti-pattern**: Running only automated tests (`cargo test`, `cargo clippy`) without actually executing `rtk <cmd>` and inspecting output.

**Example**: If fixing the `git log` filter, run `rtk git log -10` and verify:
1. Output is condensed (shorter than raw `git log -10`)
2. Critical info preserved (commit hashes, messages)
3. Format is readable and consistent
4. Exit code matches git's exit code (0 for success)

## Working Directory Confirmation

**ALWAYS confirm working directory before starting any work**:

```bash
pwd  # Verify you're in /Users/florianbruniaux/Sites/rtk-ai/rtk
git branch  # Verify correct branch (main, feature/*, etc.)
```

**Never assume** which project to work in. RTK shares parent directory with other projects (ccboard, cc-economics).

**Context**: Wrong directory detection was a common friction point in multi-repo environments. Always verify before file operations.

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

**Why**: Plan-driven execution produces better outcomes than ad-hoc implementation. Structured plans help maintain focus and prevent scope creep.

## Language & Communication

- **User communicates in French**: Respond in French unless explicitly writing English content (docs, code comments, READMEs)
- **"reprend"**: Resume previous task where it was left off
- **Be direct**: User prefers direct, factual communication (Bold Guy style - cash, bienveillant, énergique)

**Examples**:
- ✅ "Ça ne va pas marcher parce que X. Voici ce que je ferais : Y."
- ✅ "Le test échoue car le regex ne capture pas les commits merge. Fix : ajouter `(?:Merge|commit)`."
- ❌ "Je pense que peut-être nous pourrions éventuellement envisager de..." (trop verbeux, pas direct)

## Filter Development Checklist

When adding a new filter (e.g., `rtk newcmd`):

### Implementation
- [ ] Create filter module in `src/<cmd>_cmd.rs` (or extend existing)
- [ ] Add `lazy_static!` regex patterns for parsing (compile once, reuse)
- [ ] Implement fallback to raw command on error (graceful degradation)
- [ ] Preserve exit codes (`std::process::exit(code)` if non-zero)

### Testing
- [ ] Write snapshot test with real command output fixture (`tests/fixtures/<cmd>_raw.txt`)
- [ ] Verify token savings ≥60% with `count_tokens()` assertion
- [ ] Test cross-platform shell escaping (macOS, Linux, Windows)
- [ ] Write unit tests for edge cases (empty output, errors, unicode, ANSI codes)

### Integration
- [ ] Register filter in main.rs Commands enum
- [ ] Update README.md with new command support and token savings %
- [ ] Update CHANGELOG.md with feature description

### Quality Gates
- [ ] Run `cargo fmt --all && cargo clippy --all-targets && cargo test`
- [ ] Benchmark startup time with `hyperfine` (verify <10ms)
- [ ] Test manually: `rtk <cmd>` and inspect output for correctness
- [ ] Verify fallback: Break filter intentionally, confirm raw command executes

### Documentation
- [ ] Add command to this CLAUDE.md Module Responsibilities table
- [ ] Document token savings % (from tests)
- [ ] Add usage examples to README.md

**Example workflow** (adding `rtk newcmd`):

```bash
# 1. Create module
touch src/newcmd_cmd.rs

# 2. Write test first (TDD)
echo 'raw command output fixture' > tests/fixtures/newcmd_raw.txt
# Add test in src/newcmd_cmd.rs

# 3. Implement filter
# Add lazy_static regex, implement logic, add fallback

# 4. Quality checks
cargo fmt --all && cargo clippy --all-targets && cargo test

# 5. Benchmark
hyperfine 'rtk newcmd args'

# 6. Manual test
rtk newcmd args
# Inspect output, verify condensed

# 7. Document
# Update README.md, CHANGELOG.md, this file
```
