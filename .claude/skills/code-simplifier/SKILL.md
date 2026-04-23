---
name: code-simplifier
description: Review RTK Rust code for idiomatic simplification. Detects over-engineering, unnecessary allocations, verbose patterns. Applies Rust idioms without changing behavior.
triggers:
  - "simplify"
  - "too verbose"
  - "over-engineered"
  - "refactor this"
  - "make this idiomatic"
allowed-tools:
  - Read
  - Grep
  - Glob
  - Edit
effort: low
tags: [rust, simplify, refactor, idioms, rtk]
---

# RTK Code Simplifier

Review and simplify Rust code in RTK while respecting the project's constraints.

## Constraints (never simplify away)

- `lazy_static!` regex — cannot be moved inside functions even if "simpler"
- `.context()` on every `?` — verbose but mandatory
- Fallback to raw command — never remove even if it looks like dead code
- Exit code propagation — never simplify to `Ok(())`
- `#[cfg(test)] mod tests` — never remove test modules

## Simplification Patterns

### 1. Iterator chains over manual loops

```rust
// ❌ Verbose
let mut result = Vec::new();
for line in input.lines() {
    let trimmed = line.trim();
    if !trimmed.is_empty() && trimmed.starts_with("error") {
        result.push(trimmed.to_string());
    }
}

// ✅ Idiomatic
let result: Vec<String> = input.lines()
    .map(|l| l.trim())
    .filter(|l| !l.is_empty() && l.starts_with("error"))
    .map(str::to_string)
    .collect();
```

### 2. String building

```rust
// ❌ Verbose push loop
let mut out = String::new();
for (i, line) in lines.iter().enumerate() {
    out.push_str(line);
    if i < lines.len() - 1 {
        out.push('\n');
    }
}

// ✅ join
let out = lines.join("\n");
```

### 3. Option/Result chaining

```rust
// ❌ Nested match
let result = match maybe_value {
    Some(v) => match transform(v) {
        Ok(r) => r,
        Err(_) => default,
    },
    None => default,
};

// ✅ Chained
let result = maybe_value
    .and_then(|v| transform(v).ok())
    .unwrap_or(default);
```

### 4. Struct destructuring

```rust
// ❌ Repeated field access
fn process(args: &MyArgs) -> String {
    format!("{} {}", args.command, args.subcommand)
}

// ✅ Destructure
fn process(&MyArgs { ref command, ref subcommand, .. }: &MyArgs) -> String {
    format!("{} {}", command, subcommand)
}
```

### 5. Early returns over nesting

```rust
// ❌ Deeply nested
fn filter(input: &str) -> Option<String> {
    if !input.is_empty() {
        if let Some(line) = input.lines().next() {
            if line.starts_with("error") {
                return Some(line.to_string());
            }
        }
    }
    None
}

// ✅ Early return
fn filter(input: &str) -> Option<String> {
    if input.is_empty() { return None; }
    let line = input.lines().next()?;
    if !line.starts_with("error") { return None; }
    Some(line.to_string())
}
```

### 6. Avoid redundant clones

```rust
// ❌ Unnecessary clone
fn filter_output(input: &str) -> String {
    let s = input.to_string();  // Pointless clone
    s.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}

// ✅ Work with &str
fn filter_output(input: &str) -> String {
    input.lines().filter(|l| !l.is_empty()).collect::<Vec<_>>().join("\n")
}
```

### 7. Use `if let` for single-variant match

```rust
// ❌ Full match for one variant
match output {
    Ok(s) => process(&s),
    Err(_) => {},
}

// ✅ if let (but still handle errors in RTK — don't silently drop)
if let Ok(s) = output {
    process(&s);
}
// Note: in RTK filters, always handle Err with eprintln! + fallback
```

## RTK-Specific Checks

Run these after simplification:

```bash
# Verify no regressions
cargo fmt --all && cargo clippy --all-targets && cargo test

# Verify no new regex in functions
grep -n "Regex::new" src/<file>.rs
# All should be inside lazy_static! blocks

# Verify no new unwrap in production
grep -n "\.unwrap()" src/<file>.rs
# Should only appear inside #[cfg(test)] blocks
```

## What NOT to Simplify

- `lazy_static! { static ref RE: Regex = Regex::new(...).unwrap(); }` — the `.unwrap()` here is acceptable, it's init-time
- `.context("description")?` chains — verbose but required
- The fallback match arm `Err(e) => { eprintln!(...); raw_output }` — looks redundant but is the safety net
- `std::process::exit(code)` at end of run() — looks like it could be `Ok(())`but it isn't
