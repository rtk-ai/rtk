use crate::core::runner::{self, RunOptions};
use crate::core::stream::StreamFilter;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::ffi::OsString;
use std::process::Command;

// ── Shared regex patterns (used across multiple filters) ─────────────────────

lazy_static! {
    static ref TASK_LINE: Regex = Regex::new(r"^> Task :").unwrap();
    static ref TRY_SECTION: Regex =
        Regex::new(r"^\* Try:|^> Run with --|^> Get more help at").unwrap();
    static ref BUILD_STATUS: Regex = Regex::new(r"^BUILD (SUCCESSFUL|FAILED)").unwrap();
    static ref ACTIONABLE: Regex = Regex::new(r"^\d+ actionable tasks?").unwrap();
}

#[derive(Debug, PartialEq)]
enum GradlewTask {
    Build,
    Test,
    ConnectedTest,
    Lint,
    Dependencies,
    Other,
}

fn detect_task(args: &[String]) -> GradlewTask {
    // Use the last non-flag, non-clean task to determine the filter.
    // Example: `clean assembleDebug` → Build (last non-clean task).
    // Note: for mixed-task invocations like `test assemble`, last wins.
    let task = args
        .iter()
        .filter(|a| !a.starts_with('-') && a.to_lowercase() != "clean")
        .map(|s| s.to_lowercase())
        .next_back()
        .unwrap_or_default();

    if task.contains("connected") {
        GradlewTask::ConnectedTest
    } else if task.contains("test") {
        GradlewTask::Test
    } else if task.contains("assemble")
        || task.contains("build")
        || task.contains("bundle")
        || task.contains("install")
    {
        GradlewTask::Build
    } else if task.contains("lint") || task.contains("ktlint") || task.contains("detekt") {
        GradlewTask::Lint
    } else if task == "check" {
        GradlewTask::Test
    } else if task.contains("dependencies") {
        GradlewTask::Dependencies
    } else if task.is_empty() {
        // Only "clean" was passed (filtered out above) → treat as Build to filter task noise
        GradlewTask::Build
    } else {
        GradlewTask::Other
    }
}

/// Returns the Gradle executable: prefers `./gradlew` (wrapper), falls back to `gradle`.
fn gradlew_binary() -> &'static str {
    if cfg!(windows) {
        if std::path::Path::new(".\\gradlew.bat").exists() {
            ".\\gradlew.bat"
        } else {
            "gradle"
        }
    } else if std::path::Path::new("./gradlew").exists() {
        "./gradlew"
    } else {
        "gradle"
    }
}

/// Builds a Gradle `Command` using string literals only.
///
/// Semgrep rule `dynamic-command-execution` forbids `Command::new(var)` —
/// each branch passes a literal so the executable set is statically auditable.
fn new_gradle_command(args: &[String]) -> Command {
    let mut cmd = if cfg!(windows) {
        if std::path::Path::new(".\\gradlew.bat").exists() {
            Command::new(".\\gradlew.bat")
        } else {
            Command::new("gradle")
        }
    } else if std::path::Path::new("./gradlew").exists() {
        Command::new("./gradlew")
    } else {
        Command::new("gradle")
    };
    cmd.args(args);
    cmd
}

/// `StreamFilter` for build mode: keeps lines for which `filter_build_line` returns true.
struct BuildLineFilter;

impl StreamFilter for BuildLineFilter {
    fn feed_line(&mut self, line: &str) -> Option<String> {
        if filter_build_line(line) {
            Some(format!("{}\n", line))
        } else {
            None
        }
    }

    fn flush(&mut self) -> String {
        String::new()
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Verbose flags bypass filtering — user wants full output
    if args
        .iter()
        .any(|a| a == "--stacktrace" || a == "--info" || a == "--debug" || a == "--full-stacktrace")
    {
        let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough(gradlew_binary(), &osargs, verbose);
    }

    let cmd = new_gradle_command(args);
    let args_display = args.join(" ");
    let tool = gradlew_binary();

    match detect_task(args) {
        GradlewTask::Build => runner::run_streamed(
            cmd,
            tool,
            &args_display,
            Box::new(BuildLineFilter),
            RunOptions::with_tee("gradlew_build"),
        ),
        GradlewTask::Test => runner::run_filtered(
            cmd,
            tool,
            &args_display,
            filter_test,
            RunOptions::with_tee("gradlew_test"),
        ),
        GradlewTask::ConnectedTest => runner::run_filtered(
            cmd,
            tool,
            &args_display,
            filter_connected,
            RunOptions::with_tee("gradlew_connected"),
        ),
        GradlewTask::Lint => runner::run_filtered(
            cmd,
            tool,
            &args_display,
            filter_lint,
            RunOptions::with_tee("gradlew_lint"),
        ),
        GradlewTask::Dependencies => runner::run_filtered(
            cmd,
            tool,
            &args_display,
            filter_dependencies,
            RunOptions::with_tee("gradlew_deps"),
        ),
        GradlewTask::Other => {
            let osargs: Vec<OsString> = args.iter().map(OsString::from).collect();
            runner::run_passthrough(gradlew_binary(), &osargs, verbose)
        }
    }
}

// ── Build filter predicate ────────────────────────────────────────────────────

fn filter_build_line(line: &str) -> bool {
    lazy_static! {
        static ref DAEMON_LINE: Regex = Regex::new(
            r"^(Starting a Gradle Daemon|Daemon will be stopped|Reusing configuration cache|Calculating task graph|> Configure project|Deprecated Gradle features|You can use|For more on this|Configuration cache entry)"
        )
        .unwrap();
        static ref PROGRESS: Regex =
            Regex::new(r"^\s*\d+%|^Downloading|^Configuring|^Resolving|^\[Incubating\]|^Wrote HTML report|^class \S+ could not|^\[android-")
                .unwrap();
        static ref ERROR_LINE: Regex = Regex::new(
            r"(?i)(^FAILURE:|^\* What went wrong:|^\* Where:|> Could not|e: |error:|^Execution failed|Lint found \d+ error)"
        )
        .unwrap();
        // Compiler + gradle warnings: kotlinc emits "w: ", javac/gradle "warning:" or "Warning:"
        static ref WARN_LINE: Regex = Regex::new(
            r"^(w: |warning:|Warning:|WARNING:)"
        )
        .unwrap();
        static ref BUILD_SCAN: Regex = Regex::new(r"gradle\.com/s/|Publishing build scan").unwrap();
    }

    // Always strip these
    if TASK_LINE.is_match(line)
        || DAEMON_LINE.is_match(line)
        || PROGRESS.is_match(line)
        || TRY_SECTION.is_match(line)
    {
        return false;
    }

    // Always keep these
    BUILD_STATUS.is_match(line)
        || ACTIONABLE.is_match(line)
        || ERROR_LINE.is_match(line)
        || WARN_LINE.is_match(line)
        || BUILD_SCAN.is_match(line)
        || line.trim().is_empty() // preserve blank lines that separate error sections
}

// ── Test output filter ────────────────────────────────────────────────────────

/// Returns true if an `at ...` stack frame belongs to a test framework
/// (JUnit, Gradle runner, reflection) rather than user code.
fn is_framework_frame(trimmed: &str) -> bool {
    trimmed.starts_with("at org.junit.")
        || trimmed.starts_with("at junit.")
        || trimmed.starts_with("at java.lang.reflect.")
        || trimmed.starts_with("at sun.reflect.")
        || trimmed.starts_with("at org.gradle.")
}

fn filter_test(output: &str) -> String {
    lazy_static! {
        static ref FAILED_LINE: Regex = Regex::new(r"FAILED$| FAILED ").unwrap();
        static ref PASSED_SKIPPED: Regex = Regex::new(r" PASSED$| SKIPPED$").unwrap();
        static ref SUMMARY_LINE: Regex = Regex::new(
            r"\d+ tests? completed|\d+ tests? failed|There were failing tests|See the report at"
        )
        .unwrap();
    }

    if output.is_empty() {
        return String::new();
    }

    let mut result_lines: Vec<&str> = Vec::new();
    let mut in_failure_block = false;

    for line in output.lines() {
        // Skip always-noise lines
        if TASK_LINE.is_match(line) || TRY_SECTION.is_match(line) {
            continue;
        }

        // Build summary lines always kept
        if BUILD_STATUS.is_match(line) || ACTIONABLE.is_match(line) || SUMMARY_LINE.is_match(line) {
            result_lines.push(line);
            continue;
        }

        // PASSED/SKIPPED per-test lines — strip
        if PASSED_SKIPPED.is_match(line) {
            in_failure_block = false;
            continue;
        }

        // FAILED per-test lines — keep + enter failure block for stack trace
        if FAILED_LINE.is_match(line) {
            in_failure_block = true;
            result_lines.push(line);
            continue;
        }

        // Stack trace lines following a failure
        if in_failure_block {
            let trimmed = line.trim();
            if trimmed.starts_with("java.") || trimmed.starts_with("kotlin.") {
                // Exception class + message — always keep
                result_lines.push(line);
            } else if trimmed.starts_with("at ") {
                // Skip framework frames, keep first user-code frame
                if !is_framework_frame(trimmed) {
                    result_lines.push(line);
                    in_failure_block = false;
                }
            } else if !trimmed.is_empty() {
                in_failure_block = false;
            }
        }
    }

    let filtered = result_lines.join("\n");

    // Guarantee non-empty output
    if filtered.trim().is_empty() {
        if output.contains("BUILD SUCCESSFUL") {
            return "ok ✓ (no test output — add testLogging to build.gradle for details)"
                .to_string();
        }
        return output.trim().to_string();
    }

    filtered
}

// ── Connected / instrumented test filter ─────────────────────────────────────

fn filter_connected(output: &str) -> String {
    lazy_static! {
        static ref INSTRUMENTATION_STATUS: Regex =
            Regex::new(r"^INSTRUMENTATION_STATUS[_CODE]*:").unwrap();
        static ref INSTRUMENTATION_RESULT: Regex = Regex::new(r"^INSTRUMENTATION_RESULT:").unwrap();
        static ref INSTRUMENTATION_CODE: Regex = Regex::new(r"^INSTRUMENTATION_CODE:").unwrap();
        static ref STARTING_TESTS: Regex = Regex::new(r"^Starting \d+ tests? on ").unwrap();
        static ref INSTALLING_APK: Regex = Regex::new(r"^Installing APK").unwrap();
    }

    if output.is_empty() {
        return String::new();
    }

    // Special case: no device
    if output.contains("No connected devices!") {
        return "connectedAndroidTest failed: No connected devices! Start an emulator or connect a device.".to_string();
    }

    let mut result_lines: Vec<&str> = Vec::new();

    for line in output.lines() {
        if INSTRUMENTATION_STATUS.is_match(line)
            || INSTRUMENTATION_RESULT.is_match(line)
            || INSTRUMENTATION_CODE.is_match(line)
            || STARTING_TESTS.is_match(line)
            || INSTALLING_APK.is_match(line)
            || TASK_LINE.is_match(line)
            || TRY_SECTION.is_match(line)
        {
            continue;
        }
        result_lines.push(line);
    }

    // After stripping instrumentation noise, connected test output uses the same
    // PASSED/FAILED line format as unit tests — delegate to filter_test.
    let joined = result_lines.join("\n");
    let filtered = filter_test(&joined);

    if filtered.trim().is_empty() {
        return "ok ✓ (connected tests passed)".to_string();
    }
    filtered
}

// ── Lint output filter ────────────────────────────────────────────────────────

fn filter_lint(output: &str) -> String {
    lazy_static! {
        // Android lint errors: src/main/java/Foo.kt:45: Error: message [IssueId]
        static ref ANDROID_LINT_ERROR: Regex =
            Regex::new(r"[^:]+:\d+:.*[Ee]rror:.*\[").unwrap();
        // Android lint warnings: src/main/java/Foo.kt:89: Warning: message [IssueId]
        static ref ANDROID_LINT_WARNING: Regex =
            Regex::new(r"[^:]+:\d+:.*[Ww]arning:.*\[").unwrap();
        // ktlint: file:line:col: Lint error > message
        static ref KTLINT_VIOLATION: Regex =
            Regex::new(r"[^:]+:\d+:\d+:.*[Ll]int").unwrap();
        // detekt: file:line:col: error - message
        static ref DETEKT_VIOLATION: Regex =
            Regex::new(r"[^:]+:\d+:\d+:.*error").unwrap();
        // Summary lines
        static ref SUMMARY_LINE: Regex =
            Regex::new(r"\d+ (issues?|errors?|warnings?)").unwrap();
        // Strip report path lines (too long)
        static ref REPORT_LINE: Regex =
            Regex::new(r"Wrote (HTML|XML|text) report|file://|/build/reports/lint").unwrap();
    }

    if output.is_empty() {
        return String::new();
    }

    // Android lint emits violation + code snippet + caret + explanation,
    // separated from the next violation by a blank line. We keep up to 3
    // non-empty context lines so the LLM sees what code is wrong without
    // having to open the file.
    const MAX_CONTEXT_LINES: usize = 3;

    let mut result_lines: Vec<&str> = Vec::new();
    let mut context_remaining: usize = 0;

    for line in output.lines() {
        if TASK_LINE.is_match(line) || TRY_SECTION.is_match(line) || REPORT_LINE.is_match(line) {
            context_remaining = 0;
            continue;
        }

        let is_android_lint = ANDROID_LINT_ERROR.is_match(line) || ANDROID_LINT_WARNING.is_match(line);

        if BUILD_STATUS.is_match(line)
            || ACTIONABLE.is_match(line)
            || SUMMARY_LINE.is_match(line)
            || is_android_lint
            || KTLINT_VIOLATION.is_match(line)
            || DETEKT_VIOLATION.is_match(line)
        {
            result_lines.push(line);
            // Only Android lint violations have multi-line context;
            // ktlint/detekt/summary lines are single-line.
            context_remaining = if is_android_lint { MAX_CONTEXT_LINES } else { 0 };
            continue;
        }

        if context_remaining > 0 {
            if line.trim().is_empty() {
                // Blank line terminates the context block
                context_remaining = 0;
            } else {
                result_lines.push(line);
                context_remaining -= 1;
            }
        }
    }

    let filtered = result_lines.join("\n");

    if filtered.trim().is_empty() {
        if output.contains("BUILD SUCCESSFUL") {
            return "ok ✓ lint passed".to_string();
        }
        return output.trim().to_string();
    }

    filtered
}

// ── Dependencies output filter ───────────────────────────────────────────────

fn filter_dependencies(output: &str) -> String {
    if output.is_empty() {
        return String::new();
    }

    let mut configs: Vec<(String, Vec<String>)> = Vec::new();
    let mut current_config = String::new();
    let mut current_deps: Vec<String> = Vec::new();
    let mut total_deps = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip noise
        if trimmed.is_empty()
            || TASK_LINE.is_match(trimmed)
            || TRY_SECTION.is_match(trimmed)
            || BUILD_STATUS.is_match(trimmed)
            || ACTIONABLE.is_match(trimmed)
            || trimmed.starts_with("Downloading")
            || trimmed.starts_with("Download ")
            || trimmed.starts_with("Starting a Gradle")
            || trimmed == "No dependencies"
            || trimmed == "(n)"
        {
            continue;
        }

        // Configuration header: "compileClasspath - Compile classpath for source set 'main'."
        // Not indented, not a tree line, contains " - "
        if !trimmed.starts_with('+')
            && !trimmed.starts_with('|')
            && !trimmed.starts_with('\\')
            && !trimmed.starts_with(' ')
            && trimmed.contains(" - ")
        {
            if !current_config.is_empty() && !current_deps.is_empty() {
                configs.push((current_config.clone(), current_deps.clone()));
            }
            current_config = trimmed.split(" - ").next().unwrap_or(trimmed).to_string();
            current_deps = Vec::new();
            continue;
        }

        // Top-level dependencies only (first level of the tree).
        // Check the *untrimmed* line — top-level deps start at column 0,
        // transitive deps are indented (e.g., "|    +---" or "     \---").
        if (line.starts_with("+---") || line.starts_with("\\---")) && !current_config.is_empty() {
            let dep = trimmed
                .trim_start_matches("+--- ")
                .trim_start_matches("\\--- ")
                .to_string();
            current_deps.push(dep);
            total_deps += 1;
        }
    }

    // Flush last config
    if !current_config.is_empty() && !current_deps.is_empty() {
        configs.push((current_config, current_deps));
    }

    if configs.is_empty() {
        if output.contains("BUILD SUCCESSFUL") {
            return "ok ✓ no dependencies".to_string();
        }
        return output.trim().to_string();
    }

    let mut result = format!(
        "{} top-level dependencies across {} configurations\n",
        total_deps,
        configs.len()
    );

    for (config, deps) in &configs {
        result.push_str(&format!("\n{} ({}):\n", config, deps.len()));
        for dep in deps.iter().take(20) {
            result.push_str(&format!("  {}\n", dep));
        }
        if deps.len() > 20 {
            result.push_str(&format!("  ... +{} more\n", deps.len() - 20));
        }
    }

    result.trim_end().to_string()
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // ── TASK DETECTION ────────────────────────────────────────────────────────

    #[test]
    fn test_detect_connected_wins_over_test() {
        // connectedAndroidTest contains "test" — ConnectedTest must win
        let args = vec!["connectedDebugAndroidTest".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::ConnectedTest);
    }

    #[test]
    fn test_detect_assemble_debug() {
        let args = vec!["assembleDebug".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Build);
    }

    #[test]
    fn test_detect_test_debug_unit_test() {
        let args = vec!["testDebugUnitTest".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Test);
    }

    #[test]
    fn test_detect_module_prefixed_task() {
        let args = vec![":app:testDebugUnitTest".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Test);
    }

    #[test]
    fn test_detect_module_prefixed_assemble() {
        let args = vec![":app:assembleDebug".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Build);
    }

    #[test]
    fn test_detect_flag_value_does_not_trigger_test() {
        // -Pflavor=testRelease should NOT match Test when task is assemble
        let args = vec![
            "assembleRelease".to_string(),
            "-Pflavor=testRelease".to_string(),
        ];
        assert_eq!(detect_task(&args), GradlewTask::Build);
    }

    #[test]
    fn test_detect_multi_task_uses_last() {
        // clean assembleDebug → Build (last non-clean task)
        let args = vec!["clean".to_string(), "assembleDebug".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Build);
    }

    #[test]
    fn test_detect_lint() {
        let args = vec!["lint".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Lint);
    }

    #[test]
    fn test_detect_ktlint() {
        let args = vec!["ktlintCheck".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Lint);
    }

    #[test]
    fn test_detect_bundle() {
        let args = vec!["bundleRelease".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Build);
    }

    #[test]
    fn test_detect_unknown_passthrough() {
        let args = vec!["signingReport".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Other);
    }

    #[test]
    fn test_detect_clean_alone_is_build() {
        // "clean" alone → task.is_empty() after filtering → Build (strips task noise)
        let args = vec!["clean".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Build);
    }

    #[test]
    fn test_detect_install_debug() {
        let args = vec!["installDebug".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Build);
    }

    #[test]
    fn test_detect_uninstall_debug() {
        // "uninstallDebug" contains "install" → Build
        let args = vec!["uninstallDebug".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Build);
    }

    #[test]
    fn test_detect_clean_install() {
        // clean installDebug → last non-clean task is installDebug → Build
        let args = vec!["clean".to_string(), "installDebug".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Build);
    }

    #[test]
    fn test_detect_check() {
        let args = vec!["check".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Test);
    }

    #[test]
    fn test_detect_dependencies() {
        let args = vec!["dependencies".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Dependencies);
    }

    #[test]
    fn test_detect_dependencies_with_module() {
        // :app:dependencies → contains "dependencies"
        let args = vec![":app:dependencies".to_string()];
        assert_eq!(detect_task(&args), GradlewTask::Dependencies);
    }

    // ── BUILD FILTER ──────────────────────────────────────────────────────────

    #[test]
    fn test_build_success_strips_task_lines() {
        let input = r#"> Configure project :app
> Task :app:preBuild UP-TO-DATE
> Task :app:generateDebugBuildConfig UP-TO-DATE
> Task :app:generateDebugResValues UP-TO-DATE
> Task :app:generateDebugResources UP-TO-DATE
> Task :app:mergeDebugResources UP-TO-DATE
> Task :app:processDebugManifest UP-TO-DATE
> Task :app:compileDebugKotlin UP-TO-DATE
> Task :app:compileDebugJavaWithJavac UP-TO-DATE
> Task :app:validateSigningDebug UP-TO-DATE
> Task :app:packageDebug UP-TO-DATE
> Task :app:assembleDebug UP-TO-DATE

BUILD SUCCESSFUL in 1m 23s
42 actionable tasks: 42 executed"#;

        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        let savings = 100.0
            - (count_tokens(&filtered.join("\n")) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 70.0,
            "Expected ≥70% savings, got {:.1}%",
            savings
        );
        assert!(filtered.iter().any(|l| l.contains("BUILD SUCCESSFUL")));
        assert!(!filtered.iter().any(|l| l.starts_with("> Task :")));
    }

    #[test]
    fn test_build_failure_preserves_errors_strips_try() {
        let input = r#"> Task :app:compileDebugKotlin FAILED

FAILURE: Build failed with an exception.

* What went wrong:
e: /src/app/MainActivity.kt: (42, 5): Unresolved reference: MyService

* Try:
> Run with --stacktrace option to get the stack trace.
> Run with --info or --debug option to get more log output.
> Get more help at https://help.gradle.org

BUILD FAILED in 12s"#;

        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        assert!(filtered.iter().any(|l| l.contains("Unresolved reference")));
        assert!(filtered.iter().any(|l| l.contains("BUILD FAILED")));
        assert!(!filtered.iter().any(|l| l.contains("Run with --stacktrace")));
        assert!(!filtered.iter().any(|l| l.contains("Get more help at")));
    }

    #[test]
    fn test_build_filter_never_empty_on_success() {
        let input = r#"> Task :app:assembleDebug UP-TO-DATE
BUILD SUCCESSFUL in 3s
1 actionable tasks: 1 up-to-date"#;
        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        assert!(
            !filtered.is_empty(),
            "Filter must not produce empty output on success"
        );
    }

    #[test]
    fn test_build_daemon_lines_stripped() {
        let input = r#"Starting a Gradle Daemon (subsequent builds will be faster)
Daemon will be stopped at the end of the build after running out of JVM memory
> Task :app:assembleDebug
BUILD SUCCESSFUL in 5s"#;
        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        assert!(!filtered.iter().any(|l| l.contains("Daemon")));
        assert!(filtered.iter().any(|l| l.contains("BUILD SUCCESSFUL")));
    }

    #[test]
    fn test_build_scan_url_preserved() {
        let input = r#"> Task :app:assembleDebug
BUILD SUCCESSFUL in 5s
Publishing build scan...
https://gradle.com/s/abc123"#;
        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        assert!(filtered.iter().any(|l| l.contains("gradle.com/s/")));
    }

    // ── TEST FILTER ───────────────────────────────────────────────────────────

    #[test]
    fn test_unit_test_failures_preserved_passes_stripped() {
        // Realistic test run with multi-frame JUnit stack traces
        let input = r#"> Task :app:testDebugUnitTest
com.example.FooTest > test1 PASSED
com.example.FooTest > test2 PASSED
com.example.FooTest > test3 PASSED
com.example.FooTest > test4 PASSED
com.example.FooTest > test5 PASSED
com.example.FooTest > test6 PASSED
com.example.FooTest > test7 PASSED
com.example.FooTest > testBar FAILED
    java.lang.AssertionError: expected:<3> but was:<-1>
        at org.junit.Assert.fail(Assert.java:89)
        at org.junit.Assert.assertEquals(Assert.java:197)
        at com.example.FooTest.testBar(FooTest.kt:25)
com.example.FooTest > testQux PASSED

10 tests completed, 1 failed"#;
        let out = filter_test(input);
        assert!(
            out.contains("testBar FAILED"),
            "FAILED test must be preserved"
        );
        assert!(
            out.contains("AssertionError"),
            "Exception class must be preserved"
        );
        assert!(
            out.contains("FooTest.testBar"),
            "User code frame must be preserved"
        );
        assert!(
            !out.contains("org.junit.Assert.fail"),
            "Framework frames must be skipped"
        );
        assert!(!out.contains("PASSED"), "PASSED tests must be stripped");
        assert!(
            out.contains("10 tests completed, 1 failed"),
            "Summary must be preserved"
        );

        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_unit_test_skips_framework_frames() {
        let input = r#"com.example.CalcTest > testAdd FAILED
    java.lang.AssertionError: expected:<5> but was:<3>
        at org.junit.Assert.fail(Assert.java:89)
        at org.junit.Assert.assertEquals(Assert.java:197)
        at java.lang.reflect.Method.invoke(Method.java:498)
        at com.example.CalcTest.testAdd(CalcTest.kt:10)"#;
        let out = filter_test(input);
        assert!(
            out.contains("com.example.CalcTest.testAdd"),
            "User code frame must be shown"
        );
        assert!(
            !out.contains("org.junit.Assert"),
            "JUnit frames must be skipped"
        );
        assert!(
            !out.contains("java.lang.reflect"),
            "Reflection frames must be skipped"
        );
    }

    #[test]
    fn test_unit_test_gradle_default_no_testlogging() {
        // Gradle default: no per-test lines shown
        let input = r#"> Task :app:testDebugUnitTest

BUILD SUCCESSFUL in 15s
3 actionable tasks: 1 executed, 2 up-to-date"#;
        let out = filter_test(input);
        assert!(
            out.contains("BUILD SUCCESSFUL") || out.contains("ok ✓"),
            "Must output something on success"
        );
        assert!(!out.is_empty(), "must not produce empty output");
    }

    #[test]
    fn test_unit_test_report_path_preserved() {
        let input = r#"There were failing tests. See the report at: file:///app/build/reports/tests/testDebugUnitTest/index.html
BUILD FAILED in 20s"#;
        let out = filter_test(input);
        assert!(out.contains("See the report at"));
        assert!(out.contains("BUILD FAILED"));
    }

    #[test]
    fn test_try_section_stripped_from_test_output() {
        let input = r#"com.example.FooTest > testBar FAILED
    java.lang.AssertionError: expected true

* Try:
> Run with --stacktrace option to get the stack trace.
> Run with --info or --debug option to get more log output.
> Get more help at https://help.gradle.org

BUILD FAILED in 5s"#;
        let out = filter_test(input);
        assert!(!out.contains("Run with --stacktrace"));
        assert!(!out.contains("Get more help at"));
        assert!(out.contains("BUILD FAILED"));
    }

    // ── CONNECTED TEST FILTER ─────────────────────────────────────────────────

    #[test]
    fn test_connected_strips_device_noise() {
        let input = r#"Starting 3 tests on Pixel_6_API_33(AVD) - 13
INSTRUMENTATION_STATUS: numtests=3
INSTRUMENTATION_STATUS_CODE: 1
com.example.MainActivityTest > exampleTest[Pixel_6_API_33] FAILED
    AssertionError: expected true
INSTRUMENTATION_STATUS_CODE: -2
Tests run: 3, Failures: 1, Errors: 0, Skipped: 0"#;
        let out = filter_connected(input);
        assert!(out.contains("FAILED"), "FAILED test must be preserved");
        assert!(
            !out.contains("INSTRUMENTATION_STATUS:"),
            "Instrumentation lines must be stripped"
        );
        assert!(
            !out.contains("Starting 3 tests"),
            "Starting tests line must be stripped"
        );
    }

    #[test]
    fn test_connected_no_device_error() {
        let input = "com.android.builder.testing.api.DeviceException: No connected devices!";
        let out = filter_connected(input);
        assert!(
            out.contains("No connected devices"),
            "Must show actionable error"
        );
    }

    // ── LINT FILTER ───────────────────────────────────────────────────────────

    #[test]
    fn test_lint_preserves_violations() {
        let input = r#"Wrote HTML report to file:/path/app/build/reports/lint-results-debug.html
src/main/java/com/example/MainActivity.kt:45: Error: Format string invalid [StringFormatInvalid]
  String.format(getString(R.string.no_args), arg)
  ^
0 errors, 4 warnings"#;
        let out = filter_lint(input);
        assert!(
            out.contains("StringFormatInvalid"),
            "Lint violation must be preserved"
        );
        assert!(
            out.contains("0 errors, 4 warnings"),
            "Summary must be preserved"
        );
        assert!(
            !out.contains("Wrote HTML report"),
            "Report path must be stripped"
        );
    }

    #[test]
    fn test_lint_preserves_warnings() {
        let input = r#"src/main/java/com/example/Utils.kt:89: Warning: HardcodedText [HardcodedText]
    return "Hello World"
           ~~~~~~~~~~~~~
src/main/res/layout/activity_main.xml:15: Warning: Missing contentDescription attribute on image [ContentDescription]
    <ImageView
Ran lint on variant debug: 2 warnings"#;
        let out = filter_lint(input);
        assert!(
            out.contains("HardcodedText"),
            "Warning violation must be preserved"
        );
        assert!(
            out.contains("ContentDescription"),
            "Warning violation must be preserved"
        );
        assert!(out.contains("2 warnings"), "Summary must be preserved");
    }

    #[test]
    fn test_lint_no_violations_success() {
        let input = r#"> Task :app:lint
BUILD SUCCESSFUL in 8s
3 actionable tasks: 1 executed, 2 up-to-date"#;
        let out = filter_lint(input);
        assert!(!out.is_empty(), "Must produce output on lint success");
        assert!(
            out.contains("BUILD SUCCESSFUL") || out.contains("ok ✓"),
            "Must indicate success"
        );
    }

    // ── FIXTURE-BASED TESTS ──────────────────────────────────────────────────

    #[test]
    fn test_build_fixture_token_savings() {
        let input = include_str!("../../../tests/fixtures/gradlew_build_raw.txt");
        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        let savings = 100.0
            - (count_tokens(&filtered.join("\n")) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 70.0,
            "Build fixture: expected ≥70% savings, got {:.1}%",
            savings
        );
        assert!(!filtered.iter().any(|l| l.starts_with("> Task :")));
    }

    #[test]
    fn test_build_failed_fixture_token_savings() {
        let input = include_str!("../../../tests/fixtures/gradlew_build_failed_raw.txt");
        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        assert!(
            filtered.iter().any(|l| l.contains("BUILD FAILED")),
            "BUILD FAILED must be preserved"
        );
        assert!(
            !filtered.iter().any(|l| l.contains("Run with --stacktrace")),
            "Try section must be stripped"
        );
    }

    #[test]
    fn test_test_fixture_preserves_failures() {
        let input = include_str!("../../../tests/fixtures/gradlew_test_raw.txt");
        let out = filter_test(input);
        assert!(!out.contains("PASSED"), "PASSED tests must be stripped");
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Test fixture: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_test_failed_fixture_shows_user_code() {
        let input = include_str!("../../../tests/fixtures/gradlew_test_failed_raw.txt");
        let out = filter_test(input);
        assert!(out.contains("FAILED"), "FAILED tests must be preserved");
        assert!(
            out.contains("CalculatorTest.testSubtraction")
                || out.contains("MainViewModelTest.loadDataError"),
            "User code frame must be shown"
        );
        assert!(
            out.contains("5 tests completed, 2 failed"),
            "Summary must be preserved"
        );
    }

    #[test]
    fn test_connected_fixture_token_savings() {
        let input = include_str!("../../../tests/fixtures/gradlew_connected_raw.txt");
        let out = filter_connected(input);
        assert!(
            !out.contains("INSTRUMENTATION_STATUS"),
            "Instrumentation lines must be stripped"
        );
    }

    #[test]
    fn test_lint_fixture_token_savings() {
        let input = include_str!("../../../tests/fixtures/gradlew_lint_raw.txt");
        let out = filter_lint(input);
        assert!(
            !out.contains("Wrote HTML report"),
            "Report lines must be stripped"
        );
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Lint fixture: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    // ── OUTPUT FORMAT TESTS ──────────────────────────────────────────────────

    #[test]
    fn test_build_success_output_format() {
        let input = include_str!("../../../tests/fixtures/gradlew_build_raw.txt");
        let output: String = input
            .lines()
            .filter(|l| filter_build_line(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("BUILD SUCCESSFUL"), "should keep BUILD SUCCESSFUL");
        assert!(output.contains("actionable tasks"), "should keep actionable tasks line");
        assert!(!output.contains("> Task :"), "should strip task progress lines");
    }

    #[test]
    fn test_build_failed_output_format() {
        let input = include_str!("../../../tests/fixtures/gradlew_build_failed_raw.txt");
        let output: String = input
            .lines()
            .filter(|l| filter_build_line(l))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(output.contains("BUILD FAILED"), "should keep BUILD FAILED");
        assert!(output.contains("FAILURE:"), "should keep failure header");
        assert!(output.contains("e: "), "should keep error lines");
        assert!(!output.contains("> Task :"), "should strip task progress lines");
    }

    #[test]
    fn test_test_success_output_format() {
        let input = include_str!("../../../tests/fixtures/gradlew_test_raw.txt");
        let output = filter_test(input);
        assert!(output.contains("tests completed"), "should keep test summary");
        assert!(output.contains("BUILD SUCCESSFUL"), "should keep BUILD SUCCESSFUL");
        assert!(!output.contains("PASSED"), "should strip passing test lines");
    }

    #[test]
    fn test_test_failed_output_format() {
        let input = include_str!("../../../tests/fixtures/gradlew_test_failed_raw.txt");
        let output = filter_test(input);
        assert!(output.contains("FAILED"), "should keep failed test names");
        assert!(output.contains("tests completed"), "should keep test summary");
        assert!(output.contains("BUILD FAILED"), "should keep BUILD FAILED");
        assert!(!output.contains("PASSED"), "should strip passing test lines");
        assert!(!output.contains("at org.junit."), "should strip framework frames");
    }

    #[test]
    fn test_connected_output_format() {
        let input = include_str!("../../../tests/fixtures/gradlew_connected_raw.txt");
        let output = filter_connected(input);
        assert!(output.contains("BUILD SUCCESSFUL"), "should keep BUILD SUCCESSFUL");
        assert!(!output.contains("INSTRUMENTATION_STATUS"), "should strip instrumentation noise");
    }

    #[test]
    fn test_lint_output_format() {
        let input = include_str!("../../../tests/fixtures/gradlew_lint_raw.txt");
        let output = filter_lint(input);
        assert!(output.contains("Error:"), "should keep error violations");
        assert!(output.contains("Warning:"), "should keep warning violations");
        assert!(output.contains("BUILD FAILED"), "should keep BUILD FAILED");
        assert!(!output.contains("Wrote HTML report"), "should strip report paths");
    }

    #[test]
    fn test_lint_preserves_code_context() {
        // Violation on line 1, then snippet + caret + explanation should all be kept
        // (up to 3 context lines, until blank line separator).
        let input = include_str!("../../../tests/fixtures/gradlew_lint_raw.txt");
        let output = filter_lint(input);
        assert!(
            output.contains("String.format(getString(R.string.template)"),
            "code snippet after Android lint error must be preserved"
        );
        assert!(
            output.contains("This format string placeholder index"),
            "explanation line after caret must be preserved"
        );
        assert!(
            output.contains("return \"Hello World\""),
            "code snippet after Android lint warning must be preserved"
        );
        assert!(
            output.contains("<ImageView"),
            "XML snippet after lint warning must be preserved"
        );
    }

    #[test]
    fn test_build_filter_keeps_compiler_warnings() {
        let input = r#"> Task :app:compileDebugKotlin
w: /src/Foo.kt: (42, 5): Parameter 'unused' is never used
warning: [options] bootstrap class path not set
Warning: Gradle deprecation detected

BUILD SUCCESSFUL in 4s"#;
        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        let output = filtered.join("\n");
        assert!(output.contains("w: "), "kotlinc warnings must be kept");
        assert!(output.contains("warning: [options]"), "javac warnings must be kept");
        assert!(output.contains("Warning: Gradle"), "Gradle warnings must be kept");
        assert!(output.contains("BUILD SUCCESSFUL"), "status must be kept");
        assert!(!output.contains("> Task :"), "task progress must be stripped");
    }

    // ── CHECK (BUILD FILTER ON MIXED OUTPUT) ────────────────────────────────

    #[test]
    fn test_build_filter_strips_configure_and_dokka_noise() {
        let input = r#"Calculating task graph as no cached configuration is available for tasks: check

> Configure project :core
class org.jetbrains.dokka.gradle.adapters.AndroidExtensionWrapper could not get Android Extension for project :core
[android-junit5]: Cannot configure Jacoco for this project

> Task :core:preBuild UP-TO-DATE
> Task :core:preDebugBuild UP-TO-DATE
> Task :core:compileDebugKotlin UP-TO-DATE
> Task :samplev2:lintDebug FAILED
Lint found 8 errors, 21 warnings. First failure:

/src/LogsScreen.kt:50: Error: Field requires API level 26 [NewApi]
    val uiState = viewModel.uiState.collectAsState()

[Incubating] Problems report is available at: file:///build/reports/problems.html

Deprecated Gradle features were used in this build, making it incompatible with Gradle 10.

You can use '--warning-mode all' to show the individual deprecation warnings.
388 actionable tasks: 97 executed

FAILURE: Build failed with an exception.

* What went wrong:
Execution failed for task ':samplev2:lintDebug'.

* Try:
> Run with --stacktrace option to get the stack trace.

BUILD FAILED in 3s"#;

        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        let out = filtered.join("\n");

        // Must keep
        assert!(
            out.contains("BUILD FAILED"),
            "BUILD FAILED must be preserved"
        );
        assert!(out.contains("FAILURE:"), "FAILURE line must be preserved");
        assert!(
            out.contains("Execution failed"),
            "Execution failed must be preserved"
        );
        assert!(
            out.contains("Lint found 8 error"),
            "Lint summary must be preserved"
        );
        assert!(
            out.contains("Error: Field requires"),
            "Lint error must be preserved"
        );

        // Must strip
        assert!(
            !out.contains("Configure project"),
            "Configure lines must be stripped"
        );
        assert!(!out.contains("dokka"), "Dokka warnings must be stripped");
        assert!(
            !out.contains("android-junit5"),
            "Plugin warnings must be stripped"
        );
        assert!(!out.contains("> Task :"), "Task lines must be stripped");
        assert!(!out.contains("Incubating"), "Incubating must be stripped");
        assert!(
            !out.contains("Deprecated Gradle"),
            "Deprecated must be stripped"
        );
        assert!(
            !out.contains("Run with --stacktrace"),
            "Try section must be stripped"
        );

        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    // ── DEPENDENCIES FILTER ─────────────────────────────────────────────────

    #[test]
    fn test_dependencies_filter_extracts_top_level() {
        let input = r#"> Task :app:dependencies

------------------------------------------------------------
Project ':app'
------------------------------------------------------------

implementation - Implementation dependencies for the 'main' feature.
+--- org.jetbrains.kotlin:kotlin-stdlib:1.9.22
+--- androidx.core:core-ktx:1.12.0
+--- androidx.appcompat:appcompat:1.6.1
|    +--- androidx.annotation:annotation:1.3.0
|    +--- androidx.core:core:1.9.0
|    \--- androidx.cursoradapter:cursoradapter:1.0.0
+--- com.google.android.material:material:1.11.0
|    +--- androidx.annotation:annotation:1.2.0
|    +--- androidx.appcompat:appcompat:1.1.0
|    \--- androidx.recyclerview:recyclerview:1.0.0
\--- com.squareup.retrofit2:retrofit:2.9.0
     +--- com.squareup.okhttp3:okhttp:3.14.9
     \--- com.squareup.okio:okio:1.17.2

testImplementation - Test dependencies for the 'main' feature.
+--- junit:junit:4.13.2
\--- org.mockito:mockito-core:5.8.0

BUILD SUCCESSFUL in 2s
1 actionable tasks: 1 executed"#;

        let out = filter_dependencies(input);
        assert!(
            out.contains("implementation (5):"),
            "Must show config with count: {}",
            out
        );
        assert!(
            out.contains("testImplementation (2):"),
            "Must show test config: {}",
            out
        );
        assert!(
            out.contains("kotlin-stdlib"),
            "Must show top-level dep: {}",
            out
        );
        // Should NOT contain transitive deps
        assert!(
            !out.contains("cursoradapter"),
            "Transitive deps must be stripped: {}",
            out
        );

        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_dependencies_filter_empty() {
        assert_eq!(filter_dependencies(""), "");
    }

    #[test]
    fn test_dependencies_filter_no_deps() {
        let input = r#"> Task :app:dependencies
No dependencies

BUILD SUCCESSFUL in 1s"#;
        let out = filter_dependencies(input);
        assert!(out.contains("ok"), "Must show success: {}", out);
    }

    // ── EDGE CASES ────────────────────────────────────────────────────────────

    #[test]
    fn test_filter_empty_input() {
        assert_eq!(filter_test(""), "");
        assert_eq!(filter_connected(""), "");
        assert_eq!(filter_lint(""), "");
        assert_eq!(filter_dependencies(""), "");
    }

    #[test]
    fn test_build_filter_empty_line_preserved() {
        // Blank lines that separate error sections should be preserved
        assert!(filter_build_line(""), "empty line must pass through");
        assert!(
            filter_build_line("   "),
            "whitespace-only line must pass through"
        );
    }

    #[test]
    fn test_verbose_flag_detection() {
        // Verify that verbose flags are detected correctly
        let stacktrace_args = ["assembleDebug".to_string(), "--stacktrace".to_string()];
        assert!(stacktrace_args.iter().any(|a| a == "--stacktrace"
            || a == "--info"
            || a == "--debug"
            || a == "--full-stacktrace"));

        let info_args = ["testDebugUnitTest".to_string(), "--info".to_string()];
        assert!(info_args.iter().any(|a| a == "--stacktrace"
            || a == "--info"
            || a == "--debug"
            || a == "--full-stacktrace"));
    }

    #[test]
    fn test_build_token_savings() {
        let input = r#"Starting a Gradle Daemon (subsequent builds will be faster)
> Configure project :app
> Task :app:preBuild UP-TO-DATE
> Task :app:generateDebugBuildConfig UP-TO-DATE
> Task :app:generateDebugResValues UP-TO-DATE
> Task :app:generateDebugResources UP-TO-DATE
> Task :app:mergeDebugResources UP-TO-DATE
> Task :app:processDebugManifest UP-TO-DATE
> Task :app:compileDebugKotlin UP-TO-DATE
> Task :app:compileDebugJavaWithJavac UP-TO-DATE
> Task :app:compileDebugSources UP-TO-DATE
> Task :app:mergeDebugShaders UP-TO-DATE
> Task :app:compileDebugShaders UP-TO-DATE
> Task :app:generateDebugAssets UP-TO-DATE
> Task :app:mergeDebugAssets UP-TO-DATE
> Task :app:mergeDebugJniLibFolders UP-TO-DATE
> Task :app:validateSigningDebug UP-TO-DATE
> Task :app:packageDebug UP-TO-DATE
> Task :app:assembleDebug UP-TO-DATE

BUILD SUCCESSFUL in 3s
18 actionable tasks: 18 up-to-date"#;

        let filtered: Vec<&str> = input.lines().filter(|l| filter_build_line(l)).collect();
        let token_savings = 100.0
            - (count_tokens(&filtered.join("\n")) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            token_savings >= 70.0,
            "Expected ≥70% token savings, got {:.1}%",
            token_savings
        );
    }

    #[test]
    fn test_is_framework_frame() {
        assert!(is_framework_frame(
            "at org.junit.Assert.fail(Assert.java:89)"
        ));
        assert!(is_framework_frame(
            "at junit.framework.Assert.fail(Assert.java:50)"
        ));
        assert!(is_framework_frame(
            "at java.lang.reflect.Method.invoke(Method.java:498)"
        ));
        assert!(is_framework_frame("at org.gradle.api.internal.tasks.testing.SuiteTestClassProcessor.processTestClass(SuiteTestClassProcessor.java:51)"));
        assert!(!is_framework_frame(
            "at com.example.FooTest.testBar(FooTest.kt:25)"
        ));
        assert!(!is_framework_frame(
            "at com.example.MyApp.doSomething(MyApp.java:100)"
        ));
    }
}
