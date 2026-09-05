# RTK Agent Coverage and Token Savings Implementation Plan

> **For agentic workers:** Execute task-by-task with review checkpoints. Preserve unrelated changes and keep each task independently testable.

**Goal:** Make Codex and other supported workers use RTK safely, measure actual coverage, and reduce model-visible output without changing native semantics.

**Architecture:** Extend the existing command registry, semantic output model, runner, MCP adapter, hook installation, and SQLite tracker. Host adapters preserve permissions and exact output while carrying explicit audience and execution context.

**Tech Stack:** Rust CLI, SQLite, synchronous stdio MCP, native CMD/PowerShell/POSIX routes, deterministic fixtures and integration scripts.

**Spec:** `tmp/RTK_CODEX_IMPLEMENTATION_TASK.md`

## Global Constraints

- Task 1 configuration repair runs before source features.
- Preserve native exit codes, permissions, machine-readable output, and exact-output paths.
- Measure actual execution separately from rewrite eligibility and synthetic fixtures.
- Use `rtk` for eligible shell work; bypass it only for exact machine output, patches, interactive programs, or RTK diagnosis.
- Do not change unrelated profiles, providers, credentials, permissions, billing, or external repositories.

## Task sequence

1. Repair and verify the active Codex profile, absolute RTK MCP registration, instructions, and scoped model roles.
2. Establish deterministic baseline/capability fixtures and validation records.
3. Add shared output audience/context contracts and fix captured Windows CMD/PowerShell filtering.
4. Add the versioned native Codex hook, permission-preserving installer, and hook diagnostics.
5. Add doctor checks and native/CLI/SDK worker integration recipes with explicit process boundaries.
6. Extend tracking with execution identity, lifecycle events, migration coverage, and truthful session analytics.
7. Make MCP output compact, bounded, truthful, and recoverable while retaining legacy compatibility.
8. Add bounded source reads, artifact paging/search, and deterministic recovery navigation.
9. Add output-only native tool-result adapters with host schema validation and double-filter protection.
10. Complete high-value semantic route coverage and per-route diagnostics/omission contracts.
11. Add embedded build/compiler/CMake/Ninja/ESP-IDF/cppcheck routing and fixtures.
12. Measure paired baseline/candidate savings and optimize processing without semantic loss.
13. Finish installation migration, cross-platform tests, documentation, final quality gates, and completion report.

Each task must write a failing regression first, observe the expected failure, implement the smallest fix, rerun focused and full relevant tests, and record actual limitations.

## Current completion checkpoint (2026-09-05)

Tasks 1-12 have been implemented in the working tree. Task 10 now includes
bounded streaming for default human-facing `rtk rg` matches, shared semantic
runner migration for the high-value Git/Cargo/language/tool routes, and an
explicit inventory of remaining exact or compatibility paths in
`docs/validation/legacy-output-inventory.md`. The large-line regression and
semantic-route migration suites pass.

Final validation completed for this checkpoint:

- `cargo fmt --all -- --check`
- `cargo clippy --all-targets`
- `RUST_MIN_STACK=8388608 cargo test --all` on Windows: 3,357 passed, 8 ignored
- `cargo build --release`

The stack override is required for the Windows test process because the
expanded CLI parser can exceed the default test-thread stack; it is not a
runtime or release-build setting. No release, commit, merge, or PR action is
included in this checkpoint.
