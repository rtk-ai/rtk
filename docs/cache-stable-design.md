# Design: `--stable` cache-prefix alignment

Status: proposal (for review before implementation)
Target: upstream PR to `rtk-ai/rtk` (good-faith), branch off `develop`, no rebrand commits.

## Problem

LLM providers cache the KV of the longest common **prefix** across consecutive
requests. When an agent puts rtk's filtered output into its context, machine-
and run-specific noise in that output (absolute paths, volatile ordering,
timestamps) changes the bytes between otherwise-identical runs. That shrinks the
cacheable prefix, so more input tokens get recomputed and re-charged.

rtk already reduces token **count**. A stable-output mode additionally improves
cache **hit rate** - and cached input tokens cost a fraction of fresh ones, so
the savings compound.

## Goal

A deterministic, opt-in mode that makes rtk output byte-identical for the same
logical input across machines, working directories, and runs - without changing
what information is conveyed.

## Non-goals

- Not lossy. Must not drop info an agent needs, so timestamp **elision** is out
  by default (it can hide real differences).
- No per-command bespoke logic where avoidable - prefer one cross-cutting pass.

## Scope - PR #1 (small, defensible): path normalization

Global `--stable` flag + `[cache] stable = false` config (env `RTK_STABLE=1`).
When enabled, rtk applies a final, deterministic transform to rendered output:

1. **cwd -> `.`** - replace the absolute current-working-dir prefix with `.`
2. **home -> `~`** - replace the absolute home-dir prefix with `~`

Applied in that order (cwd may live inside home; rewriting cwd first avoids
emitting `~/proj` where `.` is wanted). Both are pure literal-prefix
substitutions on the final string, in one place in `core::runner`, so every
command benefits without per-command changes. `tee::display_path` already does
the home->`~` rewrite for hints; this generalizes the idea to all output.

Sorting unordered collections (the other big stabilizer) is **deferred to a
follow-up PR** - it is per-command and touches existing snapshots; keeping PR #1
to path normalization makes it easy to review and merge.

## Where it plugs in

- `core::config`: add `CacheConfig { stable: bool }` (`[cache]` section), default
  false. Mirrors the existing `TeeConfig` pattern.
- `main.rs`: add a global `--stable` flag (alongside `-u/--ultra-compact`) and an
  `RTK_STABLE` env override.
- new `core::stable`: `normalize_paths(s: &str) -> String` using
  `std::env::current_dir()` and `dirs::home_dir()`, longest-prefix-first.
- `core::runner`: in the central print path, if stable is enabled, pass output
  through `stable::normalize_paths` before emitting.

## Determinism details

- Replace cwd first, then home.
- Literal-prefix replacement of concrete absolute machine paths (safe; these are
  real paths rtk emits, not arbitrary text).
- Idempotent: applying twice == once.
- Windows separators: MVP handles the native separator rtk emits; mixed `/`+`\`
  normalization is a noted follow-up.

## Testing

- `core::stable` unit tests: home->`~`; cwd->`.`; cwd-inside-home precedence;
  no-op when disabled; idempotency.
- Integration: `rtk --stable ls .` output contains no absolute home path.
- Stability: same logical input twice -> identical bytes.

## Upstream framing (the PR)

- Title: `feat(core): add --stable mode for deterministic, cache-friendly output`
- Pitch: rtk reduces token count; `--stable` reduces token **cost** further by
  stabilizing output so provider prompt-caching keeps longer prefixes. Opt-in,
  deterministic, lossless (paths only in PR #1).
- Conventional commit, CLA signed, targets `develop`, passes
  fmt/clippy/test/security/benchmark. Sorting flagged as a planned follow-up so
  reviewers see the bounded scope.

## Open questions

- Rewriting cwd->`.` could confuse an agent that needs absolute paths - but the
  mode is opt-in and default-off, so the user chooses.
- Should `--stable` imply anything for `tee` hints? Proposal: no - keep it to
  stdout output only in PR #1.
