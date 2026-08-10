---
name: design-patterns
description: Rust design patterns for RTK. Newtype, Builder, RAII, Trait Objects, State Machine. Applied to CLI filter modules. Use when designing new modules or refactoring existing ones.
triggers:
  - "design pattern"
  - "how to structure"
  - "best pattern for"
  - "refactor to pattern"
allowed-tools:
  - Read
  - Grep
  - Glob
effort: medium
tags: [rust, design-patterns, architecture, newtype, builder, rtk]
---

# RTK Rust Design Patterns

Patterns that apply to RTK's filter module architecture. Focused on CLI tool patterns, not web/service patterns.

## Pattern 1: Newtype (Type Safety)

Use when: wrapping primitive types to prevent misuse (command names, paths, token counts).

```rust
// Without Newtype — easy to mix up
fn track(input_tokens: usize, output_tokens: usize) { ... }
track(output_tokens, input_tokens);  // Silent bug!

// With Newtype — compile error on swap
pub struct InputTokens(pub usize);
pub struct OutputTokens(pub usize);
fn track(input: InputTokens, output: OutputTokens) { ... }
track(OutputTokens(100), InputTokens(400));  // Compile error ✅
```

```rust
// Practical RTK example: command name validation
pub struct CommandName(String);
impl CommandName {
    pub fn new(s: &str) -> Result<Self> {
        if s.contains(';') || s.contains('|') || s.contains('`') {
            anyhow::bail!("Invalid command name: shell metacharacters");
        }
        Ok(Self(s.to_string()))
    }
    pub fn as_str(&self) -> &str { &self.0 }
}
```

## Pattern 2: Builder (Complex Configuration)

Use when: a struct has 4+ optional fields, many with defaults.

```rust
#[derive(Default)]
pub struct FilterConfig {
    max_lines: Option<usize>,
    strip_ansi: bool,
    show_warnings: bool,
    truncate_at: Option<usize>,
}

impl FilterConfig {
    pub fn new() -> Self { Self::default() }
    pub fn max_lines(mut self, n: usize) -> Self { self.max_lines = Some(n); self }
    pub fn strip_ansi(mut self, v: bool) -> Self { self.strip_ansi = v; self }
    pub fn show_warnings(mut self, v: bool) -> Self { self.show_warnings = v; self }
}

// Usage — readable, no positional arg confusion
let config = FilterConfig::new()
    .max_lines(50)
    .strip_ansi(true)
    .show_warnings(false);
```

When NOT to use Builder: if the struct has 1-3 fields with obvious meaning. Over-engineering for simple cases.

## Pattern 3: State Machine (Parser/Filter Flows)

Use when: parsing multi-section output (test results, build output) where context changes behavior.

```rust
// RTK example: pytest output parsing
#[derive(Debug, PartialEq)]
enum ParseState {
    LookingForTests,
    InTestOutput,
    InFailureSummary,
    Done,
}

fn parse_pytest(input: &str) -> String {
    let mut state = ParseState::LookingForTests;
    let mut failures = Vec::new();

    for line in input.lines() {
        match state {
            ParseState::LookingForTests => {
                if line.contains("FAILED") || line.contains("ERROR") {
                    state = ParseState::InFailureSummary;
                    failures.push(line);
                }
            }
            ParseState::InFailureSummary => {
                if line.starts_with("=====") { state = ParseState::Done; }
                else { failures.push(line); }
            }
            ParseState::Done => break,
            _ => {}
        }
    }
    failures.join("\n")
}
```

## Pattern 4: Trait Object (Command Dispatch)

Use when: different command families need the same interface. Avoids massive match arms.

```rust
// Define a common interface for filters
pub trait OutputFilter {
    fn filter(&self, input: &str) -> Result<String>;
    fn command_name(&self) -> &str;
}

pub struct GitFilter;
pub struct CargoFilter;

impl OutputFilter for GitFilter {
    fn filter(&self, input: &str) -> Result<String> { filter_git(input) }
    fn command_name(&self) -> &str { "git" }
}

// RTK currently uses match-based dispatch in main.rs (simpler, no dynamic dispatch overhead)
// Trait objects are useful if filter registry becomes dynamic (e.g., TOML-loaded plugins)
```

Note: RTK's current `match` dispatch in `main.rs` is intentional — static dispatch, zero overhead. Only move to trait objects if the match arm count exceeds ~20 commands.

## Pattern 5: RAII (Resource Management)

Use when: managing resources that need cleanup (temp files, SQLite connections).

```rust
// RTK tee.rs — RAII for temp output files
pub struct TeeFile {
    path: PathBuf,
}

impl TeeFile {
    pub fn create(content: &str) -> Result<Self> {
        let path = tee_path()?;
        fs::write(&path, content)
            .with_context(|| format!("Failed to write tee file: {}", path.display()))?;
        Ok(Self { path })
    }

    pub fn path(&self) -> &Path { &self.path }
}

// No explicit cleanup needed — file persists intentionally (rotation handled separately)
// If cleanup were needed: impl Drop { fn drop(&mut self) { let _ = fs::remove_file(&self.path); } }
```

## Pattern 6: Strategy (Swappable Filter Logic)

Use when: a command has multiple filtering modes (e.g., compact vs. verbose).

```rust
pub enum FilterMode {
    Compact,    // Show only failures/errors
    Summary,    // Show counts + top errors
    Full,       // Pass through unchanged
}

pub fn apply_filter(input: &str, mode: FilterMode) -> String {
    match mode {
        FilterMode::Compact => filter_compact(input),
        FilterMode::Summary => filter_summary(input),
        FilterMode::Full => input.to_string(),
    }
}
```

## Pattern 7: Extension Trait (Add Methods to External Types)

Use when: you need to add methods to types you don't own (like `&str` for RTK-specific parsing).

```rust
pub trait RtkStrExt {
    fn is_error_line(&self) -> bool;
    fn is_warning_line(&self) -> bool;
    fn token_count(&self) -> usize;
}

impl RtkStrExt for str {
    fn is_error_line(&self) -> bool {
        self.starts_with("error") || self.contains("[E")
    }
    fn is_warning_line(&self) -> bool {
        self.starts_with("warning")
    }
    fn token_count(&self) -> usize {
        self.split_whitespace().count()
    }
}

// Usage
if line.is_error_line() { ... }
let tokens = output.token_count();
```

## RTK Pattern Selection Guide

| Situation | Pattern | Avoid |
|-----------|---------|-------|
| New `*_cmd.rs` filter module | Standard module pattern (see CLAUDE.md) | Over-abstracting |
| 4+ optional config fields | Builder | Struct literal |
| Multi-phase output parsing | State Machine | Nested if/else |
| Type-safe wrapper around string | Newtype | Raw `String` |
| Adding methods to `&str` | Extension Trait | Free functions |
| Resource with cleanup | RAII / Drop | Manual cleanup |
| Dynamic filter registry | Trait Object | Match sprawl |

## Anti-Patterns in RTK Context

```rust
// ❌ Generic over-engineering for one command
pub trait Filterable<T: CommandArgs + Send + Sync + 'static> { ... }

// ✅ Just write the function
pub fn filter_git_log(input: &str) -> Result<String> { ... }

// ❌ Singleton registry with global state
static FILTER_REGISTRY: Mutex<HashMap<String, Box<dyn Filter>>> = ...;

// ✅ Match in main.rs — simple, zero overhead, easy to trace

// ❌ Async traits for "future-proofing"
#[async_trait]
pub trait Filter { async fn apply(&self, input: &str) -> Result<String>; }

// ✅ Synchronous — RTK is single-threaded by design
pub trait Filter { fn apply(&self, input: &str) -> Result<String>; }
```
