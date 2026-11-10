# Selective upstream synchronization, 2026-11-10

Branch: `sync/upstream-reliability`, based on fork `develop` at
`b851d8fd92e0bd2815e9dcc973c194dd7b48b869` (including PR #1).
Reviewed upstream: `rtk-ai/rtk:develop` at
`08a261a0d081ffe11a32986e2f4d76f3fdc33abb`.

The branches share `7624c434882bf8781a0c7281b059397371341525`.
At these revisions the fork has 13 unique commits and upstream has 1397.
A full merge produces eight conflicted files and requires migration of the CLI,
recovery backend and module declarations. This branch backports independent
correctness fixes needed by the existing ya/arc implementation instead.

## Included

- Stream decoding: `a3ba3c5f`, `ae5d1aec`. Invalid UTF-8 must not stop reading
  subsequent diagnostics or failure headings. Applies to stdout, stderr and
  filtered stdin. Local adaptation terminates on persistent I/O errors and
  preserves a final carriage return when it is not part of CRLF.
- Recovery path quoting: `b1f35f6b`, `991ed971`, `5de188be`. File hints and
  overflow `tail` commands must work with spaces, shell metacharacters and
  backslashes. PR #1's unique private files and physical-line coordinates stay
  in place.
- Compact diff correctness: `4e811fbb`, `60ec79fc`, `5d4b5492`, `844d6fa3`,
  `eac9b996`, `e59276e5`, `7409cb56`, `75dda6c0`. Keep repeated-sign content,
  respect hunk ranges, handle combined diff marker columns and quoted paths,
  and preserve adjacent context. Related word-diff passthrough and GitHub
  `--patch` handling are included with their upstream tests.

Each upstream commit was cherry-picked with `-x` to preserve provenance.
The shared diff parser emits column-zero bodies for Git. Arc uses an indented
rendering of the same parser because its second compaction pass distinguishes
file paths from hunk records by indentation. Arc retains its three-change-line
cap and multi-file stat mode; repeated `+++`/`---` inside hunks are content.

## Deferred

- SQLite retriever / `rtk recall`: a separate product and storage migration.
  Existing recovery already provides saved logs and exact failure coordinates.
- CLI move to `lib.rs`, explicit modules replacing automod, Rust 2024, engine,
  pipeline and proxy work: not dependencies of the included fixes.
- New language filters and broad argv-parser changes: no direct requirement
  for this synchronization; they need their own scope and regression checks.
- Signal-forwarding changes: useful follow-up, but should be ported as their
  complete series (flush, inherited SIG_IGN and signal termination semantics),
  with subprocess tests. Not implicitly covered by the decoding backport.

This is a selective backport, not a merge of upstream. GitHub will continue to
report divergence and full-merge conflicts; that is expected.

## Verification

Targeted unit suites: Yandex (89 passed), stream (45 passed), tee (29 passed),
and Git/GitHub/GitLab (301 passed, 3 ignored). Source-location integration suite:
10 passed, including a child that emits invalid UTF-8 before a failing test.
Regression checks cover persistent read errors, unterminated CR, Arc repeated
sign content, Arc fixture compression, and the upstream diff edge cases.

`cargo clippy --locked --all-targets`, `cargo fmt --all --check`, and
`git diff --check` passed. The complete integration run
(`cargo test --locked --test '*' --no-fail-fast`) finished with 67 passed and
one failure: `search_error_test::no_match_stays_exit_1_and_empty`. The same
failure was reproduced on the PR #1 tree before these backports: stderr contains
`No hook installed — run rtk init -g` while the test expects it to be empty.
The full unit suite was not run; the targeted unit counts above are the scope
of unit verification.
