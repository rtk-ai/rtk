---
title: Contributing Guide
description: How to contribute to RTK — design philosophy, PR process, testing, and documentation
sidebar:
  order: 1
---

# Contributing Guide

## Design philosophy

Four principles guide every RTK design decision.

### Correctness over savings

When a user explicitly requests detailed output via flags (e.g., `--nocapture`, `--verbose`, `-la`), respect that intent. Filters should be flag-aware: default output gets compressed, explicit verbose flags pass through more content.

> `rtk cargo test` shows failures only (90% savings). `rtk cargo test -- --nocapture` preserves all output.

### Transparency

RTK's output must be a valid, useful subset of the original tool's output. The filtered version should be indistinguishable from "a shorter version of the real command." Don't invent new formats or add RTK-specific headers in default output.

### Never block

If a filter fails, fall back to raw output. RTK must never prevent a command from executing. Every filter needs a fallback path. Every hook must handle malformed input gracefully and exit 0.

### Zero overhead

`<10ms` startup. No async runtime. No config file I/O on the critical path. Use `lazy_static!` for all regex. No network calls in the hot path. Benchmark before/after with `hyperfine`.

## What belongs in RTK?

**In scope:** Commands that produce text output (typically 100+ tokens) compressible 60%+ without losing essential information for the LLM.

- Test runners (vitest, pytest, cargo test, go test)
- Linters and type checkers (eslint, ruff, tsc, mypy)
- Build tools (cargo build, dotnet build, make, next build)
- VCS operations (git status/log/diff, gh pr/issue)
- Package managers (pnpm, pip, cargo install)
- File operations (ls, tree, grep, find)
- Infrastructure tools with text output (docker, kubectl, terraform)

**Out of scope:** Interactive TUIs, binary output, trivial commands, non-text output.

## TOML filter vs Rust module

| Use **TOML filter** when | Use **Rust module** when |
|--------------------------|--------------------------|
| Plain text with predictable line structure | Structured output (JSON, NDJSON) |
| Regex line filtering achieves 60%+ savings | State machine parsing needed |
| No CLI flag injection needed | Must inject flags like `--format json` |
| No cross-command routing | Routes to other commands |

## Branch naming

| Prefix | Semver | When to use |
|--------|--------|-------------|
| `fix/` | Patch | Bug fixes, filter corrections |
| `feat/` | Minor | New filters, new command support |
| `chore/` | Major | Breaking changes, API changes |

Examples: `fix/git-log-drops-merge-commits`, `feat/kubectl-pod-filter`

## PR process

1. Branch from `develop`
2. Make changes + add tests + update docs
3. Run pre-commit gate: `cargo fmt --all --check && cargo clippy --all-targets && cargo test`
4. Open PR targeting `develop`
5. Sign CLA (CLA Assistant will prompt on first PR)
6. Address review feedback
7. Maintainer merges to `develop` → eventual release to `master`

## Testing requirements

Every PR must include tests. Follow TDD (Red-Green-Refactor): write failing test first.

| Type | Location | Runner |
|------|----------|--------|
| Unit tests | `#[cfg(test)] mod tests` in each module | `cargo test` |
| Snapshot tests | `assert_snapshot!()` via `insta` | `cargo test` + `cargo insta review` |
| Smoke tests | `scripts/test-all.sh` | `bash scripts/test-all.sh` |
| Integration tests | `#[ignore]` tests | `cargo test --ignored` |

**PR testing checklist:**
- [ ] Unit tests added/updated
- [ ] Snapshot tests reviewed (`cargo insta review`)
- [ ] Token savings ≥60% verified
- [ ] `cargo fmt --all --check && cargo clippy --all-targets && cargo test` passes

## Documentation requirements

Every filter addition requires:
- Update `docs/guide/commands/<ecosystem>.md` with the new command
- Update `CHANGELOG.md` under `[Unreleased]`

For the full step-by-step checklist for adding a new command filter, see [src/cmds/README.md](https://github.com/rtk-ai/rtk/blob/master/src/cmds/README.md#adding-a-new-command-filter).

## Contributor License Agreement

All contributions require signing the CLA. CLA Assistant will post a comment on your first PR with a link to sign. You only need to sign once.
