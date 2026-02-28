use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Detect whether to use ./gradlew (wrapper) or gradle
fn gradle_executable() -> &'static str {
    if Path::new("./gradlew").exists() {
        "./gradlew"
    } else {
        "gradle"
    }
}

/// Generic gradle command runner with filtering
fn run_gradle_filtered<F>(
    subcommand: &str,
    args: &[String],
    verbose: u8,
    filter_fn: F,
) -> Result<()>
where
    F: Fn(&str) -> String,
{
    let timer = tracking::TimedExecution::start();
    let gradle = gradle_executable();

    let mut cmd = Command::new(gradle);
    cmd.arg(subcommand);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {} {}", gradle, subcommand, args.join(" "));
    }

    let output = cmd.output().with_context(|| {
        format!(
            "Failed to run {} {}. Is Gradle installed or is ./gradlew available?",
            gradle, subcommand
        )
    })?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = filter_fn(&raw);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, &format!("gradle_{}", subcommand), exit_code)
    {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("gradle {} {}", subcommand, args.join(" ")),
        &format!("rtk gradle {} {}", subcommand, args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

pub fn run_build(args: &[String], verbose: u8) -> Result<()> {
    run_gradle_filtered("build", args, verbose, filter_gradle_build)
}

pub fn run_test(args: &[String], verbose: u8) -> Result<()> {
    run_gradle_filtered("test", args, verbose, filter_gradle_test)
}

pub fn run_clean(args: &[String], verbose: u8) -> Result<()> {
    run_gradle_filtered("clean", args, verbose, filter_gradle_build)
}

pub fn run_assemble(args: &[String], verbose: u8) -> Result<()> {
    run_gradle_filtered("assemble", args, verbose, filter_gradle_build)
}

pub fn run_dependencies(args: &[String], verbose: u8) -> Result<()> {
    run_gradle_filtered("dependencies", args, verbose, filter_gradle_dependencies)
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("gradle: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();
    let gradle = gradle_executable();
    let subcommand = args[0].to_string_lossy();

    let mut cmd = Command::new(gradle);
    cmd.arg(&*subcommand);
    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {} ...", gradle, subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {} {}", gradle, subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    print!("{}", stdout);
    eprint!("{}", stderr);

    timer.track(
        &format!("gradle {}", subcommand),
        &format!("rtk gradle {}", subcommand),
        &raw,
        &raw,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Filter Gradle build/assemble/clean output.
/// Strips download progress, configuration lines, and task noise.
/// Keeps: errors, warnings, BUILD result, and actionable output.
fn filter_gradle_build(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut error_count = 0;
    let mut warning_count = 0;
    let mut in_error = false;
    let mut current_error: Vec<String> = Vec::new();
    let mut build_result = String::new();
    let mut task_count = 0;
    let mut actionable_tasks = 0;
    let mut up_to_date = 0;
    let mut from_cache = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip noise: downloads, progress indicators, empty lines
        if trimmed.is_empty()
            || trimmed.starts_with("Downloading")
            || trimmed.starts_with("Download ")
            || trimmed.starts_with("Starting a Gradle")
            || trimmed.starts_with("Welcome to Gradle")
            || trimmed.starts_with("To honour the JVM settings")
            || trimmed.starts_with("Daemon will be")
            || trimmed.starts_with("Configuration on demand")
            || trimmed.starts_with("Calculating task graph")
            || trimmed.starts_with("Type-safe ")
            || trimmed.starts_with("See https://")
            || trimmed.starts_with("https://")
        {
            continue;
        }

        // Track task execution for summary
        if trimmed.starts_with("> Task :") {
            task_count += 1;
            if trimmed.contains("UP-TO-DATE") {
                up_to_date += 1;
            } else if trimmed.contains("FROM-CACHE") {
                from_cache += 1;
            } else if !trimmed.contains("SKIPPED") && !trimmed.contains("NO-SOURCE") {
                actionable_tasks += 1;
            }
            continue;
        }

        // Capture BUILD SUCCESSFUL/FAILED line
        if trimmed.starts_with("BUILD SUCCESSFUL") || trimmed.starts_with("BUILD FAILED") {
            build_result = trimmed.to_string();
            continue;
        }

        // Skip timing lines like "5 actionable tasks: 3 executed, 2 up-to-date"
        if trimmed.contains("actionable task") {
            continue;
        }

        // Detect error blocks
        if trimmed.starts_with("e:") || trimmed.starts_with("FAILURE:") {
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            error_count += 1;
            in_error = true;
            current_error.push(trimmed.to_string());
            continue;
        }

        // Detect "* What went wrong:" blocks
        if trimmed == "* What went wrong:" {
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            in_error = true;
            continue;
        }

        // End of error context
        if in_error && (trimmed.starts_with("* Try:") || trimmed.starts_with("* Get more help")) {
            if !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            in_error = false;
            continue;
        }

        // Collect error continuation lines
        if in_error {
            current_error.push(trimmed.to_string());
            continue;
        }

        // Collect warnings
        if trimmed.starts_with("w:") || trimmed.starts_with("warning:") {
            warning_count += 1;
            warnings.push(trimmed.to_string());
            continue;
        }
    }

    // Flush any remaining error
    if !current_error.is_empty() {
        errors.push(current_error.join("\n"));
    }

    // Build output
    let mut result = String::new();

    // Success case
    if build_result.contains("SUCCESSFUL") && error_count == 0 {
        result.push_str(&format!("✓ Gradle: {}", build_result));
        if task_count > 0 {
            let mut parts: Vec<String> = Vec::new();
            if actionable_tasks > 0 {
                parts.push(format!("{} executed", actionable_tasks));
            }
            if up_to_date > 0 {
                parts.push(format!("{} up-to-date", up_to_date));
            }
            if from_cache > 0 {
                parts.push(format!("{} cached", from_cache));
            }
            if !parts.is_empty() {
                result.push_str(&format!(" ({})", parts.join(", ")));
            }
        }

        if warning_count > 0 {
            result.push_str(&format!(
                "\n{} warning{}",
                warning_count,
                if warning_count > 1 { "s" } else { "" }
            ));
            for w in warnings.iter().take(5) {
                result.push_str(&format!("\n  {}", truncate(w, 120)));
            }
            if warnings.len() > 5 {
                result.push_str(&format!("\n  ... +{} more", warnings.len() - 5));
            }
        }

        return result;
    }

    // Failure case
    if error_count > 0 || build_result.contains("FAILED") {
        result.push_str(&format!(
            "Gradle build: {} error{}",
            error_count,
            if error_count > 1 { "s" } else { "" }
        ));
        if warning_count > 0 {
            result.push_str(&format!(", {} warnings", warning_count));
        }
        result.push('\n');
        result.push_str("═══════════════════════════════════════\n");

        for (i, error) in errors.iter().take(10).enumerate() {
            result.push_str(&format!("{}. {}\n", i + 1, truncate(error, 200)));
        }
        if errors.len() > 10 {
            result.push_str(&format!("\n... +{} more errors\n", errors.len() - 10));
        }

        return result.trim().to_string();
    }

    // No build result found — probably non-build task or empty output
    if output.trim().is_empty() {
        return "✓ Gradle: Complete".to_string();
    }

    // Fallback: return abbreviated output
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= 5 {
        return output.trim().to_string();
    }

    result.push_str(&format!("Gradle: {} lines of output\n", lines.len()));
    for line in lines.iter().take(5) {
        result.push_str(&format!("  {}\n", truncate(line, 120)));
    }
    result.push_str(&format!("  ... +{} more lines", lines.len() - 5));
    result.trim().to_string()
}

/// Filter Gradle test output.
/// Strips per-test progress, keeps only failures and summary.
fn filter_gradle_test(output: &str) -> String {
    let mut total_tests = 0;
    let mut failures = 0;
    let mut skipped = 0;
    let mut failed_tests: Vec<(String, Vec<String>)> = Vec::new();
    let mut current_failure: Option<(String, Vec<String>)> = None;
    let mut in_failure = false;
    let mut build_result = String::new();
    let mut suites_run: HashMap<String, (usize, usize, usize)> = HashMap::new(); // suite -> (pass, fail, skip)

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip noise
        if trimmed.is_empty()
            || trimmed.starts_with("Downloading")
            || trimmed.starts_with("Download ")
            || trimmed.starts_with("Starting a Gradle")
            || trimmed.starts_with("> Task :")
            || trimmed.starts_with("Calculating")
            || trimmed.starts_with("Type-safe")
            || trimmed.starts_with("See https://")
            || trimmed.starts_with("https://")
            || trimmed.contains("actionable task")
        {
            continue;
        }

        // Capture BUILD result
        if trimmed.starts_with("BUILD SUCCESSFUL") || trimmed.starts_with("BUILD FAILED") {
            build_result = trimmed.to_string();
            continue;
        }

        // Parse test result summary lines like:
        // "com.example.MyTest > myTestMethod PASSED"
        // "com.example.MyTest > myTestMethod FAILED"
        if let Some(status) = extract_test_result(trimmed) {
            total_tests += 1;

            // Get suite name (class name before " > ")
            let suite = trimmed.split(" > ").next().unwrap_or("unknown").to_string();
            let entry = suites_run.entry(suite).or_insert((0, 0, 0));

            match status {
                TestStatus::Passed => {
                    entry.0 += 1;
                    // Flush any in-progress failure
                    if let Some(f) = current_failure.take() {
                        failed_tests.push(f);
                    }
                    in_failure = false;
                }
                TestStatus::Failed => {
                    failures += 1;
                    entry.1 += 1;
                    // Flush any in-progress failure
                    if let Some(f) = current_failure.take() {
                        failed_tests.push(f);
                    }
                    let test_name = trimmed
                        .rsplit(" FAILED")
                        .nth(1)
                        .unwrap_or(trimmed)
                        .to_string();
                    current_failure = Some((test_name, Vec::new()));
                    in_failure = true;
                }
                TestStatus::Skipped => {
                    skipped += 1;
                    entry.2 += 1;
                    // Flush any in-progress failure
                    if let Some(f) = current_failure.take() {
                        failed_tests.push(f);
                    }
                    in_failure = false;
                }
            }
            continue;
        }

        // Parse Gradle test report summary like:
        // "100 tests completed, 2 failed, 3 skipped"
        if trimmed.contains("tests completed") {
            if let Some((t, f, s)) = parse_test_summary_line(trimmed) {
                // Use report numbers if we didn't individually track
                if total_tests == 0 {
                    total_tests = t;
                    failures = f;
                    skipped = s;
                }
            }
            continue;
        }

        // Collect failure output
        if in_failure {
            if let Some(ref mut f) = current_failure {
                f.1.push(trimmed.to_string());
            }
        }
    }

    // Flush last failure
    if let Some(f) = current_failure.take() {
        failed_tests.push(f);
    }

    let passed = if total_tests >= failures + skipped {
        total_tests - failures - skipped
    } else {
        0
    };

    // No tests found
    if total_tests == 0 && failures == 0 {
        if build_result.contains("SUCCESSFUL") {
            return "✓ Gradle test: No tests found (BUILD SUCCESSFUL)".to_string();
        }
        if build_result.contains("FAILED") {
            // Build failure, use build filter
            return filter_gradle_build(output);
        }
        return "Gradle test: No test results found".to_string();
    }

    // All passed
    if failures == 0 {
        let mut result = format!("✓ Gradle test: {} passed", passed);
        if skipped > 0 {
            result.push_str(&format!(", {} skipped", skipped));
        }
        if suites_run.len() > 1 {
            result.push_str(&format!(" in {} suites", suites_run.len()));
        }
        return result;
    }

    // Has failures
    let mut result = String::new();
    result.push_str(&format!(
        "Gradle test: {} passed, {} failed",
        passed, failures
    ));
    if skipped > 0 {
        result.push_str(&format!(", {} skipped", skipped));
    }
    result.push('\n');
    result.push_str("═══════════════════════════════════════\n");

    for (test_name, output_lines) in failed_tests.iter().take(10) {
        result.push_str(&format!("  ❌ {}\n", test_name));

        // Show relevant failure lines (assertions, expected/actual, exceptions)
        let relevant: Vec<&String> = output_lines
            .iter()
            .filter(|l| {
                let lower = l.to_lowercase();
                !l.trim().is_empty()
                    && (lower.contains("expected")
                        || lower.contains("actual")
                        || lower.contains("assert")
                        || lower.contains("error")
                        || lower.contains("exception")
                        || lower.contains("but was")
                        || lower.contains("at "))
            })
            .take(5)
            .collect();

        for line in relevant {
            result.push_str(&format!("     {}\n", truncate(line, 100)));
        }
    }

    if failed_tests.len() > 10 {
        result.push_str(&format!(
            "\n... +{} more failed tests\n",
            failed_tests.len() - 10
        ));
    }

    result.trim().to_string()
}

/// Filter Gradle dependencies output.
/// Compacts the dependency tree, removing duplicate configurations.
fn filter_gradle_dependencies(output: &str) -> String {
    let mut configs: HashMap<String, Vec<String>> = HashMap::new();
    let mut current_config = String::new();
    let mut current_deps: Vec<String> = Vec::new();
    let mut total_deps = 0;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip noise
        if trimmed.is_empty()
            || trimmed.starts_with("Downloading")
            || trimmed.starts_with("Download ")
            || trimmed.starts_with("> Task")
            || trimmed.starts_with("BUILD")
            || trimmed.starts_with("Starting a Gradle")
            || trimmed.contains("actionable task")
        {
            continue;
        }

        // Detect configuration header like "compileClasspath - Compile classpath"
        if !trimmed.starts_with('+')
            && !trimmed.starts_with('|')
            && !trimmed.starts_with('\\')
            && !trimmed.starts_with(' ')
            && trimmed.contains(" - ")
        {
            // Save previous config
            if !current_config.is_empty() && !current_deps.is_empty() {
                configs.insert(current_config.clone(), current_deps.clone());
            }
            current_config = trimmed.split(" - ").next().unwrap_or(trimmed).to_string();
            current_deps = Vec::new();
            continue;
        }

        // Detect "No dependencies" marker
        if trimmed == "No dependencies" || trimmed == "(n)" {
            continue;
        }

        // Collect top-level dependencies (first level of tree)
        if (trimmed.starts_with("+---") || trimmed.starts_with("\\---"))
            && !current_config.is_empty()
        {
            let dep = trimmed
                .trim_start_matches("+--- ")
                .trim_start_matches("\\--- ")
                .to_string();
            current_deps.push(dep);
            total_deps += 1;
        }
    }

    // Save last config
    if !current_config.is_empty() && !current_deps.is_empty() {
        configs.insert(current_config, current_deps);
    }

    if configs.is_empty() {
        return "Gradle dependencies: No dependencies found".to_string();
    }

    // Build compact output: only show configs with deps, max 20 deps each
    let mut result = String::new();
    result.push_str(&format!(
        "Gradle dependencies: {} top-level across {} configurations\n",
        total_deps,
        configs.len()
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Sort configs for deterministic output
    let mut sorted_configs: Vec<_> = configs.into_iter().collect();
    sorted_configs.sort_by(|a, b| a.0.cmp(&b.0));

    for (config, deps) in &sorted_configs {
        result.push_str(&format!("\n{} ({}):\n", config, deps.len()));
        for dep in deps.iter().take(20) {
            result.push_str(&format!("  {}\n", truncate(dep, 100)));
        }
        if deps.len() > 20 {
            result.push_str(&format!("  ... +{} more\n", deps.len() - 20));
        }
    }

    result.trim().to_string()
}

#[derive(Debug, PartialEq)]
enum TestStatus {
    Passed,
    Failed,
    Skipped,
}

/// Extract test result status from a Gradle test output line.
/// Lines look like: "com.example.MyTest > myTestMethod PASSED"
fn extract_test_result(line: &str) -> Option<TestStatus> {
    let trimmed = line.trim();
    if !trimmed.contains(" > ") {
        return None;
    }

    if trimmed.ends_with(" PASSED") {
        Some(TestStatus::Passed)
    } else if trimmed.ends_with(" FAILED") {
        Some(TestStatus::Failed)
    } else if trimmed.ends_with(" SKIPPED") {
        Some(TestStatus::Skipped)
    } else {
        None
    }
}

/// Parse a Gradle test summary line like "100 tests completed, 2 failed, 3 skipped"
fn parse_test_summary_line(line: &str) -> Option<(usize, usize, usize)> {
    let total = line
        .split("tests completed")
        .next()?
        .trim()
        .rsplit(' ')
        .next()?
        .parse::<usize>()
        .ok()?;

    let failed = if line.contains("failed") {
        line.split("failed")
            .next()?
            .trim()
            .rsplit(|c: char| !c.is_ascii_digit())
            .next()?
            .parse::<usize>()
            .unwrap_or(0)
    } else {
        0
    };

    let skipped = if line.contains("skipped") {
        line.split("skipped")
            .next()?
            .trim()
            .rsplit(|c: char| !c.is_ascii_digit())
            .next()?
            .parse::<usize>()
            .unwrap_or(0)
    } else {
        0
    };

    Some((total, failed, skipped))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_gradle_build_success() {
        let output = r#"Starting a Gradle Daemon (subsequent builds will be faster)

> Task :compileJava UP-TO-DATE
> Task :processResources NO-SOURCE
> Task :classes UP-TO-DATE
> Task :jar UP-TO-DATE
> Task :assemble UP-TO-DATE
> Task :compileTestJava UP-TO-DATE
> Task :processTestResources NO-SOURCE
> Task :testClasses UP-TO-DATE
> Task :test UP-TO-DATE
> Task :check UP-TO-DATE
> Task :build UP-TO-DATE

BUILD SUCCESSFUL in 2s
7 actionable tasks: 7 up-to-date"#;

        let result = filter_gradle_build(output);
        assert!(result.contains("✓ Gradle"));
        assert!(result.contains("SUCCESSFUL"));
        assert!(result.contains("up-to-date"));

        let savings = 100.0 - (count_tokens(&result) as f64 / count_tokens(output) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_gradle_build_with_errors() {
        let output = r#"> Task :compileJava FAILED
e: src/main/java/com/example/App.java:10:5: error: cannot find symbol
e: src/main/java/com/example/App.java:15:2: error: incompatible types

FAILURE: Build failed with an exception.

* What went wrong:
Execution failed for task ':compileJava'.
> Compilation failed; see the compiler error output for details.

* Try:
> Run with --stacktrace option to get the stack trace.

BUILD FAILED in 3s
1 actionable task: 1 executed"#;

        let result = filter_gradle_build(output);
        assert!(result.contains("error"));
        assert!(!result.contains("Task :compileJava"));
        assert!(!result.contains("--stacktrace"));
    }

    #[test]
    fn test_filter_gradle_build_empty() {
        let result = filter_gradle_build("");
        assert!(result.contains("✓ Gradle"));
    }

    #[test]
    fn test_filter_gradle_test_all_pass() {
        let output = r#"> Task :compileJava UP-TO-DATE
> Task :processResources NO-SOURCE
> Task :classes UP-TO-DATE
> Task :compileTestJava UP-TO-DATE
> Task :processTestResources NO-SOURCE
> Task :testClasses UP-TO-DATE
> Task :test

com.example.AppTest > testAddition PASSED
com.example.AppTest > testSubtraction PASSED
com.example.AppTest > testMultiplication PASSED
com.example.UtilsTest > testStringFormat PASSED
com.example.UtilsTest > testParsing PASSED

BUILD SUCCESSFUL in 5s
3 actionable tasks: 1 executed, 2 up-to-date"#;

        let result = filter_gradle_test(output);
        assert!(result.contains("✓ Gradle test"));
        assert!(result.contains("5 passed"));
        assert!(result.contains("2 suites"));

        let savings = 100.0 - (count_tokens(&result) as f64 / count_tokens(output) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_gradle_test_with_failures() {
        let output = r#"> Task :test

com.example.AppTest > testAddition PASSED
com.example.AppTest > testBroken FAILED

    org.opentest4j.AssertionFailedError: expected: <5> but was: <3>
        at org.junit.jupiter.api.AssertEquals.assertEquals(AssertEquals.java:166)
        at com.example.AppTest.testBroken(AppTest.java:25)

com.example.AppTest > testAnother PASSED

3 tests completed, 1 failed

BUILD FAILED in 4s
3 actionable tasks: 1 executed, 2 up-to-date"#;

        let result = filter_gradle_test(output);
        assert!(result.contains("2 passed"));
        assert!(result.contains("1 failed"));
        assert!(result.contains("testBroken"));
        assert!(result.contains("expected: <5> but was: <3>"));
    }

    #[test]
    fn test_filter_gradle_test_with_skipped() {
        let output = r#"> Task :test

com.example.AppTest > testOk PASSED
com.example.AppTest > testDisabled SKIPPED
com.example.AppTest > testOther PASSED

BUILD SUCCESSFUL in 2s"#;

        let result = filter_gradle_test(output);
        assert!(result.contains("✓ Gradle test"));
        assert!(result.contains("2 passed"));
        assert!(result.contains("1 skipped"));
    }

    #[test]
    fn test_filter_gradle_test_no_tests() {
        let output = r#"> Task :compileJava UP-TO-DATE
> Task :test NO-SOURCE

BUILD SUCCESSFUL in 1s"#;

        let result = filter_gradle_test(output);
        assert!(result.contains("No tests found"));
    }

    #[test]
    fn test_filter_gradle_dependencies() {
        let output = r#"> Task :dependencies

------------------------------------------------------------
Root project 'myapp'
------------------------------------------------------------

compileClasspath - Compile classpath for source set 'main'.
+--- org.springframework.boot:spring-boot-starter-web:3.2.0
+--- com.google.guava:guava:32.1.3-jre
\--- org.projectlombok:lombok:1.18.30

runtimeClasspath - Runtime classpath of source set 'main'.
+--- org.springframework.boot:spring-boot-starter-web:3.2.0
+--- com.google.guava:guava:32.1.3-jre
+--- org.projectlombok:lombok:1.18.30
\--- com.h2database:h2:2.2.224

testCompileClasspath - Compile classpath for source set 'test'.
+--- org.springframework.boot:spring-boot-starter-test:3.2.0
\--- org.junit.jupiter:junit-jupiter:5.10.1

BUILD SUCCESSFUL in 1s
1 actionable task: 1 executed"#;

        let result = filter_gradle_dependencies(output);
        assert!(result.contains("Gradle dependencies:"));
        assert!(result.contains("compileClasspath"));
        assert!(result.contains("runtimeClasspath"));
        assert!(result.contains("spring-boot-starter-web"));

        let savings = 100.0 - (count_tokens(&result) as f64 / count_tokens(output) as f64 * 100.0);
        assert!(
            savings >= 30.0,
            "Expected ≥30% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_extract_test_result() {
        assert_eq!(
            extract_test_result("com.example.AppTest > testAdd PASSED"),
            Some(TestStatus::Passed)
        );
        assert_eq!(
            extract_test_result("com.example.AppTest > testBroken FAILED"),
            Some(TestStatus::Failed)
        );
        assert_eq!(
            extract_test_result("com.example.AppTest > testDisabled SKIPPED"),
            Some(TestStatus::Skipped)
        );
        assert_eq!(extract_test_result("some random line"), None);
        assert_eq!(extract_test_result("> Task :test"), None);
    }

    #[test]
    fn test_parse_test_summary_line() {
        assert_eq!(
            parse_test_summary_line("100 tests completed, 2 failed, 3 skipped"),
            Some((100, 2, 3))
        );
        assert_eq!(
            parse_test_summary_line("50 tests completed, 1 failed"),
            Some((50, 1, 0))
        );
        assert_eq!(
            parse_test_summary_line("10 tests completed"),
            Some((10, 0, 0))
        );
    }

    #[test]
    fn test_filter_gradle_build_with_warnings() {
        let output = r#"> Task :compileJava
w: src/main/java/App.java:5: warning: [deprecation] method is deprecated
w: src/main/java/App.java:12: warning: [unchecked] unchecked conversion
> Task :build

BUILD SUCCESSFUL in 3s
3 actionable tasks: 3 executed"#;

        let result = filter_gradle_build(output);
        assert!(result.contains("✓ Gradle"));
        assert!(result.contains("2 warning"));
        assert!(result.contains("deprecation"));
    }

    #[test]
    fn test_gradle_build_token_savings() {
        let output = r#"Starting a Gradle Daemon, 1 incompatible Daemon could not be reused, use --status for details

Welcome to Gradle 8.5!

Here are the highlights of this release:
 - Support for running on Java 21
 - Improved error messages
 - Type-safe project accessors

See https://docs.gradle.org/8.5/release-notes.html

> Task :compileJava UP-TO-DATE
> Task :processResources UP-TO-DATE
> Task :classes UP-TO-DATE
> Task :compileTestJava UP-TO-DATE
> Task :processTestResources UP-TO-DATE
> Task :testClasses UP-TO-DATE
> Task :test UP-TO-DATE
> Task :check UP-TO-DATE
> Task :build UP-TO-DATE

BUILD SUCCESSFUL in 500ms
8 actionable tasks: 8 up-to-date"#;

        let result = filter_gradle_build(output);
        let input_tokens = count_tokens(output);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 70.0,
            "Expected ≥70% savings on verbose Gradle build, got {:.1}% (input: {}, output: {})",
            savings,
            input_tokens,
            output_tokens
        );
    }
}
