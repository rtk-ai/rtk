---
description: CLI performance optimization - startup time, memory usage, token savings benchmarking
---

# Performance Optimization Skill

Systematic performance analysis and optimization for RTK CLI tool, focusing on **startup time (<10ms)**, **memory usage (<5MB)**, and **token savings (60-90%)**.

## When to Use

- **Automatically triggered**: After filter changes, regex modifications, or dependency additions
- **Manual invocation**: When performance degradation suspected or before release
- **Proactive**: After any code change that could impact startup time or memory

## RTK Performance Targets

| Metric | Target | Verification Method | Failure Threshold |
|--------|--------|---------------------|-------------------|
| **Startup time** | <10ms | `hyperfine 'rtk <cmd>'` | >15ms = blocker |
| **Memory usage** | <5MB resident | `/usr/bin/time -l rtk <cmd>` (macOS) | >7MB = blocker |
| **Token savings** | 60-90% | Tests with `count_tokens()` | <60% = blocker |
| **Binary size** | <5MB stripped | `ls -lh target/release/rtk` | >8MB = investigate |

## Performance Analysis Workflow

### 1. Establish Baseline

Before making any changes, capture current performance:

```bash
# Startup time baseline
hyperfine 'rtk git status' --warmup 3 --export-json /tmp/baseline_startup.json

# Memory usage baseline (macOS)
/usr/bin/time -l rtk git status 2>&1 | grep "maximum resident set size" > /tmp/baseline_memory.txt

# Memory usage baseline (Linux)
/usr/bin/time -v rtk git status 2>&1 | grep "Maximum resident set size" > /tmp/baseline_memory.txt

# Binary size baseline
ls -lh target/release/rtk | tee /tmp/baseline_binary_size.txt
```

### 2. Make Changes

Implement optimization or feature changes.

### 3. Rebuild and Measure

```bash
# Rebuild with optimizations
cargo build --release

# Measure startup time
hyperfine 'target/release/rtk git status' --warmup 3 --export-json /tmp/after_startup.json

# Measure memory usage
/usr/bin/time -l target/release/rtk git status 2>&1 | grep "maximum resident set size" > /tmp/after_memory.txt

# Check binary size
ls -lh target/release/rtk | tee /tmp/after_binary_size.txt
```

### 4. Compare Results

```bash
# Startup time comparison
hyperfine 'rtk git status' 'target/release/rtk git status' --warmup 3

# Example output:
#   Benchmark 1: rtk git status
#     Time (mean ± σ):       6.2 ms ±   0.3 ms    [User: 4.1 ms, System: 1.8 ms]
#   Benchmark 2: target/release/rtk git status
#     Time (mean ± σ):       7.8 ms ±   0.4 ms    [User: 5.2 ms, System: 2.1 ms]
#
#   Summary
#     'rtk git status' ran 1.26 times faster than 'target/release/rtk git status'

# Memory comparison
diff /tmp/baseline_memory.txt /tmp/after_memory.txt

# Binary size comparison
diff /tmp/baseline_binary_size.txt /tmp/after_binary_size.txt
```

### 5. Identify Regressions

**Startup time regression** (>15% increase or >2ms absolute):
```bash
# Profile with flamegraph
cargo install flamegraph
cargo flamegraph -- target/release/rtk git status

# Open flamegraph.svg
open flamegraph.svg
# Look for:
# - Regex compilation (should be in lazy_static init)
# - Excessive allocations
# - File I/O on startup (should be zero)
```

**Memory regression** (>20% increase or >1MB absolute):
```bash
# Profile allocations (requires nightly)
cargo +nightly build --release -Z build-std
RUSTFLAGS="-C link-arg=-fuse-ld=lld" cargo +nightly build --release

# Use DHAT for heap profiling
cargo install dhat
# Add to main.rs:
# #[global_allocator]
# static ALLOC: dhat::Alloc = dhat::Alloc;
```

**Token savings regression** (<60% savings):
```bash
# Run token accuracy tests
cargo test test_token_savings

# Example failure output:
# Git log filter: expected ≥60% savings, got 52.3%

# Fix: Improve filter condensation logic
```

## Common Performance Issues

### Issue 1: Regex Recompilation

**Symptom**: Startup time >20ms, flamegraph shows regex compilation in hot path

**Detection**:
```bash
# Flamegraph shows Regex::new() calls during execution
cargo flamegraph -- target/release/rtk git log -10
# Look for "regex::Regex::new" in non-lazy_static sections
```

**Fix**:
```rust
// ❌ WRONG: Recompiled on every call
fn filter_line(line: &str) -> Option<&str> {
    let re = Regex::new(r"pattern").unwrap(); // RECOMPILED!
    re.find(line).map(|m| m.as_str())
}

// ✅ RIGHT: Compiled once with lazy_static
use lazy_static::lazy_static;

lazy_static! {
    static ref LINE_PATTERN: Regex = Regex::new(r"pattern").unwrap();
}

fn filter_line(line: &str) -> Option<&str> {
    LINE_PATTERN.find(line).map(|m| m.as_str())
}
```

### Issue 2: Excessive Allocations

**Symptom**: Memory usage >5MB, many small allocations in flamegraph

**Detection**:
```bash
# DHAT heap profiling
cargo +nightly build --release
valgrind --tool=dhat target/release/rtk git status
```

**Fix**:
```rust
// ❌ WRONG: Allocates Vec for every line
fn filter_lines(input: &str) -> String {
    input.lines()
        .map(|line| line.to_string()) // Allocates String
        .collect::<Vec<_>>()
        .join("\n")
}

// ✅ RIGHT: Borrow slices, single allocation
fn filter_lines(input: &str) -> String {
    input.lines()
        .collect::<Vec<_>>() // Vec of &str (no String allocation)
        .join("\n")
}
```

### Issue 3: Startup I/O

**Symptom**: Startup time varies wildly (5ms to 50ms), flamegraph shows file reads

**Detection**:
```bash
# strace on Linux
strace -c target/release/rtk git status 2>&1 | grep -E "open|read"

# dtrace on macOS (requires SIP disabled)
sudo dtrace -n 'syscall::open*:entry { @[execname] = count(); }' &
target/release/rtk git status
sudo pkill dtrace
```

**Fix**:
```rust
// ❌ WRONG: File I/O on startup
fn main() {
    let config = load_config().unwrap(); // Reads ~/.config/rtk/config.toml
    // ...
}

// ✅ RIGHT: Lazy config loading (only if needed)
fn main() {
    // No I/O on startup
    // Config loaded on-demand when first accessed
}
```

### Issue 4: Dependency Bloat

**Symptom**: Binary size >5MB, many unused dependencies in `Cargo.toml`

**Detection**:
```bash
# Analyze dependency tree
cargo tree

# Find heavy dependencies
cargo install cargo-bloat
cargo bloat --release --crates

# Example output:
#  File  .text     Size Crate
#  0.5%   2.1%  42.3KB regex
#  0.4%   1.8%  36.1KB clap
# ...
```

**Fix**:
```toml
# ❌ WRONG: Full feature set (bloat)
[dependencies]
clap = { version = "4", features = ["derive", "color", "suggestions"] }

# ✅ RIGHT: Minimal features
[dependencies]
clap = { version = "4", features = ["derive"], default-features = false }
```

## Optimization Techniques

### Technique 1: Lazy Static Initialization

**Use case**: Regex patterns, static configuration, one-time allocations

**Implementation**:
```rust
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref COMMIT_HASH: Regex = Regex::new(r"[0-9a-f]{7,40}").unwrap();
    static ref AUTHOR_LINE: Regex = Regex::new(r"^Author: (.+)$").unwrap();
    static ref DATE_LINE: Regex = Regex::new(r"^Date: (.+)$").unwrap();
}

// All regex compiled once at startup, reused forever
```

**Impact**: ~5-10ms saved per regex pattern (if compiled at runtime)

### Technique 2: Zero-Copy String Processing

**Use case**: Filter output without allocating intermediate Strings

**Implementation**:
```rust
// ❌ WRONG: Allocates String for every line
fn filter(input: &str) -> String {
    input.lines()
        .filter(|line| !line.is_empty())
        .map(|line| line.to_string()) // Allocates!
        .collect::<Vec<_>>()
        .join("\n")
}

// ✅ RIGHT: Borrow slices, single final allocation
fn filter(input: &str) -> String {
    input.lines()
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>() // Vec<&str> (no String alloc)
        .join("\n") // Single allocation for joined result
}
```

**Impact**: ~1-2MB memory saved, ~1-2ms startup saved

### Technique 3: Minimal Dependencies

**Use case**: Reduce binary size and compile time

**Implementation**:
```toml
# Only include features you actually use
[dependencies]
clap = { version = "4", features = ["derive"], default-features = false }
serde = { version = "1", features = ["derive"], default-features = false }

# Avoid heavy dependencies
# ❌ Avoid: tokio (adds 5-10ms startup overhead)
# ❌ Avoid: full regex (use regex-lite if possible)
# ✅ Use: anyhow (lightweight error handling)
# ✅ Use: lazy_static (zero runtime overhead)
```

**Impact**: ~1-2MB binary size reduction, ~2-5ms startup saved

## Performance Testing Checklist

Before committing filter changes:

### Startup Time
- [ ] Benchmark with `hyperfine 'rtk <cmd>' --warmup 3`
- [ ] Verify <10ms mean time
- [ ] Check variance (σ) is small (<1ms)
- [ ] Compare against baseline (regression <2ms)

### Memory Usage
- [ ] Profile with `/usr/bin/time -l rtk <cmd>`
- [ ] Verify <5MB resident set size
- [ ] Compare against baseline (regression <1MB)

### Token Savings
- [ ] Run `cargo test test_token_savings`
- [ ] Verify all filters achieve ≥60% savings
- [ ] Check real fixtures used (not synthetic)

### Binary Size
- [ ] Check `ls -lh target/release/rtk`
- [ ] Verify <5MB stripped binary
- [ ] Run `cargo bloat --release --crates` if >5MB

## Continuous Performance Monitoring

### Pre-Commit Hook

Add to `.claude/hooks/bash/pre-commit-performance.sh`:

```bash
#!/bin/bash
# Performance regression check before commit

echo "🚀 Running performance checks..."

# Benchmark startup time
CURRENT_TIME=$(hyperfine 'rtk git status' --warmup 3 --export-json /tmp/perf.json 2>&1 | grep "Time (mean" | awk '{print $4}')

# Extract numeric value (remove "ms")
CURRENT_MS=$(echo $CURRENT_TIME | sed 's/ms//')

# Check if > 10ms
if (( $(echo "$CURRENT_MS > 10" | bc -l) )); then
    echo "❌ Startup time regression: ${CURRENT_MS}ms (target: <10ms)"
    exit 1
fi

# Check binary size
BINARY_SIZE=$(ls -l target/release/rtk | awk '{print $5}')
MAX_SIZE=$((5 * 1024 * 1024))  # 5MB

if [ $BINARY_SIZE -gt $MAX_SIZE ]; then
    echo "❌ Binary size regression: $(($BINARY_SIZE / 1024 / 1024))MB (target: <5MB)"
    exit 1
fi

echo "✅ Performance checks passed"
```

### CI/CD Integration

Add to `.github/workflows/ci.yml`:

```yaml
- name: Performance Regression Check
  run: |
    cargo build --release
    cargo install hyperfine

    # Benchmark startup time
    hyperfine 'target/release/rtk git status' --warmup 3 --max-runs 10

    # Check binary size
    BINARY_SIZE=$(ls -l target/release/rtk | awk '{print $5}')
    MAX_SIZE=$((5 * 1024 * 1024))
    if [ $BINARY_SIZE -gt $MAX_SIZE ]; then
      echo "Binary too large: $(($BINARY_SIZE / 1024 / 1024))MB"
      exit 1
    fi
```

## Performance Optimization Priorities

**Priority order** (highest to lowest impact):

1. **🔴 Lazy static regex** (5-10ms per pattern if compiled at runtime)
2. **🔴 Remove startup I/O** (10-50ms for config file reads)
3. **🟡 Zero-copy processing** (1-2MB memory, 1-2ms startup)
4. **🟡 Minimal dependencies** (1-2MB binary, 2-5ms startup)
5. **🟢 Algorithm optimization** (varies, measure first)

**When in doubt**: Profile first with `flamegraph`, then optimize the hottest path.

## Tools Reference

| Tool | Purpose | Command |
|------|---------|---------|
| **hyperfine** | Benchmark startup time | `hyperfine 'rtk <cmd>' --warmup 3` |
| **time** | Memory usage (macOS) | `/usr/bin/time -l rtk <cmd>` |
| **time** | Memory usage (Linux) | `/usr/bin/time -v rtk <cmd>` |
| **flamegraph** | CPU profiling | `cargo flamegraph -- rtk <cmd>` |
| **cargo bloat** | Binary size analysis | `cargo bloat --release --crates` |
| **cargo tree** | Dependency tree | `cargo tree` |
| **DHAT** | Heap profiling | `cargo +nightly build && valgrind --tool=dhat` |
| **strace** | System call tracing (Linux) | `strace -c target/release/rtk <cmd>` |
| **dtrace** | System call tracing (macOS) | `sudo dtrace -n 'syscall::open*:entry'` |

**Install tools**:
```bash
# macOS
brew install hyperfine

# Linux / cross-platform via cargo
cargo install hyperfine
cargo install flamegraph
cargo install cargo-bloat
```
