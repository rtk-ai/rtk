use crate::core::runner;
use crate::core::stream::{self, FilterMode, StdinMode};
use crate::core::tracking;
use crate::core::utils::{exit_code_from_status, resolved_command, strip_ansi, truncate};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

const MAX_FALLBACK_TAIL_LINES: usize = 10;

lazy_static! {
    static ref POETRY_BANNER_RE: Regex = Regex::new(r"^(Creating|Updating|Installing|Installed|Resolving|Writing|Adding|Removing|Upgrading|Downgrading|Downloading|Found|No\s+root\s+package)\s").unwrap();
    static ref POETRY_PACKAGE_RE: Regex = Regex::new(r"^\s*[-─]\s").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let args_display = args.join(" ");
    let original_cmd = format!("poetry {args_display}");
    let rtk_cmd = format!("rtk poetry {args_display}");

    let mut cmd = resolved_command("poetry");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: {original_cmd}");
    }

    if args.first().map(String::as_str) != Some("run") {
        let status = cmd.status().context("Failed to run poetry")?;
        timer.track_passthrough(&original_cmd, &format!("{rtk_cmd} (passthrough)"));
        return Ok(exit_code_from_status(&status, "poetry"));
    }

    let result = stream::run_streaming(&mut cmd, StdinMode::Inherit, FilterMode::CaptureOnly)
        .context("Failed to run poetry")?;
    let filtered = filter_poetry_run_output(&result.raw, result.exit_code);

    runner::print_with_hint(&filtered, &result.raw, &result.raw, "poetry", result.exit_code);
    timer.track(&original_cmd, &rtk_cmd, &result.raw, &filtered);

    Ok(result.exit_code)
}

fn filter_poetry_run_output(output: &str, exit_code: i32) -> String {
    let clean = strip_ansi(output);
    let lines: Vec<&str> = clean.lines().collect();
    let mut selected: Vec<String> = Vec::new();

    for line in &lines {
        let trimmed = line.trim();

        if trimmed.is_empty() || is_banner_line(trimmed) {
            continue;
        }

        selected.push(truncate(trimmed, 200));
    }

    let filtered = selected.join("\n").trim().to_string();
    if !filtered.is_empty() {
        return filtered;
    }

    if exit_code == 0 {
        return "ok".to_string();
    }

    let tail: Vec<String> = clean
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty())
        .map(|line| truncate(line, 200))
        .collect();

    if tail.is_empty() {
        return format!("[FAIL] poetry run failed (exit code: {exit_code})");
    }

    let summary = tail
        .into_iter()
        .rev()
        .take(MAX_FALLBACK_TAIL_LINES)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>();

    format!(
        "[FAIL] poetry run failed (exit code: {exit_code})\n{}",
        summary.join("\n")
    )
}

fn is_banner_line(line: &str) -> bool {
    POETRY_BANNER_RE.is_match(line) || POETRY_PACKAGE_RE.is_match(line)
}

#[cfg(test)]
mod tests {
    use super::filter_poetry_run_output;
    use crate::core::utils::count_tokens;

    #[test]
    fn test_filter_poetry_run_suppresses_success_noise() {
        let output = "\
Creating virtualenv my-project-py3.12
Installing dependencies from lock file
Resolving dependencies...
  - Package: pygments (1.2MiB)
Installed 5 packages in 7ms
hello from script
";
        let result = filter_poetry_run_output(output, 0);
        assert_eq!(result, "hello from script");
    }

    #[test]
    fn test_filter_poetry_run_preserves_errors() {
        let output = "\
Creating virtualenv my-project-py3.12
Installing dependencies from lock file
Resolving dependencies...
  - Package: requests (2.31.0)
FAILED tests/test_api.py::test_healthcheck - AssertionError: expected 200
1 failed, 12 passed in 0.31s
";
        let result = filter_poetry_run_output(output, 1);
        assert!(result.contains("FAILED tests/test_api.py::test_healthcheck"));
        assert!(result.contains("1 failed, 12 passed in 0.31s"));
        assert!(!result.contains("Creating virtualenv"));
        assert!(!result.contains("Installing dependencies"));
    }

    #[test]
    fn test_filter_poetry_run_ok_when_empty() {
        let output = "\
Creating virtualenv my-project-py3.12
Resolving dependencies...
";
        let result = filter_poetry_run_output(output, 0);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_filter_poetry_run_preserves_error_message() {
        let output = "could not find pyproject.toml";
        let result = filter_poetry_run_output(output, 2);
        assert!(result.contains("could not find pyproject.toml"));
    }

    #[test]
    fn test_filter_poetry_run_failure_fallback_empty() {
        let output = "";
        let result = filter_poetry_run_output(output, 2);
        assert!(result.contains("[FAIL] poetry run failed (exit code: 2)"));
    }

    #[test]
    fn test_filter_poetry_run_pytest_fixture_token_savings() {
        let input = "\
Creating virtualenv my-project-py3.12
Installing dependencies from lock file
Resolving dependencies...
  - Package: pygments (1.2MiB)
  - Package: pytest (8.4.1)
Installed 15 packages in 120ms
============================= test session starts ==============================
platform darwin -- Python 3.13.5, pytest-8.4.1, pluggy-1.6.0
rootdir: /tmp/my-project
collected 2 items

tests/test_users.py .F                                                   [100%]

=================================== FAILURES ===================================
______________________ test_normalize_user_rejects_empty _______________________

    def test_normalize_user_rejects_empty():
>       assert normalize_user(\"   \") == \"anonymous\"
E       AssertionError: assert '' == 'anonymous'
E
E         - anonymous

tests/test_users.py:10: AssertionError
=========================== short test summary info ============================
FAILED tests/test_users.py::test_normalize_user_rejects_empty - AssertionError:
========================= 1 failed, 1 passed in 0.01s ==========================
";
        let output = filter_poetry_run_output(input, 1);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 20.0,
            "poetry run pytest: expected >=20% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
        assert!(output.contains("FAILED tests/test_users.py::test_normalize_user_rejects_empty"));
        assert!(output.contains("1 failed, 1 passed"));
        assert!(!output.contains("Creating virtualenv"));
    }
}
