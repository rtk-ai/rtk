# Parser Infrastructure

## Overview

The parser infrastructure provides a unified, three-tier parsing system for tool outputs with graceful degradation:

- **Tier 1 (Full)**: Complete JSON parsing with all structured data
- **Tier 2 (Degraded)**: Partial parsing with warnings (fallback regex)
- **Tier 3 (Passthrough)**: Raw output truncation with error markers

This ensures RTK **never returns false data silently** while maintaining maximum token efficiency.

## Architecture

```
┌─────────────────────────────────────────────────────────┐
│                    ToolCommand Builder                   │
│  Command::new("vitest").arg("--reporter=json")          │
└─────────────────────┬───────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────┐
│                   OutputParser<T> Trait                  │
│  parse() → ParseResult<T>                               │
│    ├─ Full(T)           - Tier 1: Complete JSON parse   │
│    ├─ Degraded(T, warn) - Tier 2: Partial with warnings │
│    └─ Passthrough(str)  - Tier 3: Truncated raw output  │
└─────────────────────┬───────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────┐
│                  Canonical Types                         │
│  TestResult, LintResult, DependencyState, BuildOutput   │
└─────────────────────┬───────────────────────────────────┘
                      │
┌─────────────────────▼───────────────────────────────────┐
│                  TokenFormatter Trait                    │
│  format_compact() / format_verbose() / format_ultra()   │
└─────────────────────────────────────────────────────────┘
```

## Usage Example

### 1. Define Tool-Specific Parser

```rust
use crate::parser::{OutputParser, ParseResult, TestResult};

struct VitestParser;

impl OutputParser for VitestParser {
    type Output = TestResult;

    fn parse(input: &str) -> ParseResult<TestResult> {
        // Tier 1: Try JSON parsing
        match serde_json::from_str::<VitestJsonOutput>(input) {
            Ok(json) => {
                let result = TestResult {
                    total: json.num_total_tests,
                    passed: json.num_passed_tests,
                    failed: json.num_failed_tests,
                    // ... map fields
                };
                ParseResult::Full(result)
            }
            Err(e) => {
                // Tier 2: Try regex extraction
                if let Some(stats) = extract_stats_regex(input) {
                    ParseResult::Degraded(
                        stats,
                        vec![format!("JSON parse failed: {}", e)]
                    )
                } else {
                    // Tier 3: Passthrough
                    ParseResult::Passthrough(truncate_output(input, 500))
                }
            }
        }
    }
}
```

### 2. Use Parser in Command Module

```rust
use crate::parser::{OutputParser, TokenFormatter, FormatMode};

pub fn run_vitest(args: &[String], verbose: u8) -> Result<()> {
    let mut cmd = Command::new("pnpm");
    cmd.arg("vitest").arg("--reporter=json");
    // ... add args

    let output = cmd.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);

    // Parse output
    let result = VitestParser::parse(&stdout);

    // Format based on verbosity
    let mode = FormatMode::from_verbosity(verbose);
    let formatted = match result {
        ParseResult::Full(data) => data.format(mode),
        ParseResult::Degraded(data, warnings) => {
            if verbose > 0 {
                for warn in warnings {
                    eprintln!("[RTK:DEGRADED] {}", warn);
                }
            }
            data.format(mode)
        }
        ParseResult::Passthrough(raw) => {
            eprintln!("[RTK:PASSTHROUGH] Parser failed, showing truncated output");
            raw
        }
    };

    println!("{}", formatted);
    Ok(())
}
```

## Canonical Types

### TestResult
For test runners (vitest, playwright, jest, etc.)
- Fields: `total`, `passed`, `failed`, `skipped`, `duration_ms`, `failures`
- Formatter: Shows summary + failure details (compact: top 5, verbose: all)

### LintResult
For linters (eslint, biome, tsc, etc.)
- Fields: `total_files`, `files_with_issues`, `total_issues`, `errors`, `warnings`, `issues`
- Formatter: Groups by rule_id, shows top violations

### DependencyState
For package managers (pnpm, npm, cargo, etc.)
- Fields: `total_packages`, `outdated_count`, `dependencies`
- Formatter: Shows upgrade paths (current → latest)

### BuildOutput
For build tools (next, webpack, vite, cargo, etc.)
- Fields: `success`, `duration_ms`, `bundles`, `routes`, `warnings`, `errors`
- Formatter: Shows bundle sizes, route metrics

## Format Modes

### Compact (default, verbosity=0)
- Summary only
- Top 5-10 items
- Token-optimized

### Verbose (verbosity=1)
- Full details
- All items (up to 20)
- Human-readable

### Ultra (verbosity=2+)
- Symbols: ✓✗⚠📦⬆️
- Ultra-compressed
- 30-50% token reduction

## Error Handling

### ParseError Types
- `JsonError`: Line/column context for debugging
- `PatternMismatch`: Regex pattern failed
- `PartialParse`: Some fields missing
- `InvalidFormat`: Unexpected structure
- `MissingField`: Required field absent
- `VersionMismatch`: Tool version incompatible
- `EmptyOutput`: No data to parse

### Degradation Warnings

```
[RTK:DEGRADED] vitest parser: JSON parse failed at line 42, using regex fallback
[RTK:PASSTHROUGH] playwright parser: Pattern mismatch, showing truncated output
```

## Migration Guide

### Existing Module → Parser Trait

**Before:**
```rust
fn run_vitest(args: &[String]) -> Result<()> {
    let output = Command::new("vitest").output()?;
    let filtered = filter_vitest_output(&output.stdout);
    println!("{}", filtered);
    Ok(())
}
```

**After:**
```rust
fn run_vitest(args: &[String], verbose: u8) -> Result<()> {
    let output = Command::new("vitest")
        .arg("--reporter=json")
        .output()?;

    let result = VitestParser::parse(&output.stdout);
    let mode = FormatMode::from_verbosity(verbose);

    match result {
        ParseResult::Full(data) | ParseResult::Degraded(data, _) => {
            println!("{}", data.format(mode));
        }
        ParseResult::Passthrough(raw) => {
            println!("{}", raw);
        }
    }
    Ok(())
}
```

## Testing

### Unit Tests
```bash
cargo test parser::tests
```

### Integration Tests
```bash
# Test with real tool outputs
echo '{"testResults": [...]}' | cargo run -- vitest parse
```

### Tier Validation
```rust
#[test]
fn test_vitest_json_parsing() {
    let json = include_str!("fixtures/vitest-v1.json");
    let result = VitestParser::parse(json);
    assert_eq!(result.tier(), 1); // Full parse
    assert!(result.is_ok());
}

#[test]
fn test_vitest_regex_fallback() {
    let text = "Test Files  2 passed (2)\n Tests  13 passed (13)";
    let result = VitestParser::parse(text);
    assert_eq!(result.tier(), 2); // Degraded
    assert!(!result.warnings().is_empty());
}
```

## Benefits

1. **Maintenance**: Tool version changes break gracefully (Tier 2/3 fallback)
2. **Reliability**: Never silent failures or false data
3. **Observability**: Clear degradation markers in verbose mode
4. **Token Efficiency**: Structured data enables better compression
5. **Consistency**: Unified interface across all tool types
6. **Testing**: Fixture-based regression tests for multiple versions

## Roadmap

### Phase 4: Module Migration
- [ ] vitest_cmd.rs → VitestParser
- [ ] playwright_cmd.rs → PlaywrightParser
- [ ] pnpm_cmd.rs → PnpmParser (list, outdated)
- [ ] lint_cmd.rs → EslintParser
- [ ] tsc_cmd.rs → TscParser
- [ ] gh_cmd.rs → GhParser

### Phase 5: Observability
- [ ] Extend tracking.db: `parse_tier`, `format_mode`
- [ ] `rtk parse-health` command
- [ ] Alert if degradation > 10%
