# RTK Testing Patterns Reference

## Untested Modules Backlog

Prioritized by testability (pure functions first, I/O-heavy last).

### High Priority (pure functions, trivial to test)

| Module | Testable Functions | Notes |
|--------|-------------------|-------|
| `diff_cmd.rs` | `compute_diff`, `similarity`, `truncate`, `condense_unified_diff` | 4 pure functions, 0 tests |
| `env_cmd.rs` | `mask_value`, `is_lang_var`, `is_cloud_var`, `is_tool_var`, `is_interesting_var` | 5 categorization functions |

### Medium Priority (need tempfile or parsed input)

| Module | Testable Functions | Notes |
|--------|-------------------|-------|
| `tracking.rs` | `estimate_tokens`, `Tracker::new`, query methods | Use tempfile for SQLite |
| `config.rs` | `Config::default`, config parsing | Test default values and TOML parsing |
| `deps.rs` | Dependency file parsing | Test with sample Cargo.toml/package.json strings |
| `summary.rs` | Output type detection heuristics | Pure string analysis |

### Low Priority (heavy I/O, CLI wiring)

| Module | Testable Functions | Notes |
|--------|-------------------|-------|
| `container.rs` | Docker/kubectl output filters | Requires mocking Command output |
| `find_cmd.rs` | Directory grouping logic | Filesystem-dependent |
| `wget_cmd.rs` | `compact_url`, `format_size`, `truncate_line`, `extract_filename_from_output` | Some pure helpers worth testing |
| `gain.rs` | Display formatting | Depends on tracking DB |
| `init.rs` | CLAUDE.md generation | File I/O |
| `main.rs` | CLI routing | Covered by smoke tests |

## RTK Test Patterns

### Pattern 1: Filter Function (most common in RTK)

```rust
#[test]
fn test_FILTER_happy_path() {
    // Arrange: raw command output as string literal
    let input = r#"
line of noise
line with relevant data
more noise
"#;
    // Act
    let result = filter_COMMAND(input);
    // Assert: output contains expected, excludes noise
    assert!(result.contains("relevant data"));
    assert!(!result.contains("noise"));
}
```

Used in: `git.rs`, `grep_cmd.rs`, `lint_cmd.rs`, `tsc_cmd.rs`, `vitest_cmd.rs`, `pnpm_cmd.rs`, `next_cmd.rs`, `prettier_cmd.rs`, `playwright_cmd.rs`, `prisma_cmd.rs`

### Pattern 2: Pure Computation

```rust
#[test]
fn test_FUNCTION_deterministic() {
    assert_eq!(truncate("hello world", 8), "hello...");
    assert_eq!(truncate("short", 10), "short");
}
```

Used in: `gh_cmd.rs` (`truncate`), `utils.rs` (`truncate`, `format_tokens`, `format_usd`)

### Pattern 3: Validation / Security

```rust
#[test]
fn test_VALIDATOR_rejects_injection() {
    assert!(!is_valid("malicious; rm -rf /"));
    assert!(!is_valid("../../../etc/passwd"));
}
```

Used in: `pnpm_cmd.rs` (`is_valid_package_name`)

### Pattern 4: ANSI Stripping

```rust
#[test]
fn test_strip_ansi() {
    let input = "\x1b[32mgreen\x1b[0m normal";
    let output = strip_ansi(input);
    assert_eq!(output, "green normal");
    assert!(!output.contains("\x1b["));
}
```

Used in: `vitest_cmd.rs`, `utils.rs`

## Test Skeleton Template

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_FUNCTION_happy_path() {
        // Arrange
        let input = r#"..."#;
        // Act
        let result = FUNCTION(input);
        // Assert
        assert!(result.contains("expected"));
        assert!(!result.contains("noise"));
    }

    #[test]
    fn test_FUNCTION_empty_input() {
        let result = FUNCTION("");
        assert!(...);
    }

    #[test]
    fn test_FUNCTION_edge_case() {
        // Boundary conditions: very long input, special chars, unicode
    }
}
```
