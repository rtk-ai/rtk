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
    crate::git::filter_log_output(input, 50, false)
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
