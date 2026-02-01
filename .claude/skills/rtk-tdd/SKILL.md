---
name: rtk-tdd
description: >
  Enforces TDD (Red-Green-Refactor) for Rust development. Auto-triggers on
  implementation, testing, refactoring, and bug fixing tasks. Provides
  Rust-idiomatic testing patterns with anyhow/thiserror, cfg(test), and
  Arrange-Act-Assert workflow.
---

# Rust TDD Workflow

## Three Laws of TDD

1. Do NOT write production code without a failing test
2. Write only enough test to fail (including compilation failure)
3. Write only enough production code to pass the failing test

Cycle: **RED** (test fails) -> **GREEN** (minimum to pass) -> **REFACTOR** (cleanup, cargo test)

## Red-Green-Refactor Steps

```
1. Write test in #[cfg(test)] mod tests of the SAME file
2. cargo test MODULE::tests::test_name  -- must FAIL (red)
3. Implement the minimum in the function
4. cargo test MODULE::tests::test_name  -- must PASS (green)
5. Refactor if needed, re-run cargo test (still green)
6. cargo fmt && cargo clippy --all-targets && cargo test  (final gate)
```

Never skip step 2. If the test passes immediately, it tests nothing.

## Idiomatic Rust Test Patterns

| Pattern | Usage | When |
|---------|-------|------|
| Arrange-Act-Assert | Base structure for every test | Always |
| `assert_eq!` / `assert!` | Direct comparison / booleans | Deterministic values |
| `assert!(result.is_err())` | Error path testing | Invalid inputs |
| `Result<()>` return type | Tests with `?` operator | Fallible functions |
| `#[should_panic]` | Expected panic | Invariants, preconditions |
| `tempfile::NamedTempFile` | File/I/O tests | Filesystem-dependent code |

## Patterns by Code Type

| Code Type | Test Pattern | Example |
|-----------|-------------|---------|
| Pure function (str -> str) | Input literal -> assert output | `assert_eq!(truncate("hello", 3), "...")` |
| Parsing/filtering | Raw string -> filter -> contains/not-contains | `assert!(filter(raw).contains("expected"))` |
| Validation/security | Boundary inputs -> assert bool | `assert!(!is_valid("../etc/passwd"))` |
| Error handling | Bad input -> `is_err()` | `assert!(parse("garbage").is_err())` |
| Struct/enum roundtrip | Construct -> serialize -> deserialize -> eq | `assert_eq!(from_str(to_str(x)), x)` |

## Naming Convention

```
test_{function}_{scenario}
test_{function}_{input_type}
```

Examples: `test_truncate_edge_case`, `test_parse_invalid_input`, `test_filter_empty_string`

## When NOT to Use Pure TDD

- Functions calling `Command::new()` -> test the parser, not the execution
- `std::process::exit()` -> refactor to `Result` first, then test the Result
- Direct I/O (SQLite, network) -> use tempfile/mock or test the pure logic separately
- Main/CLI wiring -> covered by integration/smoke tests

## Pre-Commit Gate

```bash
cargo fmt --all --check
cargo clippy --all-targets
cargo test
```

All 3 must pass. No exceptions. No `#[allow(...)]` without documented justification.
