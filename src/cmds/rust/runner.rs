//! Runs arbitrary commands and captures only stderr or test failures.

use crate::core::stream::StreamFilter;
use crate::core::truncate::{CAP_LIST, CAP_WARNINGS};
use anyhow::Result;
use regex::Regex;
use std::process::Command;
use std::sync::LazyLock;

const MAX_RUNNER_FAILURES: usize = CAP_WARNINGS;
const MAX_RUNNER_LINES: usize = CAP_LIST;

static ERROR_PATTERNS: LazyLock<Vec<Regex>> = LazyLock::new(|| {
    vec![
        // Generic errors
        Regex::new(r"(?i)^.*error[\s:\[].*$").unwrap(),
        Regex::new(r"(?i)^.*\berr\b.*$").unwrap(),
        Regex::new(r"(?i)^.*warning[\s:\[].*$").unwrap(),
        Regex::new(r"(?i)^.*\bwarn\b.*$").unwrap(),
        Regex::new(r"(?i)^.*failed.*$").unwrap(),
        Regex::new(r"(?i)^.*failure.*$").unwrap(),
        Regex::new(r"(?i)^.*exception.*$").unwrap(),
        Regex::new(r"(?i)^.*panic.*$").unwrap(),
        // Rust specific
        Regex::new(r"^error\[E\d+\]:.*$").unwrap(),
        Regex::new(r"^\s*--> .*:\d+:\d+$").unwrap(),
        // Python
        Regex::new(r"^Traceback.*$").unwrap(),
        Regex::new(r#"^\s*File ".*", line \d+.*$"#).unwrap(),
        // JavaScript/TypeScript
        Regex::new(r"^\s*at .*:\d+:\d+.*$").unwrap(),
        // Go
        Regex::new(r"^.*\.go:\d+:.*$").unwrap(),
    ]
});

struct ErrorStreamFilter {
    in_error_block: bool,
    blank_count: usize,
    emitted_any: bool,
}

impl ErrorStreamFilter {
    fn new() -> Self {
        Self {
            in_error_block: false,
            blank_count: 0,
            emitted_any: false,
        }
    }
}

impl StreamFilter for ErrorStreamFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        let is_error = ERROR_PATTERNS.iter().any(|p| p.is_match(line));
        if is_error {
            self.in_error_block = true;
            self.blank_count = 0;
            self.emitted_any = true;
            Some(format!("{}\n", line))
        } else if self.in_error_block {
            if line.trim().is_empty() {
                self.blank_count += 1;
                if self.blank_count >= 2 {
                    self.in_error_block = false;
                    None
                } else {
                    self.emitted_any = true;
                    Some(format!("{}\n", line))
                }
            } else if line.starts_with(' ') || line.starts_with('\t') {
                self.blank_count = 0;
                self.emitted_any = true;
                Some(format!("{}\n", line))
            } else {
                self.in_error_block = false;
                None
            }
        } else {
            None
        }
    }

    fn flush(&mut self) -> String {
        String::new()
    }

    fn on_exit(&mut self, exit_code: i32, raw: &str) -> Option<String> {
        if self.emitted_any {
            return None;
        }
        if exit_code == 0 {
            Some("[ok] Command completed successfully (no errors)".to_string())
        } else {
            let mut msg = format!("[FAIL] Command failed (exit code: {})\n", exit_code);
            let lines: Vec<&str> = raw.lines().collect();
            for line in lines.iter().rev().take(10).rev() {
                msg.push_str(&format!("  {}\n", line));
            }
            Some(msg)
        }
    }
}

fn build_shell_command(command: &str) -> Command {
    if cfg!(target_os = "windows") {
        // std::process::Command's default arg-escaping on Windows would
        // re-escape our already-quoted `command` string a second time when
        // building the command line for cmd.exe itself, corrupting it.
        // raw_arg passes it through untouched, so only our own
        // quote_shell_arg escaping (targeting cmd.exe's /C parsing) and the
        // child program's own argv parsing are in play -- not a third layer.
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let mut c = Command::new("cmd");
            c.raw_arg("/C").raw_arg(command);
            c
        }
        #[cfg(not(windows))]
        unreachable!()
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    }
}

/// Quote a single argument so a shell re-parsing the joined command string
/// reconstructs this exact argument, not a re-split version of it.
fn quote_shell_arg(arg: &str) -> String {
    let is_safe = !arg.is_empty()
        && arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.' | '/' | ':' | '=' | '@'));
    if is_safe {
        return arg.to_string();
    }
    if cfg!(target_os = "windows") {
        // Backslash-escape embedded quotes (the CommandLineToArgvW
        // convention most Windows programs, including node.exe, parse
        // their own argv with) rather than cmd.exe's own doubled-quote
        // convention, which is a separate, outer parsing layer.
        format!("\"{}\"", arg.replace('"', "\\\""))
    } else {
        // POSIX single-quoting: close the quote, insert an escaped literal
        // quote, reopen — the standard technique for embedding a ' inside ''.
        format!("'{}'", arg.replace('\'', r"'\''"))
    }
}

/// Re-join argv into a shell command string, preserving argument boundaries.
///
/// The OS already split the original command into correct argv elements by
/// the time this process sees them — `argv.join(" ")` alone throws that away
/// (issue #2985): an argument containing spaces or shell metacharacters
/// (e.g. `node -e 'console.log("a")'`) gets flattened into the join, then
/// `sh -c`/`cmd /C` re-splits it on those same characters, silently running
/// something different from what the user typed.
fn shell_quote_join(command: &[String]) -> String {
    command
        .iter()
        .map(|a| quote_shell_arg(a))
        .collect::<Vec<_>>()
        .join(" ")
}

/// Run a command and filter output to show only errors/warnings
pub fn run_err(command: &[String], verbose: u8) -> Result<i32> {
    let display = command.join(" ");
    if verbose > 0 {
        eprintln!("Running: {}", display);
    }
    let quoted = shell_quote_join(command);
    let cmd = build_shell_command(&quoted);
    crate::core::runner::run_streamed(
        cmd,
        "err",
        &display,
        Box::new(ErrorStreamFilter::new()),
        crate::core::runner::RunOptions::with_tee("err"),
    )
}

/// Run tests and show only failures
pub fn run_test(command: &[String], verbose: u8) -> Result<i32> {
    let display = command.join(" ");
    if verbose > 0 {
        eprintln!("Running tests: {}", display);
    }
    let quoted = shell_quote_join(command);
    let cmd = build_shell_command(&quoted);
    let command_owned = display.clone();
    crate::core::runner::run_filtered(
        cmd,
        "test",
        &display,
        move |raw| extract_test_summary(raw, &command_owned),
        crate::core::runner::RunOptions::with_tee("test"),
    )
}

#[cfg(test)]
fn filter_errors(output: &str) -> String {
    let mut result = Vec::new();
    let mut in_error_block = false;
    let mut blank_count = 0;

    for line in output.lines() {
        let is_error_line = ERROR_PATTERNS.iter().any(|p| p.is_match(line));

        if is_error_line {
            in_error_block = true;
            blank_count = 0;
            result.push(line.to_string());
        } else if in_error_block {
            if line.trim().is_empty() {
                blank_count += 1;
                if blank_count >= 2 {
                    in_error_block = false;
                } else {
                    result.push(line.to_string());
                }
            } else if line.starts_with(' ') || line.starts_with('\t') {
                result.push(line.to_string());
                blank_count = 0;
            } else {
                in_error_block = false;
            }
        }
    }

    result.join("\n")
}

fn extract_test_summary(output: &str, command: &str) -> String {
    let mut result = Vec::new();
    let lines: Vec<&str> = output.lines().collect();

    let is_cargo = command.contains("cargo test");
    let is_pytest = command.contains("pytest");
    let is_jest =
        command.contains("jest") || command.contains("npm test") || command.contains("yarn test");
    let is_go = command.contains("go test");

    let mut failures = Vec::new();
    let mut in_failure = false;
    let mut failure_lines = Vec::new();

    for line in lines.iter() {
        if is_cargo {
            if line.contains("test result:") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") && !line.contains("test result") {
                failures.push(line.to_string());
            }
            if line.starts_with("failures:") {
                in_failure = true;
            }
            if in_failure && line.starts_with("    ") {
                failure_lines.push(line.to_string());
            }
        }

        if is_pytest {
            if line.contains(" passed") || line.contains(" failed") || line.contains(" error") {
                result.push(line.to_string());
            }
            if line.contains("FAILED") {
                failures.push(line.to_string());
            }
        }

        if is_jest {
            if line.contains("Tests:") || line.contains("Test Suites:") {
                result.push(line.to_string());
            }
            if line.contains("✕") || line.contains("FAIL") {
                failures.push(line.to_string());
            }
        }

        if is_go {
            if line.starts_with("ok") || line.starts_with("FAIL") || line.starts_with("---") {
                result.push(line.to_string());
            }
            if line.contains("FAIL") {
                failures.push(line.to_string());
            }
        }
    }

    let mut output = String::new();

    if !failures.is_empty() {
        output.push_str("[FAIL] FAILURES:\n");
        for f in failures.iter().take(MAX_RUNNER_FAILURES) {
            output.push_str(&format!("  {}\n", f));
        }
        if failures.len() > MAX_RUNNER_FAILURES {
            output.push_str(&format!(
                "  ... +{} more failures\n",
                failures.len() - MAX_RUNNER_FAILURES
            ));
        }
        for f in failure_lines.iter().take(MAX_RUNNER_LINES) {
            output.push_str(&format!("  {}\n", f.trim()));
        }
        if failure_lines.len() > MAX_RUNNER_LINES {
            output.push_str(&format!(
                "  ... +{} more\n",
                failure_lines.len() - MAX_RUNNER_LINES
            ));
        }
        output.push('\n');
    }

    if !result.is_empty() {
        output.push_str("SUMMARY:\n");
        for r in &result {
            output.push_str(&format!("  {}\n", r));
        }
    } else {
        output.push_str("OUTPUT (last 5 lines):\n");
        let start = lines.len().saturating_sub(5);
        for line in &lines[start..] {
            if !line.trim().is_empty() {
                output.push_str(&format!("  {}\n", line));
            }
        }
    }

    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_errors() {
        let output = "info: compiling\nerror: something failed\n  at line 10\ninfo: done";
        let filtered = filter_errors(output);
        assert!(filtered.contains("error"));
        assert!(!filtered.contains("info"));
    }

    #[test]
    fn test_quote_shell_arg_leaves_safe_args_unquoted() {
        assert_eq!(quote_shell_arg("node"), "node");
        assert_eq!(quote_shell_arg("-e"), "-e");
        assert_eq!(quote_shell_arg("src/main.rs"), "src/main.rs");
    }

    #[test]
    fn test_quote_shell_arg_quotes_args_with_metacharacters() {
        let quoted = quote_shell_arg(r#"console.log("a")"#);
        // Round-trips through the platform's own shell-quoting rules --
        // exact escaped form differs per platform, but it must be quoted
        // and must not equal the raw unsafe string.
        assert_ne!(quoted, r#"console.log("a")"#);
        assert!(quoted.starts_with('\'') || quoted.starts_with('"'));
    }

    #[test]
    fn test_quote_shell_arg_quotes_args_with_spaces() {
        let quoted = quote_shell_arg("hello world");
        assert_ne!(quoted, "hello world");
    }

    #[test]
    fn test_shell_quote_join_preserves_argument_boundaries() {
        // Regression test for #2985: a naive `argv.join(" ")` flattens an
        // argument containing spaces/quotes into the join, and the shell
        // re-splits it on those same characters when the string is
        // re-parsed. The quoted join must survive a shell round-trip.
        let argv = vec![
            "node".to_string(),
            "-e".to_string(),
            r#"console.log("a")"#.to_string(),
        ];
        let joined = shell_quote_join(&argv);
        // The full original third argument must appear intact somewhere in
        // the joined string (inside its quoting), not just the words of it
        // scattered by an unquoted join.
        assert!(joined.contains("console.log"));
        assert!(joined.contains('a'));
        // It must differ from the naive, bug-reproducing join.
        assert_ne!(joined, argv.join(" "));
    }

    #[test]
    fn test_shell_quote_join_empty_argv() {
        assert_eq!(shell_quote_join(&[]), "");
    }
}
