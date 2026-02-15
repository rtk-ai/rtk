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

use crate::utils;

/// Filter types for different command categories
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum FilterType {
    Git,
    Cargo,
    Test,
    Pnpm,
    Npm,
    Generic,
    None,
}

/// Determine which filter to apply based on binary name
pub fn get_filter_type(binary: &str) -> FilterType {
    match binary {
        "git" => FilterType::Git,
        "cargo" => FilterType::Cargo,
        "npm" | "npx" => FilterType::Npm,
        "pnpm" => FilterType::Pnpm,
        "pytest" | "go" | "vitest" | "jest" | "mocha" => FilterType::Test,
        "ls" | "find" | "grep" | "rg" | "fd" => FilterType::Generic,
        _ => FilterType::None,
    }
}

/// Apply filter to already-captured string output
pub fn apply_to_string(filter: FilterType, output: &str) -> String {
    match filter {
        FilterType::Git => utils::strip_ansi(output),
        FilterType::Cargo => filter_cargo_output(output),
        FilterType::Test => filter_test_output(output),
        FilterType::Generic => truncate_lines(output, 100),
        FilterType::Npm | FilterType::Pnpm => utils::strip_ansi(output),
        FilterType::None => output.to_string(),
    }
}

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

/// Truncate output to max lines
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

#[cfg(test)]
mod tests {
    use super::*;

    // === GET_FILTER_TYPE TESTS ===

    #[test]
    fn test_filter_type_git() {
        assert_eq!(get_filter_type("git"), FilterType::Git);
    }

    #[test]
    fn test_filter_type_cargo() {
        assert_eq!(get_filter_type("cargo"), FilterType::Cargo);
    }

    #[test]
    fn test_filter_type_npm() {
        assert_eq!(get_filter_type("npm"), FilterType::Npm);
        assert_eq!(get_filter_type("npx"), FilterType::Npm);
    }

    #[test]
    fn test_filter_type_generic() {
        assert_eq!(get_filter_type("ls"), FilterType::Generic);
        assert_eq!(get_filter_type("grep"), FilterType::Generic);
    }

    #[test]
    fn test_filter_type_none() {
        assert_eq!(get_filter_type("unknown_command"), FilterType::None);
    }

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

    // === TRUNCATE_LINES TESTS ===

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

    // === APPLY_TO_STRING TESTS ===

    #[test]
    fn test_apply_to_string_none() {
        let input = "hello world";
        let output = apply_to_string(FilterType::None, input);
        assert_eq!(output, input);
    }

    #[test]
    fn test_apply_to_string_git() {
        let input = "\x1b[32mgreen\x1b[0m";
        let output = apply_to_string(FilterType::Git, input);
        assert_eq!(output, "green");
    }
}
