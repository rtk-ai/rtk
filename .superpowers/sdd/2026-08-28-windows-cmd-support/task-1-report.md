# Task 1 Report — CMD Parser and Built-in Catalog

## Status

Complete. The new `src/cmds/windows/` subsystem provides a span-preserving CMD lexer/parser, explicit opaque/fail-open classification, and a checked-in catalog of CMD intrinsic/extension commands with aliases, mode metadata, and adapter strategies.

## Commit

`feat(windows): add CMD parser and builtin catalog` (the feature branch `HEAD` at delivery)

## Files changed

- `src/cmds/mod.rs` — exposes the Windows CMD subsystem.
- `src/cmds/windows/mod.rs` — module boundary.
- `src/cmds/windows/parser.rs` — source-span parser, operator recognition, and opaque classification.
- `src/cmds/windows/catalog.rs` — built-in metadata and catalog validation.
- `src/cmds/windows/tests.rs` — table-driven parser/catalog tests.
- `docs/superpowers/plans/2026-08-28-windows-cmd-support.md` — approved implementation plan, added because it was untracked.
- `.superpowers/sdd/2026-08-28-windows-cmd-support/task-1-report.md` — this report.

## TDD evidence

### Red

Before parser or catalog production modules existed, added table-driven tests that imported `super::parser` and `super::catalog`, then ran:

```text
rtk cargo test cmds::windows::tests
cargo test: 3 errors, 1 warnings (1 crates)
error[E0432]: unresolved import `super::catalog`
error[E0432]: unresolved import `super::parser`
```

The failure was expected: the parser/catalog APIs had not been implemented.

### Green

Implemented the minimal parser and catalog required by the tests, corrected two test-only Rust ownership assertions, formatted the code, and reran:

```text
rtk cargo test cmds::windows::tests
cargo test: 8 passed, 2742 filtered out (9 suites, 0.00s)
```

## Verification commands and exact summaries

```text
rtk cargo fmt -- --check
```

Initial check reported formatting changes; after `rtk cargo fmt`, the same check completed with no output and exit code 0.

```text
rtk cargo test cmds::windows::tests
cargo test: 8 passed, 2742 filtered out (9 suites, 0.00s)

rtk cargo test --all
cargo test: 2742 passed, 8 ignored (9 suites, 4.95s)

rtk git diff --check
```

The final diff check completed with no output and exit code 0.

## Self-review

- Parser spans are byte offsets into the original source and exclude surrounding whitespace, preserving all untouched formatting for Task 2 replacement.
- The lexer observes `&`, `&&`, `||`, pipes, input/output redirections, parentheses, double quotes, caret escapes, CRLF, `%VAR%`, `!VAR!`, and `@` prefixes.
- Pipes, output redirects, parentheses, control commands, batch files, delayed expansion, drive changes, and malformed quote/caret input return an opaque reason, preventing partial rewrites.
- The catalog includes aliases, intrinsic/extension origin, query/mutation/stateful/control/interactive mode, and an explicit structured or identity strategy for every entry.
- Validation rejects duplicate canonical names/aliases and missing strategies.

## Concerns

- The parser deliberately rejects ambiguous constructs rather than reproducing CMD's undocumented edge cases; Task 2 must treat any `opaque_reason` as a full native-CMD fallback.
- The catalog is staged for Task 3's adapters: only `dir`, display-form `set`, `help`, `assoc`, and `ftype` are marked structured; all other entries intentionally preserve native behavior through identity strategies.

## Fix Round 1

### Changes

- Treats unquoted CRLF as a CMD command boundary and records it as a `LineBreak` operator, so each line has an independent replacement span.
- Preserves quoted executable command words through the closing quote before classification; quoted `.cmd` and `.bat` paths now fail open as batch invocations.
- Rejects empty operands before or after top-level `&`, `&&`, and `||` as malformed input.
- Updates the prior CRLF fixture to express two actual commands rather than a leading `&` after a line boundary, which is now correctly malformed.

### Covering tests

- `src/cmds/windows/tests.rs`: `crlf_is_a_cmd_command_boundary`, `quoted_batch_path_is_opaque`, and `empty_operands_around_chain_operators_are_malformed`.

### TDD and verification

Added the three behavioral regressions before changing parser production code, then ran:

```text
rtk cargo test cmds::windows::tests
test result: FAILED. 8 passed; 3 failed; 0 ignored; 0 measured; 2717 filtered out; finished in 0.00s
```

The failures were the expected missing CRLF split, quoted-batch detection, and malformed-empty-operand behavior. After the minimal parser changes:

```text
rtk cargo test cmds::windows::tests
cargo test: 11 passed, 2742 filtered out (9 suites, 0.00s)

rtk cargo test --all
cargo test: 2745 passed, 8 ignored (9 suites, 4.67s)

rtk git diff --check
```

The final diff check completed with no output and exit code 0.

## Fix Round 2

### Changes

- Guards the command-word lookup after a leading `@`, preventing an out-of-bounds byte index for `@` and `@ `.
- Keeps the empty post-prefix command span representable, then classifies it as `MalformedInput` so the caller fails open instead of panicking.

### Covering test

- `src/cmds/windows/tests.rs`: `bare_at_prefix_is_malformed_without_panicking` covers `@` and `@ `.

### TDD and verification

Added the behavioral regression before production changes, then ran:

```text
rtk cargo test bare_at_prefix_is_malformed_without_panicking
test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 2728 filtered out; finished in 0.00s
thread 'cmds::windows::tests::bare_at_prefix_is_malformed_without_panicking' panicked at src\cmds\windows\parser.rs:238:26:
index out of bounds: the len is 1 but the index is 1
```

After the guard and malformed classification:

```text
rtk cargo test bare_at_prefix_is_malformed_without_panicking
cargo test: 1 passed, 2753 filtered out (9 suites, 0.00s)

rtk cargo test cmds::windows::tests
cargo test: 12 passed, 2742 filtered out (9 suites, 0.00s)

rtk cargo test --all
cargo test: 2746 passed, 8 ignored (9 suites, 4.60s)

rtk git diff --check
```

The final diff check completed with no output and exit code 0.
