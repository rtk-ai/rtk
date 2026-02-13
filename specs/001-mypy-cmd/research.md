# Research: RTK Mypy Command

**Date**: 2026-02-13
**Status**: Complete

## Findings

### Mypy Output Format Stability

- **Decision**: Parse the standard `file:line: severity: message [code]` format.
- **Rationale**: This format has been stable since mypy 0.9 (2020). The optional column format (`file:line:col:`) was added later but is backwards-compatible. Both formats coexist in modern mypy (1.x).
- **Alternatives considered**: Parsing mypy's `--output json` flag was considered but rejected -- it would require RTK to inject flags, which conflicts with the argument passthrough design (FR-010). The text format is sufficient and avoids modifying user intent.

### Command Discovery (mypy vs python -m mypy)

- **Decision**: Try `mypy` directly first, fall back to `python3 -m mypy`.
- **Rationale**: This is the same pattern used by `pytest_cmd.rs` (lines 18-25). Users who install mypy via pip have it on PATH. Users in virtualenvs or poetry/pipx may only have it via `python -m mypy`.
- **Alternatives considered**: Only supporting `mypy` directly was considered but would miss users who haven't activated their virtualenv.

### ANSI Stripping

- **Decision**: Reuse existing `utils::strip_ansi()` (src/utils.rs:47).
- **Rationale**: Mypy colorizes output when stdout is a TTY. Since RTK captures via `Command::output()` (not a TTY), mypy typically does not emit ANSI. However, users may have `MYPY_FORCE_COLOR=1` or `--color-output` set, so stripping is a safety measure.
- **Alternatives considered**: None -- the utility already exists.

### Error Code Format

- **Decision**: Display error codes in bracket format `[error-code]` matching mypy's native format.
- **Rationale**: Mypy uses bracketed codes like `[return-value]`, `[name-defined]`, `[assignment]`. This differs from tsc which uses `TS2322` style. Using brackets preserves the code as-is for easy copy-paste into mypy configuration (`# type: ignore[error-code]`).
- **Alternatives considered**: Stripping brackets was considered but reduces utility for suppression comments.

## No Unresolved Unknowns

All technical decisions are resolved. No NEEDS CLARIFICATION items remain.
