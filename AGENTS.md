# Repository Guidelines

## Project Structure & Module Organization

This repository is a Rust CLI project for `rtk`, a command proxy that compresses tool output for LLM agents. The entry point is `src/main.rs`. Core reusable logic lives in `src/core`, command implementations in `src/cmds`, rewrite/discovery logic in `src/discover`, hook installation and runtime code in `src/hooks`, and analytics in `src/analytics`. Command domains are split under `src/cmds/{git,js,python,rust,system,...}`. Agent hook templates and awareness files live in `hooks/`. Test fixtures are under `tests/fixtures`; ad hoc validation scripts belong in `scripts/` or `scratch/`.

## Build, Test, and Development Commands

- `cargo build` builds the debug binary.
- `cargo build --release` builds the optimized CLI used for local smoke tests.
- `cargo test -- --test-threads=1` runs the full suite serially; prefer this on Windows to reduce process pressure.
- `cargo test hooks::init -- --test-threads=1 --nocapture` runs focused hook installer tests.
- `cargo test cmds::system::read -- --test-threads=1 --nocapture` validates `rtk read` behavior.
- `rtk init -g --show` and `rtk init -g --codex --show` inspect installed Claude/Codex hook state.

## Coding Style & Naming Conventions

Use Rust 2021 idioms and keep code formatted with `cargo fmt`. `Cargo.toml` denies `unsafe_code` and warnings, so fix warnings instead of suppressing them. Prefer small pure formatter/filter functions with unit tests. Use snake_case for functions/modules, PascalCase for types/enums, and explicit names such as `filter_git_status_output` or `rewrite_powershell_cmdlet`.

## Testing Guidelines

Add focused unit tests near the changed module and fixture-based tests when parsing real command output. For hook work, cover Bash, Shell, and PowerShell matchers. For compression changes, test both token savings and safety: preserve exit codes, retain key signals, and fall back to raw output on unsupported or failed parsing.

For deterministic bugs, follow TDD: reproduce the exact command against the installed and source binaries and compare native or `rtk proxy` behavior first. Once RTK ownership is confirmed, add a minimal regression test that fails for the observed reason (RED), then make the smallest production change that passes it (GREEN). Re-run the new test, the historical tests for the same issue area, and the original command matrix before the full gates. Never weaken or remove a correct older assertion to make a new fix pass. Treat host policy, third-party tooling, Prompt/UX/model behavior, and performance uncertainty as bounded attribution or benchmark work unless a stable deterministic contract exists.

## Commit & Pull Request Guidelines

Recent history uses conventional prefixes such as `fix:`, `feat:`, and `perf:`. Keep commit subjects imperative and scoped, for example `fix: preserve git failure exit code` or `perf: auto-window large read output`. PRs should describe the user-visible behavior, include validation commands, note Windows-specific effects, and link any relevant issue or evidence directory.

## Agent-Specific Instructions

Read `RTK.md` before running repository commands in an agent session. Do not manually prefix every command with `rtk`; installed hooks should rewrite supported commands automatically. When validating hooks, separate config checks, direct hook JSON tests, real Claude/Codex instance smoke tests, and `rtk gain` evidence.

When `.codegraph/` exists and the task involves symbols, call paths, impact, or test selection, use `D:\tools\codegraph\codegraph.cmd`. On first use in a session, run `upgrade --check`; install an available update with `upgrade`, then refresh the workspace index with `sync <repo-path>`. Use `explore`, `node`, or `impact` for discovery, and treat `affected` only as a candidate-test filter rather than a replacement for the repository gates.

## RTK Bug Maintenance Contract

When asked to fix RTK bugs, use the active Windows worktree and the issue evidence under `D:\AI\RTK\RTK_run_log`:

1. Sync `origin/develop` into the Windows feature branch without discarding Windows-native hook behavior; create a backup ref before resolving a non-trivial merge.
2. Reproduce against the currently installed `rtk.exe` and the updated source binary. Record the binary path, version, SHA256, original command, rewrite output, exit code, and relevant host-hook output.
3. Separate RTK rewrite/runner failures from Codex host policy, third-party package-manager, shell, or environment failures. Add an RTK regression test before changing code; move disproven reports to `RTK_run_log/archive/not-rtk/` with current evidence instead of weakening RTK safety rules. Move verified RTK fixes to `RTK_run_log/archive/fixed/`; the log root contains unresolved reports only.
4. Prefer Windows semantics when behavior differs: preserve PowerShell quoting, PATHEXT command resolution, lifecycle `PATH`, exact command arguments, host permission flow, and native exit codes. Keep Linux/macOS behavior intact.
5. Run focused regression tests first, then `cargo fmt --all -- --check`, `cargo clippy --all-targets -- -D warnings`, serial full tests on Windows, `git diff --check`, and a release build. Use `scripts/rtk-windows-oracle.ps1` for the aggregated Windows gate, and re-test installed Claude/Codex hooks separately from direct `rtk rewrite` probes. Keep generated oracle artifacts under ignored `target/`, not in commits.
6. Update this file and `CLAUDE.md` together when the maintenance workflow changes. Publish only scoped files, push the Windows branch, and update or open the upstream PR with root cause, Windows impact, and exact validation evidence. For the Windows PR release gate, attach current real `rtk gain` data with a real `cmd.exe` screenshot, enumerate every active-log resolution, and report PR/source, installed binary, signature, and upstream review/check status as separate outcomes.

@RTK.md
