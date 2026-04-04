---
title: Coding Standards
description: Rust patterns, error handling rules, and anti-patterns for RTK development
sidebar:
  order: 3
---

# Coding Standards

RTK-specific Rust constraints. These override general Rust conventions.

## Non-negotiable rules

1. **No async** — Zero `tokio`, `async-std`, `futures`. Single-threaded by design. Async adds 5-10ms startup.
2. **No `.unwrap()` in production** — Use `.context("description")?`. Tests: use `.expect("reason")`.
3. **Lazy regex** — `Regex::new()` inside a function recompiles on every call. Always `lazy_static!`.
4. **Fallback pattern** — If filter fails, execute raw command unchanged. Never block the user.
5. **Exit code propagation** — `std::process::exit(code)` if underlying command fails.

## Error handling

```rust
use anyhow::{Context, Result};

// ✅ Correct
fn read_config(path: &Path) -> Result<Config> {
    let content = fs::read_to_string(path)
        .with_context(|| format!("Failed to read config: {}", path.display()))?;
    toml::from_str(&content).context("Failed to parse config TOML")
}

// ❌ Wrong — no context
fn read_config(path: &Path) -> Result<Config> {
    let content = fs::read_to_string(path)?;
    Ok(toml::from_str(&content)?)
}
```

## Fallback pattern (mandatory for all filters)

```rust
pub fn run(args: MyArgs) -> Result<()> {
    let output = execute_command("mycmd", &args.to_cmd_args())
        .context("Failed to execute mycmd")?;

    let filtered = filter_output(&output.stdout)
        .unwrap_or_else(|e| {
            eprintln!("rtk: filter warning: {}", e);
            output.stdout.clone()    // passthrough on failure
        });

    tracking::record("mycmd", &output.stdout, &filtered)?;
    print!("{}", filtered);

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }
    Ok(())
}
```

## Regex — always lazy_static

```rust
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref ERROR_RE: Regex = Regex::new(r"^error\[").unwrap();
    static ref HASH_RE: Regex = Regex::new(r"^[0-9a-f]{7,40}").unwrap();
}

// ✅ Compiled once at first use
fn is_error_line(line: &str) -> bool {
    ERROR_RE.is_match(line)
}

// ❌ Recompiles on every call
fn is_error_line(line: &str) -> bool {
    let re = Regex::new(r"^error\[").unwrap();
    re.is_match(line)
}
```

Note: `lazy_static!` with `.unwrap()` for initialization is the established RTK pattern — acceptable because a bad regex literal is a programming error caught at first use.

## Module structure

Every `*_cmd.rs` follows this pattern:

```rust
// 1. Imports
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

// 2. Args struct
pub struct MyArgs { ... }

// 3. Lazy regexes
lazy_static! { static ref MY_RE: Regex = ...; }

// 4. Public entry point
pub fn run(args: MyArgs) -> Result<()> { ... }

// 5. Private filter functions
fn filter_output(input: &str) -> Result<String> { ... }

// 6. Tests (always present)
#[cfg(test)]
mod tests {
    use super::*;
    fn count_tokens(s: &str) -> usize { s.split_whitespace().count() }
    // snapshot tests, savings tests...
}
```

## Anti-patterns

| Pattern | Problem | Fix |
|---------|---------|-----|
| `Regex::new()` in function | Recompiles every call | `lazy_static!` |
| `.unwrap()` in production | Panic breaks user workflow | `.context()?` |
| `tokio::main` or `async fn` | +5-10ms startup | Blocking I/O only |
| `Err(_) => {}` | User gets no output | Log warning + fallback |
| `println!` in filter path | Debug artifact in output | Use `eprintln!` |
| Early return without exit code | CI thinks command succeeded | `std::process::exit(code)` |
| `.clone()` of large strings in hot path | Extra allocation | Borrow with `&str` |
