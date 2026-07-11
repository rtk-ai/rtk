# RTK Coding Practices v1.0

This document follows the [Design Philosophy](../../CONTRIBUTING.md#design-philosophy) in `CONTRIBUTING.md`. Once you understand the mental model there, this guide describes the coding practices we use day-to-day in RTK and what reviewers will look for on your PR.

Our goal is to keep the codebase consistent and easy to extend. PRs that deviate from these practices may be asked for changes during review — this is guidance, not a gate. If a rule seems wrong for your specific case, flag it in the PR and we'll discuss.

> **Heads up:** RTK has grown quickly and some code in the repository predates these practices. You may spot modules that don't fully follow them — this is expected, and core/ecosystem maintainers will refactor them over time. When in doubt, follow the practices below for new code rather than mirroring older patterns.

---

## Quick Start for Contributors

New to RTK? The fastest path to a mergeable first PR:

1. **Read the flow once.** Start at [`CONTRIBUTING.md`](../../CONTRIBUTING.md), then skim [`docs/contributing/TECHNICAL.md`](TECHNICAL.md) to see how a command flows from `main.rs` → a `*_cmd.rs` filter → tracking → stdout.
2. **Look at a good example.** [`src/cmds/git/git.rs`](../../src/cmds/git/git.rs) is a representative filter — it shows the `run()` entry point, `lazy_static!` regex setup, filter helpers, and embedded tests all in one file.
3. **Know the shared helpers before reimplementing.** Two files cover most of what you need:
   - [`src/core/runner.rs`](../../src/core/runner.rs) — command execution wrappers: `run_filtered()` (run a command, then apply your filter function), `run_passthrough()` (run unfiltered but tracked), `run_streamed()` (streaming filter).
   - [`src/core/utils.rs`](../../src/core/utils.rs) — shared utilities: `resolved_command()`, `strip_ansi()`, `truncate()`, `count_tokens()`, and more.
4. **Follow the checklist.** [`src/cmds/README.md — Adding a New Command Filter`](../../src/cmds/README.md#adding-a-new-command-filter) walks you through creating a filter, registering it, and adding tests.
5. **Write the test first.** We follow Red-Green-Refactor. A snapshot test plus a token-savings assertion (see [Testing](#testing) below) is enough for most filters.

If you're unsure whether your approach fits, open a draft PR or a discussion early — we'd rather help shape the design than ask for a rewrite at review.

---

## Design Philosophy

For the full framing (Correctness vs. Token Savings, Transparency, Never Block, Zero Overhead, Extensibility), see the [Design Philosophy](../../CONTRIBUTING.md#design-philosophy) section in `CONTRIBUTING.md`.

Two practical reminders that come up often in review:

**Portability.** RTK should behave the same across platforms. Use `#[cfg(target_os = "...")]` for platform-specific code; never assume a single OS.

**Extensibility.** RTK should be modular. Before writing a new feature or filter, check whether an existing entry point fits — `runner::run_filtered()`, `runner::run_passthrough()`, helpers in `src/core/utils.rs`, etc. If your logic could be reused elsewhere, lift it into a shared component rather than burying it in one `*_cmd.rs` file.

---

## Files, Functions, and Documentation

Each folder contains a root `README.md` that explains the main principles, flows, and specificities of the source files it owns. These READMEs should describe concepts and cases — not list individual source files or counts, to avoid stale lists as the code evolves. Because the root README reflects core features and logic, it should not change often; meaningful edits usually imply a core refactor.

Tests live in the same file as the code they test (inside `#[cfg(test)] mod tests { ... }`), not in a separate test file. This keeps the filter, its fixtures, and its assertions close together.

---

## Edge Cases

When you add an edge-case branch or a non-obvious exception, leave a short comment above it explaining *why* it exists. This prevents a future contributor from removing it because the reason isn't visible from the code alone.

Referencing an issue is often the clearest form:

```rust
// ISSUE #463: some `git log` output contains NUL bytes when --format=%x00 is used;
// skip the line rather than panicking on invalid UTF-8.
if line.contains('\0') {
    continue;
}
```

---

## Comments

Prefer code that reads clearly over code that needs comments to explain it. In particular, avoid redundant comments that restate what the function signature already says.

Comments are welcome when they add information the code cannot carry on its own. The common cases:

- **File header (`//!`)** — purpose and scope of the current file.
- **Edge case** — a non-obvious branch or exception, as described above.
- **Issue reference** — e.g. `// ISSUE #463: the fix for this`.
- **"Why, not what"** — when the intent or tradeoff behind a decision isn't obvious from the code.

In short: avoid noise comments; keep the ones that would save a future reader a trip to `git blame`.

---

## Variables

Use explicit, descriptive names for variables, just like for functions.

Do not hardcode repetitive patterns or values that control behavior — extract them into named constants at the top of the file. For anything a user might want to tune (thresholds, limits, display cutoffs), use `config::limits()` so it flows through `~/.config/rtk/config.toml`.

Example from `src/cmds/git/git.rs`:

```rust
let limits = config::limits();
let max_files = limits.status_max_files;
let max_untracked = limits.status_max_untracked;
```

---

## Function and File Size

**Prefer functions under ~60 lines.** Shorter functions are easier to read, test, and reuse. If a function grows beyond that, it's usually a sign the logic should be split into helpers — but this is a guideline, not a hard cap.

Legitimate exceptions include:
- Dispatcher / match functions that route to subcommands, where each arm delegates to a focused helper.
- State-machine parsers where splitting would harm readability.

When you keep a longer function, aim to make each block obviously cohesive — and consider leaving a short comment on *why* splitting it would hurt.

**Files are expected to be large** in RTK because each module keeps its tests and fixtures alongside the implementation. When a file becomes hard to navigate, split responsibilities across multiple files where possible. If it isn't possible, a big file is acceptable for now.

---

## Imports and Dependencies

RTK is a low-dependency project. Before adding a crate, check whether the functionality is already covered by `std`, an existing dependency, or `src/core/utils.rs`. If a few lines of straightforward code will do the job, prefer that over a new dependency.

When a new dependency is genuinely needed, justify it in the PR description. For non-trivial additions, it's worth opening a discussion with maintainers first.

---

## Error Handling

Use `anyhow::Result` everywhere, and always attach context with `.context("description")?` or `.with_context(|| format!(...))`.

Never silently swallow errors (`Err(_) => {}`). Either log with `eprintln!` and fall back to raw output (the common case for filters), or propagate the error.

Example of the standard fallback pattern for a filter:

```rust
let filtered = filter_output(&output.stdout)
    .unwrap_or_else(|e| {
        eprintln!("rtk: filter warning: {}", e);
        output.stdout.clone() // passthrough on failure — never block the user
    });
```

For the full error-handling architecture (propagation chain, exit code preservation), see [ARCHITECTURE.md — Error Handling](ARCHITECTURE.md#error-handling).

---

## Testing

See [`CONTRIBUTING.md` — Testing](../../CONTRIBUTING.md#testing) for the full strategy. In short, for a new filter you typically want:

- **Unit + snapshot tests** in the same file, using the `insta` crate.
- **A token-savings assertion** verifying the filter hits the ≥60% target on a real fixture.

Minimal example:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    fn count_tokens(s: &str) -> usize { s.split_whitespace().count() }

    #[test]
    fn filter_git_log_snapshot() {
        let input = include_str!("../../../tests/fixtures/git_log_raw.txt");
        let output = filter_git_log(input);
        assert_snapshot!(output);
    }

    #[test]
    fn filter_git_log_savings() {
        let input = include_str!("../../../tests/fixtures/git_log_raw.txt");
        let output = filter_git_log(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(savings >= 60.0, "expected ≥60% savings, got {:.1}%", savings);
    }
}
```

Fixtures go in `tests/fixtures/` and should be captured from real command output rather than hand-written.

---

## Security

RTK executes shell commands on behalf of the user, so security is a first-class concern.

**Command execution.** All commands go through argument arrays via `Command::new().args()` — never through shell string concatenation. This prevents injection. Always use `resolved_command()` from `src/core/utils.rs` instead of a raw `Command::new()`.

**Hook integrity.** RTK verifies hook files via SHA-256 hashes before operational commands. If a hook has been tampered with, RTK exits with code 1. See [`src/hooks/integrity.rs`](../../src/hooks/integrity.rs).

**Project filter trust.** `.rtk/filters.toml` files are not loaded until the user explicitly trusts them, and content changes require re-trust. See [`src/hooks/trust.rs`](../../src/hooks/trust.rs).

**Permission whitelist.** `is_operational_command()` in `main.rs` uses a whitelist pattern — new commands are *not* integrity-checked until explicitly added. This is an intentional security posture: fail-open with an audit trail is preferred over false confidence.

**`unsafe` code.** Not allowed except for Unix signal handling in proxy mode, which is correctly scoped to `#[cfg(unix)]`.
