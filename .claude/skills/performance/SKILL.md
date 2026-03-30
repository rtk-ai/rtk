---
name: performance
description: RTK CLI performance analysis and optimization. Startup time (<10ms), binary size (<5MB), regex compilation, memory usage. Use when adding dependencies, changing initialization, or suspecting regressions.
triggers:
  - "startup time"
  - "performance regression"
  - "too slow"
  - "benchmark"
  - "binary size"
  - "memory usage"
allowed-tools:
  - Bash
  - Read
  - Grep
effort: medium
tags: [performance, benchmark, startup, binary-size, memory, rtk]
---

# RTK Performance Analysis

## Hard Targets (Non-Negotiable)

| Metric | Target | Blocker |
|--------|--------|---------|
| Startup time | <10ms | Release blocker |
| Binary size (stripped) | <5MB | Release blocker |
| Memory (resident) | <5MB | Release blocker |
| Token savings per filter | ≥60% | Release blocker |

## Benchmark Startup Time

```bash
# Install hyperfine (once)
brew install hyperfine

# Baseline (before changes)
hyperfine 'rtk git status' --warmup 3 --export-json /tmp/before.json

# After changes — rebuild first
cargo build --release

# Compare against installed
hyperfine 'target/release/rtk git status' 'rtk git status' --warmup 3

# Target: <10ms mean time
```

## Check Binary Size

```bash
# Release build with strip=true (already in Cargo.toml)
cargo build --release
ls -lh target/release/rtk
# Should be <5MB

# If too large — check what's contributing
cargo bloat --release --crates
cargo bloat --release -n 20
# Install: cargo install cargo-bloat
```

## Memory Usage

```bash
# macOS
/usr/bin/time -l target/release/rtk git status 2>&1 | grep "maximum resident"
# Target: <5,000,000 bytes (5MB)

# Linux
/usr/bin/time -v target/release/rtk git status 2>&1 | grep "Maximum resident"
# Target: <5,000 kbytes
```

## Regex Compilation Audit

Regex compilation on every function call is a common perf killer:

```bash
# Find all Regex::new calls
grep -n "Regex::new" src/*.rs

# Verify ALL are inside lazy_static! blocks
# Any Regex::new outside lazy_static! = performance bug
```

```rust
// ❌ Recompiles on every filter_line() call
fn filter_line(line: &str) -> bool {
    let re = Regex::new(r"^error").unwrap();  // BAD
    re.is_match(line)
}

// ✅ Compiled once at first use
lazy_static! {
    static ref ERROR_RE: Regex = Regex::new(r"^error").unwrap();
}
fn filter_line(line: &str) -> bool {
    ERROR_RE.is_match(line)  // GOOD
}
```

## Dependency Impact Assessment

Before adding any new crate:

```bash
# Check startup impact (measure before adding)
hyperfine 'rtk git status' --warmup 3

# Add dependency to Cargo.toml
# Rebuild
cargo build --release

# Measure after
hyperfine 'target/release/rtk git status' --warmup 3

# If startup increased >1ms — investigate
# If startup increased >3ms — reject the dependency
```

### Forbidden dependencies

| Crate | Reason | Alternative |
|-------|--------|-------------|
| `tokio` | +5-10ms startup | Blocking `std::process::Command` |
| `async-std` | +5-10ms startup | Blocking I/O |
| `rayon` | Thread pool init overhead | Sequential iteration |
| `reqwest` | Pulls tokio | `ureq` (blocking) if HTTP needed |

### Dependency weight check

```bash
# After cargo build --release
cargo build --release --timings
# Open target/cargo-timings/cargo-timing.html
# Look for crates with long compile times (correlates with complexity)
```

## Allocation Profiling

```bash
# macOS — use Instruments
instruments -t Allocations target/release/rtk git log -10

# Or use cargo-instruments
cargo install cargo-instruments
cargo instruments --release -t Allocations -- git log -10
```

Common RTK allocation hotspots:

```rust
// ❌ Allocates new String on every line
let lines: Vec<String> = input.lines().map(|l| l.to_string()).collect();

// ✅ Borrow slices
let lines: Vec<&str> = input.lines().collect();

// ❌ Clone large output unnecessarily
let raw_copy = output.stdout.clone();

// ✅ Use reference until you actually need to own
let display = &output.stdout;
```

## Token Savings Measurement

```rust
// In tests — always verify claims
fn count_tokens(text: &str) -> usize {
    text.split_whitespace().count()
}

#[test]
fn test_savings_claim() {
    let input = include_str!("../tests/fixtures/mycmd_raw.txt");
    let output = filter_output(input).unwrap();

    let input_tokens = count_tokens(input);
    let output_tokens = count_tokens(&output);
    let savings = 100.0 * (1.0 - output_tokens as f64 / input_tokens as f64);

    assert!(
        savings >= 60.0,
        "Expected ≥60% savings, got {:.1}% ({} → {} tokens)",
        savings, input_tokens, output_tokens
    );
}
```

## Before/After Regression Check

Template for any performance-sensitive change:

```bash
# 1. Baseline
cargo build --release
hyperfine 'target/release/rtk git status' --warmup 5 --export-json /tmp/before.json
/usr/bin/time -l target/release/rtk git status 2>&1 | grep "maximum resident"
ls -lh target/release/rtk

# 2. Make changes
# ... edit code ...

# 3. Rebuild and compare
cargo build --release
hyperfine 'target/release/rtk git status' --warmup 5 --export-json /tmp/after.json
/usr/bin/time -l target/release/rtk git status 2>&1 | grep "maximum resident"
ls -lh target/release/rtk

# 4. Compare
# Startup: jq '.results[0].mean' /tmp/before.json /tmp/after.json
# If after > before + 1ms: investigate
# If after > 10ms: regression, do not merge
```
