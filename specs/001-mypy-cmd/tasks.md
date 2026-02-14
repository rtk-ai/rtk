# Tasks: RTK Mypy Command

**Input**: Design documents from `/specs/001-mypy-cmd/`
**Prerequisites**: plan.md, spec.md, research.md

**Tests**: Included (TDD explicitly requested in spec and CLAUDE.md conventions).

**Organization**: Tasks grouped by user story. US1 and US2 are both P1 but US1 (filter) must precede US2 (invocation) since the run function calls the filter. US3 (discovery/hooks) is P2.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files, no dependencies)
- **[Story]**: Which user story this task belongs to (e.g., US1, US2, US3)
- Include exact file paths in descriptions

---

## Phase 1: Setup (Wiring)

**Purpose**: Register the mypy module and command in the CLI so that `cargo build` compiles the new module.

- [x] T001 Add `mod mypy_cmd;` declaration in `src/main.rs` (after `mod local_llm;`)
- [x] T002 Add `Mypy` variant to `Commands` enum in `src/main.rs` with `#[arg(trailing_var_arg = true, allow_hyphen_values = true)] args: Vec<String>`
- [x] T003 Add `Commands::Mypy { args }` match arm in `src/main.rs` dispatching to `mypy_cmd::run(&args, cli.verbose)?`
- [x] T004 Create stub `src/mypy_cmd.rs` with `pub fn run(args: &[String], verbose: u8) -> Result<()> { todo!() }` and empty test module

**Checkpoint**: `cargo check` passes with the new module wired in (run function is a stub).

---

## Phase 2: User Story 1 - Filter Mypy Error Output (Priority: P1)

**Goal**: Parse raw mypy output and produce grouped, compact output with summary header, top error codes, and every error preserved.

**Independent Test**: Pass raw mypy output strings to `filter_mypy_output()` and assert structured output.

### Tests for User Story 1

> **Write these tests FIRST, ensure they FAIL before implementation (TDD Red phase)**

- [x] T005 [P] [US1] Write `test_filter_mypy_errors_grouped_by_file` in `src/mypy_cmd.rs` -- multi-file input, assert summary header "mypy: N errors in M files", assert file grouping with per-file counts, assert files sorted by error count descending (FR-001, FR-003, FR-004)
- [x] T006 [P] [US1] Write `test_filter_mypy_with_column_numbers` in `src/mypy_cmd.rs` -- input with `file.py:10:5: error:` format, assert line number extracted and error parsed correctly (FR-002)
- [x] T007 [P] [US1] Write `test_filter_mypy_top_codes_summary` in `src/mypy_cmd.rs` -- input with 3+ distinct error codes, assert "Top codes:" line shows up to 5 codes sorted by frequency (FR-005)
- [x] T008 [P] [US1] Write `test_filter_mypy_single_code_no_summary` in `src/mypy_cmd.rs` -- input with only one error code repeated, assert no "Top codes:" line (FR-005)
- [x] T009 [P] [US1] Write `test_filter_mypy_every_error_shown` in `src/mypy_cmd.rs` -- 3 errors in same file, assert each error message appears individually with line number and code (FR-006)
- [x] T010 [P] [US1] Write `test_filter_mypy_note_continuation` in `src/mypy_cmd.rs` -- error followed by note: line, assert note preserved as indented context under parent error (FR-007)
- [x] T011 [P] [US1] Write `test_filter_mypy_fileless_errors` in `src/mypy_cmd.rs` -- input with config/import errors (no file prefix), assert displayed verbatim before grouped output (FR-008)
- [x] T012 [P] [US1] Write `test_filter_mypy_no_errors` in `src/mypy_cmd.rs` -- input with "Success: no issues found", assert output is "mypy: No issues found" (FR-013)
- [x] T013 [P] [US1] Write `test_filter_mypy_no_file_limit` in `src/mypy_cmd.rs` -- 15 files with errors, assert all 15 files appear in output

**Checkpoint**: All 9 tests exist and FAIL (`cargo test mypy_cmd` shows 9 failures).

### Implementation for User Story 1

- [x] T014 [US1] Implement `filter_mypy_output()` in `src/mypy_cmd.rs` -- regex parsing, MypyError struct, file grouping by HashMap, error code counting, formatted output generation (FR-001 through FR-009, FR-013)

**Checkpoint**: All 9 filter tests PASS (`cargo test mypy_cmd` shows 9 passing).

---

## Phase 3: User Story 2 - Transparent Mypy Invocation (Priority: P1)

**Goal**: Wire `pub fn run()` to execute mypy, capture output, filter it, track savings, and preserve exit code.

**Independent Test**: Verified by `cargo check` (type-safe command construction) and manual invocation.

**Depends on**: Phase 2 (filter function must exist for run to call it)

### Implementation for User Story 2

- [x] T015 [US2] Implement `pub fn run()` in `src/mypy_cmd.rs` -- try `mypy` then fallback to `python3 -m mypy`, forward all args, capture stdout+stderr, strip ANSI via `utils::strip_ansi()`, call `filter_mypy_output()`, track via `tracking::TimedExecution`, exit with mypy's exit code (FR-010, FR-011, FR-012)

**Checkpoint**: `cargo build` succeeds. If mypy is installed: `cargo run -- mypy --version` works. Full pre-commit gate passes: `cargo fmt --all --check && cargo clippy --all-targets && cargo test`.

---

## Phase 4: User Story 3 - Discovery and Hook Integration (Priority: P2)

**Goal**: Auto-rewrite `mypy` commands to `rtk mypy` and classify them in the discover registry.

**Independent Test**: Registry unit tests + hook script grep assertions.

**Depends on**: Phase 1 (module must exist) but NOT on Phase 2/3.

### Tests for User Story 3

> **Write tests FIRST (TDD Red phase)**

- [x] T016 [P] [US3] Write `test_classify_mypy` in `src/discover/registry.rs` -- assert `classify_command("mypy src/")` returns `Supported { rtk_equivalent: "rtk mypy", category: "Build", estimated_savings_pct: 80.0 }` (FR-014)
- [x] T017 [P] [US3] Write `test_classify_python_m_mypy` in `src/discover/registry.rs` -- assert `classify_command("python3 -m mypy --strict")` returns `Supported` with same fields (FR-014)

**Checkpoint**: Both tests FAIL.

### Implementation for User Story 3

- [x] T018 [US3] Add mypy pattern `r"^(python3?\s+-m\s+)?mypy(\s|$)"` to `PATTERNS` array in `src/discover/registry.rs`
- [x] T019 [US3] Add corresponding `RtkRule` to `RULES` array in `src/discover/registry.rs` -- `rtk_cmd: "rtk mypy"`, `category: "Build"`, `savings_pct: 80.0`
- [x] T020 [P] [US3] Add mypy rewrite patterns to `.claude/hooks/rtk-rewrite.sh` in the Python tooling section (after ruff block): `mypy` and `python -m mypy` rewrites (FR-015)
- [x] T021 [P] [US3] Mirror the same hook changes in `hooks/rtk-rewrite.sh` (FR-015)

**Checkpoint**: Registry tests PASS. `cargo test registry` shows all existing + 2 new tests passing. Hook patterns grep-verifiable.

---

## Phase 5: Polish & Verification

**Purpose**: Full verification, documentation, and pre-commit gate.

- [x] T022 Run full pre-commit gate: `cargo fmt --all --check && cargo clippy --all-targets && cargo test`
- [x] T023 Update CLAUDE.md architecture table to add mypy_cmd.rs entry with description and token strategy
- [x] T024 Verify `PATTERNS.len() == RULES.len()` assertion still passes in registry (existing test `test_patterns_rules_length_match`)

**Checkpoint**: All tests pass, clippy clean, CLAUDE.md updated. Feature complete.

---

## Dependencies & Execution Order

### Phase Dependencies

- **Phase 1 (Setup)**: No dependencies -- start immediately
- **Phase 2 (US1 Filter)**: Depends on Phase 1 (module must compile)
- **Phase 3 (US2 Invocation)**: Depends on Phase 2 (run calls filter)
- **Phase 4 (US3 Discovery)**: Depends on Phase 1 only (registry/hook are independent of filter implementation)
- **Phase 5 (Polish)**: Depends on all previous phases

### User Story Dependencies

- **US1 (Filter)**: Standalone after setup -- core value
- **US2 (Invocation)**: Depends on US1 (run function calls filter_mypy_output)
- **US3 (Discovery/Hooks)**: Independent of US1/US2 (registry + hooks don't import mypy_cmd)

### Parallel Opportunities

**Within Phase 2 (US1 Tests)**:
All 9 test tasks (T005-T013) are [P] -- they write to the same file but to independent test functions. Can be written in a single batch.

**Within Phase 4 (US3)**:
- T016 + T017 (registry tests) are [P] with T020 + T021 (hook changes) -- different files
- T018 + T019 must be sequential (same array in same file)

**Cross-phase**:
- Phase 4 (US3) can run in parallel with Phase 2 (US1) after Phase 1 completes -- different files entirely

---

## Notes

- All tasks in a single file should be done sequentially to avoid conflicts
- TDD is mandatory per project CLAUDE.md -- write tests first, verify they fail, then implement
- The filter function is pure (no I/O) making it fully unit-testable
- The run function follows the exact pattern of tsc_cmd.rs and pytest_cmd.rs
- Registry pattern+rule arrays must stay aligned (existing test enforces this)
