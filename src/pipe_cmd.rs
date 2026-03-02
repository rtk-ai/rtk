//! `rtk pipe` — Read stdin, reduce tokens, print filtered output.
//!
//! Enables `command | rtk pipe [--filter <name>]` usage where RTK acts as a
//! pure token-reduction filter in a Unix pipeline.
//!
//! # Supported filters
//! | Name | Source module | Typical input |
//! |------|--------------|---------------|
//! | `cargo-test` | cargo_cmd::filter_cargo_test | `cargo test` output |
//! | `pytest` | pytest_cmd::filter_pytest_output | `pytest` output |
//! | `go-test` | go_cmd::filter_go_test_json | `go test -json` output |
//! | `go-build` | go_cmd::filter_go_build | `go build` stderr |
//! | `tsc` | tsc_cmd::filter_tsc_output | `tsc` compiler output |
//! | `vitest` | vitest_cmd::filter_vitest_output | `vitest --reporter=json` |
//! | `grep` / `rg` | grep_cmd::filter_grep_raw | `rg -n --no-heading` output |
//! | `find` / `fd` | grep_cmd::filter_find_output | `find` / `fd` path output |
//! | `git-log` | git::filter_log_output | `git log` output |
//! | `git-diff` | git::compact_diff | `git diff` output |
//! | `git-status` | git::format_status_output | `git status --porcelain=v1` |
//! | `mypy` | mypy_cmd::filter_mypy_output | `mypy` type check output |
//! | `ruff-check` | ruff_cmd::filter_ruff_check_json | `ruff check --output-format=json` |
//! | `ruff-format` | ruff_cmd::filter_ruff_format | `ruff format --check` output |
//! | `prettier` | prettier_cmd::filter_prettier_output | `prettier --check` output |

use anyhow::Result;
use std::io::Read;

/// Resolve a filter name to a `fn(&str) -> String` function pointer.
///
/// Returns `None` if the filter name is not recognised.
pub fn resolve_filter(name: &str) -> Option<fn(&str) -> String> {
    match name {
        "cargo-test" | "cargo" => Some(crate::cargo_cmd::filter_cargo_test),
        "pytest" => Some(crate::pytest_cmd::filter_pytest_output),
        "go-test" => Some(go_test_wrapper),
        "go-build" => Some(crate::go_cmd::filter_go_build),
        "tsc" => Some(crate::tsc_cmd::filter_tsc_output),
        "vitest" => Some(crate::vitest_cmd::filter_vitest_output),
        "grep" | "rg" => Some(crate::grep_cmd::filter_grep_raw),
        "find" | "fd" => Some(crate::grep_cmd::filter_find_output),
        "git-log" => Some(git_log_wrapper),
        "git-diff" => Some(git_diff_wrapper),
        "git-status" => Some(crate::git::format_status_output),
        "mypy" => Some(crate::mypy_cmd::filter_mypy_output),
        "ruff-check" => Some(crate::ruff_cmd::filter_ruff_check_json),
        "ruff-format" => Some(crate::ruff_cmd::filter_ruff_format),
        "prettier" => Some(crate::prettier_cmd::filter_prettier_output),
        _ => None,
    }
}

// Wrappers to adapt functions with extra parameters to fn(&str) -> String

fn go_test_wrapper(input: &str) -> String {
    crate::go_cmd::filter_go_test_json(input)
}

fn git_log_wrapper(input: &str) -> String {
    // Default to 50 log lines when used as a pipe filter
    crate::git::filter_log_output(input, 50)
}

fn git_diff_wrapper(input: &str) -> String {
    // Default to 200 diff lines
    crate::git::compact_diff(input, 200)
}

/// Auto-detect the appropriate filter based on input content heuristics.
///
/// Falls back to identity (no-op) if no filter is detected.
pub fn auto_detect_filter(input: &str) -> fn(&str) -> String {
    let first_1k = &input[..input.len().min(1024)];

    // cargo test: "test result: ok. N passed; M failed"
    if first_1k.contains("test result:") && first_1k.contains("passed;") {
        return crate::cargo_cmd::filter_cargo_test;
    }

    // pytest: starts with "=== test session starts"
    if first_1k.contains("=== test session starts") {
        return crate::pytest_cmd::filter_pytest_output;
    }

    // go test -json: NDJSON with "Action" key
    let first_trimmed = first_1k.trim_start();
    if first_trimmed.starts_with('{') && first_1k.contains("\"Action\"") {
        return go_test_wrapper;
    }

    // mypy: lines like "file.py:42: error: ..." with optional [error-code]
    if first_1k.contains(": error:") && first_1k.contains(".py:") {
        return crate::mypy_cmd::filter_mypy_output;
    }

    // grep/rg: lines matching file:number:content pattern
    if first_1k
        .lines()
        .take(5)
        .filter(|l| !l.trim().is_empty())
        .any(|l| {
            let parts: Vec<_> = l.splitn(3, ':').collect();
            parts.len() == 3 && parts[1].parse::<usize>().is_ok()
        })
    {
        return crate::grep_cmd::filter_grep_raw;
    }

    // vitest: JSON with "testResults" key
    if first_1k.contains("\"testResults\"") || first_1k.contains("\"numTotalTests\"") {
        return crate::vitest_cmd::filter_vitest_output;
    }

    // find/fd: all non-empty lines look like file paths (contain '/' or '.', no ':' separator)
    // Require at least 3 path-like lines to avoid false positives.
    let path_like_lines: usize = first_1k
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty()
                && !t.contains(':')
                && (t.starts_with('.') || t.starts_with('/') || t.contains('/'))
        })
        .count();
    let nonempty_lines: usize = first_1k.lines().filter(|l| !l.trim().is_empty()).count();
    if nonempty_lines >= 3 && path_like_lines == nonempty_lines {
        return crate::grep_cmd::filter_find_output;
    }

    // Default: identity (no-op)
    identity_filter
}

fn identity_filter(input: &str) -> String {
    input.to_string()
}

/// Run `rtk pipe`: read stdin, apply filter, print result.
pub fn run(filter_name: Option<&str>, passthrough: bool) -> Result<()> {
    // Read all stdin (stdin is complete when piped; no streaming benefit here)
    let mut buf = String::new();
    std::io::stdin()
        .read_to_string(&mut buf)
        .map_err(|e| anyhow::anyhow!("Failed to read stdin: {}", e))?;

    if passthrough {
        print!("{}", buf);
        return Ok(());
    }

    let filter_fn = match filter_name {
        Some(name) => resolve_filter(name).ok_or_else(|| {
            anyhow::anyhow!(
                "Unknown filter '{}'. Available: cargo-test, pytest, go-test, go-build, \
                 tsc, vitest, grep, rg, find, fd, git-log, git-diff, git-status, \
                 mypy, ruff-check, ruff-format, prettier",
                name
            )
        })?,
        None => auto_detect_filter(&buf),
    };

    let output = filter_fn(&buf);
    print!("{}", output);
    Ok(())
}

// ─── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── resolve_filter ─────────────────────────────────────────────────────────

    #[test]
    fn test_resolve_filter_cargo_test() {
        let f = resolve_filter("cargo-test").expect("cargo-test filter must exist");
        let out = f("test result: ok. 5 passed; 0 failed");
        assert!(out.contains("passed"), "Should contain pass count: {}", out);
    }

    #[test]
    fn test_resolve_filter_cargo_alias() {
        // "cargo" is an alias for "cargo-test"
        assert!(resolve_filter("cargo").is_some());
    }

    #[test]
    fn test_resolve_filter_grep() {
        let f = resolve_filter("grep").expect("grep filter must exist");
        let input = "src/main.rs:42:fn main() {\nsrc/lib.rs:10:pub fn helper() {}\n";
        let out = f(input);
        assert!(
            out.contains("main.rs") || out.contains("matches"),
            "out={}",
            out
        );
    }

    #[test]
    fn test_resolve_filter_rg_alias() {
        // "rg" is an alias for "grep"
        assert!(resolve_filter("rg").is_some());
    }

    #[test]
    fn test_resolve_filter_pytest() {
        assert!(resolve_filter("pytest").is_some());
    }

    #[test]
    fn test_resolve_filter_go_test() {
        assert!(resolve_filter("go-test").is_some());
    }

    #[test]
    fn test_resolve_filter_tsc() {
        assert!(resolve_filter("tsc").is_some());
    }

    #[test]
    fn test_resolve_filter_vitest() {
        assert!(resolve_filter("vitest").is_some());
    }

    #[test]
    fn test_resolve_filter_git_log() {
        assert!(resolve_filter("git-log").is_some());
    }

    #[test]
    fn test_resolve_filter_git_diff() {
        assert!(resolve_filter("git-diff").is_some());
    }

    #[test]
    fn test_resolve_filter_git_status() {
        assert!(resolve_filter("git-status").is_some());
    }

    #[test]
    fn test_resolve_filter_unknown_returns_none() {
        assert!(resolve_filter("nonexistent-filter").is_none());
    }

    // ── auto_detect_filter ────────────────────────────────────────────────────

    #[test]
    fn test_auto_detect_cargo_test() {
        let input = "test result: ok. 5 passed; 0 failed; 0 ignored; 0 measured\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        assert!(!out.is_empty(), "cargo-test filter should produce output");
    }

    #[test]
    fn test_auto_detect_pytest() {
        let input = "=== test session starts ===\ncollected 3 items\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        assert!(!out.is_empty(), "pytest filter should produce output");
    }

    #[test]
    fn test_auto_detect_grep_format() {
        // rg -n --no-heading format: file:line_num:content
        let input = "src/main.rs:42:fn main() {\nsrc/lib.rs:10:pub fn helper() {}\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        // Should use grep filter and produce grouped output or matches
        assert!(!out.is_empty());
    }

    #[test]
    fn test_auto_detect_go_test_ndjson() {
        let input = r#"{"Time":"2024-01-01T00:00:00Z","Action":"run","Package":"example/pkg"}
{"Time":"2024-01-01T00:00:01Z","Action":"pass","Package":"example/pkg","Elapsed":0.5}
"#;
        let f = auto_detect_filter(input);
        let out = f(input);
        assert!(!out.is_empty());
    }

    #[test]
    fn test_auto_detect_unknown_returns_identity() {
        let input = "some random text that doesn't match any filter pattern\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        // Identity filter returns input unchanged
        assert_eq!(out, input);
    }

    // ── git wrappers ───────────────────────────────────────────────────────────

    #[test]
    fn test_git_log_wrapper() {
        let input = "abc1234 Fix bug in parser (2 days ago) <alice>\n\
                     def5678 Add new feature (3 days ago) <bob>\n";
        let out = git_log_wrapper(input);
        assert!(!out.is_empty());
    }

    #[test]
    fn test_git_diff_wrapper() {
        let input = "diff --git a/src/main.rs b/src/main.rs\n\
                     --- a/src/main.rs\n\
                     +++ b/src/main.rs\n\
                     @@ -1,3 +1,4 @@\n\
                     +// new comment\n\
                      fn main() {}\n";
        let out = git_diff_wrapper(input);
        assert!(!out.is_empty());
    }

    // ── resolve_filter: find/fd ────────────────────────────────────────────────

    #[test]
    fn test_resolve_filter_find() {
        let f = resolve_filter("find").expect("find filter must exist");
        let input = "./src/main.rs\n./src/lib.rs\n./tests/foo.rs\n";
        let out = f(input);
        assert!(out.contains("3 files"), "out={}", out);
    }

    #[test]
    fn test_resolve_filter_fd_alias() {
        // "fd" is an alias for "find" filter
        assert!(resolve_filter("fd").is_some());
    }

    #[test]
    fn test_resolve_filter_unknown_error_message_lists_find() {
        // Confirm the error message mentions find/fd
        assert!(resolve_filter("not-a-filter").is_none());
        // We can't easily test the error message from resolve_filter (returns None),
        // but we verify the mapping exists
        assert!(resolve_filter("find").is_some());
        assert!(resolve_filter("fd").is_some());
    }

    // ── auto_detect_filter: find/fd ────────────────────────────────────────────

    #[test]
    fn test_auto_detect_find_paths() {
        // find/fd output: one path per line, no colons
        let input = "./src/main.rs\n./src/lib.rs\n./src/cmd/mod.rs\n./tests/foo.rs\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        assert!(out.contains("4 files"), "out={}", out);
    }

    #[test]
    fn test_auto_detect_find_absolute_paths() {
        let input = "/home/user/src/main.rs\n/home/user/src/lib.rs\n/home/user/tests/foo.rs\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        assert!(out.contains("3 files"), "out={}", out);
    }

    #[test]
    fn test_auto_detect_find_not_triggered_for_few_lines() {
        // Only 2 path-like lines — should NOT trigger find filter (below threshold of 3)
        let input = "./src/main.rs\n./src/lib.rs\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        // identity filter: output equals input
        assert_eq!(out, input);
    }

    #[test]
    fn test_auto_detect_find_not_triggered_for_grep_output() {
        // grep output has colons — should NOT be treated as find paths
        let input = "src/main.rs:42:fn main() {\nsrc/lib.rs:10:pub fn helper() {}\nsrc/a.rs:1:x\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        // grep filter runs (has colons), find filter must NOT be triggered
        assert!(
            !out.contains("files"),
            "should not trigger find filter: out={}",
            out
        );
    }

    // ── pipe_cmd edge cases ────────────────────────────────────────────────────

    #[test]
    fn test_auto_detect_empty_input_is_identity() {
        let f = auto_detect_filter("");
        let out = f("");
        assert_eq!(out, "");
    }

    #[test]
    fn test_auto_detect_single_line_unknown() {
        // Single line of unknown content → identity
        let input = "hello world\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        assert_eq!(out, input);
    }

    #[test]
    fn test_resolve_filter_go_build() {
        assert!(resolve_filter("go-build").is_some());
    }

    // ── mypy / ruff / prettier filters ──────────────────────────────────────────

    #[test]
    fn test_resolve_filter_mypy() {
        assert!(resolve_filter("mypy").is_some());
    }

    #[test]
    fn test_resolve_filter_ruff_check() {
        assert!(resolve_filter("ruff-check").is_some());
    }

    #[test]
    fn test_resolve_filter_ruff_format() {
        assert!(resolve_filter("ruff-format").is_some());
    }

    #[test]
    fn test_resolve_filter_prettier() {
        assert!(resolve_filter("prettier").is_some());
    }

    #[test]
    fn test_auto_detect_mypy_output() {
        let input = "src/app.py:42: error: Argument 1 has incompatible type [arg-type]\n\
                     src/utils.py:10: error: Missing return statement [return]\n\
                     Found 2 errors in 2 files\n";
        let f = auto_detect_filter(input);
        let out = f(input);
        // Should use mypy filter (not identity)
        assert!(!out.is_empty());
    }
}
