---
title: Testing Strategy
description: Snapshot tests, token accuracy tests, cross-platform tests, and performance benchmarks
sidebar:
  order: 4
---

# Testing Strategy

## Snapshot tests (critical)

Use the `insta` crate for output validation. This is the primary testing strategy for RTK filters.

```rust
use insta::assert_snapshot;

#[test]
fn test_git_log_output() {
    let input = include_str!("../tests/fixtures/git_log_raw.txt");
    let output = filter_git_log(input);
    assert_snapshot!(output);
}
```

**Workflow:**
1. Write test with `assert_snapshot!(output)`
2. `cargo test` (creates new snapshot on first run)
3. `cargo insta review` (interactive review — press `a` to accept)
4. Snapshot saved in `src/cmds/<ecosystem>/snapshots/`

## Token accuracy tests (critical)

All filters must verify 60-90% savings claims with real fixtures.

```rust
fn count_tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

#[test]
fn test_git_log_savings() {
    let input = include_str!("../tests/fixtures/git_log_raw.txt");
    let output = filter_git_log(input);
    let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
    assert!(savings >= 60.0, "Expected ≥60% savings, got {:.1}%", savings);
}
```

**Savings targets:**

| Filter | Minimum | Mechanism |
|--------|---------|-----------|
| `git log` | 80% | Condense commits to hash + message |
| `cargo test` | 90% | Show failures only |
| `gh pr view` | 87% | Remove ASCII art + verbose metadata |
| `pnpm list` | 70% | Compact dependency tree |
| `docker ps` | 60% | Essential fields only |

## Creating fixtures

Use real command output, not synthetic data:

```bash
git log -20 > tests/fixtures/git_log_raw.txt
cargo test 2>&1 > tests/fixtures/cargo_test_raw.txt
gh pr view 123 > tests/fixtures/gh_pr_view_raw.txt
```

## Cross-platform tests

RTK must work on macOS (zsh), Linux (bash), and Windows (PowerShell). Test shell escaping with `cfg` guards:

```rust
#[test]
fn test_shell_escaping() {
    let escaped = escape_for_shell("test");
    #[cfg(target_os = "windows")]
    assert_eq!(escaped, "\"test\"");
    #[cfg(not(target_os = "windows"))]
    assert_eq!(escaped, "test");
}
```

## Performance tests

RTK targets `<10ms` startup and `<5MB` memory.

```bash
# Benchmark before/after changes
hyperfine 'rtk git log -10' --warmup 3

# Memory usage (macOS)
/usr/bin/time -l rtk git status
# "maximum resident set size" should be <5MB
```

## Integration tests

Run against an installed binary with `#[ignore]`:

```rust
#[test]
#[ignore]
fn test_real_git_log() {
    let output = std::process::Command::new("rtk")
        .args(&["git", "log", "-10"])
        .output()
        .expect("Failed to run rtk");
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.len() < 5000, "Output too large, filter not working");
}
```

Run with: `cargo test --ignored`

## Test organization

```
src/cmds/<ecosystem>/
  <cmd>.rs            # filter + embedded unit tests
  snapshots/          # insta snapshots (auto-generated)
tests/
  common/mod.rs       # count_tokens + shared helpers
  fixtures/           # real command output (txt files)
  integration_test.rs # #[ignore] end-to-end tests
```

## Pre-commit gate

All three must pass before any commit:

```bash
cargo fmt --all --check && cargo clippy --all-targets && cargo test
```

## Anti-patterns

- **Don't** test with hardcoded synthetic strings — use real fixture files
- **Don't** skip cross-platform tests — use `cfg` guards
- **Don't** ignore savings drops below 60% — investigate and fix
- **Don't** commit without running `cargo insta review`
