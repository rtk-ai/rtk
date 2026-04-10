# Parser Infrastructure

> See also [docs/contributing/TECHNICAL.md](../../docs/contributing/TECHNICAL.md) for the full architecture overview

## Overview

The parser infrastructure provides a unified, three-tier parsing system for tool outputs with graceful degradation:

- **Tier 1 (Full)**: Complete JSON parsing with all structured data
- **Tier 2 (Degraded)**: Partial parsing with warnings (fallback regex)
- **Tier 3 (Passthrough)**: Raw output truncation with error markers

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

## Usage Pattern

1. **Implement `OutputParser`** for a tool — try JSON (Tier 1), fall back to regex (Tier 2), then passthrough (Tier 3)
2. **In command module**: call `Parser::parse()`, then `data.format(FormatMode::from_verbosity(verbose))`
3. **Degradation warnings**: print `[RTK:DEGRADED]` in verbose mode, `[RTK:PASSTHROUGH]` on full fallback

See `src/parser/types.rs` for the `OutputParser` trait and `ParseResult` enum.

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
- Symbols: ✓✗⚠ pkg: ^
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

Replace direct `filter_*_output()` calls with `Parser::parse()` + `FormatMode`. Key change: add `--reporter=json` flag injection, match on `ParseResult` (Full/Degraded/Passthrough), format with `data.format(mode)`. Degraded and Passthrough tiers handle tool version changes gracefully.

## Testing

Run `cargo test parser::tests`. Each parser should have tier validation tests: assert `result.tier() == 1` for valid JSON fixtures, `tier() == 2` for regex fallback inputs, and `tier() == 3` for completely malformed output.

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
