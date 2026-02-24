use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

/// Detect gradle binary: prefer ./gradlew wrapper, fall back to gradle
fn detect_gradle_binary() -> String {
    #[cfg(target_os = "windows")]
    {
        if std::path::Path::new("gradlew.bat").exists() {
            return "gradlew.bat".to_string();
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        if std::path::Path::new("./gradlew").exists() {
            return "./gradlew".to_string();
        }
    }
    "gradle".to_string()
}

/// Filter gradle test output: show failures only, strip task progress
fn filter_gradle_test(output: &str) -> String {
    if output.trim().is_empty() {
        return "Gradle test: no output".to_string();
    }

    let mut result = String::new();
    let mut in_failure_block = false;
    let mut in_test_failure = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            if in_test_failure || in_failure_block {
                result.push('\n');
            }
            continue;
        }

        // Skip > Task lines (progress noise)
        if trimmed.starts_with("> Task") {
            in_test_failure = false;
            continue;
        }

        // Test summary line: "N tests completed, M failed"
        if trimmed.contains("tests completed") {
            in_test_failure = false;
            result.push_str(trimmed);
            result.push('\n');
            continue;
        }

        // FAILURE: block (ends test failure context)
        if trimmed.starts_with("FAILURE:") {
            in_test_failure = false;
            in_failure_block = true;
            result.push('\n');
            result.push_str(trimmed);
            result.push('\n');
            continue;
        }

        // Test failure line: "ClassName > method() FAILED"
        if trimmed.contains("FAILED") && trimmed.contains(" > ") {
            in_test_failure = true;
            result.push_str(trimmed);
            result.push('\n');
            continue;
        }

        // Exception/assertion details (all lines under a failed test are diagnostic)
        if in_test_failure {
            result.push_str("    ");
            result.push_str(trimmed);
            result.push('\n');
            continue;
        }

        // "* What went wrong:" and following lines
        if in_failure_block && trimmed.starts_with("* What went wrong:") {
            result.push_str(trimmed);
            result.push('\n');
            continue;
        }

        if in_failure_block && trimmed.starts_with("> ") && !trimmed.starts_with("> Task") {
            result.push_str("  ");
            result.push_str(trimmed);
            result.push('\n');
            continue;
        }

        // Stop failure block at advice
        if trimmed.starts_with("* Try:") || trimmed.starts_with("* Get more help") {
            in_failure_block = false;
            continue;
        }

        // BUILD SUCCESSFUL / BUILD FAILED summary
        if trimmed.starts_with("BUILD SUCCESSFUL") || trimmed.starts_with("BUILD FAILED") {
            in_failure_block = false;
            result.push('\n');
            result.push_str(trimmed);
            result.push('\n');
            continue;
        }

        // Actionable tasks line
        if trimmed.contains("actionable task") {
            result.push_str(trimmed);
            result.push('\n');
            continue;
        }
    }

    let output = result.trim().to_string();
    if output.is_empty() {
        "Gradle test: no output".to_string()
    } else {
        output
    }
}

/// Return the appropriate filter for a Gradle task, if one exists.
fn get_filter(task: &str) -> Option<fn(&str) -> String> {
    if task == "test" || task.ends_with(":test") {
        Some(filter_gradle_test)
    } else {
        None
    }
}

/// Execute a Gradle task, applying a filter if one exists for it.
pub fn run_task(task: &str, args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();
    let gradle = detect_gradle_binary();
    let filter = get_filter(task);

    let mut cmd = Command::new(&gradle);
    cmd.arg("--console=plain");
    cmd.arg(task);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {} {}", gradle, task, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {} {}. Is Gradle installed?", gradle, task))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    let filtered = match filter {
        Some(f) => {
            let result = f(&raw);
            if let Some(hint) =
                crate::tee::tee_and_hint(&raw, &format!("gradle_{}", task), exit_code)
            {
                println!("{}\n{}", result, hint);
            } else {
                println!("{}", result);
            }
            result
        }
        None => {
            print!("{}", stdout);
            eprint!("{}", stderr);
            raw.clone()
        }
    };

    timer.track(
        &format!("{} {} {}", gradle, task, args.join(" ")),
        &format!("rtk gradle {} {}", task, args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    const GRADLE_TEST_PASS: &str = r#"> Task :compileJava UP-TO-DATE
> Task :processResources NO-SOURCE
> Task :classes UP-TO-DATE
> Task :compileTestJava UP-TO-DATE
> Task :processTestResources NO-SOURCE
> Task :testClasses UP-TO-DATE
> Task :test

BUILD SUCCESSFUL in 5s
4 actionable tasks: 1 executed, 3 up-to-date"#;

    const GRADLE_TEST_FAIL: &str = r#"> Task :compileJava UP-TO-DATE
> Task :processResources NO-SOURCE
> Task :classes UP-TO-DATE
> Task :compileTestJava UP-TO-DATE
> Task :processTestResources NO-SOURCE
> Task :testClasses UP-TO-DATE

> Task :test FAILED

com.example.UserServiceTest > testCreateUser() FAILED
    org.opentest4j.AssertionFailedError: expected: <201> but was: <400>
        at com.example.UserServiceTest.testCreateUser(UserServiceTest.java:42)

com.example.UserServiceTest > testDeleteUser() FAILED
    java.lang.NullPointerException: Cannot invoke method on null
        at com.example.UserServiceTest.testDeleteUser(UserServiceTest.java:67)

com.example.OrderServiceTest > testPlaceOrder() FAILED
    org.opentest4j.AssertionFailedError: expected: true but was: false
        at com.example.OrderServiceTest.testPlaceOrder(OrderServiceTest.java:31)

15 tests completed, 3 failed

FAILURE: Build failed with an exception.

* What went wrong:
Execution failed for task ':test'.
> There were failing tests. See the report at: file:///home/user/project/build/reports/tests/test/index.html

* Try:
> Run with --stacktrace option to get the stack trace.
> Run with --info or --debug option to get more log output.
> Run with --scan to get full insights.
> Get more help at https://help.gradle.org.

BUILD FAILED in 8s
6 actionable tasks: 1 executed, 5 up-to-date"#;

    // --- detect_gradle_binary ---

    #[test]
    fn test_detect_gradle_binary_returns_string() {
        let binary = detect_gradle_binary();
        assert!(
            binary == "gradle" || binary == "./gradlew" || binary == "gradlew.bat",
            "Expected gradle, ./gradlew, or gradlew.bat, got: {}",
            binary
        );
    }

    // --- filter_gradle_test: passing ---

    #[test]
    fn test_filter_gradle_test_all_pass_contains_success() {
        let result = filter_gradle_test(GRADLE_TEST_PASS);
        assert!(result.contains("BUILD SUCCESSFUL"));
    }

    #[test]
    fn test_filter_gradle_test_all_pass_no_task_lines() {
        let result = filter_gradle_test(GRADLE_TEST_PASS);
        assert!(!result.contains("> Task :compileJava"));
    }

    #[test]
    fn test_filter_gradle_test_all_pass_token_savings() {
        let result = filter_gradle_test(GRADLE_TEST_PASS);
        let input_tokens = count_tokens(GRADLE_TEST_PASS);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 30.0,
            "Expected >=30% savings on passing tests, got {:.1}%",
            savings
        );
    }

    // --- filter_gradle_test: failures ---

    #[test]
    fn test_filter_gradle_test_failures_shows_failed_tests() {
        let result = filter_gradle_test(GRADLE_TEST_FAIL);
        assert!(result.contains("testCreateUser"));
        assert!(result.contains("testDeleteUser"));
        assert!(result.contains("testPlaceOrder"));
    }

    #[test]
    fn test_filter_gradle_test_failures_shows_exception_details() {
        let result = filter_gradle_test(GRADLE_TEST_FAIL);
        assert!(result.contains("AssertionFailedError"));
        assert!(result.contains("NullPointerException"));
    }

    #[test]
    fn test_filter_gradle_test_failures_shows_summary() {
        let result = filter_gradle_test(GRADLE_TEST_FAIL);
        assert!(result.contains("15 tests completed, 3 failed"));
    }

    #[test]
    fn test_filter_gradle_test_failures_shows_what_went_wrong() {
        let result = filter_gradle_test(GRADLE_TEST_FAIL);
        assert!(result.contains("What went wrong"));
    }

    #[test]
    fn test_filter_gradle_test_failures_no_task_progress() {
        let result = filter_gradle_test(GRADLE_TEST_FAIL);
        assert!(!result.contains("> Task :compileJava UP-TO-DATE"));
        assert!(!result.contains("> Task :processResources"));
    }

    #[test]
    fn test_filter_gradle_test_failures_strips_try_advice() {
        let result = filter_gradle_test(GRADLE_TEST_FAIL);
        assert!(!result.contains("Run with --stacktrace"));
        assert!(!result.contains("Get more help"));
    }

    #[test]
    fn test_filter_gradle_test_failures_shows_build_failed() {
        let result = filter_gradle_test(GRADLE_TEST_FAIL);
        assert!(result.contains("BUILD FAILED"));
    }

    #[test]
    fn test_filter_gradle_test_failures_token_savings() {
        let result = filter_gradle_test(GRADLE_TEST_FAIL);
        let input_tokens = count_tokens(GRADLE_TEST_FAIL);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 30.0,
            "Expected >=30% savings on failing tests, got {:.1}%",
            savings
        );
    }

    // --- edge cases ---

    #[test]
    fn test_filter_gradle_test_empty_input() {
        let result = filter_gradle_test("");
        assert!(!result.is_empty());
    }

    // --- get_filter: submodule routing ---

    #[test]
    fn test_get_filter_matches_top_level_test() {
        assert!(get_filter("test").is_some());
    }

    #[test]
    fn test_get_filter_matches_submodule_test() {
        assert!(get_filter(":moduleA:test").is_some());
        assert!(get_filter(":app:test").is_some());
    }

    #[test]
    fn test_get_filter_no_match_for_other_tasks() {
        assert!(get_filter("build").is_none());
        assert!(get_filter("assemble").is_none());
        assert!(get_filter(":app:build").is_none());
    }
}
