---
title: Cargo (Rust)
description: RTK filters for cargo — build, test, check, clippy, and nextest
sidebar:
  order: 2
---

# Cargo (Rust)

RTK filters the most verbose cargo outputs and passes through unrecognized subcommands unchanged.

## cargo test — 90% savings

```bash
rtk cargo test [args...]
```

Shows failures only. On success, shows a compact summary.

**Before (200+ lines on failure):**
```
running 15 tests
test utils::test_parse ... ok
test utils::test_format ... ok
test utils::test_edge_case ... FAILED

failures:

---- utils::test_edge_case stdout ----
thread 'utils::test_edge_case' panicked at 'assertion failed: ...'
...150 lines of backtrace...
```

**After (~5 lines):**
```
FAILED: 2/15 tests
  test_edge_case: assertion failed at utils.rs:42
  test_overflow: panic at utils.rs:18
[full output: ~/.local/share/rtk/tee/cargo_test_1234.log]
```

The tee file path lets you (or your AI assistant) read the full output if needed without re-running the command.

## cargo nextest — failures only

```bash
rtk cargo nextest [run|list|--lib] [args...]
```

Same behavior as `cargo test` — filters to failures only.

## cargo build — 80% savings

```bash
rtk cargo build [args...]
```

Removes all "Compiling..." lines, keeps errors and the final result.

**Before:**
```
   Compiling proc-macro2 v1.0.79
   Compiling unicode-ident v1.0.12
   Compiling quote v1.0.35
   ...200 crate lines...
   Compiling rtk v0.28.0
    Finished release [optimized] target(s) in 12.34s
```

**After:**
```
Finished release [optimized] in 12.34s
```

## cargo check — 80% savings

```bash
rtk cargo check [args...]
```

Removes "Checking..." lines, keeps errors.

## cargo clippy — 80% savings

```bash
rtk cargo clippy [args...]
```

Groups warnings by lint rule.

**Before (50 lines for 3 warnings):**
```
warning: unused variable `x`
  --> src/main.rs:42:9
   |
42 |     let x = 5;
   |         ^ help: if this is intentional, prefix it with an underscore: `_x`
   = note: `#[warn(unused_variables)]` on by default
...
```

**After:**
```
src/main.rs — 2 warnings
  unused_variables (x2): src/main.rs:42, src/main.rs:67
```

## cargo install

```bash
rtk cargo install [args...]
```

Removes dependency compilation noise, keeps the install result and any errors.

## Generic test wrapper

```bash
rtk test <command...>
```

Runs any test command and shows failures only. Works with any test runner:

```bash
rtk test cargo test
rtk test npm test
rtk test bun test
rtk test pytest
```

## Error-only wrapper

```bash
rtk err <command...>
```

Runs any command and shows errors and warnings only:

```bash
rtk err cargo build
rtk err npm run build
```
