//! Filter Registry — basic token reduction for `rtk run` native execution.
//!
//! This module provides **basic filtering (20-40% savings)** for commands
//! executed through rtk run. It is a **fallback** for commands
//! without dedicated RTK implementations.
//!
//! For **specialized filtering (60-90% savings)**, use dedicated modules:
//! - `src/git.rs` — git commands (diff, log, status, etc.)
//! - `src/runner.rs` — test commands (cargo test, pytest, etc.)
//! - `src/grep_cmd.rs` — code search (grep, ripgrep)
//! - `src/pnpm_cmd.rs` — package managers

use crate::stream::{FilterMode, LineFilter};
use crate::utils;

/// Filter cargo output: remove verbose "Compiling" lines
fn filter_cargo_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let line = line.trim();
            !line.starts_with("Compiling ") || line.contains("error") || line.contains("warning")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Filter test output: remove passing tests, keep failures
fn filter_test_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let line = line.trim();
            line.contains("FAILED")
                || line.contains("error")
                || line.contains("Error")
                || line.contains("failed")
                || line.contains("test result:")
                || line.starts_with("----")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Map a binary name to a [`FilterMode`] for use with [`crate::stream::run_streaming`].
///
/// Used by `spawn_with_filter` in exec.rs for external commands without dedicated modules.
pub fn get_filter_mode(binary: &str) -> FilterMode {
    match binary {
        // Streaming: per-line ANSI strip + line truncation (low savings, low overhead)
        "ls" | "find" | "grep" | "rg" | "fd" => {
            FilterMode::Streaming(Box::new(LineFilter::new(|l| {
                let stripped = utils::strip_ansi(l);
                let truncated = if stripped.len() > 120 {
                    format!("{}...", &stripped[..117])
                } else {
                    stripped
                };
                Some(format!("{}\n", truncated))
            })))
        }
        // Buffered: cargo, git, and test runners use simple filters here
        // (dedicated modules like cargo_cmd.rs / go_cmd.rs provide 60-90% savings)
        "cargo" => FilterMode::Buffered(filter_cargo_output),
        "pytest" | "jest" | "mocha" | "vitest" | "mypy" | "ruff" | "golangci-lint" => {
            FilterMode::Buffered(filter_test_output)
        }
        // git: ANSI strip per-line (dedicated git.rs handles git subcommands)
        "git" => FilterMode::Streaming(Box::new(LineFilter::new(|l| {
            Some(format!("{}\n", utils::strip_ansi(l)))
        }))),
        // npm/pnpm: ANSI strip (dedicated pnpm_cmd.rs handles specific subcommands)
        "npm" | "npx" | "pnpm" => FilterMode::Streaming(Box::new(LineFilter::new(|l| {
            Some(format!("{}\n", utils::strip_ansi(l)))
        }))),
        // Unknown commands: passthrough (no filtering, preserves all output)
        _ => FilterMode::Passthrough,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === STRIP_ANSI TESTS (now testing utils::strip_ansi) ===

    #[test]
    fn test_strip_ansi_no_codes() {
        assert_eq!(utils::strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn test_strip_ansi_color() {
        assert_eq!(utils::strip_ansi("\x1b[32mgreen\x1b[0m"), "green");
    }

    #[test]
    fn test_strip_ansi_bold() {
        assert_eq!(utils::strip_ansi("\x1b[1mbold\x1b[0m"), "bold");
    }

    #[test]
    fn test_strip_ansi_multiple() {
        assert_eq!(
            utils::strip_ansi("\x1b[31mred\x1b[0m \x1b[32mgreen\x1b[0m"),
            "red green"
        );
    }

    #[test]
    fn test_strip_ansi_complex() {
        assert_eq!(
            utils::strip_ansi("\x1b[1;31;42mbold red on green\x1b[0m"),
            "bold red on green"
        );
    }

    // === FILTER_CARGO_OUTPUT TESTS ===

    #[test]
    fn test_filter_cargo_keeps_errors() {
        let input = "Compiling dep1\nerror: something wrong\nCompiling dep2";
        let output = filter_cargo_output(input);
        assert!(output.contains("error"));
        assert!(!output.contains("Compiling dep1"));
    }

    #[test]
    fn test_filter_cargo_keeps_warnings() {
        let input = "Compiling dep1\nwarning: unused variable\nCompiling dep2";
        let output = filter_cargo_output(input);
        assert!(output.contains("warning"));
    }

    // === FILTER_TEST_OUTPUT TESTS ===

    #[test]
    fn test_filter_test_keeps_failures() {
        let input = "test foo ... ok\ntest bar ... FAILED\ntest result: 1 passed; 1 failed";
        let output = filter_test_output(input);
        assert!(output.contains("FAILED"));
        assert!(output.contains("test result:"));
        assert!(!output.contains("test foo"));
    }

    // === TRUNCATE_LINES TESTS ===

    /// Truncate output to max lines (test utility)
    fn truncate_lines(output: &str, max_lines: usize) -> String {
        let lines: Vec<&str> = output.lines().collect();
        if lines.len() <= max_lines {
            output.to_string()
        } else {
            let truncated: Vec<&str> = lines.iter().take(max_lines).copied().collect();
            format!(
                "{}\n... ({} more lines)",
                truncated.join("\n"),
                lines.len() - max_lines
            )
        }
    }

    #[test]
    fn test_truncate_short() {
        let input = "line1\nline2\nline3";
        let output = truncate_lines(input, 10);
        assert_eq!(output, input);
    }

    #[test]
    fn test_truncate_long() {
        let input = "line1\nline2\nline3\nline4\nline5";
        let output = truncate_lines(input, 3);
        assert!(output.contains("line3"));
        assert!(!output.contains("line4"));
        assert!(output.contains("2 more lines"));
    }

    // === GET_FILTER_MODE TESTS ===

    #[test]
    fn test_get_filter_mode_grep_is_streaming() {
        assert!(matches!(get_filter_mode("grep"), FilterMode::Streaming(_)));
    }

    #[test]
    fn test_get_filter_mode_rg_is_streaming() {
        assert!(matches!(get_filter_mode("rg"), FilterMode::Streaming(_)));
    }

    #[test]
    fn test_get_filter_mode_find_is_streaming() {
        assert!(matches!(get_filter_mode("find"), FilterMode::Streaming(_)));
    }

    #[test]
    fn test_get_filter_mode_fd_is_streaming() {
        assert!(matches!(get_filter_mode("fd"), FilterMode::Streaming(_)));
    }

    #[test]
    fn test_get_filter_mode_ls_is_streaming() {
        assert!(matches!(get_filter_mode("ls"), FilterMode::Streaming(_)));
    }

    #[test]
    fn test_get_filter_mode_cargo_is_buffered() {
        assert!(matches!(get_filter_mode("cargo"), FilterMode::Buffered(_)));
    }

    #[test]
    fn test_get_filter_mode_mypy_is_buffered() {
        assert!(matches!(get_filter_mode("mypy"), FilterMode::Buffered(_)));
    }

    #[test]
    fn test_get_filter_mode_ruff_is_buffered() {
        assert!(matches!(get_filter_mode("ruff"), FilterMode::Buffered(_)));
    }

    #[test]
    fn test_get_filter_mode_golangci_lint_is_buffered() {
        assert!(matches!(
            get_filter_mode("golangci-lint"),
            FilterMode::Buffered(_)
        ));
    }

    #[test]
    fn test_get_filter_mode_npm_is_streaming() {
        assert!(matches!(get_filter_mode("npm"), FilterMode::Streaming(_)));
    }

    #[test]
    fn test_get_filter_mode_pnpm_is_streaming() {
        assert!(matches!(get_filter_mode("pnpm"), FilterMode::Streaming(_)));
    }

    #[test]
    fn test_get_filter_mode_git_is_streaming() {
        assert!(matches!(get_filter_mode("git"), FilterMode::Streaming(_)));
    }

    #[test]
    fn test_get_filter_mode_unknown_is_passthrough() {
        assert!(matches!(
            get_filter_mode("unknowncmd"),
            FilterMode::Passthrough
        ));
    }

    #[test]
    fn test_get_filter_mode_grep_strips_ansi_and_emits() {
        // Feed a line with ANSI codes; the streaming filter must strip them and emit.
        let mut mode = get_filter_mode("grep");
        if let FilterMode::Streaming(ref mut filter) = mode {
            let result = filter.feed_line("\x1b[32msrc/main.rs:42:fn main\x1b[0m");
            assert!(result.is_some(), "streaming filter must emit a line");
            let out = result.unwrap();
            assert!(
                out.contains("src/main.rs"),
                "ANSI stripped, path preserved: {}",
                out
            );
            assert!(
                !out.contains("\x1b["),
                "ANSI codes must be stripped: {}",
                out
            );
        } else {
            panic!("Expected FilterMode::Streaming for 'grep'");
        }
    }

    #[test]
    fn test_get_filter_mode_find_truncates_long_lines() {
        // Feed a line > 120 chars; the streaming filter must truncate it.
        let long_line = "a".repeat(200);
        let mut mode = get_filter_mode("find");
        if let FilterMode::Streaming(ref mut filter) = mode {
            let result = filter.feed_line(&long_line);
            assert!(result.is_some());
            let out = result.unwrap();
            // Truncated content should end with "..." and be ≤ 120+3+1 ("\n") chars
            assert!(
                out.len() <= 125,
                "line must be truncated: len={}",
                out.len()
            );
            assert!(out.contains("..."), "truncated line must contain '...'");
        } else {
            panic!("Expected FilterMode::Streaming for 'find'");
        }
    }

    #[test]
    fn test_get_filter_mode_rg_short_line_passes_through() {
        let short_line = "src/foo.rs:10:hello";
        let mut mode = get_filter_mode("rg");
        if let FilterMode::Streaming(ref mut filter) = mode {
            let result = filter.feed_line(short_line);
            assert!(result.is_some());
            let out = result.unwrap();
            assert!(out.contains("src/foo.rs"), "out={}", out);
        } else {
            panic!("Expected FilterMode::Streaming for 'rg'");
        }
    }

    // === EDGE CASE: go subcommands not handled by go_cmd.rs must passthrough ===

    #[test]
    fn test_get_filter_mode_go_is_passthrough() {
        // go mod tidy, go get, go run etc. are not in the registry's Only(&["test","build","vet"])
        // so they reach spawn_with_filter. They must passthrough, not get test-filtered.
        assert!(matches!(get_filter_mode("go"), FilterMode::Passthrough));
    }

    // === EDGE CASE: npx should get ANSI stripping ===

    #[test]
    fn test_get_filter_mode_npx_is_streaming() {
        assert!(matches!(get_filter_mode("npx"), FilterMode::Streaming(_)));
    }

    // === EDGE CASE: npm streaming filter strips ANSI ===

    #[test]
    fn test_get_filter_mode_npm_strips_ansi() {
        let mut mode = get_filter_mode("npm");
        if let FilterMode::Streaming(ref mut filter) = mode {
            let result = filter.feed_line("\x1b[33mWARN\x1b[0m deprecated package");
            assert!(result.is_some());
            let out = result.unwrap();
            assert!(out.contains("WARN"), "content preserved: {}", out);
            assert!(!out.contains("\x1b["), "ANSI codes stripped: {}", out);
        } else {
            panic!("Expected FilterMode::Streaming for 'npm'");
        }
    }

    // === EDGE CASE: filter_test_output with no matching lines returns empty ===

    #[test]
    fn test_filter_test_output_no_failures_returns_empty() {
        let input = "test foo ... ok\ntest bar ... ok\ntest baz ... ok";
        let output = filter_test_output(input);
        assert!(
            output.is_empty(),
            "all-passing tests should produce empty output"
        );
    }

    // === EDGE CASE: filter_cargo_output with only Compiling lines ===

    #[test]
    fn test_filter_cargo_output_only_compiling() {
        let input = "Compiling dep1\nCompiling dep2\nCompiling dep3";
        let output = filter_cargo_output(input);
        assert!(
            output.is_empty() || output.trim().is_empty(),
            "pure Compiling output should be filtered out"
        );
    }

    // === EDGE CASE: filter_test_output keeps error separator lines ===

    #[test]
    fn test_filter_test_output_keeps_separator_lines() {
        let input = "test foo ... ok\n---- test_bar stdout ----\nerror: assertion failed\ntest result: 0 passed; 1 failed";
        let output = filter_test_output(input);
        assert!(output.contains("----"), "separator lines preserved");
        assert!(output.contains("error:"), "error lines preserved");
        assert!(output.contains("test result:"), "summary preserved");
        assert!(!output.contains("test foo"), "passing test filtered out");
    }
}
