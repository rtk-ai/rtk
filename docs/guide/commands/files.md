---
title: File System Commands
description: RTK filters for ls, read, grep, find, diff, and wc
sidebar:
  order: 3
---

# File System Commands

RTK replaces common file and search commands with compact, token-optimized equivalents.

## ls — 80% savings

```bash
rtk ls [args...]
```

Replaces `ls` and `tree` with a compact directory tree. All native `ls` flags are supported (`-l`, `-a`, `-h`, `-R`, etc.).

**Before (45 lines):**
```
drwxr-xr-x  15 user staff 480 ...
-rw-r--r--   1 user staff 1234 ...
...40 more lines...
```

**After (12 lines):**
```
my-project/
+-- src/ (8 files)
|   +-- main.rs
+-- Cargo.toml
+-- README.md
```

## read — up to 74% savings

```bash
rtk read <file> [options]
rtk read -               # read from stdin
```

Replaces `cat`, `head`, `tail` with intelligent content filtering.

| Option | Short | Default | Description |
|--------|-------|---------|-------------|
| `--level` | `-l` | `minimal` | Filtering level: `none`, `minimal`, `aggressive` |
| `--max-lines` | `-m` | unlimited | Maximum number of lines |
| `--line-numbers` | `-n` | off | Show line numbers |

**Filtering levels:**

| Level | Description | Savings |
|-------|-------------|---------|
| `none` | Raw output, no filtering | 0% |
| `minimal` | Removes excessive blank lines and comments | ~30% |
| `aggressive` | Signatures only — removes function bodies | ~74% |

**Before — `cat main.rs` (~200 lines):**
```rust
fn main() -> Result<()> {
    let config = Config::load()?;
    let data = process_data(&input);
    for item in data {
        println!("{}", item);
    }
    Ok(())
}
...
```

**After — `rtk read main.rs -l aggressive` (~50 lines):**
```rust
fn main() -> Result<()> { ... }
fn process_data(input: &str) -> Vec<u8> { ... }
struct Config { ... }
impl Config { fn load() -> Result<Self> { ... } }
```

Supported languages for filtering: Rust, Python, JavaScript, TypeScript, Go, C, C++, Java, Ruby, Shell.

## grep — 80% savings

```bash
rtk grep <pattern> [path] [options]
```

Replaces `grep` and `rg` with results grouped by file and truncated.

| Option | Short | Default | Description |
|--------|-------|---------|-------------|
| `--max-len` | `-l` | 80 | Maximum line length |
| `--max` | `-m` | 50 | Maximum number of results |
| `--context-only` | `-c` | off | Show only match context |
| `--file-type` | `-t` | all | Filter by type (ts, py, rust, etc.) |

Additional arguments are passed to `rg` (ripgrep).

**Before (20 lines):**
```
src/git.rs:45:pub fn run(...)
src/git.rs:120:fn run_status(...)
src/ls.rs:12:pub fn run(...)
src/ls.rs:25:fn run_tree(...)
```

**After (10 lines):**
```
src/git.rs
  45: pub fn run(...)
  120: fn run_status(...)
src/ls.rs
  12: pub fn run(...)
  25: fn run_tree(...)
```

## find — 80% savings

```bash
rtk find [args...]
```

Replaces `find` and `fd` with results grouped by directory. Both RTK syntax and native `find` flags (`-name`, `-type`, etc.) are supported.

**Before (30 lines):**
```
./src/main.rs
./src/git.rs
./src/config.rs
./src/tracking.rs
./src/filter.rs
./src/utils.rs
...24 more lines...
```

**After (8 lines):**
```
src/ (12 .rs)
  main.rs, git.rs, config.rs
  tracking.rs, filter.rs, utils.rs
  ...6 more
tests/ (3 .rs)
  test_git.rs, test_ls.rs, test_filter.rs
```

## tree

```bash
rtk tree [args...]
```

Proxy to native `tree` with filtered output. All native flags supported (`-L`, `-d`, `-a`, etc.).

**Savings:** ~80%

## diff — 60% savings

```bash
rtk diff <file1> <file2>
rtk diff <file1>              # stdin as second file
```

Ultra-compact diff showing only changed lines.

## wc

```bash
rtk wc [args...]
```

Replaces `wc` with compact output (removes paths and padding). All native flags supported (`-l`, `-w`, `-c`, etc.).

## smart — 95% savings

```bash
rtk smart <file>
```

Generates a 2-line technical summary of a source file using heuristics.

```bash
$ rtk smart src/tracking.rs
SQLite-based token tracking system for command executions.
Records input/output tokens, savings %, execution times with 90-day retention.
```
