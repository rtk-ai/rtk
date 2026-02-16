# CLI Testing Strategy

Comprehensive testing rules for RTK CLI tool development.

## Snapshot Testing (🔴 Critical)

**Priority**: 🔴 **Triggers**: All filter changes, output format modifications

Use `insta` crate for output validation. This is the **primary testing strategy** for RTK filters.

### Basic Snapshot Test

```rust
use insta::assert_snapshot;

#[test]
fn test_git_log_output() {
    let input = include_str!("../tests/fixtures/git_log_raw.txt");
    let output = filter_git_log(input);

    // Snapshot test - will fail if output changes
    assert_snapshot!(output);
}
```

### Workflow

1. **Write test**: Add `assert_snapshot!(output);` in test
2. **Run tests**: `cargo test` (creates new snapshots on first run)
3. **Review snapshots**: `cargo insta review` (interactive review)
4. **Accept changes**: `cargo insta accept` (if output is correct)

### When to Use

- **Every new filter**: All filters must have snapshot test
- **Output format changes**: When modifying filter logic
- **Regression detection**: Catch unintended changes

### Example Workflow

```bash
# 1. Create fixture from real command
git log -20 > tests/fixtures/git_log_raw.txt

# 2. Write test with assert_snapshot!
cat > src/git.rs <<'EOF'
#[cfg(test)]
mod tests {
    use insta::assert_snapshot;

    #[test]
    fn test_git_log_format() {
        let input = include_str!("../tests/fixtures/git_log_raw.txt");
        let output = filter_git_log(input);
        assert_snapshot!(output);
    }
}
EOF

# 3. Run test (creates snapshot)
cargo test test_git_log_format

# 4. Review snapshot
cargo insta review
# Press 'a' to accept, 'r' to reject

# 5. Snapshot saved in src/snapshots/git.rs.snap
```

## Token Accuracy Testing (🔴 Critical)

**Priority**: 🔴 **Triggers**: All filter implementations, token savings claims

All filters **MUST** verify 60-90% token savings claims with real fixtures.

### Token Count Test

```rust
#[cfg(test)]
mod tests {
    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_git_log_savings() {
        let input = include_str!("../tests/fixtures/git_log_raw.txt");
        let output = filter_git_log(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "Git log filter: expected ≥60% savings, got {:.1}%",
            savings
        );
    }
}
```

### Creating Fixtures

**Use real command output**, not synthetic data:

```bash
# Capture real output
git log -20 > tests/fixtures/git_log_raw.txt
cargo test 2>&1 > tests/fixtures/cargo_test_raw.txt
gh pr view 123 > tests/fixtures/gh_pr_view_raw.txt
pnpm list > tests/fixtures/pnpm_list_raw.txt

# Then use in tests:
# let input = include_str!("../tests/fixtures/git_log_raw.txt");
```

### Savings Targets by Filter

| Filter | Expected Savings | Rationale |
|--------|------------------|-----------|
| `git log` | 80%+ | Condense commits to hash + message |
| `cargo test` | 90%+ | Show failures only |
| `gh pr view` | 87%+ | Remove ASCII art, verbose metadata |
| `pnpm list` | 70%+ | Compact dependency tree |
| `docker ps` | 60%+ | Essential fields only |

**Release blocker**: If savings drop below 60% for any filter, investigate and fix before merge.

## Cross-Platform Testing (🔴 Critical)

**Priority**: 🔴 **Triggers**: Shell escaping changes, command execution logic

RTK must work on macOS (zsh), Linux (bash), Windows (PowerShell). Shell escaping differs.

### Platform-Specific Tests

```rust
#[cfg(target_os = "windows")]
const EXPECTED_SHELL: &str = "cmd.exe";

#[cfg(target_os = "macos")]
const EXPECTED_SHELL: &str = "zsh";

#[cfg(target_os = "linux")]
const EXPECTED_SHELL: &str = "bash";

#[test]
fn test_shell_escaping() {
    let cmd = r#"git log --format="%H %s""#;
    let escaped = escape_for_shell(cmd);

    #[cfg(target_os = "windows")]
    assert_eq!(escaped, r#"git log --format=\"%H %s\""#);

    #[cfg(not(target_os = "windows"))]
    assert_eq!(escaped, r#"git log --format="%H %s""#);
}
```

### Testing Platforms

**macOS (primary)**:
```bash
cargo test  # Local testing
```

**Linux (via Docker)**:
```bash
docker run --rm -v $(pwd):/rtk -w /rtk rust:latest cargo test
```

**Windows (via CI)**:
Trust GitHub Actions CI/CD pipeline or test manually if Windows machine available.

### Shell Differences

| Platform | Shell | Quote Escape | Path Sep |
|----------|-------|--------------|----------|
| macOS | zsh | `'single'` or `"double"` | `/` |
| Linux | bash | `'single'` or `"double"` | `/` |
| Windows | PowerShell | `` `backtick `` or `"double"` | `\` |

## Integration Tests (🟡 Important)

**Priority**: 🟡 **Triggers**: New filter, command routing changes, release preparation

Integration tests execute real commands via RTK to verify end-to-end behavior.

### Real Command Execution

```rust
#[test]
#[ignore] // Run with: cargo test --ignored
fn test_real_git_log() {
    // Requires:
    // 1. RTK binary installed (cargo install --path .)
    // 2. Git repository available

    let output = std::process::Command::new("rtk")
        .args(&["git", "log", "-10"])
        .output()
        .expect("Failed to run rtk");

    assert!(output.status.success());
    assert!(!output.stdout.is_empty());

    // Verify condensed (not raw git output)
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.len() < 5000, "Output too large, filter not working");
}
```

### Running Integration Tests

```bash
# 1. Install RTK locally
cargo install --path .

# 2. Run integration tests
cargo test --ignored

# 3. Run specific test
cargo test --ignored test_real_git_log
```

### When to Run

- **Before release**: Always run integration tests
- **After filter changes**: Verify filter works with real command
- **After hook changes**: Verify Claude Code integration works

## Performance Testing (🟡 Important)

**Priority**: 🟡 **Triggers**: Performance-related changes, release preparation

RTK targets <10ms startup time and <5MB memory usage.

### Benchmark Startup Time

```bash
# Install hyperfine
brew install hyperfine  # macOS
cargo install hyperfine  # or via cargo

# Benchmark RTK vs raw command
hyperfine 'rtk git status' 'git status' --warmup 3

# Should show RTK startup <10ms
# Example output:
#   rtk git status    6.2 ms ±  0.3 ms
#   git status        8.1 ms ±  0.4 ms
```

### Memory Usage

```bash
# macOS
/usr/bin/time -l rtk git status
# Look for "maximum resident set size" - should be <5MB

# Linux
/usr/bin/time -v rtk git status
# Look for "Maximum resident set size" - should be <5000 kbytes
```

### Regression Detection

**Before changes**:
```bash
hyperfine 'rtk git log -10' --warmup 3 > /tmp/before.txt
```

**After changes**:
```bash
cargo build --release
hyperfine 'target/release/rtk git log -10' --warmup 3 > /tmp/after.txt
```

**Compare**:
```bash
diff /tmp/before.txt /tmp/after.txt
# If startup time increased >2ms, investigate
```

### Performance Targets

| Metric | Target | Verification |
|--------|--------|--------------|
| Startup time | <10ms | `hyperfine 'rtk <cmd>'` |
| Memory usage | <5MB | `time -l rtk <cmd>` |
| Binary size | <5MB | `ls -lh target/release/rtk` |

## Test Organization

**Directory structure**:

```
rtk/
├── src/
│   ├── git.rs                  # Filter implementation
│   │   └── #[cfg(test)] mod tests { ... }  # Unit tests
│   ├── snapshots/              # Insta snapshots
│   │   └── git.rs.snap         # Snapshot for git tests
├── tests/
│   ├── common/
│   │   └── mod.rs              # Shared test utilities (count_tokens)
│   ├── fixtures/               # Real command output
│   │   ├── git_log_raw.txt
│   │   ├── cargo_test_raw.txt
│   │   └── gh_pr_view_raw.txt
│   └── integration_test.rs     # Integration tests (#[ignore])
```

**Best practices**:
- **Unit tests**: Embedded in module (`#[cfg(test)] mod tests`)
- **Fixtures**: Real command output in `tests/fixtures/`
- **Snapshots**: Auto-generated in `src/snapshots/` (by insta)
- **Shared utils**: `tests/common/mod.rs` (count_tokens, helpers)
- **Integration**: `tests/` with `#[ignore]` attribute

## Testing Checklist

When adding/modifying a filter:

### Implementation Phase
- [ ] Create fixture from real command output
- [ ] Add snapshot test with `assert_snapshot!()`
- [ ] Add token accuracy test (verify ≥60% savings)
- [ ] Test cross-platform shell escaping (if applicable)

### Quality Checks
- [ ] Run `cargo test --all` (all tests pass)
- [ ] Run `cargo insta review` (review snapshots)
- [ ] Run `cargo test --ignored` (integration tests pass)
- [ ] Benchmark startup time with `hyperfine` (<10ms)

### Before Merge
- [ ] All tests passing (`cargo test --all`)
- [ ] Snapshots reviewed and accepted (`cargo insta accept`)
- [ ] Token savings ≥60% verified
- [ ] Cross-platform tests passed (macOS + Linux)
- [ ] Performance benchmarks passed (<10ms startup)

### Before Release
- [ ] Integration tests passed (`cargo test --ignored`)
- [ ] Performance regression check (hyperfine comparison)
- [ ] Memory usage verified (<5MB with `time -l`)
- [ ] Cross-platform CI passed (macOS + Linux + Windows)

## Common Testing Patterns

### Pattern: Snapshot + Token Accuracy

**Use case**: Testing filter output format and savings

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_output_format() {
        let input = include_str!("../tests/fixtures/cmd_raw.txt");
        let output = filter_cmd(input);
        assert_snapshot!(output);
    }

    #[test]
    fn test_token_savings() {
        let input = include_str!("../tests/fixtures/cmd_raw.txt");
        let output = filter_cmd(input);

        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 60.0, "Expected ≥60% savings, got {:.1}%", savings);
    }
}
```

### Pattern: Edge Case Testing

**Use case**: Testing filter robustness

```rust
#[test]
fn test_empty_input() {
    let output = filter_cmd("");
    assert_eq!(output, "");
}

#[test]
fn test_malformed_input() {
    let malformed = "not valid command output";
    let output = filter_cmd(malformed);
    // Should either:
    // 1. Return best-effort filtered output, OR
    // 2. Return original input unchanged (fallback)
    // Both acceptable - just don't panic!
    assert!(!output.is_empty());
}

#[test]
fn test_unicode_input() {
    let unicode = "commit 日本語メッセージ";
    let output = filter_cmd(unicode);
    assert!(output.contains("commit"));
}

#[test]
fn test_ansi_codes() {
    let ansi = "\x1b[32mSuccess\x1b[0m";
    let output = filter_cmd(ansi);
    // Should strip ANSI or preserve, but not break
    assert!(output.contains("Success") || output.contains("\x1b[32m"));
}
```

### Pattern: Integration Test

**Use case**: Verify end-to-end behavior

```rust
#[test]
#[ignore]
fn test_real_command_execution() {
    let output = std::process::Command::new("rtk")
        .args(&["cmd", "args"])
        .output()
        .expect("Failed to run rtk");

    assert!(output.status.success());
    assert!(!output.stdout.is_empty());

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.len() < 5000, "Output too large");
}
```

## Anti-Patterns

❌ **DON'T** test with hardcoded synthetic data
```rust
// ❌ WRONG
let input = "commit abc123\nAuthor: John";
let output = filter_git_log(input);
// Synthetic data doesn't reflect real command output
```

✅ **DO** use real command fixtures
```rust
// ✅ RIGHT
let input = include_str!("../tests/fixtures/git_log_raw.txt");
let output = filter_git_log(input);
// Real output from `git log -20`
```

❌ **DON'T** skip cross-platform tests
```rust
// ❌ WRONG - only tests current platform
#[test]
fn test_shell_escaping() {
    let escaped = escape("test");
    assert_eq!(escaped, "test");
}
```

✅ **DO** test all platforms with cfg
```rust
// ✅ RIGHT - tests all platforms
#[test]
fn test_shell_escaping() {
    let escaped = escape("test");

    #[cfg(target_os = "windows")]
    assert_eq!(escaped, "\"test\"");

    #[cfg(not(target_os = "windows"))]
    assert_eq!(escaped, "test");
}
```

❌ **DON'T** ignore performance regressions
```rust
// ❌ WRONG - no performance tracking
#[test]
fn test_filter() {
    let output = filter_cmd(input);
    assert!(!output.is_empty());
}
```

✅ **DO** benchmark and track performance
```bash
# ✅ RIGHT - benchmark before/after
hyperfine 'rtk cmd' --warmup 3 > /tmp/before.txt
# Make changes
cargo build --release
hyperfine 'target/release/rtk cmd' --warmup 3 > /tmp/after.txt
diff /tmp/before.txt /tmp/after.txt
```

❌ **DON'T** accept <60% token savings
```rust
// ❌ WRONG - no savings verification
#[test]
fn test_filter() {
    let output = filter_cmd(input);
    assert!(!output.is_empty());
}
```

✅ **DO** verify savings claims
```rust
// ✅ RIGHT - verify ≥60% savings
#[test]
fn test_token_savings() {
    let savings = calculate_savings(input, output);
    assert!(savings >= 60.0, "Expected ≥60%, got {:.1}%", savings);
}
```
