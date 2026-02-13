# Feature Specification: RTK Mypy Command

**Feature Branch**: `001-mypy-cmd`
**Created**: 2026-02-13
**Status**: Draft
**Input**: User description: "Add a mypy command module to RTK that filters and compresses mypy type checker output, grouping errors by file and error code, with token savings of 75-85%. Structurally similar to tsc_cmd.rs. Includes registry/hook updates and TDD."

## User Scenarios & Testing *(mandatory)*

### User Story 1 - Filter Mypy Error Output (Priority: P1)

A developer runs `rtk mypy` on a Python project. Mypy produces verbose type-checking output with errors scattered across many files. RTK parses this output, groups errors by file (sorted by error count descending), shows a summary header with total error/file counts and top error codes, and displays every individual error with file, line number, error code, and message. The output is 75-85% smaller than raw mypy output while preserving all actionable information.

**Why this priority**: This is the core value proposition. Without error filtering and grouping, the command has no reason to exist.

**Independent Test**: Can be fully tested by passing raw mypy output strings to the filter function and asserting the structured output contains correct groupings, counts, and every error message.

**Acceptance Scenarios**:

1. **Given** mypy output with errors in multiple files, **When** the user runs `rtk mypy`, **Then** output shows a summary header ("mypy: N errors in M files"), errors grouped by file with per-file counts, and every error's line number, error code, and message.
2. **Given** mypy output with zero errors, **When** the user runs `rtk mypy`, **Then** output shows a success message ("mypy: No issues found").
3. **Given** mypy output with errors using a single repeated error code, **When** the filter runs, **Then** the "Top codes" summary line is omitted (only shown when 2+ distinct codes exist).

---

### User Story 2 - Transparent Mypy Invocation (Priority: P1)

A developer runs `rtk mypy src/` or `rtk mypy --strict --config-file mypy.ini` and all arguments are forwarded to the underlying mypy command. RTK locates and runs mypy, captures its output, filters it, tracks token savings, and preserves mypy's exit code so CI/CD pipelines can gate on type-check results.

**Why this priority**: Argument passthrough and exit code preservation are required for the command to be usable in real workflows. Without them, users cannot replace `mypy` with `rtk mypy`.

**Independent Test**: Can be tested by verifying that the command construction passes all user-provided arguments to mypy, and that the process exit code matches mypy's exit code.

**Acceptance Scenarios**:

1. **Given** the user runs `rtk mypy src/ --strict`, **When** RTK executes, **Then** mypy is invoked with arguments `src/ --strict`.
2. **Given** mypy exits with code 1 (errors found), **When** RTK finishes, **Then** RTK also exits with code 1.
3. **Given** mypy is not installed, **When** RTK tries to run it, **Then** an error message tells the user how to install mypy (e.g., "pip install mypy").

---

### User Story 3 - Discovery and Hook Integration (Priority: P2)

When a developer uses `mypy` or `python3 -m mypy` in a Claude Code session, the RTK auto-rewrite hook transparently rewrites the command to `rtk mypy`. The RTK discover module classifies `mypy` commands as "Supported" and estimates token savings. This ensures the user gets token savings without changing their habits.

**Why this priority**: Hooks and discovery are force multipliers -- they make the mypy command discoverable and automatic. But they only add value after the core filtering (P1) works.

**Independent Test**: Can be tested by running the classify_command function against `mypy` and `python3 -m mypy` inputs and asserting they return Supported with the correct RTK equivalent and savings estimate.

**Acceptance Scenarios**:

1. **Given** a command `mypy src/`, **When** the discover registry classifies it, **Then** it returns Supported with `rtk_equivalent: "rtk mypy"`, category "Build", and estimated savings 80%.
2. **Given** a command `python3 -m mypy --strict`, **When** the discover registry classifies it, **Then** it returns Supported with the same RTK equivalent.
3. **Given** the auto-rewrite hook receives `mypy src/ --strict`, **When** it processes the command, **Then** it outputs a JSON rewrite to `rtk mypy src/ --strict`.

---

### Edge Cases

- What happens when mypy produces note-level output (not errors)? Notes are informational context lines that follow errors. They should be preserved as continuation lines under the parent error, not treated as standalone errors.
- What happens when mypy output contains color/ANSI codes? ANSI escape sequences must be stripped before parsing.
- What happens when mypy produces "error:" lines without a file reference (e.g., configuration errors, import errors)? These should be displayed verbatim at the top of the output, before any grouped file errors.
- What happens when mypy output contains column numbers (e.g., `file.py:10:5: error:`)? The column number should be parsed but not displayed (line number is sufficient for navigation).

## Requirements *(mandatory)*

### Functional Requirements

- **FR-001**: System MUST parse mypy error lines in the format `file.py:LINE: error: MESSAGE [error-code]` and extract file, line number, error code, and message.
- **FR-002**: System MUST also parse the extended format `file.py:LINE:COL: error: MESSAGE [error-code]` (with column number).
- **FR-003**: System MUST group parsed errors by file, sorted by error count (most errors first).
- **FR-004**: System MUST display a summary header: "mypy: N errors in M files".
- **FR-005**: System MUST display a "Top codes" line when 2 or more distinct error codes are present, showing up to 5 codes with occurrence counts, sorted by frequency.
- **FR-006**: System MUST display every individual error with line number, error code, and message (no collapsing or truncation of error messages beyond a reasonable line length limit).
- **FR-007**: System MUST preserve continuation/note lines (indented or starting with "note:") as context under their parent error.
- **FR-008**: System MUST display file-less errors (configuration errors, import failures) verbatim before grouped output.
- **FR-009**: System MUST strip ANSI escape sequences from mypy output before parsing.
- **FR-010**: System MUST forward all user-provided arguments to the mypy command unchanged.
- **FR-011**: System MUST preserve mypy's exit code as the process exit code.
- **FR-012**: System MUST track token savings (input vs output size) in the RTK tracking database.
- **FR-013**: System MUST show a success message when mypy reports no errors.
- **FR-014**: The discover registry MUST classify `mypy` and `python3 -m mypy` commands as Supported.
- **FR-015**: The auto-rewrite hook MUST rewrite `mypy` and `python3 -m mypy` commands to `rtk mypy`.

### Key Entities

- **Mypy Error**: A single type-checking diagnostic with file path, line number, optional column, severity (error/note/warning), optional error code, and message text.
- **File Group**: A collection of mypy errors sharing the same file path, with a count of errors in that file.
- **Error Code Summary**: An aggregate count of how many times each error code appears across all files.

## Assumptions

- Mypy is installed and available on the user's PATH (either as `mypy` directly or via `python3 -m mypy`). RTK does not install or manage mypy.
- Mypy's output format is stable across versions 0.9+ (the `file:line: severity: message [code]` format has been consistent since mypy 0.9).
- The discover registry and hook patterns follow the same conventions as existing commands (ruff, pytest, pip).
- Token savings are estimated at 80% for the discover registry, based on the structural similarity to tsc (83%) adjusted for mypy's typically shorter output lines.

## Success Criteria *(mandatory)*

### Measurable Outcomes

- **SC-001**: Running `rtk mypy` on a project with 50+ type errors produces output that is 75-85% smaller than raw mypy output while preserving every individual error message, file path, line number, and error code.
- **SC-002**: All user-provided mypy arguments are forwarded correctly, including flags like `--strict`, `--config-file`, path arguments, and `--ignore-missing-imports`.
- **SC-003**: The RTK process exit code matches mypy's exit code in all cases (0 for success, 1 for errors, 2 for fatal errors).
- **SC-004**: The discover registry correctly classifies both `mypy` and `python3 -m mypy` commands as Supported with the correct RTK equivalent.
- **SC-005**: The module includes comprehensive tests following the project's TDD patterns, with test coverage for: error parsing, file grouping, success output, continuation lines, ANSI stripping, file-less errors, and edge cases.
