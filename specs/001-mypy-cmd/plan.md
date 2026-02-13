# Implementation Plan: RTK Mypy Command

**Branch**: `001-mypy-cmd` | **Date**: 2026-02-13 | **Spec**: [spec.md](spec.md)
**Input**: Feature specification from `/specs/001-mypy-cmd/spec.md`

## Summary

Add `rtk mypy` command that filters and compresses mypy type-checker output by parsing errors, grouping by file (most errors first), displaying a summary header with total counts and top error codes, and preserving every individual error. Follows the exact same structural pattern as `tsc_cmd.rs`. Includes discover registry pattern, auto-rewrite hook entry, and comprehensive TDD tests.

## Technical Context

**Language/Version**: Rust 2021 edition
**Primary Dependencies**: regex (1), lazy_static (1.4), anyhow (1.0) -- all already in Cargo.toml
**Storage**: SQLite via rusqlite (existing tracking.rs)
**Testing**: cargo test with embedded `#[cfg(test)] mod tests`
**Target Platform**: macOS, Linux (cross-platform CLI)
**Project Type**: Single Rust binary (existing)
**Performance Goals**: N/A (output filtering is string processing on small inputs)
**Constraints**: No new dependencies. Must follow existing module patterns exactly.
**Scale/Scope**: 6 files modified/created total

## Constitution Check

*No constitution file exists. Skipping gate check.*

## Project Structure

### Documentation (this feature)

```text
specs/001-mypy-cmd/
├── spec.md
├── plan.md              # This file
├── research.md          # Phase 0 output
├── checklists/
│   └── requirements.md  # Already created
└── tasks.md             # Phase 2 output (created by /speckit.tasks)
```

### Source Code (repository root)

```text
src/
├── mypy_cmd.rs          # NEW: mypy command module (filter + run)
├── main.rs              # MODIFY: add Mypy variant to Commands enum + match arm
├── discover/
│   └── registry.rs      # MODIFY: add mypy + python3 -m mypy patterns and rules

.claude/hooks/
└── rtk-rewrite.sh       # MODIFY: add mypy rewrite patterns

hooks/
└── rtk-rewrite.sh       # MODIFY: mirror of .claude/hooks/rtk-rewrite.sh
```

**Structure Decision**: This is a leaf module addition following the exact pattern of `tsc_cmd.rs`, `ruff_cmd.rs`, and `pytest_cmd.rs`. No new directories, no new dependencies, no architectural changes.

## Design

### mypy_cmd.rs -- Core Module

**Pattern**: Mirrors `tsc_cmd.rs` exactly.

**Public API**:
```
pub fn run(args: &[String], verbose: u8) -> Result<()>
```

**Internal filter function** (unit-testable):
```
fn filter_mypy_output(output: &str) -> String
```

**Mypy output format** (the input we parse):
```
src/module.py:12: error: Incompatible return value type  [return-value]
src/module.py:12:5: error: Incompatible return value type  [return-value]
src/module.py:15: note: Expected "int"
src/other.py:8: error: Name "foo" is not defined  [name-defined]
Found 3 errors in 2 files (checked 10 source files)
```

**Key patterns**:
- Error line regex: `^(.+?):(\d+)(?::(\d+))?: (error|warning|note): (.+?)(?:\s+\[(.+)\])?$`
- Continuation: `note:` lines attach to the preceding error
- File-less errors: Lines matching `error:` without a file path prefix (e.g., mypy config errors) -- display verbatim at top
- Summary line: `Found N errors in M files` -- replaced by our header

**RTK output format** (what we produce):
```
mypy: 3 errors in 2 files
=======================================
Top codes: return-value (1x), name-defined (1x)

src/module.py (2 errors)
  L12: [return-value] Incompatible return value type
    Expected "int"
  L15: [some-code] Another error

src/other.py (1 error)
  L8: [name-defined] Name "foo" is not defined
```

**Command execution flow**:
1. Try `mypy` directly via `Command::new("mypy")`
2. If not found, try `python3 -m mypy` as fallback (same pattern as pytest_cmd.rs)
3. Forward all user args
4. Capture stdout + stderr, combine
5. Strip ANSI codes via `utils::strip_ansi()`
6. Filter through `filter_mypy_output()`
7. Track via `tracking::TimedExecution`
8. Exit with mypy's exit code via `std::process::exit()`

### main.rs -- Wiring

Add to `Commands` enum (alphabetical placement near other Python tools):
```
Mypy {
    #[arg(trailing_var_arg = true, allow_hyphen_values = true)]
    args: Vec<String>,
}
```

Add match arm:
```
Commands::Mypy { args } => {
    mypy_cmd::run(&args, cli.verbose)?;
}
```

Add module declaration:
```
mod mypy_cmd;
```

### discover/registry.rs -- Discovery

Add pattern after the existing Python tool patterns (after ruff, before docker):
```
r"^(python3?\s+-m\s+)?mypy(\s|$)"
```

Add corresponding rule:
```
RtkRule {
    rtk_cmd: "rtk mypy",
    category: "Build",
    savings_pct: 80.0,
    subcmd_savings: &[],
    subcmd_status: &[],
}
```

### rtk-rewrite.sh -- Hook (both locations)

Add after the ruff rewrite block in the "Python tooling" section:
```bash
elif echo "$MATCH_CMD" | grep -qE '^mypy([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^mypy/rtk mypy/')"
elif echo "$MATCH_CMD" | grep -qE '^python[[:space:]]+-m[[:space:]]+mypy([[:space:]]|$)'; then
  REWRITTEN="${ENV_PREFIX}$(echo "$CMD_BODY" | sed 's/^python -m mypy/rtk mypy/')"
```

## File Change Summary

| File | Action | Lines (est.) | Risk |
|------|--------|-------------|------|
| `src/mypy_cmd.rs` | CREATE | ~200 | Low -- follows tsc_cmd.rs pattern |
| `src/main.rs` | MODIFY | +6 (mod decl, enum variant, match arm) | Low |
| `src/discover/registry.rs` | MODIFY | +10 (pattern + rule + test) | Low |
| `.claude/hooks/rtk-rewrite.sh` | MODIFY | +4 (two elif blocks) | Low |
| `hooks/rtk-rewrite.sh` | MODIFY | +4 (mirror) | Low |

**Total**: 1 new file, 4 modified files. ~224 new lines.

## Testing Strategy

All tests follow TDD (Red-Green-Refactor) per project conventions.

**Unit tests in mypy_cmd.rs** (embedded `#[cfg(test)] mod tests`):

| Test | Validates |
|------|-----------|
| `test_filter_mypy_errors_grouped_by_file` | FR-001, FR-003, FR-004: Multi-file errors grouped correctly |
| `test_filter_mypy_with_column_numbers` | FR-002: Extended format `file:line:col:` parsed |
| `test_filter_mypy_top_codes_summary` | FR-005: Top codes shown when 2+ distinct codes |
| `test_filter_mypy_single_code_no_summary` | FR-005: Top codes omitted with 1 code |
| `test_filter_mypy_every_error_shown` | FR-006: No error messages collapsed |
| `test_filter_mypy_note_continuation` | FR-007: note: lines preserved as context |
| `test_filter_mypy_fileless_errors` | FR-008: Config errors shown verbatim at top |
| `test_filter_mypy_no_errors` | FR-013: Success message for clean output |
| `test_filter_mypy_no_file_limit` | All files shown (mirrors tsc test) |

**Unit tests in discover/registry.rs** (added to existing test module):

| Test | Validates |
|------|-----------|
| `test_classify_mypy` | FR-014: `mypy src/` classified as Supported |
| `test_classify_python_m_mypy` | FR-014: `python3 -m mypy` classified as Supported |

## Complexity Tracking

No constitution violations. No complexity justification needed.
