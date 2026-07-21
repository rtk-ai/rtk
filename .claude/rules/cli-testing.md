# CLI Testing Strategy

Comprehensive testing rules for RTK CLI tool development.

## Unit Testing (🔴 Critical)

**Priority**: 🔴 **Triggers**: All filter changes, output format modifications

Use plain `#[cfg(test)] mod tests` block colocated in the same file as the filter,
using `assert_eq!`/`assert!` directly against expected output.

### Basic Unit Test

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_git_log_output() {
        let input = "abc1234 fix: handle empty commit\ndef5678 feat: add filter\n";
        let output = filter_git_log(input);
        assert_eq!(output, "abc1234 fix: handle empty commit\ndef5678 feat: add filter");
    }
}
```

### Fixture strategy

Two patterns coexist, pick based on what the filter needs:

1. **Inline literal strings** (most common for `src/cmds/**` unit tests) — build a small
   representative string directly in the test body. Used throughout `src/cmds/git/git.rs`,
   `src/cmds/git/gh_cmd.rs`, etc. Good for quick coverage of a specific format/edge case.
2. **Real captured fixtures via `include_str!`** — used when the raw output is large or
   format-sensitive enough that inline strings would be unreadable or drift from reality.
   `src/cmds/jvm/mvn_cmd.rs` is the reference example (23+ `include_str!` fixtures). Fixtures
   for these live in `tests/fixtures/` (e.g. `tests/fixtures/mvn_test_pass_slice_raw.txt`,
   `tests/fixtures/gradlew_build_raw.txt`, `tests/fixtures/glab_mr_list_raw.json`).

### When to Use

- **Every new filter**: cover the common case and at least one edge case (empty input, error output).
- **Output format changes**: update the relevant `assert_eq!` expectations when filter logic changes.
- **Regression detection**: prefer a real fixture (`include_str!`) over an inline string once
  output size/format makes hand-written strings brittle or unrepresentative.

### Example Workflow

```bash
# 1. Capture real output for a fixture-backed test (only needed for the include_str! pattern)
mvn test > tests/fixtures/mvn_test_example_raw.txt

# 2. Write the test in the module under test
cat >> src/cmds/jvm/mvn_cmd.rs <<'EOF'
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_mvn_test_example() {
        let input = include_str!("../../../tests/fixtures/mvn_test_example_raw.txt");
        let output = filter_mvn_test(input);
        assert!(output.contains("FAILED") || output.contains("PASSED"));
    }
}
EOF

# 3. Run the test
cargo test test_mvn_test_example
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
        let input = "..."; // inline string or include_str! fixture
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

### Savings Target

There is a single enforced floor, not a per-filter table: **≥60% savings is the release
blocker** (see CLAUDE.md's "Pre-commit Gate" / performance targets). Individual filters often
exceed this by a wide margin, but don't assert specific per-command percentages (e.g. "87% for
`gh pr view`") unless you've verified the actual number against that filter's own fixtures —
asserted thresholds vary per filter and doc tables listing invented numbers rot immediately.

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

**Linux/macOS (primary)**:
```bash
cargo test  # Local testing
```

**Windows (via CI)**:
Trust GitHub Actions CI/CD pipeline or test manually if a Windows machine is available.

### Shell Differences

| Platform | Shell | Quote Escape | Path Sep |
|----------|-------|--------------|----------|
| macOS | zsh | `'single'` or `"double"` | `/` |
| Linux | bash | `'single'` or `"double"` | `/` |
| Windows | PowerShell | `` `backtick `` or `"double"` | `\` |

## Integration Tests (🟡 Important)

**Priority**: 🟡 **Triggers**: New filter, command routing changes, release preparation

Integration tests live as top-level files in `tests/` (not colocated with `src/`), e.g.
`tests/grep_context_test.rs`, `tests/grep_faithful_format_test.rs`,
`tests/guard_integration_test.rs`, `tests/search_compress_test.rs`,
`tests/search_error_test.rs`, `tests/search_faithful_test.rs`. These exercise cross-cutting
behavior (search/grep compression, guard rails, faithful formatting) rather than a single
filter module, and several of them draw on `tests/fixtures/` (real captured aws/glab/gradlew/
mvn/phpstan/dotnet output) alongside their own inline cases.

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

# 2. Run all tests, including top-level tests/*.rs integration tests
cargo test --all

# 3. Run ignored (real-process) integration tests
cargo test --ignored

# 4. Run specific test
cargo test --ignored test_real_git_log
```

### When to Run

- **Before release**: Always run integration tests.
- **After filter changes**: Verify the filter works with real command output.
- **After hook changes**: Verify Claude Code integration works.

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
│   ├── cmds/
│   │   ├── git/
│   │   │   ├── git.rs              # Filter implementation
│   │   │   │   └── #[cfg(test)] mod tests { ... }
│   │   ├── jvm/                    # gradlew, mvn — reference example for include_str! fixtures
│   │   ├── php/                    # php, artisan, phpunit, phpstan, pest, paratest, ecs, pint
│   │   └── ...
│   ├── core/                       # Shared infrastructure
│   ├── hooks/                      # Hook system
│   └── analytics/                  # Token savings analytics
├── tests/
│   ├── fixtures/                   # Real captured command output
│   │   ├── mvn_test_pass_slice_raw.txt
│   │   ├── gradlew_build_raw.txt
│   │   ├── glab_mr_list_raw.json
│   │   └── ...
│   ├── grep_context_test.rs        # Top-level integration tests
│   ├── grep_faithful_format_test.rs
│   ├── guard_integration_test.rs
│   ├── search_compress_test.rs
│   ├── search_error_test.rs
│   └── search_faithful_test.rs
```

**Best practices**:
- **Unit tests**: Embedded in module (`#[cfg(test)] mod tests`), colocated with the filter.
- **Fixtures**: Prefer inline strings for small/quick cases; use `include_str!` from
  `tests/fixtures/` (real command output) once a case gets large or format-sensitive — see
  `src/cmds/jvm/mvn_cmd.rs` for the pattern.
- **`count_tokens` helper**: currently duplicated per test module — don't assume a shared
  `tests/common/mod.rs` exists.
- **Integration**: top-level `tests/*.rs` files, some with `#[ignore]`-tagged real-process tests.

## Testing Checklist

When adding/modifying a filter:

### Implementation Phase
- [ ] Write a unit test in the filter's own `#[cfg(test)] mod tests` block (inline string, or
      `include_str!` fixture for larger/real output)
- [ ] Add a token accuracy test (verify ≥60% savings) using a locally-defined `count_tokens`
- [ ] Test cross-platform shell escaping (if applicable)

### Quality Checks
- [ ] Run `cargo test --all` (all tests pass)
- [ ] Run `cargo test --ignored` (integration tests pass)
- [ ] Benchmark startup time with `hyperfine` (<10ms)

### Before Merge
- [ ] All tests passing (`cargo test --all`)
- [ ] Token savings ≥60% verified
- [ ] Cross-platform tests passed (Linux + macOS)
- [ ] Performance benchmarks passed (<10ms startup)

### Before Release
- [ ] Integration tests passed (`cargo test --ignored`)
- [ ] Performance regression check (hyperfine comparison)
- [ ] Memory usage verified (<5MB with `time -l`)
- [ ] Cross-platform CI passed (Linux + macOS + Windows)

## Common Testing Patterns

### Pattern: Inline Fixture + Token Accuracy

**Use case**: Testing filter output format and savings for a small/synthetic case

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_output_format() {
        let input = "raw command output here";
        let output = filter_cmd(input);
        assert_eq!(output, "expected filtered output");
    }

    #[test]
    fn test_token_savings() {
        let input = "raw command output here";
        let output = filter_cmd(input);

        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 60.0, "Expected >=60% savings, got {:.1}%", savings);
    }
}
```

### Pattern: `include_str!` Fixture (real captured output)

**Use case**: Filter output is large enough or format-sensitive enough that hand-written
strings would drift from reality — see `src/cmds/jvm/mvn_cmd.rs`.

```rust
#[test]
fn test_mvn_test_pass() {
    let input = include_str!("../../../tests/fixtures/mvn_test_pass_slice_raw.txt");
    let output = filter_mvn_test(input);
    assert!(output.contains("BUILD SUCCESS"));
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

✅ **DO** assert directly on expected output
```rust
// ✅ RIGHT
let output = filter_git_log(input);
assert_eq!(output, "expected output");
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
