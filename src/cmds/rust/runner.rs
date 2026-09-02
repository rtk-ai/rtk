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

/// Build the child command with argv passed through verbatim, matching
/// `rtk proxy` semantics: a single argument is split shell-style respecting
/// quotes (#388), so `rtk err 'cargo build --release'` still works, while
/// multi-argument invocations keep each argument intact. Joining argv into a
/// `sh -c` string re-split quoted arguments and masked child exit codes
/// (#2389).
fn build_argv_command(argv: &[String], usage: &str) -> Result<(Command, String)> {
    let parts: Vec<String> = if argv.len() == 1 {
        let split = crate::discover::lexer::shell_split(&argv[0]);
        if split.len() > 1 { split } else { argv.to_vec() }
    } else {
        argv.to_vec()
    };
    let Some((program, args)) = parts.split_first() else {
        anyhow::bail!("missing command\nUsage: rtk {usage} <command> [args...]");
    };
    let display = parts.join(" ");
    let mut cmd = crate::core::utils::resolved_command(program);
    cmd.args(args);
    Ok((cmd, display))
}

/// Run a command and filter output to show only errors/warnings
pub fn run_err(command: &[String], verbose: u8) -> Result<i32> {
    let (cmd, display) = build_argv_command(command, "err")?;
    if verbose > 0 {
        eprintln!("Running: {}", display);
    }
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
    let (cmd, display) = build_argv_command(command, "test")?;
    if verbose > 0 {
        eprintln!("Running tests: {}", display);
    }
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

    fn argv_of(cmd: &Command) -> Vec<String> {
        cmd.get_args()
            .map(|a| a.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn quoted_argument_with_spaces_is_preserved_verbatim() {
        // Regression for #2389: `rtk err sh -c 'exit 7'` must reach the child
        // as ["-c", "exit 7"], not ["-c", "exit", "7"].
        let argv = vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()];
        let (cmd, display) = build_argv_command(&argv, "err").unwrap();
        assert_eq!(argv_of(&cmd), vec!["-c", "exit 7"]);
        assert_eq!(display, "sh -c exit 7");
    }

    #[test]
    fn single_argument_is_shell_split_like_proxy() {
        // Parity with `rtk proxy` single-string handling (#388).
        let argv = vec!["cargo build --release".to_string()];
        let (cmd, _) = build_argv_command(&argv, "err").unwrap();
        assert!(cmd.get_program().to_string_lossy().contains("cargo"));
        assert_eq!(argv_of(&cmd), vec!["build", "--release"]);
    }

    #[test]
    fn single_argument_quotes_are_respected_when_splitting() {
        let argv = vec!["git log --format=\"%H %s\"".to_string()];
        let (cmd, _) = build_argv_command(&argv, "err").unwrap();
        assert_eq!(argv_of(&cmd), vec!["log", "--format=%H %s"]);
    }

    #[test]
    fn empty_command_is_an_error() {
        assert!(build_argv_command(&[], "err").is_err());
    }

    #[test]
    #[cfg(unix)]
    fn child_exit_code_is_propagated() {
        // `sh -c 'exit 7'` corrupted to `sh -c exit 7` exits 0; the verbatim
        // argv must surface the real exit code.
        let argv = vec!["sh".to_string(), "-c".to_string(), "exit 7".to_string()];
        let (mut cmd, _) = build_argv_command(&argv, "err").unwrap();
        let status = cmd.status().unwrap();
        assert_eq!(status.code(), Some(7));
    }
}
