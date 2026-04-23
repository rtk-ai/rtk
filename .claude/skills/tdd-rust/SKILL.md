---
name: tdd-rust
description: TDD workflow for RTK filter development. Red-Green-Refactor with Rust idioms. Real fixtures, token savings assertions, snapshot tests with insta. Auto-triggers on new filter implementation.
triggers:
  - "new filter"
  - "implement filter"
  - "add command"
  - "write tests for"
  - "test coverage"
  - "fix failing test"
allowed-tools:
  - Read
  - Write
  - Edit
  - Bash
effort: medium
tags: [tdd, testing, rust, filters, snapshots, token-savings, rtk]
---

# RTK TDD Workflow

Enforce Red-Green-Refactor for all RTK filter development.

## The Loop

```
1. RED   — Write failing test with real fixture
2. GREEN — Implement minimum code to pass
3. REFACTOR — Clean up, verify still passing
4. SAVINGS — Verify ≥60% token reduction
5. SNAPSHOT — Lock output format with insta
```

## Step 1: Real Fixture First

Never write synthetic test data. Capture real command output:

```bash
# Capture real output from the actual command
git log -20 > tests/fixtures/git_log_raw.txt
cargo test 2>&1 > tests/fixtures/cargo_test_raw.txt
cargo clippy 2>&1 > tests/fixtures/cargo_clippy_raw.txt
gh pr view 42 > tests/fixtures/gh_pr_view_raw.txt

# For commands with ANSI codes — capture as-is
script -q /dev/null cargo test 2>&1 > tests/fixtures/cargo_test_ansi_raw.txt
```

Fixture naming: `tests/fixtures/<command>_raw.txt`

## Step 2: Write the Test (Red)

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    // Test 1: Output format (snapshot)
    #[test]
    fn test_filter_output_format() {
        let input = include_str!("../tests/fixtures/mycmd_raw.txt");
        let output = filter_mycmd(input).expect("filter should not fail");
        assert_snapshot!(output);
    }

    // Test 2: Token savings ≥60%
    #[test]
    fn test_token_savings() {
        let input = include_str!("../tests/fixtures/mycmd_raw.txt");
        let output = filter_mycmd(input).expect("filter should not fail");

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 * (1.0 - output_tokens as f64 / input_tokens as f64);

        assert!(
            savings >= 60.0,
            "Expected ≥60% token savings, got {:.1}% ({} → {} tokens)",
            savings, input_tokens, output_tokens
        );
    }

    // Test 3: Edge cases
    #[test]
    fn test_empty_input() {
        let result = filter_mycmd("");
        assert!(result.is_ok());
        // Empty input = empty output OR passthrough, never panic
    }

    #[test]
    fn test_malformed_input() {
        let result = filter_mycmd("not valid command output\nrandom text\n");
        // Must not panic — either filter best-effort or return input unchanged
        assert!(result.is_ok());
    }
}
```

Run: `cargo test` → should fail (function doesn't exist yet).

## Step 3: Minimum Implementation (Green)

```rust
// src/mycmd_cmd.rs

use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref ERROR_RE: Regex = Regex::new(r"^error").unwrap();
}

pub fn filter_mycmd(input: &str) -> Result<String> {
    if input.is_empty() {
        return Ok(String::new());
    }

    let filtered: Vec<&str> = input.lines()
        .filter(|line| ERROR_RE.is_match(line))
        .collect();

    Ok(filtered.join("\n"))
}
```

Run: `cargo test` → green.

## Step 4: Accept Snapshot

```bash
# First run creates the snapshot
cargo test test_filter_output_format

# Review what was captured
cargo insta review
# Press 'a' to accept

# Snapshot saved to src/snapshots/mycmd_cmd__tests__test_filter_output_format.snap
```

## Step 5: Wire to main.rs (Integration)

```rust
// src/main.rs
mod mycmd_cmd;

#[derive(Subcommand)]
pub enum Commands {
    // ... existing commands ...
    Mycmd(MycmdArgs),
}

// In match:
Commands::Mycmd(args) => mycmd_cmd::run(args),
```

```rust
// src/mycmd_cmd.rs — add run() function
pub fn run(args: MycmdArgs) -> Result<()> {
    let output = execute_command("mycmd", &args.to_vec())
        .context("Failed to execute mycmd")?;

    let filtered = filter_mycmd(&output.stdout)
        .unwrap_or_else(|e| {
            eprintln!("rtk: filter warning: {}", e);
            output.stdout.clone()
        });

    tracking::record("mycmd", &output.stdout, &filtered)?;
    print!("{}", filtered);

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }
    Ok(())
}
```

## Step 6: Quality Gate

```bash
cargo fmt --all && cargo clippy --all-targets && cargo test
```

All 3 must pass. Zero clippy warnings.

## Arrange-Act-Assert Pattern

```rust
#[test]
fn test_filters_only_errors() {
    // Arrange
    let input = "info: starting build\nerror[E0001]: undefined\nwarning: unused\n";

    // Act
    let output = filter_mycmd(input).expect("should succeed");

    // Assert
    assert!(output.contains("error[E0001]"), "Should keep error lines");
    assert!(!output.contains("info:"), "Should drop info lines");
    assert!(!output.contains("warning:"), "Should drop warning lines");
}
```

## RTK-Specific Test Patterns

### Test ANSI stripping

```rust
#[test]
fn test_strips_ansi_codes() {
    let input = "\x1b[32mSuccess\x1b[0m\n\x1b[31merror: failed\x1b[0m\n";
    let output = filter_mycmd(input).expect("should succeed");
    assert!(!output.contains("\x1b["), "ANSI codes should be stripped");
    assert!(output.contains("error: failed"), "Content should be preserved");
}
```

### Test fallback behavior

```rust
#[test]
fn test_filter_handles_unexpected_format() {
    // Give it something completely unexpected
    let input = "completely unexpected\x00binary\xff data";
    // Should not panic — returns Ok() with either empty or passthrough
    let result = filter_mycmd(input);
    assert!(result.is_ok(), "Filter must not panic on unexpected input");
}
```

### Test savings at multiple sizes

```rust
#[test]
fn test_savings_large_output() {
    // 1000-line fixture → must still hit ≥60%
    let large_input: String = (0..1000)
        .map(|i| format!("info: processing item {}\n", i))
        .collect();
    let output = filter_mycmd(&large_input).expect("should succeed");

    let savings = 100.0 * (1.0 - count_tokens(&output) as f64 / count_tokens(&large_input) as f64);
    assert!(savings >= 60.0, "Large output savings: {:.1}%", savings);
}
```

## What "Done" Looks Like

Checklist before moving on:

- [ ] `tests/fixtures/<cmd>_raw.txt` — real command output
- [ ] `filter_<cmd>()` function returns `Result<String>`
- [ ] Snapshot test passes and accepted via `cargo insta review`
- [ ] Token savings test: ≥60% verified
- [ ] Empty input test: no panic
- [ ] Malformed input test: no panic
- [ ] `run()` function with fallback pattern
- [ ] Registered in `main.rs` Commands enum
- [ ] `cargo fmt --all && cargo clippy --all-targets && cargo test` — all green

## Never Do This

```rust
// ❌ Synthetic fixture data
let input = "fake error: something went wrong";  // Not real cargo output

// ❌ Missing savings test
#[test]
fn test_filter() {
    let output = filter_mycmd(input);
    assert!(!output.is_empty());  // No savings verification
}

// ❌ unwrap() in production code
let filtered = filter_mycmd(input).unwrap();  // Panic in prod

// ❌ Regex inside the filter function
fn filter_mycmd(input: &str) -> Result<String> {
    let re = Regex::new(r"^error").unwrap();  // Recompiles every call
    ...
}
```
