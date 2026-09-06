//! Filters pytest output to show only failures and the summary line.

use crate::core::config;
use crate::core::runner;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::{resolve_binary, resolved_command, strip_ansi, truncate};
use anyhow::{Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

const MAX_XFAIL: usize = CAP_WARNINGS;
const MAX_PYTEST_FAILURES: usize = CAP_WARNINGS;

#[derive(Debug, PartialEq)]
enum ParseState {
    Header,
    TestProgress,
    Failures,
    Summary,
}

#[derive(Debug, PartialEq)]
enum PytestInvocation {
    Executable(PathBuf),
    PythonModule(PathBuf),
}

impl PytestInvocation {
    fn display(&self) -> String {
        match self {
            Self::Executable(path) => path.display().to_string(),
            Self::PythonModule(path) => format!("{} -m pytest", path.display()),
        }
    }

    fn into_command(self) -> Command {
        match self {
            Self::Executable(path) => resolved_command(path.to_string_lossy().as_ref()),
            Self::PythonModule(path) => {
                let mut command = resolved_command(path.to_string_lossy().as_ref());
                command.arg("-m").arg("pytest");
                command
            }
        }
    }
}

fn venv_executable(root: &Path, tool: &str) -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        root.join("Scripts").join(format!("{tool}.exe"))
    }
    #[cfg(not(target_os = "windows"))]
    {
        root.join("bin").join(tool)
    }
}

fn resolve_pytest_invocation<F>(
    virtual_env: Option<&Path>,
    project_dir: &Path,
    mut resolve_on_path: F,
) -> Option<PytestInvocation>
where
    F: FnMut(&str) -> Option<PathBuf>,
{
    let mut environment_roots = Vec::new();
    if let Some(root) = virtual_env.filter(|root| !root.as_os_str().is_empty()) {
        environment_roots.push(root.to_path_buf());
    }
    for root in [project_dir.join(".venv"), project_dir.join("venv")] {
        if !environment_roots.contains(&root) {
            environment_roots.push(root);
        }
    }

    for root in &environment_roots {
        let pytest = venv_executable(root, "pytest");
        if pytest.is_file() {
            return Some(PytestInvocation::Executable(pytest));
        }
    }

    for root in &environment_roots {
        let python = venv_executable(root, "python");
        if python.is_file() {
            return Some(PytestInvocation::PythonModule(python));
        }
    }

    if let Some(pytest) = resolve_on_path("pytest") {
        return Some(PytestInvocation::Executable(pytest));
    }
    for python_name in ["python", "python3"] {
        if let Some(python) = resolve_on_path(python_name) {
            return Some(PytestInvocation::PythonModule(python));
        }
    }

    None
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let project_dir = std::env::current_dir().context("Failed to determine current directory")?;
    let virtual_env = std::env::var_os("VIRTUAL_ENV").map(PathBuf::from);
    let invocation = resolve_pytest_invocation(
        virtual_env.as_deref(),
        &project_dir,
        |name| resolve_binary(name).ok(),
    )
    .context("pytest not found (checked VIRTUAL_ENV, .venv, venv, and PATH)")?;
    let invocation_display = invocation.display();
    let mut cmd = invocation.into_command();

    let has_tb_flag = args.iter().any(|a| a.starts_with("--tb"));
    let has_quiet_flag = args.iter().any(|a| a == "-q" || a == "--quiet");
    // Only treat a short `-r…` as pytest's report flag (not `--randomly-seed` etc.)
    let has_report_flag = args.iter().any(|a| a.starts_with("-r") && !a.starts_with("--"));

    if !has_tb_flag {
        cmd.arg("--tb=short");
    }
    if !has_quiet_flag {
        cmd.arg("-q");
    }
    // Surface xfailed/xpassed (and their reasons) in the short summary section
    // so the compact output can report expected failures and — crucially —
    // unexpected passes (XPASS), which signal a behavior change.
    if !has_report_flag {
        cmd.arg("-rxX");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!(
            "Running: {} --tb=short -q {}",
            invocation_display,
            args.join(" ")
        );
    }

    runner::run_filtered_with_exit(
        cmd,
        "pytest",
        &args.join(" "),
        |raw, exit_code| {
            let clean = strip_ansi(raw);
            let filtered = filter_pytest_output(&clean);
            // Any other failure parsed as empty means the run broke before reporting.
            if exit_code != 0 && exit_code != PYTEST_EXIT_NO_TESTS && filtered == PYTEST_NO_TESTS {
                return truncate(clean.trim(), config::limits().passthrough_max_chars);
            }
            filtered
        },
        runner::RunOptions::stdout_only().tee("pytest"),
    )
}

const PYTEST_NO_TESTS: &str = "Pytest: No tests collected";
const PYTEST_EXIT_NO_TESTS: i32 = 5;

pub(crate) fn filter_pytest_output(output: &str) -> String {
    let mut state = ParseState::Header;
    let mut test_files: Vec<String> = Vec::new();
    let mut failures: Vec<String> = Vec::new();
    let mut current_failure: Vec<String> = Vec::new();
    let mut xfail_lines: Vec<String> = Vec::new();
    let mut summary_line = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // State transitions
        if trimmed.starts_with("===") && trimmed.contains("test session starts") {
            state = ParseState::Header;
            continue;
        } else if trimmed.starts_with("===") && trimmed.contains("FAILURES") {
            state = ParseState::Failures;
            continue;
        } else if trimmed.starts_with("===") && trimmed.contains("short test summary") {
            state = ParseState::Summary;
            // Save current failure if any
            if !current_failure.is_empty() {
                failures.push(current_failure.join("\n"));
                current_failure.clear();
            }
            continue;
        } else if trimmed.starts_with("===")
            && (trimmed.contains("passed")
                || trimmed.contains("failed")
                || trimmed.contains("skipped"))
        {
            summary_line = trimmed.to_string();
            continue;
        // quiet mode (-q): bare summary without === wrapper, e.g. "5 failed, 1698 passed, 2 skipped in 108.89s"
        } else if summary_line.is_empty()
            && !trimmed.starts_with("===")
            && !trimmed.starts_with("FAILED")
            && !trimmed.starts_with("ERROR")
            && (trimmed.contains(" passed")
                || trimmed.contains(" failed")
                || trimmed.contains(" skipped"))
            && trimmed.contains(" in ")
        {
            summary_line = trimmed.to_string();
            continue;
        }

        // Process based on state
        match state {
            ParseState::Header => {
                if trimmed.starts_with("collected") {
                    state = ParseState::TestProgress;
                }
            }
            ParseState::TestProgress => {
                // Lines like "tests/test_foo.py ....  [ 40%]"
                if !trimmed.is_empty()
                    && !trimmed.starts_with("===")
                    && (trimmed.contains(".py") || trimmed.contains("%]"))
                {
                    test_files.push(trimmed.to_string());
                }
            }
            ParseState::Failures => {
                // Collect failure details
                if trimmed.starts_with("___") {
                    // New failure section
                    if !current_failure.is_empty() {
                        failures.push(current_failure.join("\n"));
                        current_failure.clear();
                    }
                    current_failure.push(trimmed.to_string());
                } else if !trimmed.is_empty() && !trimmed.starts_with("===") {
                    current_failure.push(trimmed.to_string());
                }
            }
            ParseState::Summary => {
                // FAILED test lines
                if trimmed.starts_with("FAILED") || trimmed.starts_with("ERROR") {
                    failures.push(trimmed.to_string());
                } else if trimmed.starts_with("XFAIL") || trimmed.starts_with("XPASS") {
                    xfail_lines.push(trimmed.to_string());
                }
            }
        }
    }

    // Save last failure if any
    if !current_failure.is_empty() {
        failures.push(current_failure.join("\n"));
    }

    // Build compact output
    build_pytest_summary(&summary_line, &test_files, &failures, &xfail_lines)
}

#[derive(Default)]
struct PytestCounts {
    passed: usize,
    failed: usize,
    skipped: usize,
    xfailed: usize,
    xpassed: usize,
}

fn build_pytest_summary(
    summary: &str,
    _test_files: &[String],
    failures: &[String],
    xfail_lines: &[String],
) -> String {
    let counts = parse_summary_line(summary);
    let PytestCounts {
        passed,
        failed,
        skipped,
        xfailed,
        xpassed,
    } = counts;

    if passed == 0 && failed == 0 && skipped == 0 && xfailed == 0 && xpassed == 0 {
        return PYTEST_NO_TESTS.to_string();
    }

    let extras_present = skipped > 0 || xfailed > 0 || xpassed > 0 || !xfail_lines.is_empty();

    if failed == 0 && passed > 0 && !extras_present {
        return format!("Pytest: {} passed", passed);
    }

    let mut result = String::new();
    result.push_str(&format!("Pytest: {} passed, {} failed", passed, failed));
    if skipped > 0 {
        result.push_str(&format!(", {} skipped", skipped));
    }
    if xfailed > 0 {
        result.push_str(&format!(", {} xfailed", xfailed));
    }
    if xpassed > 0 {
        result.push_str(&format!(", {} xpassed", xpassed));
    }
    result.push('\n');

    // Surface xfail/xpass entries (with their reasons) — XPASS in particular
    // signals that something expected-to-fail now passes.
    if !xfail_lines.is_empty() {
        result.push_str("\nExpected-failure outcomes:\n");
        for line in xfail_lines.iter().take(MAX_XFAIL) {
            result.push_str(&format!("  {}\n", truncate(line, 120)));
        }
        if xfail_lines.len() > MAX_XFAIL {
            result.push_str(&format!("  … +{} more\n", xfail_lines.len() - MAX_XFAIL));
            let all_xfail = xfail_lines.join("\n");
            if let Some(hint) = crate::core::tee::force_tee_tail_hint(&all_xfail, "pytest-xfail", MAX_XFAIL + 1) {
                result.push_str(&format!("  {}\n", hint));
            }
        }
    }

    if failures.is_empty() {
        return result.trim().to_string();
    }

    // Show failures (limit to key information)
    result.push_str("\nFailures:\n");

    for (i, failure) in failures.iter().take(MAX_PYTEST_FAILURES).enumerate() {
        // Extract test name and key error info
        let lines: Vec<&str> = failure.lines().collect();

        // First line is usually test name (after ___)
        if let Some(first_line) = lines.first() {
            if first_line.starts_with("___") {
                // Extract test name between ___
                let test_name = first_line.trim_matches('_').trim();
                result.push_str(&format!("{}. [FAIL] {}\n", i + 1, test_name));
            } else if first_line.starts_with("FAILED") {
                // Summary format: "FAILED tests/test_foo.py::test_bar - AssertionError"
                let parts: Vec<&str> = first_line.split(" - ").collect();
                if let Some(test_path) = parts.first() {
                    let test_name = test_path.trim_start_matches("FAILED ");
                    result.push_str(&format!("{}. [FAIL] {}\n", i + 1, test_name));
                }
                if parts.len() > 1 {
                    result.push_str(&format!("     {}\n", truncate(parts[1], 100)));
                }
                continue;
            }
        }

        // Show relevant error lines (assertions, errors, file locations)
        let mut relevant_lines = 0;
        for line in &lines[1..] {
            let line_lower = line.to_lowercase();
            let is_relevant = line.trim().starts_with('>')
                || line.trim().starts_with('E')
                || line_lower.contains("assert")
                || line_lower.contains("error")
                || line.contains(".py:");

            if is_relevant && relevant_lines < 3 {
                result.push_str(&format!("     {}\n", truncate(line, 100)));
                relevant_lines += 1;
            }
        }

        if i < failures.len() - 1 {
            result.push('\n');
        }
    }

    if failures.len() > MAX_PYTEST_FAILURES {
        result.push_str(&format!(
            "\n… +{} more failures\n",
            failures.len() - MAX_PYTEST_FAILURES
        ));
        let all_failures = failures.join("\n\n");
        if let Some(hint) = crate::core::tee::force_tee_hint(&all_failures, "pytest-failures") {
            result.push_str(&format!("  {}\n", hint));
        }
    }

    result.trim().to_string()
}

fn parse_summary_line(summary: &str) -> PytestCounts {
    let mut counts = PytestCounts::default();

    // Parse lines like "=== 4 passed, 1 failed, 2 xfailed, 1 xpassed in 0.50s ==="
    for part in summary.split(',') {
        let words: Vec<&str> = part.split_whitespace().collect();
        for (i, word) in words.iter().enumerate() {
            if i == 0 {
                continue;
            }
            let Ok(n) = words[i - 1].parse::<usize>() else {
                continue;
            };
            // Order matters: "xpassed"/"xfailed" contain "passed"/"failed".
            if word.contains("xpassed") {
                counts.xpassed = n;
            } else if word.contains("xfailed") {
                counts.xfailed = n;
            } else if word.contains("passed") {
                counts.passed = n;
            } else if word.contains("failed") {
                counts.failed = n;
            } else if word.contains("skipped") {
                counts.skipped = n;
            }
        }
    }

    counts
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_pytest_all_pass() {
        let output = r#"=== test session starts ===
platform darwin -- Python 3.11.0
collected 5 items

tests/test_foo.py .....                                            [100%]

=== 5 passed in 0.50s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("Pytest"));
        assert!(result.contains("5 passed"));
    }

    #[test]
    fn test_filter_pytest_with_failures() {
        let output = r#"=== test session starts ===
collected 5 items

tests/test_foo.py ..F..                                            [100%]

=== FAILURES ===
___ test_something ___

    def test_something():
>       assert False
E       assert False

tests/test_foo.py:10: AssertionError

=== short test summary info ===
FAILED tests/test_foo.py::test_something - assert False
=== 4 passed, 1 failed in 0.50s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("4 passed, 1 failed"));
        assert!(result.contains("test_something"));
        assert!(result.contains("assert False"));
    }

    #[test]
    fn test_filter_pytest_multiple_failures() {
        let output = r#"=== test session starts ===
collected 3 items

tests/test_foo.py FFF                                              [100%]

=== FAILURES ===
___ test_one ___
E   AssertionError: expected 5

___ test_two ___
E   ValueError: invalid value

=== short test summary info ===
FAILED tests/test_foo.py::test_one - AssertionError: expected 5
FAILED tests/test_foo.py::test_two - ValueError: invalid value
FAILED tests/test_foo.py::test_three - KeyError
=== 3 failed in 0.20s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("3 failed"));
        assert!(result.contains("test_one"));
        assert!(result.contains("test_two"));
        assert!(result.contains("expected 5"));
    }

    #[test]
    fn test_filter_pytest_no_tests() {
        let output = r#"=== test session starts ===
collected 0 items

=== no tests ran in 0.00s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("No tests collected"));
    }

    #[test]
    fn test_parse_summary_line() {
        let c = parse_summary_line("=== 5 passed in 0.50s ===");
        assert_eq!((c.passed, c.failed, c.skipped), (5, 0, 0));

        let c = parse_summary_line("=== 4 passed, 1 failed in 0.50s ===");
        assert_eq!((c.passed, c.failed, c.skipped), (4, 1, 0));

        let c = parse_summary_line("=== 3 passed, 1 failed, 2 skipped in 1.0s ===");
        assert_eq!((c.passed, c.failed, c.skipped), (3, 1, 2));

        let c = parse_summary_line("=== 2 passed, 1 failed, 2 xfailed, 1 xpassed in 1.0s ===");
        assert_eq!(
            (c.passed, c.failed, c.xfailed, c.xpassed),
            (2, 1, 2, 1)
        );
    }

    #[test]
    fn test_filter_pytest_xfail_caps_and_tee_hint() {
        let mut lines = String::from("=== test session starts ===\ncollected 30 items\n\n");
        lines.push_str("test_x.py ");
        for _ in 0..15 {
            lines.push('x');
        }
        lines.push_str("\n\n=== short test summary info ===\n");
        for i in 0..15 {
            lines.push_str(&format!(
                "XFAIL test_x.py::test_case_{i} - known issue #{i}\n"
            ));
        }
        lines.push_str("=== 0 passed, 15 xfailed in 0.05s ===\n");

        let result = filter_pytest_output(&lines);
        let xfail_in_section = result
            .split("Expected-failure outcomes:")
            .nth(1)
            .unwrap_or("");
        let listed = xfail_in_section
            .lines()
            .filter(|l| l.trim().starts_with("XFAIL"))
            .count();
        assert!(
            listed <= 10,
            "MAX_XFAIL cap not enforced: listed {listed}"
        );
        assert!(result.contains("… +5 more"), "missing '+N more': {result}");
    }

    #[test]
    fn test_filter_pytest_xfail_xpass() {
        let output = r#"=== test session starts ===
collected 5 items

test_math.py ..xxX                                                 [100%]

=== short test summary info ===
XFAIL test_math.py::test_division_by_zero - known bug in division
XFAIL test_math.py::test_float_precision - float precision issue — bug #42
XPASS test_math.py::test_unexpected_pass - this should fail but currently passes
=== 2 passed, 2 xfailed, 1 xpassed in 0.05s ==="#;

        let result = filter_pytest_output(output);
        assert!(result.contains("xfailed"), "got: {result}");
        assert!(result.contains("xpassed"), "got: {result}");
        assert!(result.contains("XPASS"), "got: {result}");
        assert!(result.contains("float precision"), "got: {result}");
        assert!(result.contains("test_division_by_zero"), "got: {result}");
    }

    #[test]
    fn test_filter_pytest_quiet_mode_failures() {
        // In -q mode, the final summary line has NO === wrapper
        // This was causing "No tests collected" to be reported incorrectly
        let output = r#"=== test session starts ===
platform linux -- Python 3.12.11, pytest-8.1.0
collected 1705 items

.......F.......

=== FAILURES ===
___ test_something ___

E   AssertionError: expected True

=== short test summary info ===
FAILED tests/test_foo.py::test_something - AssertionError
5 failed, 1698 passed, 2 skipped in 108.89s"#;

        let result = filter_pytest_output(output);
        assert!(
            !result.contains("No tests collected"),
            "Should not report 'No tests collected' when tests ran. Got: {}",
            result
        );
        assert!(
            result.contains("1698") || result.contains("5 failed"),
            "Should show actual test counts. Got: {}",
            result
        );
    }

    #[test]
    fn test_filter_pytest_only_skipped() {
        // If only skipped tests, should NOT say "No tests collected"
        let output = r#"=== test session starts ===
collected 3 items

=== 3 skipped in 0.10s ==="#;

        let result = filter_pytest_output(output);
        assert!(
            !result.contains("No tests collected"),
            "Should not say 'No tests collected' when tests were skipped. Got: {}",
            result
        );
    }

    fn create_venv_tool(root: &std::path::Path, tool: &str) -> std::path::PathBuf {
        let path = venv_executable(root, tool);
        std::fs::create_dir_all(path.parent().expect("tool has parent"))
            .expect("create virtualenv bin directory");
        std::fs::write(&path, b"").expect("create virtualenv tool");
        path
    }

    #[test]
    fn test_pytest_runner_prefers_virtual_env_over_project_venv() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let virtual_env = temp.path().join("active-venv");
        let project = temp.path().join("project");
        let active_pytest = create_venv_tool(&virtual_env, "pytest");
        create_venv_tool(&project.join(".venv"), "pytest");

        let runner = resolve_pytest_invocation(Some(&virtual_env), &project, |_| None);

        assert_eq!(runner, Some(PytestInvocation::Executable(active_pytest)));
    }

    #[test]
    fn test_pytest_runner_discovers_project_dot_venv() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let project_pytest = create_venv_tool(&temp.path().join(".venv"), "pytest");
        let path_pytest = temp.path().join("path-pytest");

        let runner = resolve_pytest_invocation(None, temp.path(), |name| match name {
            "pytest" => Some(path_pytest.clone()),
            _ => None,
        });

        assert_eq!(runner, Some(PytestInvocation::Executable(project_pytest)));
    }

    #[test]
    fn test_pytest_runner_uses_venv_python_module_when_entrypoint_is_missing() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let project_python = create_venv_tool(&temp.path().join("venv"), "python");

        let runner = resolve_pytest_invocation(None, temp.path(), |_| None);

        assert_eq!(runner, Some(PytestInvocation::PythonModule(project_python)));
    }

    #[test]
    fn test_pytest_runner_falls_back_to_path_pytest_then_python3() {
        let temp = tempfile::tempdir().expect("create tempdir");
        let path_pytest = temp.path().join("path-pytest");
        let path_python3 = temp.path().join("path-python3");

        let pytest_runner = resolve_pytest_invocation(None, temp.path(), |name| match name {
            "pytest" => Some(path_pytest.clone()),
            "python3" => Some(path_python3.clone()),
            _ => None,
        });
        assert_eq!(
            pytest_runner,
            Some(PytestInvocation::Executable(path_pytest))
        );

        let python_runner = resolve_pytest_invocation(None, temp.path(), |name| match name {
            "python3" => Some(path_python3.clone()),
            _ => None,
        });
        assert_eq!(
            python_runner,
            Some(PytestInvocation::PythonModule(path_python3))
        );
    }

    #[test]
    fn test_pytest_runner_returns_none_when_no_runner_exists() {
        let temp = tempfile::tempdir().expect("create tempdir");

        let runner = resolve_pytest_invocation(None, temp.path(), |_| None);

        assert_eq!(runner, None);
    }

    #[test]
    fn test_python_module_invocation_adds_pytest_module_arguments() {
        let python = std::path::PathBuf::from("project-python");

        let command = PytestInvocation::PythonModule(python.clone()).into_command();

        assert_eq!(command.get_program(), python.as_os_str());
        assert_eq!(
            command.get_args().collect::<Vec<_>>(),
            ["-m", "pytest"].map(std::ffi::OsStr::new)
        );
    }
}
