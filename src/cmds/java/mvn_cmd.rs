use crate::core::tracking;
use crate::core::utils::truncate;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Detect whether to use ./mvnw, mvnd, or mvn.
/// Prefers ./mvnw if it exists, then mvnd if on PATH, then mvn.
fn detect_mvn_binary() -> &'static str {
    if Path::new("./mvnw").exists() {
        "./mvnw"
    } else if which_exists("mvnd") {
        "mvnd"
    } else {
        "mvn"
    }
}

fn which_exists(cmd: &str) -> bool {
    Command::new("which")
        .arg(cmd)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// Build a Maven command with auto-injected flags.
/// Injects `-B` (batch mode) and `--no-transfer-progress` unless already present.
fn build_mvn_command(goal: &str, args: &[String]) -> Command {
    let binary = detect_mvn_binary();
    let mut cmd = Command::new(binary);
    cmd.arg(goal);

    // Auto-inject batch mode unless already present
    if !args.iter().any(|a| a == "-B" || a == "--batch-mode") {
        cmd.arg("-B");
    }

    // Auto-inject no-transfer-progress unless already present
    if !args
        .iter()
        .any(|a| a == "-ntp" || a == "--no-transfer-progress")
    {
        cmd.arg("--no-transfer-progress");
    }

    for arg in args {
        cmd.arg(arg);
    }

    cmd
}

/// Shared runner for all filtered Maven goals.
/// Follows the cargo_cmd.rs:run_cargo_filtered pattern.
fn run_mvn_filtered<F>(
    goal: &str,
    tee_key: &str,
    args: &[String],
    verbose: u8,
    filter_fn: F,
) -> Result<i32>
where
    F: Fn(&str) -> String,
{
    let timer = tracking::TimedExecution::start();

    let mut cmd = build_mvn_command(goal, args);

    if verbose > 0 {
        eprintln!("Running: mvn {} {}", goal, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run mvn {}. Is Maven installed?", goal))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = filter_fn(&raw);

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, tee_key, exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("mvn {} {}", goal, args.join(" ")),
        &format!("rtk mvn {} {}", goal, args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}

pub fn run_compile(args: &[String], verbose: u8) -> Result<i32> {
    run_mvn_filtered("compile", "mvn_compile", args, verbose, filter_mvn_compile)
}

pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    run_mvn_filtered("test", "mvn_test", args, verbose, filter_mvn_test)
}

pub fn run_package(args: &[String], verbose: u8) -> Result<i32> {
    run_mvn_filtered("package", "mvn_package", args, verbose, filter_mvn_package)
}

pub fn run_clean(args: &[String], verbose: u8) -> Result<i32> {
    run_mvn_filtered("clean", "mvn_clean", args, verbose, filter_mvn_clean)
}

pub fn run_integration_test(args: &[String], verbose: u8) -> Result<i32> {
    run_mvn_filtered(
        "integration-test",
        "mvn_integration_test",
        args,
        verbose,
        |raw| {
            let filtered = filter_mvn_test(raw);
            // Failsafe defers failure reporting to the `verify` phase.
            // Warn users if BUILD SUCCESS + failsafe detected.
            if raw.contains("BUILD SUCCESS")
                && (raw.contains("failsafe") || raw.contains("Failsafe"))
            {
                format!(
                    "{}\n\n  note: Failsafe defers failure reporting to `mvn verify`.\n  \
                     Use `rtk mvn verify` for accurate integration-test results.",
                    filtered
                )
            } else {
                filtered
            }
        },
    )
}

pub fn run_install(args: &[String], verbose: u8) -> Result<i32> {
    run_mvn_filtered("install", "mvn_install", args, verbose, filter_mvn_install)
}

pub fn run_dependency_tree(args: &[String], verbose: u8) -> Result<i32> {
    run_mvn_filtered(
        "dependency:tree",
        "mvn_dependency_tree",
        args,
        verbose,
        filter_mvn_dependency_tree,
    )
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<i32> {
    if args.is_empty() {
        anyhow::bail!("mvn: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();

    let binary = detect_mvn_binary();
    let subcommand = args[0].to_string_lossy();
    let mut cmd = Command::new(binary);
    cmd.arg(&*subcommand);

    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: mvn {} ...", subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run mvn {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    print!("{}", stdout);
    eprint!("{}", stderr);

    timer.track(
        &format!("mvn {}", subcommand),
        &format!("rtk mvn {}", subcommand),
        &raw,
        &raw, // No filtering for unsupported commands
    );

    Ok(output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 }))
}

// ── Shared filter helpers ──

/// Check if a line is Maven noise (separators, download lines, plugin execution, empty [INFO])
fn is_maven_noise(line: &str) -> bool {
    let trimmed = line.trim();

    // Separator lines
    if trimmed.starts_with("[INFO] --------")
        || trimmed.starts_with("[INFO] ========")
        || trimmed == "[INFO]"
    {
        return true;
    }

    // Strip [INFO] prefix for content checks
    let content = if let Some(rest) = trimmed.strip_prefix("[INFO] ") {
        rest
    } else if trimmed == "[INFO]" {
        return true;
    } else {
        trimmed
    };

    // Download/transfer progress lines
    if content.starts_with("Downloading from ")
        || content.starts_with("Downloaded from ")
        || content.starts_with("Uploading to ")
        || content.starts_with("Uploaded to ")
        || content.starts_with("Progress (")
    {
        return true;
    }

    // Plugin execution lines: "--- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---"
    if content.starts_with("--- ") && content.ends_with(" ---") {
        return true;
    }

    // Reactor build order / summary noise
    if content.starts_with("Reactor Build Order:") || content.starts_with("Reactor Summary") {
        return true;
    }

    // Scanning for projects line
    if content.starts_with("Scanning for projects...") {
        return true;
    }

    // Build metadata lines (Building X, Finished at, Total time)
    // Exclude "Building jar:" and "Building war:" — those are artifact info used by extract_artifact_info
    if content.starts_with("Building ")
        && !content.starts_with("Building jar:")
        && !content.starts_with("Building war:")
    {
        return true;
    }
    if content.starts_with("Finished at:") {
        return true;
    }

    if content.starts_with("Total time:") {
        return true;
    }

    // BUILD SUCCESS / BUILD FAILURE banners
    if content.contains("BUILD SUCCESS") || content.contains("BUILD FAILURE") {
        return true;
    }

    false
}

/// Extract build result line (BUILD SUCCESS/FAILURE) and timing
fn extract_build_result(output: &str) -> (bool, Option<String>) {
    let mut success = false;
    let mut timing = None;

    for line in output.lines() {
        let trimmed = line.trim();
        let content = trimmed
            .strip_prefix("[INFO] ")
            .or_else(|| trimmed.strip_prefix("[ERROR] "))
            .unwrap_or(trimmed);

        if content.contains("BUILD SUCCESS") {
            success = true;
        } else if content.contains("BUILD FAILURE") {
            success = false;
        } else if content.starts_with("Total time:") {
            timing = Some(content.to_string());
        }
    }

    (success, timing)
}

/// Extract and deduplicate warnings with counts
fn extract_warnings(output: &str) -> Vec<String> {
    let mut warning_counts: HashMap<String, usize> = HashMap::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(content) = trimmed.strip_prefix("[WARNING] ") {
            if !content.is_empty() && !content.starts_with("---") {
                *warning_counts.entry(content.to_string()).or_insert(0) += 1;
            }
        }
    }

    let mut warnings: Vec<String> = warning_counts
        .into_iter()
        .map(|(msg, count)| {
            if count > 1 {
                format!("{} (x{})", msg, count)
            } else {
                msg
            }
        })
        .collect();
    warnings.sort();
    warnings
}

/// Extract [ERROR] lines
fn extract_errors(output: &str) -> Vec<String> {
    let mut errors = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if let Some(content) = trimmed.strip_prefix("[ERROR] ") {
            if !content.is_empty() {
                errors.push(content.to_string());
            }
        }
    }

    errors
}

/// Count compilation sources (e.g., "Compiling 42 source files")
fn extract_compile_count(output: &str) -> Option<usize> {
    for line in output.lines() {
        let trimmed = line.trim();
        let content = trimmed.strip_prefix("[INFO] ").unwrap_or(trimmed);
        if content.starts_with("Compiling ") && content.contains(" source file") {
            // Parse "Compiling 42 source files to ..."
            if let Some(num_str) = content
                .strip_prefix("Compiling ")
                .and_then(|s| s.split_whitespace().next())
            {
                if let Ok(n) = num_str.parse::<usize>() {
                    return Some(n);
                }
            }
        }
    }
    None
}

/// Parse a single "Tests run: N, Failures: N, Errors: N, Skipped: N" line into a tuple.
fn parse_tests_run_line(content: &str) -> (usize, usize, usize, usize) {
    let mut tests = 0;
    let mut failures = 0;
    let mut errors = 0;
    let mut skipped = 0;

    for part in content.split(',') {
        let part = part.trim();
        if let Some(val) = part.strip_prefix("Tests run:") {
            tests = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = part.strip_prefix("Failures:") {
            failures = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = part.strip_prefix("Errors:") {
            errors = val.trim().parse().unwrap_or(0);
        } else if let Some(val) = part.strip_prefix("Skipped:") {
            // Handle "Skipped: 0, Time elapsed: 0.5 s" by stripping trailing non-digit
            let val_str = val.trim();
            let digits: String = val_str.chars().take_while(|c| c.is_ascii_digit()).collect();
            skipped = digits.parse().unwrap_or(0);
        }
    }

    (tests, failures, errors, skipped)
}

/// Parse Surefire/Failsafe test results summary.
/// Accumulates aggregate "Tests run:" lines (those without "Time elapsed:") across modules.
/// Per-class lines (with "Time elapsed:") are ignored when aggregates exist.
/// In multi-module reactors without aggregate lines, accumulates all lines.
fn parse_test_summary(output: &str) -> Option<(usize, usize, usize, usize)> {
    let mut aggregate = None; // Accumulates summary lines (no "Time elapsed:")
    let mut per_class = None; // Accumulates per-class lines (with "Time elapsed:")

    for line in output.lines() {
        let trimmed = line.trim();
        let content = trimmed
            .strip_prefix("[INFO] ")
            .or_else(|| trimmed.strip_prefix("[ERROR] "))
            .unwrap_or(trimmed);

        if content.starts_with("Tests run:") {
            let (tests, failures, errors, skipped) = parse_tests_run_line(content);
            let is_per_class = content.contains("Time elapsed:");

            let target = if is_per_class {
                &mut per_class
            } else {
                &mut aggregate
            };

            if let Some((rt, rf, re, rs)) = *target {
                *target = Some((rt + tests, rf + failures, re + errors, rs + skipped));
            } else {
                *target = Some((tests, failures, errors, skipped));
            }
        }
    }

    // Prefer aggregate summary lines; fall back to per-class if no aggregates exist
    aggregate.or(per_class)
}

/// Extract failed test names and their output from Surefire reports
fn extract_test_failures(output: &str) -> Vec<(String, Vec<String>)> {
    let mut failures: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_failure_section = false;
    let mut current_test: Option<String> = None;
    let mut current_output: Vec<String> = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Detect failure section start
        if trimmed.contains("<<< FAILURE!") || trimmed.contains("<<< ERROR!") {
            // Extract test name from lines like:
            // "testMethod(com.example.MyTest)  Time elapsed: 0.1 s  <<< FAILURE!"
            if let Some(test_name) = trimmed.split("<<<").next() {
                let name = test_name.trim().to_string();
                if !name.is_empty() {
                    current_test = Some(name);
                }
            }
            in_failure_section = true;
            continue;
        }

        // Detect "[ERROR] Tests run:" line which ends a module's test section
        if trimmed.starts_with("[ERROR] Tests run:") {
            // Flush current failure if any
            if let Some(test) = current_test.take() {
                failures.push((test, current_output.clone()));
                current_output.clear();
            }
            in_failure_section = false;
            continue;
        }

        // Detect individual failure detail blocks:
        // "[ERROR] testMethod(com.example.MyTest)"
        if trimmed.starts_with("[ERROR] ") && !trimmed.contains("Tests run:") {
            let content = trimmed.strip_prefix("[ERROR] ").unwrap_or(trimmed);

            // Check if this looks like a Surefire test name: methodName(fully.qualified.ClassName)
            // Exclude stack traces ("at ...") and assertion/error messages
            let trimmed_content = content.trim();
            let is_test_name = content.contains('(')
                && content.contains(')')
                && !trimmed_content.starts_with("at ")
                && !trimmed_content.contains("Error:")
                && !trimmed_content.contains("Exception:");

            if is_test_name {
                // Flush previous test before starting new one
                if let Some(test) = current_test.take() {
                    failures.push((test, current_output.clone()));
                    current_output.clear();
                }
                current_test = Some(content.to_string());
                in_failure_section = true;
            } else if in_failure_section {
                // Collect as failure output for current test
                if !trimmed_content.starts_with("at org.apache.")
                    && !trimmed_content.starts_with("at sun.")
                    && !trimmed_content.starts_with("at java.")
                    && !trimmed_content.starts_with("at org.junit.")
                    && !trimmed_content.starts_with("at org.mockito.")
                {
                    current_output.push(content.to_string());
                }
            } else {
                current_output.push(content.to_string());
            }
            continue;
        }

        // Collect failure output (assert messages, stack traces)
        if in_failure_section {
            let content = trimmed
                .strip_prefix("[ERROR] ")
                .or_else(|| trimmed.strip_prefix("[INFO] "))
                .unwrap_or(trimmed);

            if !content.is_empty()
                && !is_maven_noise(trimmed)
                && !content.starts_with("at org.apache.")
                && !content.starts_with("at sun.")
                && !content.starts_with("at java.")
                && !content.starts_with("at org.junit.")
                && !content.starts_with("at org.mockito.")
            {
                current_output.push(content.to_string());
            }
        }
    }

    // Flush last failure
    if let Some(test) = current_test {
        failures.push((test, current_output));
    }

    failures
}

/// Extract artifact info from package/install output
fn extract_artifact_info(output: &str) -> Option<String> {
    for line in output.lines() {
        let trimmed = line.trim();
        let content = trimmed.strip_prefix("[INFO] ").unwrap_or(trimmed);

        // "Building jar: /path/to/target/myapp-1.0.jar"
        if content.starts_with("Building jar:") || content.starts_with("Building war:") {
            if let Some(path) = content.split(':').nth(1) {
                let path = path.trim();
                // Show just the filename
                if let Some(filename) = Path::new(path).file_name() {
                    return Some(filename.to_string_lossy().to_string());
                }
            }
        }

        // "Installing /path/to/myapp-1.0.jar to /path/to/repo/..."
        if content.starts_with("Installing ") && content.contains(" to ") {
            if let Some(from_path) = content.strip_prefix("Installing ") {
                if let Some(filename) = from_path
                    .split(" to ")
                    .next()
                    .and_then(|p| Path::new(p.trim()).file_name())
                {
                    return Some(filename.to_string_lossy().to_string());
                }
            }
        }
    }
    None
}

// ── Filter functions ──

pub fn filter_mvn_compile(output: &str) -> String {
    let (success, timing) = extract_build_result(output);
    let errors = extract_errors(output);
    let warnings = extract_warnings(output);
    let compile_count = extract_compile_count(output);

    if success && errors.is_empty() {
        let mut result = String::from("✓ mvn compile: Success");
        if let Some(count) = compile_count {
            result = format!("✓ mvn compile: {} sources compiled", count);
        }
        if let Some(ref time) = timing {
            result.push_str(&format!(" ({})", time));
        }
        if !warnings.is_empty() {
            result.push_str(&format!("\n  {} warnings", warnings.len()));
            for w in warnings.iter().take(5) {
                result.push_str(&format!("\n  ⚠ {}", truncate(w, 120)));
            }
            if warnings.len() > 5 {
                result.push_str(&format!("\n  ... +{} more", warnings.len() - 5));
            }
        }
        return result;
    }

    let mut result = String::new();
    result.push_str(&format!("mvn compile: {} errors\n", errors.len()));
    result.push_str("═══════════════════════════════════════\n");

    for (i, error) in errors.iter().take(20).enumerate() {
        result.push_str(&format!("{}. {}\n", i + 1, truncate(error, 120)));
    }

    if errors.len() > 20 {
        result.push_str(&format!("\n... +{} more errors\n", errors.len() - 20));
    }

    if !warnings.is_empty() {
        result.push_str(&format!("\n{} warnings\n", warnings.len()));
        for w in warnings.iter().take(5) {
            result.push_str(&format!("  ⚠ {}\n", truncate(w, 120)));
        }
    }

    result.trim().to_string()
}

pub fn filter_mvn_test(output: &str) -> String {
    let (success, timing) = extract_build_result(output);
    let test_summary = parse_test_summary(output);

    if let Some((tests, failures, errors, skipped)) = test_summary {
        let has_failures = failures > 0 || errors > 0;

        if !has_failures {
            let mut result = format!("✓ mvn test: {} passed", tests);
            if skipped > 0 {
                result.push_str(&format!(", {} skipped", skipped));
            }
            if let Some(ref time) = timing {
                result.push_str(&format!(" ({})", time));
            }
            return result;
        }

        let mut result = format!(
            "mvn test: {} passed, {} failed, {} errors",
            tests.saturating_sub(failures).saturating_sub(errors),
            failures,
            errors
        );
        if skipped > 0 {
            result.push_str(&format!(", {} skipped", skipped));
        }
        result.push('\n');
        result.push_str("═══════════════════════════════════════\n");

        // Show failure details
        let test_failures = extract_test_failures(output);
        for (test_name, output_lines) in test_failures.iter().take(10) {
            result.push_str(&format!("\n❌ {}\n", truncate(test_name, 120)));
            for line in output_lines.iter().take(5) {
                result.push_str(&format!("   {}\n", truncate(line, 100)));
            }
            if output_lines.len() > 5 {
                result.push_str(&format!("   ... +{} more lines\n", output_lines.len() - 5));
            }
        }

        if test_failures.len() > 10 {
            result.push_str(&format!(
                "\n... +{} more failures\n",
                test_failures.len() - 10
            ));
        }

        return result.trim().to_string();
    }

    // No test summary found — might be a compilation failure before tests ran
    if !success {
        let errors = extract_errors(output);
        if !errors.is_empty() {
            let mut result = String::from("mvn test: Build failed (no tests ran)\n");
            result.push_str("═══════════════════════════════════════\n");
            for (i, error) in errors.iter().take(10).enumerate() {
                result.push_str(&format!("{}. {}\n", i + 1, truncate(error, 120)));
            }
            return result.trim().to_string();
        }
    }

    // Fallback: success with no test output
    let mut result = String::from("✓ mvn test: No tests found");
    if let Some(ref time) = timing {
        result.push_str(&format!(" ({})", time));
    }
    result
}

pub fn filter_mvn_package(output: &str) -> String {
    let (success, timing) = extract_build_result(output);
    let test_summary = parse_test_summary(output);
    let artifact = extract_artifact_info(output);
    let errors = extract_errors(output);
    let warnings = extract_warnings(output);

    if success {
        let mut result = String::from("✓ mvn package: Success");
        if let Some(ref art) = artifact {
            result = format!("✓ mvn package: {}", art);
        }
        if let Some((tests, _failures, _errors, skipped)) = test_summary {
            result.push_str(&format!(" ({} tests passed", tests));
            if skipped > 0 {
                result.push_str(&format!(", {} skipped", skipped));
            }
            result.push(')');
        }
        if let Some(ref time) = timing {
            result.push_str(&format!(" [{}]", time));
        }
        if !warnings.is_empty() {
            result.push_str(&format!("\n  {} warnings", warnings.len()));
        }
        return result;
    }

    // Failure
    let mut result = String::from("mvn package: FAILED\n");
    result.push_str("═══════════════════════════════════════\n");

    // Show test failures if any
    if let Some((tests, failures, errs, _)) = test_summary {
        if failures > 0 || errs > 0 {
            result.push_str(&format!(
                "Tests: {} passed, {} failed, {} errors\n",
                tests.saturating_sub(failures).saturating_sub(errs),
                failures,
                errs
            ));
            let test_failures = extract_test_failures(output);
            for (test_name, _) in test_failures.iter().take(5) {
                result.push_str(&format!("  ❌ {}\n", truncate(test_name, 120)));
            }
        }
    }

    // Show errors
    for (i, error) in errors.iter().take(10).enumerate() {
        result.push_str(&format!("{}. {}\n", i + 1, truncate(error, 120)));
    }

    result.trim().to_string()
}

pub fn filter_mvn_clean(output: &str) -> String {
    let (success, timing) = extract_build_result(output);

    if success {
        let mut result = String::from("✓ mvn clean: Done");
        if let Some(ref time) = timing {
            result.push_str(&format!(" ({})", time));
        }
        return result;
    }

    let errors = extract_errors(output);
    let mut result = String::from("mvn clean: FAILED\n");
    for error in errors.iter().take(5) {
        result.push_str(&format!("  {}\n", truncate(error, 120)));
    }
    result.trim().to_string()
}

pub fn filter_mvn_install(output: &str) -> String {
    let (success, timing) = extract_build_result(output);
    let test_summary = parse_test_summary(output);
    let artifact = extract_artifact_info(output);
    let errors = extract_errors(output);
    let warnings = extract_warnings(output);

    if success {
        let mut result = String::from("✓ mvn install: Success");
        if let Some(ref art) = artifact {
            result = format!("✓ mvn install: {} installed", art);
        }
        if let Some((tests, _failures, _errors, skipped)) = test_summary {
            result.push_str(&format!(" ({} tests passed", tests));
            if skipped > 0 {
                result.push_str(&format!(", {} skipped", skipped));
            }
            result.push(')');
        }
        if let Some(ref time) = timing {
            result.push_str(&format!(" [{}]", time));
        }
        if !warnings.is_empty() {
            result.push_str(&format!("\n  {} warnings", warnings.len()));
        }
        return result;
    }

    // Failure
    let mut result = String::from("mvn install: FAILED\n");
    result.push_str("═══════════════════════════════════════\n");

    if let Some((tests, failures, errs, _)) = test_summary {
        if failures > 0 || errs > 0 {
            result.push_str(&format!(
                "Tests: {} passed, {} failed, {} errors\n",
                tests.saturating_sub(failures).saturating_sub(errs),
                failures,
                errs
            ));
        }
    }

    for (i, error) in errors.iter().take(10).enumerate() {
        result.push_str(&format!("{}. {}\n", i + 1, truncate(error, 120)));
    }

    result.trim().to_string()
}

pub fn filter_mvn_dependency_tree(output: &str) -> String {
    let mut tree_lines: Vec<String> = Vec::new();
    let mut in_tree = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip non-INFO lines in tree context
        let content = if let Some(rest) = trimmed.strip_prefix("[INFO] ") {
            rest
        } else {
            continue; // Skip [WARNING], [ERROR], etc.
        };

        // Skip noise
        if is_maven_noise(trimmed) {
            continue;
        }

        // Detect tree start (artifact line with tree chars or root artifact)
        // Tree lines contain dependency tree characters: +- \- |
        if content.contains(":jar:")
            || content.contains(":war:")
            || content.contains(":pom:")
            || content.contains(":compile")
            || content.contains(":runtime")
            || content.contains(":test")
            || content.contains(":provided")
            || content.contains(":system")
        {
            in_tree = true;
            // Compact: remove scope annotations if present
            let compacted = compact_dependency_line(content);
            tree_lines.push(compacted);
            continue;
        }

        // If we're in the tree section, include lines with tree drawing chars
        if in_tree
            && (content.starts_with('+')
                || content.starts_with('|')
                || content.starts_with('\\')
                || content.starts_with(' '))
        {
            let compacted = compact_dependency_line(content);
            if !compacted.trim().is_empty() {
                tree_lines.push(compacted);
            }
        }
    }

    if tree_lines.is_empty() {
        return "✓ mvn dependency:tree: No dependencies".to_string();
    }

    let mut result = format!("mvn dependency:tree ({} entries)\n", tree_lines.len());
    result.push_str("═══════════════════════════════════════\n");

    for line in &tree_lines {
        result.push_str(&format!("{}\n", truncate(line, 120)));
    }

    result.trim().to_string()
}

/// Compact a dependency tree line by removing redundant scope annotations
fn compact_dependency_line(line: &str) -> String {
    // Remove common verbose parts:
    // "com.example:artifact:jar:1.0.0:compile" -> "com.example:artifact:1.0.0"
    // Keep the tree drawing characters intact
    let mut result = String::new();
    let mut chars = line.chars().peekable();

    // Preserve tree-drawing prefix (|, +, \, -, space)
    while let Some(&c) = chars.peek() {
        if c == '|' || c == '+' || c == '\\' || c == '-' || c == ' ' {
            result.push(c);
            chars.next();
        } else {
            break;
        }
    }

    // The rest is the artifact coordinate
    let artifact: String = chars.collect();
    let parts: Vec<&str> = artifact.split(':').collect();

    match parts.len() {
        4 => {
            // group:artifact:packaging:version
            let (group, name, version) = (parts[0], parts[1], parts[3]);
            result.push_str(&format!("{}:{}:{}", group, name, version));
        }
        5 => {
            // group:artifact:packaging:version:scope
            let (group, name, version) = (parts[0], parts[1], parts[3]);
            result.push_str(&format!("{}:{}:{}", group, name, version));
        }
        6 => {
            // group:artifact:packaging:classifier:version:scope
            let (group, name, classifier, version) = (parts[0], parts[1], parts[3], parts[4]);
            result.push_str(&format!("{}:{}:{}:{}", group, name, classifier, version));
        }
        _ => result.push_str(&artifact),
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // ── Compile filter tests ──

    #[test]
    fn test_filter_compile_success() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-resources-plugin:3.3.1:resources (default-resources) @ myapp ---
[INFO] Copying 3 resources from src/main/resources to target/classes
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 42 source files to /home/user/myapp/target/classes
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  3.456 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_compile(input);
        assert!(result.contains("✓ mvn compile"));
        assert!(result.contains("42 sources compiled"));
        assert!(result.contains("Total time:"));
    }

    #[test]
    fn test_filter_compile_with_errors() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[ERROR] /src/main/java/com/example/App.java:[15,9] cannot find symbol
[ERROR]   symbol:   variable unknownVar
[ERROR]   location: class com.example.App
[INFO] ------------------------------------------------------------------------
[INFO] BUILD FAILURE
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  1.234 s
[ERROR] Failed to execute goal org.apache.maven.plugins:maven-compiler-plugin:3.11.0:compile"#;

        let result = filter_mvn_compile(input);
        assert!(result.contains("errors"));
        assert!(result.contains("cannot find symbol"));
    }

    #[test]
    fn test_filter_compile_token_savings() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-resources-plugin:3.3.1:resources (default-resources) @ myapp ---
[INFO] Copying 3 resources from src/main/resources to target/classes
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 42 source files to /home/user/myapp/target/classes
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  3.456 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let output = filter_mvn_compile(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Compile filter: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    // ── Test filter tests ──

    #[test]
    fn test_filter_test_all_pass() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Nothing to compile - all classes are up to date
[INFO]
[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ myapp ---
[INFO] Using auto detected provider org.apache.maven.surefire.junitplatform.JUnitPlatformProvider
[INFO]
[INFO] -------------------------------------------------------
[INFO]  T E S T S
[INFO] -------------------------------------------------------
[INFO] Running com.example.AppTest
[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.123 s
[INFO] Running com.example.UtilsTest
[INFO] Tests run: 10, Failures: 0, Errors: 0, Skipped: 2, Time elapsed: 0.456 s
[INFO]
[INFO] Results:
[INFO]
[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 2
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  5.678 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_test(input);
        assert!(result.contains("✓ mvn test"));
        assert!(result.contains("15 passed"));
        assert!(result.contains("2 skipped"));
    }

    #[test]
    fn test_filter_test_with_failures() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ myapp ---
[INFO]
[INFO] -------------------------------------------------------
[INFO]  T E S T S
[INFO] -------------------------------------------------------
[INFO] Running com.example.AppTest
[ERROR] Tests run: 5, Failures: 2, Errors: 1, Skipped: 0, Time elapsed: 0.5 s <<< FAILURE!
[ERROR] testAdd(com.example.AppTest)  Time elapsed: 0.01 s  <<< FAILURE!
[ERROR] org.opentest4j.AssertionFailedError: expected: <4> but was: <3>
[ERROR] 	at org.junit.jupiter.api.AssertEquals.assertEquals(AssertEquals.java:150)
[ERROR] 	at com.example.AppTest.testAdd(AppTest.java:15)
[INFO]
[INFO] Results:
[INFO]
[ERROR] Tests run: 5, Failures: 2, Errors: 1, Skipped: 0
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD FAILURE
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  3.456 s
[ERROR] Failed to execute goal org.apache.maven.plugins:maven-surefire-plugin"#;

        let result = filter_mvn_test(input);
        assert!(result.contains("failed"));
        assert!(result.contains("error"));
    }

    #[test]
    fn test_filter_test_token_savings() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Nothing to compile - all classes are up to date
[INFO]
[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ myapp ---
[INFO] Using auto detected provider org.apache.maven.surefire.junitplatform.JUnitPlatformProvider
[INFO]
[INFO] -------------------------------------------------------
[INFO]  T E S T S
[INFO] -------------------------------------------------------
[INFO] Running com.example.AppTest
[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.123 s
[INFO] Running com.example.ServiceTest
[INFO] Tests run: 8, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.234 s
[INFO] Running com.example.UtilsTest
[INFO] Tests run: 12, Failures: 0, Errors: 0, Skipped: 1, Time elapsed: 0.345 s
[INFO] Running com.example.ControllerTest
[INFO] Tests run: 20, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.789 s
[INFO]
[INFO] Results:
[INFO]
[INFO] Tests run: 45, Failures: 0, Errors: 0, Skipped: 1
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  8.123 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let output = filter_mvn_test(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Test filter: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    // ── Clean filter tests ──

    #[test]
    fn test_filter_clean_success() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-clean-plugin:3.3.1:clean (default-clean) @ myapp ---
[INFO] Deleting /home/user/myapp/target
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  0.456 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_clean(input);
        assert!(result.contains("✓ mvn clean: Done"));
    }

    #[test]
    fn test_filter_clean_token_savings() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-clean-plugin:3.3.1:clean (default-clean) @ myapp ---
[INFO] Deleting /home/user/myapp/target
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  0.456 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let output = filter_mvn_clean(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Clean filter: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    // ── Package filter tests ──

    #[test]
    fn test_filter_package_success() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 42 source files to /home/user/myapp/target/classes
[INFO]
[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ myapp ---
[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 0
[INFO]
[INFO] --- maven-jar-plugin:3.3.0:jar (default-jar) @ myapp ---
[INFO] Building jar: /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  12.345 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_package(input);
        assert!(result.contains("✓ mvn package"));
        assert!(result.contains("myapp-1.0-SNAPSHOT.jar"));
        assert!(result.contains("15 tests passed"));
    }

    #[test]
    fn test_filter_package_token_savings() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-resources-plugin:3.3.1:resources (default-resources) @ myapp ---
[INFO] Copying 5 resources from src/main/resources to target/classes
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 42 source files to /home/user/myapp/target/classes
[INFO]
[INFO] --- maven-resources-plugin:3.3.1:testResources (default-testResources) @ myapp ---
[INFO] Copying 2 resources from src/test/resources to target/test-classes
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:testCompile (default-testCompile) @ myapp ---
[INFO] Compiling 15 source files to /home/user/myapp/target/test-classes
[INFO]
[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ myapp ---
[INFO] Using auto detected provider org.apache.maven.surefire.junitplatform.JUnitPlatformProvider
[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 1.234 s
[INFO]
[INFO] Results:
[INFO]
[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 0
[INFO]
[INFO] --- maven-jar-plugin:3.3.0:jar (default-jar) @ myapp ---
[INFO] Building jar: /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  12.345 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let output = filter_mvn_package(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Package filter: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    // ── Install filter tests ──

    #[test]
    fn test_filter_install_success() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 42 source files to /home/user/myapp/target/classes
[INFO]
[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ myapp ---
[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 0
[INFO]
[INFO] --- maven-jar-plugin:3.3.0:jar (default-jar) @ myapp ---
[INFO] Building jar: /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar
[INFO]
[INFO] --- maven-install-plugin:3.1.1:install (default-install) @ myapp ---
[INFO] Installing /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar to /home/user/.m2/repository/com/example/myapp/1.0-SNAPSHOT/myapp-1.0-SNAPSHOT.jar
[INFO] Installing /home/user/myapp/pom.xml to /home/user/.m2/repository/com/example/myapp/1.0-SNAPSHOT/myapp-1.0-SNAPSHOT.pom
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  15.678 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_install(input);
        assert!(result.contains("✓ mvn install"));
        assert!(result.contains("installed"));
        assert!(result.contains("15 tests passed"));
    }

    #[test]
    fn test_filter_install_token_savings() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-resources-plugin:3.3.1:resources (default-resources) @ myapp ---
[INFO] Copying 5 resources from src/main/resources to target/classes
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 42 source files to /home/user/myapp/target/classes
[INFO]
[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ myapp ---
[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 1.234 s
[INFO]
[INFO] Results:
[INFO]
[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 0
[INFO]
[INFO] --- maven-jar-plugin:3.3.0:jar (default-jar) @ myapp ---
[INFO] Building jar: /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar
[INFO]
[INFO] --- maven-install-plugin:3.1.1:install (default-install) @ myapp ---
[INFO] Installing /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar to /home/user/.m2/repository/com/example/myapp/1.0-SNAPSHOT/myapp-1.0-SNAPSHOT.jar
[INFO] Installing /home/user/myapp/pom.xml to /home/user/.m2/repository/com/example/myapp/1.0-SNAPSHOT/myapp-1.0-SNAPSHOT.pom
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  15.678 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let output = filter_mvn_install(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Install filter: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    // ── Dependency tree filter tests ──

    #[test]
    fn test_filter_dependency_tree() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-dependency-plugin:3.6.0:tree (default-cli) @ myapp ---
[INFO] com.example:myapp:jar:1.0-SNAPSHOT
[INFO] +- org.springframework.boot:spring-boot-starter-web:jar:3.2.0:compile
[INFO] |  +- org.springframework.boot:spring-boot-starter:jar:3.2.0:compile
[INFO] |  |  +- org.springframework.boot:spring-boot:jar:3.2.0:compile
[INFO] |  |  +- org.springframework.boot:spring-boot-autoconfigure:jar:3.2.0:compile
[INFO] |  +- org.springframework.boot:spring-boot-starter-json:jar:3.2.0:compile
[INFO] |  +- org.springframework.boot:spring-boot-starter-tomcat:jar:3.2.0:compile
[INFO] +- org.projectlombok:lombok:jar:1.18.30:provided
[INFO] +- org.springframework.boot:spring-boot-starter-test:jar:3.2.0:test
[INFO] |  +- org.junit.jupiter:junit-jupiter:jar:5.10.1:test
[INFO] |  \- org.mockito:mockito-core:jar:5.7.0:test
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  2.345 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_dependency_tree(input);
        assert!(result.contains("dependency:tree"));
        assert!(result.contains("entries"));
        // Verify compaction removed :jar: and scope
        assert!(result.contains("spring-boot-starter-web"));
    }

    #[test]
    fn test_filter_dependency_tree_token_savings() {
        // Realistic mvn dependency:tree output with full Maven boilerplate
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] Downloading from central: https://repo.maven.apache.org/maven2/org/apache/maven/plugins/maven-dependency-plugin/3.6.0/maven-dependency-plugin-3.6.0.pom
[INFO] Downloaded from central: https://repo.maven.apache.org/maven2/org/apache/maven/plugins/maven-dependency-plugin/3.6.0/maven-dependency-plugin-3.6.0.pom (24 kB at 1.2 MB/s)
[INFO] Downloading from central: https://repo.maven.apache.org/maven2/org/apache/maven/plugins/maven-dependency-plugin/3.6.0/maven-dependency-plugin-3.6.0.jar
[INFO] Downloaded from central: https://repo.maven.apache.org/maven2/org/apache/maven/plugins/maven-dependency-plugin/3.6.0/maven-dependency-plugin-3.6.0.jar (224 kB at 5.6 MB/s)
[INFO] Downloading from central: https://repo.maven.apache.org/maven2/org/apache/maven/shared/maven-dependency-tree/3.2.1/maven-dependency-tree-3.2.1.pom
[INFO] Downloaded from central: https://repo.maven.apache.org/maven2/org/apache/maven/shared/maven-dependency-tree/3.2.1/maven-dependency-tree-3.2.1.pom (8.1 kB at 890 kB/s)
[INFO] Downloading from central: https://repo.maven.apache.org/maven2/org/apache/maven/shared/maven-dependency-tree/3.2.1/maven-dependency-tree-3.2.1.jar
[INFO] Downloaded from central: https://repo.maven.apache.org/maven2/org/apache/maven/shared/maven-dependency-tree/3.2.1/maven-dependency-tree-3.2.1.jar (42 kB at 2.1 MB/s)
[INFO]
[INFO] --- maven-dependency-plugin:3.6.0:tree (default-cli) @ myapp ---
[INFO] Downloading from central: https://repo.maven.apache.org/maven2/org/springframework/boot/spring-boot-starter-web/3.2.0/spring-boot-starter-web-3.2.0.pom
[INFO] Downloaded from central: https://repo.maven.apache.org/maven2/org/springframework/boot/spring-boot-starter-web/3.2.0/spring-boot-starter-web-3.2.0.pom (3.0 kB at 450 kB/s)
[INFO] Downloading from central: https://repo.maven.apache.org/maven2/org/springframework/boot/spring-boot-starter/3.2.0/spring-boot-starter-3.2.0.pom
[INFO] Downloaded from central: https://repo.maven.apache.org/maven2/org/springframework/boot/spring-boot-starter/3.2.0/spring-boot-starter-3.2.0.pom (3.2 kB at 410 kB/s)
[INFO] com.example:myapp:jar:1.0-SNAPSHOT
[INFO] +- org.springframework.boot:spring-boot-starter-web:jar:3.2.0:compile
[INFO] |  +- org.springframework.boot:spring-boot-starter:jar:3.2.0:compile
[INFO] |  |  +- org.springframework.boot:spring-boot:jar:3.2.0:compile
[INFO] |  |  +- org.springframework.boot:spring-boot-autoconfigure:jar:3.2.0:compile
[INFO] |  |  +- org.springframework.boot:spring-boot-starter-logging:jar:3.2.0:compile
[INFO] |  |  |  +- ch.qos.logback:logback-classic:jar:1.4.14:compile
[INFO] |  |  |  +- org.apache.logging.log4j:log4j-to-slf4j:jar:2.21.1:compile
[INFO] |  |  |  \- org.slf4j:jul-to-slf4j:jar:2.0.9:compile
[INFO] |  |  +- jakarta.annotation:jakarta.annotation-api:jar:2.1.1:compile
[INFO] |  |  \- org.yaml:snakeyaml:jar:2.2:compile
[INFO] |  +- org.springframework.boot:spring-boot-starter-json:jar:3.2.0:compile
[INFO] |  |  +- com.fasterxml.jackson.core:jackson-databind:jar:2.15.3:compile
[INFO] |  |  +- com.fasterxml.jackson.datatype:jackson-datatype-jdk8:jar:2.15.3:compile
[INFO] |  |  \- com.fasterxml.jackson.module:jackson-module-parameter-names:jar:2.15.3:compile
[INFO] |  +- org.springframework.boot:spring-boot-starter-tomcat:jar:3.2.0:compile
[INFO] |  |  +- org.apache.tomcat.embed:tomcat-embed-core:jar:10.1.16:compile
[INFO] |  |  +- org.apache.tomcat.embed:tomcat-embed-el:jar:10.1.16:compile
[INFO] |  |  \- org.apache.tomcat.embed:tomcat-embed-websocket:jar:10.1.16:compile
[INFO] |  +- org.springframework:spring-web:jar:6.1.1:compile
[INFO] |  \- org.springframework:spring-webmvc:jar:6.1.1:compile
[INFO] +- org.projectlombok:lombok:jar:1.18.30:provided
[INFO] +- org.springframework.boot:spring-boot-starter-test:jar:3.2.0:test
[INFO] |  +- org.junit.jupiter:junit-jupiter:jar:5.10.1:test
[INFO] |  +- org.mockito:mockito-core:jar:5.7.0:test
[INFO] |  +- org.assertj:assertj-core:jar:3.24.2:test
[INFO] |  \- org.hamcrest:hamcrest:jar:2.2:test
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  2.345 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let output = filter_mvn_dependency_tree(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Dependency tree filter: expected >=60% savings, got {:.1}% (input={}, output={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── Edge cases ──

    #[test]
    fn test_filter_compile_empty_input() {
        let result = filter_mvn_compile("");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_filter_test_empty_input() {
        let result = filter_mvn_test("");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_filter_clean_empty_input() {
        let result = filter_mvn_clean("");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_filter_dependency_tree_empty_input() {
        let result = filter_mvn_dependency_tree("");
        assert!(!result.is_empty());
    }

    #[test]
    fn test_is_maven_noise() {
        assert!(is_maven_noise("[INFO] --------"));
        assert!(is_maven_noise(
            "[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---"
        ));
        assert!(is_maven_noise(
            "[INFO] Downloading from central: https://repo.maven.apache.org/"
        ));
        assert!(is_maven_noise(
            "[INFO] Downloaded from central: https://repo.maven.apache.org/"
        ));
        assert!(is_maven_noise("[INFO] Scanning for projects..."));
        assert!(is_maven_noise("[INFO]"));

        assert!(is_maven_noise("[INFO] BUILD SUCCESS"));
        assert!(is_maven_noise("[INFO] Building myapp 1.0-SNAPSHOT"));
        assert!(is_maven_noise("[INFO] Total time:  3.456 s"));
        assert!(is_maven_noise("[INFO] Finished at: 2024-01-15T10:30:00Z"));
        assert!(!is_maven_noise("[ERROR] compilation failure"));
        assert!(!is_maven_noise(
            "[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 0"
        ));
        // Step 6: "Building jar:" and "Building war:" must NOT be noise
        assert!(!is_maven_noise(
            "[INFO] Building jar: /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar"
        ));
        assert!(!is_maven_noise(
            "[INFO] Building war: /home/user/myapp/target/myapp-1.0-SNAPSHOT.war"
        ));
        // But generic "Building X" is still noise
        assert!(is_maven_noise("[INFO] Building myapp 1.0-SNAPSHOT"));
    }

    #[test]
    fn test_parse_test_summary() {
        let output = "[INFO] Tests run: 42, Failures: 2, Errors: 1, Skipped: 3";
        let result = parse_test_summary(output);
        assert_eq!(result, Some((42, 2, 1, 3)));
    }

    #[test]
    fn test_compact_dependency_line() {
        assert_eq!(
            compact_dependency_line("+- org.springframework:spring-web:jar:6.1.1:compile"),
            "+- org.springframework:spring-web:6.1.1"
        );
        assert_eq!(
            compact_dependency_line(
                "|  +- com.fasterxml.jackson.core:jackson-databind:jar:2.15.3:compile"
            ),
            "|  +- com.fasterxml.jackson.core:jackson-databind:2.15.3"
        );
    }

    // Step 5: 6-part classifier coordinates
    #[test]
    fn test_compact_dependency_line_with_classifier() {
        // 6-part: group:artifact:packaging:classifier:version:scope
        assert_eq!(
            compact_dependency_line(
                "+- io.netty:netty-transport-native-epoll:jar:linux-x86_64:4.1.100:compile"
            ),
            "+- io.netty:netty-transport-native-epoll:linux-x86_64:4.1.100"
        );
        // 4-part: group:artifact:packaging:version (no scope)
        assert_eq!(
            compact_dependency_line("com.example:myapp:jar:1.0-SNAPSHOT"),
            "com.example:myapp:1.0-SNAPSHOT"
        );
    }

    // Step 1: Multi-module without aggregate (only per-class lines) — accumulates
    #[test]
    fn test_parse_test_summary_multi_module_no_aggregate() {
        let output = r#"[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ module-a ---
[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.123 s
[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ module-b ---
[ERROR] Tests run: 10, Failures: 2, Errors: 1, Skipped: 1, Time elapsed: 0.456 s"#;
        let result = parse_test_summary(output);
        // No aggregate lines → accumulates per-class: 5+10=15, 0+2=2, 0+1=1, 0+1=1
        assert_eq!(result, Some((15, 2, 1, 1)));
    }

    // Step 1: With aggregate line — aggregate preferred over per-class
    #[test]
    fn test_parse_test_summary_with_aggregate() {
        let output = r#"[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.1 s
[INFO] Tests run: 8, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.2 s
[INFO] Tests run: 13, Failures: 1, Errors: 0, Skipped: 0"#;
        let result = parse_test_summary(output);
        // Aggregate line (no "Time elapsed:") wins: 13 tests, 1 failure
        assert_eq!(result, Some((13, 1, 0, 0)));
    }

    // Step 1: Multi-module with multiple aggregate lines — accumulates aggregates
    #[test]
    fn test_parse_test_summary_multi_module_aggregates() {
        let output = r#"[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.1 s
[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 0
[INFO] Tests run: 10, Failures: 2, Errors: 1, Skipped: 1, Time elapsed: 0.2 s
[ERROR] Tests run: 10, Failures: 2, Errors: 1, Skipped: 1"#;
        let result = parse_test_summary(output);
        // Two aggregate lines: (5,0,0,0) + (10,2,1,1) = (15,2,1,1)
        assert_eq!(result, Some((15, 2, 1, 1)));
    }

    // Step 2: Assertion messages should not become test names
    #[test]
    fn test_extract_test_failures_assertion_not_test_name() {
        let input = r#"[ERROR] testAdd(com.example.AppTest)  Time elapsed: 0.01 s  <<< FAILURE!
[ERROR] org.opentest4j.AssertionFailedError: expected: <4> but was: <3>
[ERROR] 	at org.junit.jupiter.api.AssertEquals.assertEquals(AssertEquals.java:150)
[ERROR] 	at com.example.AppTest.testAdd(AppTest.java:15)
[ERROR] Tests run: 5, Failures: 1, Errors: 0, Skipped: 0"#;
        let failures = extract_test_failures(input);
        assert_eq!(failures.len(), 1);
        assert!(failures[0].0.contains("testAdd"));
        // The assertion message should be in the output, not treated as a new test name
        assert!(failures[0]
            .1
            .iter()
            .any(|l| l.contains("AssertionFailedError")));
    }

    #[test]
    fn test_extract_build_result() {
        let output = r#"[INFO] BUILD SUCCESS
[INFO] Total time:  3.456 s"#;
        let (success, timing) = extract_build_result(output);
        assert!(success);
        assert!(timing.unwrap().contains("3.456"));
    }

    #[test]
    fn test_extract_warnings_deduplication() {
        let output = r#"[WARNING] Using deprecated API
[WARNING] Using deprecated API
[WARNING] Using deprecated API
[WARNING] Unused import"#;
        let warnings = extract_warnings(output);
        assert_eq!(warnings.len(), 2);
        assert!(warnings.iter().any(|w| w.contains("(x3)")));
    }

    // ── Integration-test filter tests ──

    #[test]
    fn test_filter_integration_test_failsafe_warning() {
        // Simulate Failsafe output with BUILD SUCCESS
        let raw = r#"[INFO] Scanning for projects...
[INFO]
[INFO] --- maven-failsafe-plugin:3.1.2:integration-test (default) @ myapp ---
[INFO] Tests run: 3, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 5.0 s
[INFO]
[INFO] Results:
[INFO]
[INFO] Tests run: 3, Failures: 0, Errors: 0, Skipped: 0
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  8.0 s"#;

        // run_integration_test uses this closure
        let filtered = filter_mvn_test(raw);
        let result = if raw.contains("BUILD SUCCESS")
            && (raw.contains("failsafe") || raw.contains("Failsafe"))
        {
            format!(
                "{}\n\n  note: Failsafe defers failure reporting to `mvn verify`.\n  \
                 Use `rtk mvn verify` for accurate integration-test results.",
                filtered
            )
        } else {
            filtered
        };

        assert!(result.contains("3 passed"));
        assert!(result.contains("Failsafe defers"));
        assert!(result.contains("rtk mvn verify"));
    }

    #[test]
    fn test_filter_integration_test_no_failsafe_no_warning() {
        // Surefire-only output — no failsafe warning
        let raw = r#"[INFO] --- maven-surefire-plugin:3.1.2:test (default-test) @ myapp ---
[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 0
[INFO] BUILD SUCCESS
[INFO] Total time:  3.0 s"#;

        let filtered = filter_mvn_test(raw);
        let result = if raw.contains("BUILD SUCCESS")
            && (raw.contains("failsafe") || raw.contains("Failsafe"))
        {
            format!("{}\n\n  note: Failsafe defers...", filtered)
        } else {
            filtered
        };

        assert!(result.contains("5 passed"));
        assert!(!result.contains("Failsafe"));
    }

    #[test]
    fn test_filter_integration_test_token_savings() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Nothing to compile - all classes are up to date
[INFO]
[INFO] --- maven-failsafe-plugin:3.1.2:integration-test (default) @ myapp ---
[INFO] Using auto detected provider org.apache.maven.surefire.junitplatform.JUnitPlatformProvider
[INFO]
[INFO] -------------------------------------------------------
[INFO]  T E S T S
[INFO] -------------------------------------------------------
[INFO] Running com.example.IntegrationTest
[INFO] Tests run: 8, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 12.345 s
[INFO]
[INFO] Results:
[INFO]
[INFO] Tests run: 8, Failures: 0, Errors: 0, Skipped: 0
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  18.456 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let output = filter_mvn_test(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Integration-test filter: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    // ── Snapshot-style exact output tests ──

    #[test]
    fn test_filter_compile_success_exact_output() {
        let input = r#"[INFO] Scanning for projects...
[INFO] -----------------------< com.example:myapp >------------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 42 source files to /home/user/myapp/target/classes
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  3.456 s
[INFO] Finished at: 2024-01-15T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_compile(input);
        assert_eq!(
            result,
            "✓ mvn compile: 42 sources compiled (Total time:  3.456 s)"
        );
    }

    #[test]
    fn test_filter_test_all_pass_exact_output() {
        let input = r#"[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 2
[INFO] BUILD SUCCESS
[INFO] Total time:  5.678 s"#;

        let result = filter_mvn_test(input);
        assert_eq!(
            result,
            "✓ mvn test: 15 passed, 2 skipped (Total time:  5.678 s)"
        );
    }

    #[test]
    fn test_filter_clean_success_exact_output() {
        let input = r#"[INFO] --- maven-clean-plugin:3.3.1:clean (default-clean) @ myapp ---
[INFO] Deleting /home/user/myapp/target
[INFO] BUILD SUCCESS
[INFO] Total time:  0.456 s"#;

        let result = filter_mvn_clean(input);
        assert_eq!(result, "✓ mvn clean: Done (Total time:  0.456 s)");
    }

    #[test]
    fn test_filter_package_success_exact_output() {
        let input = r#"[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 0
[INFO] Building jar: /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar
[INFO] BUILD SUCCESS
[INFO] Total time:  12.345 s"#;

        let result = filter_mvn_package(input);
        assert_eq!(
            result,
            "✓ mvn package: myapp-1.0-SNAPSHOT.jar (15 tests passed) [Total time:  12.345 s]"
        );
    }

    #[test]
    fn test_filter_install_success_exact_output() {
        let input = r#"[INFO] Tests run: 15, Failures: 0, Errors: 0, Skipped: 0
[INFO] Installing /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar to /home/user/.m2/repository/com/example/myapp/1.0-SNAPSHOT/myapp-1.0-SNAPSHOT.jar
[INFO] BUILD SUCCESS
[INFO] Total time:  15.678 s"#;

        let result = filter_mvn_install(input);
        assert_eq!(
            result,
            "✓ mvn install: myapp-1.0-SNAPSHOT.jar installed (15 tests passed) [Total time:  15.678 s]"
        );
    }

    #[test]
    fn test_filter_compile_with_warnings() {
        let input = r#"[INFO] Scanning for projects...
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 10 source files to /home/user/myapp/target/classes
[WARNING] /src/main/java/com/example/App.java:[5,1] [deprecation] oldMethod() in OldClass has been deprecated
[WARNING] /src/main/java/com/example/Service.java:[12,1] [unchecked] unchecked conversion
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  2.0 s"#;

        let result = filter_mvn_compile(input);
        assert!(result.contains("✓ mvn compile"));
        assert!(result.contains("10 sources compiled"));
        assert!(result.contains("2 warnings"));
    }
}
