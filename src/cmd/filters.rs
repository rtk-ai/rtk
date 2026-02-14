//! Filter Registry
//! Connects binaries to their specific RTK token reducers.

use std::io::Read;
use std::process::{ChildStderr, ChildStdout};

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

/// Apply token reduction to command output
/// Returns (filtered_stdout, filtered_stderr)
pub fn apply(
    filter: FilterType,
    stdout: &mut ChildStdout,
    stderr: &mut ChildStderr,
) -> anyhow::Result<(String, String)> {
    let mut out_str = String::new();
    let mut err_str = String::new();

    stdout.read_to_string(&mut out_str)?;
    stderr.read_to_string(&mut err_str)?;

    // Apply basic filtering based on type
    let filtered_out = match filter {
        FilterType::Git => {
            // Strip ANSI and apply basic git formatting
            strip_ansi(&out_str)
        }
        FilterType::Cargo => {
            // Strip "Compiling" lines, keep errors
            filter_cargo_output(&out_str)
        }
        FilterType::Test => {
            // Strip success lines, keep failures
            filter_test_output(&out_str)
        }
        FilterType::Generic => {
            // Apply line truncation
            truncate_lines(&out_str, 100)
        }
        FilterType::Npm | FilterType::Pnpm => {
            // Strip npm boilerplate
            strip_ansi(&out_str)
        }
        FilterType::None => out_str,
    };

    Ok((filtered_out, strip_ansi(&err_str)))
}

/// Apply filter to already-captured string output
pub fn apply_to_string(filter: FilterType, output: &str) -> String {
    match filter {
        FilterType::Git => strip_ansi(output),
        FilterType::Cargo => filter_cargo_output(output),
        FilterType::Test => filter_test_output(output),
        FilterType::Generic => truncate_lines(output, 100),
        FilterType::Npm | FilterType::Pnpm => strip_ansi(output),
        FilterType::None => output.to_string(),
    }
}

/// Strip ANSI escape codes from string
pub fn strip_ansi(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();

    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if let Some(&'[') = chars.peek() {
                chars.next(); // consume '['
                while let Some(&ch) = chars.peek() {
                    chars.next();
                    if ch.is_ascii_alphabetic() {
                        break;
                    }
                }
                continue;
            }
        }
        result.push(c);
    }
    result
}

/// Filter cargo output: remove verbose "Compiling" lines
fn filter_cargo_output(output: &str) -> String {
    output
        .lines()
        .filter(|line| {
            let line = line.trim();
            // Keep errors, warnings, and summaries
            !line.starts_with("Compiling ") ||
            line.contains("error") ||
            line.contains("warning")
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
            // Keep failures, errors, and summaries
            line.contains("FAILED") ||
            line.contains("error") ||
            line.contains("Error") ||
            line.contains("failed") ||
            line.contains("test result:") ||
            line.starts_with("----")
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
        format!("{}\n... ({} more lines)", truncated.join("\n"), lines.len() - max_lines)
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

    // === STRIP_ANSI TESTS ===

    #[test]
    fn test_strip_ansi_no_codes() {
        assert_eq!(strip_ansi("hello world"), "hello world");
    }

    #[test]
    fn test_strip_ansi_color() {
        assert_eq!(strip_ansi("\x1b[32mgreen\x1b[0m"), "green");
    }

    #[test]
    fn test_strip_ansi_bold() {
        assert_eq!(strip_ansi("\x1b[1mbold\x1b[0m"), "bold");
    }

    #[test]
    fn test_strip_ansi_multiple() {
        assert_eq!(
            strip_ansi("\x1b[31mred\x1b[0m \x1b[32mgreen\x1b[0m"),
            "red green"
        );
    }

    #[test]
    fn test_strip_ansi_complex() {
        assert_eq!(
            strip_ansi("\x1b[1;31;42mbold red on green\x1b[0m"),
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
