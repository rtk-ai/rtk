use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use std::ffi::OsString;
use std::path::Path;
use std::process::Command;

/// Detect whether to use ./mvnw (wrapper) or mvn
fn mvn_executable() -> &'static str {
    if Path::new("./mvnw").exists() {
        "./mvnw"
    } else {
        "mvn"
    }
}

/// Generic Maven command runner with filtering
fn run_mvn_filtered<F>(subcommand: &str, args: &[String], verbose: u8, filter_fn: F) -> Result<()>
where
    F: Fn(&str) -> String,
{
    let timer = tracking::TimedExecution::start();
    let mvn = mvn_executable();

    let mut cmd = Command::new(mvn);
    cmd.arg(subcommand);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {} {}", mvn, subcommand, args.join(" "));
    }

    let output = cmd.output().with_context(|| {
        format!(
            "Failed to run {} {}. Is Maven installed or is ./mvnw available?",
            mvn, subcommand
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

    if let Some(hint) = crate::tee::tee_and_hint(&raw, &format!("mvn_{}", subcommand), exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("mvn {} {}", subcommand, args.join(" ")),
        &format!("rtk mvn {} {}", subcommand, args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

pub fn run_compile(args: &[String], verbose: u8) -> Result<()> {
    run_mvn_filtered("compile", args, verbose, filter_mvn_build)
}

pub fn run_test(args: &[String], verbose: u8) -> Result<()> {
    run_mvn_filtered("test", args, verbose, filter_mvn_test)
}

pub fn run_package(args: &[String], verbose: u8) -> Result<()> {
    run_mvn_filtered("package", args, verbose, filter_mvn_build)
}

pub fn run_install(args: &[String], verbose: u8) -> Result<()> {
    run_mvn_filtered("install", args, verbose, filter_mvn_build)
}

pub fn run_clean(args: &[String], verbose: u8) -> Result<()> {
    run_mvn_filtered("clean", args, verbose, filter_mvn_build)
}

pub fn run_dependency_tree(args: &[String], verbose: u8) -> Result<()> {
    run_mvn_filtered("dependency:tree", args, verbose, filter_mvn_dependency_tree)
}

pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("mvn: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();
    let mvn = mvn_executable();
    let subcommand = args[0].to_string_lossy();

    let mut cmd = Command::new(mvn);
    cmd.arg(&*subcommand);
    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {} ...", mvn, subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {} {}", mvn, subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    print!("{}", stdout);
    eprint!("{}", stderr);

    timer.track(
        &format!("mvn {}", subcommand),
        &format!("rtk mvn {}", subcommand),
        &raw,
        &raw,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Filter Maven build/compile/package/install output.
/// Strips [INFO] noise, download progress, keeps errors, warnings, and BUILD result.
fn filter_mvn_build(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut build_result = String::new();
    let mut in_error_block = false;
    let mut current_error: Vec<String> = Vec::new();
    let mut total_time = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty lines
        if trimmed.is_empty() {
            continue;
        }

        // Strip [INFO], [WARNING], [ERROR] prefixes for analysis
        let content = strip_mvn_prefix(trimmed);

        // Skip noise: download progress, artifact resolution
        if content.starts_with("Downloading from")
            || content.starts_with("Downloaded from")
            || content.starts_with("Progress")
            || content.starts_with("Downloading:")
            || content.starts_with("Downloaded:")
            || content.contains("kB downloaded")
            || content.contains("KB downloaded")
            || content.starts_with("---")
            || content == "---"
            || content.starts_with("Scanning for projects")
            || content.starts_with("Using the builder")
            || content.starts_with("Nothing to compile")
        {
            continue;
        }

        // Skip separator lines
        if content.chars().all(|c| c == '-' || c == '=') && content.len() > 3 {
            continue;
        }

        // Skip Apache Maven banner
        if content.starts_with("Apache Maven")
            || content.starts_with("Maven home:")
            || content.starts_with("Java version:")
            || content.starts_with("Java home:")
            || content.starts_with("Default locale:")
            || content.starts_with("OS name:")
        {
            continue;
        }

        // Skip compilation/resource processing [INFO] noise
        if trimmed.starts_with("[INFO]") {
            // Keep BUILD SUCCESS/FAILURE
            if content.starts_with("BUILD SUCCESS") || content.starts_with("BUILD FAILURE") {
                build_result = content.to_string();
                continue;
            }

            // Capture total time
            if content.starts_with("Total time:") {
                total_time = content.to_string();
                continue;
            }

            // Skip verbose module info
            if content.starts_with("Building ")
                || content.starts_with("Compiling ")
                || content.starts_with("Copying ")
                || content.starts_with("Changes detected")
                || content.starts_with("No sources to compile")
                || content.starts_with("skip non existing")
                || content.starts_with("Replacing ")
                || content.starts_with("Including ")
                || content.starts_with("Building jar:")
                || content.starts_with("Installing ")
                || content.starts_with("Surefire report")
                || content.starts_with("Tests run:")
                || content.starts_with("Reactor Summary")
                || content.starts_with("Reactor Build Order")
                || content.contains("SUCCESS")
                || content.contains("SKIPPED")
            {
                // Keep "Tests run:" summary line from surefire
                if content.starts_with("Tests run:") {
                    // This is handled by test filter, skip in build filter
                }
                continue;
            }

            // Skip all other [INFO] lines (most verbose noise)
            continue;
        }

        // Capture [ERROR] blocks
        if trimmed.starts_with("[ERROR]") {
            if !in_error_block {
                // Flush previous error
                if !current_error.is_empty() {
                    errors.push(current_error.join("\n"));
                    current_error.clear();
                }
                in_error_block = true;
            }
            current_error.push(content.to_string());
            continue;
        }

        // End of error block on non-error line
        if in_error_block && !trimmed.starts_with("[ERROR]") {
            if !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            in_error_block = false;
        }

        // Capture [WARNING] lines
        if trimmed.starts_with("[WARNING]") {
            warnings.push(content.to_string());
            continue;
        }
    }

    // Flush any remaining error
    if !current_error.is_empty() {
        errors.push(current_error.join("\n"));
    }

    let mut result = String::new();

    // Success case
    if build_result.contains("SUCCESS") && errors.is_empty() {
        result.push_str(&format!("✓ Maven: {}", build_result));
        if !total_time.is_empty() {
            result.push_str(&format!(
                " ({})",
                total_time.trim_start_matches("Total time: ")
            ));
        }

        if !warnings.is_empty() {
            result.push_str(&format!(
                "\n{} warning{}",
                warnings.len(),
                if warnings.len() > 1 { "s" } else { "" }
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
    if !errors.is_empty() || build_result.contains("FAILURE") {
        result.push_str(&format!(
            "Maven build: {} error{}",
            errors.len(),
            if errors.len() > 1 { "s" } else { "" }
        ));
        if !warnings.is_empty() {
            result.push_str(&format!(", {} warnings", warnings.len()));
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

    // No build result found
    if output.trim().is_empty() {
        return "✓ Maven: Complete".to_string();
    }

    // Fallback: abbreviated output
    let lines: Vec<&str> = output.lines().collect();
    if lines.len() <= 5 {
        return output.trim().to_string();
    }

    result.push_str(&format!("Maven: {} lines of output\n", lines.len()));
    for line in lines.iter().take(5) {
        result.push_str(&format!("  {}\n", truncate(line, 120)));
    }
    result.push_str(&format!("  ... +{} more lines", lines.len() - 5));
    result.trim().to_string()
}

/// Filter Maven test output (mvn test / mvn verify).
/// Shows only failures and summary, strips per-test noise.
fn filter_mvn_test(output: &str) -> String {
    let mut total_tests = 0;
    let mut total_failures = 0;
    let mut total_errors = 0;
    let mut total_skipped = 0;
    let mut failed_tests: Vec<(String, Vec<String>)> = Vec::new();
    let mut in_failure_block = false;
    let mut current_failure_name = String::new();
    let mut current_failure_lines: Vec<String> = Vec::new();
    let mut build_result = String::new();
    let mut total_time = String::new();
    let mut seen_summary = false;

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        let content = strip_mvn_prefix(trimmed);

        // Skip noise
        if content.starts_with("Downloading from")
            || content.starts_with("Downloaded from")
            || content.starts_with("Scanning for projects")
            || content.starts_with("Using the builder")
            || content.starts_with("Surefire report")
            || content.starts_with("Apache Maven")
            || content.starts_with("Maven home:")
            || content.starts_with("Java version:")
            || content.starts_with("Java home:")
            || content.starts_with("Default locale:")
            || content.starts_with("OS name:")
        {
            continue;
        }

        // Skip separator lines
        if content.chars().all(|c| c == '-' || c == '=') && content.len() > 3 {
            continue;
        }

        // BUILD result
        if content.starts_with("BUILD SUCCESS") || content.starts_with("BUILD FAILURE") {
            build_result = content.to_string();
            continue;
        }

        // Total time
        if content.starts_with("Total time:") {
            total_time = content.to_string();
            continue;
        }

        // Parse Surefire/Failsafe summary: "Tests run: 15, Failures: 2, Errors: 0, Skipped: 1"
        // Skip per-class lines (contain "-- in ClassName"), only use final aggregate summary
        if content.starts_with("Tests run:") && content.contains("Failures:") {
            if !content.contains(" -- in ") {
                if let Some((t, f, e, s)) = parse_surefire_summary(&content) {
                    total_tests += t;
                    total_failures += f;
                    total_errors += e;
                    total_skipped += s;
                    seen_summary = true;
                }
            }
            continue;
        }

        // Detect test failure marker (Surefire format)
        // Lines like: "testMethodName(com.example.AppTest)  Time elapsed: 0.001 s  <<< FAILURE!"
        // or: "testMethodName(com.example.AppTest)  Time elapsed: 0.001 s  <<< ERROR!"
        if content.contains("<<< FAILURE!") || content.contains("<<< ERROR!") {
            // Flush previous failure
            if !current_failure_name.is_empty() {
                failed_tests.push((current_failure_name.clone(), current_failure_lines.clone()));
                current_failure_lines.clear();
            }
            // Extract test name
            current_failure_name = content
                .split("Time elapsed")
                .next()
                .unwrap_or(&content)
                .trim()
                .to_string();
            in_failure_block = true;
            continue;
        }

        // Collect failure output lines
        if in_failure_block {
            // End of failure block on next test or [INFO]/[ERROR] marker
            if content.starts_with("Tests run:")
                || (trimmed.starts_with("[INFO]")
                    && (content.starts_with("Building ")
                        || content.starts_with("Compiling ")
                        || content.starts_with("Reactor")))
            {
                if !current_failure_name.is_empty() {
                    failed_tests
                        .push((current_failure_name.clone(), current_failure_lines.clone()));
                    current_failure_name.clear();
                    current_failure_lines.clear();
                }
                in_failure_block = false;
                // Re-process this line (it might be a summary)
                if content.starts_with("Tests run:")
                    && content.contains("Failures:")
                    && !content.contains(" -- in ")
                {
                    if let Some((t, f, e, s)) = parse_surefire_summary(&content) {
                        total_tests += t;
                        total_failures += f;
                        total_errors += e;
                        total_skipped += s;
                        seen_summary = true;
                    }
                }
                continue;
            }
            current_failure_lines.push(content.to_string());
            continue;
        }
    }

    // Flush last failure
    if !current_failure_name.is_empty() {
        failed_tests.push((current_failure_name, current_failure_lines));
    }

    let total_fail = total_failures + total_errors;
    let passed = if total_tests >= total_fail + total_skipped {
        total_tests - total_fail - total_skipped
    } else {
        0
    };

    // No test results found
    if !seen_summary && total_tests == 0 {
        if build_result.contains("SUCCESS") {
            return "✓ Maven test: No tests found (BUILD SUCCESS)".to_string();
        }
        if build_result.contains("FAILURE") {
            return filter_mvn_build(output);
        }
        return "Maven test: No test results found".to_string();
    }

    // All passed
    if total_fail == 0 {
        let mut result = format!("✓ Maven test: {} passed", passed);
        if total_skipped > 0 {
            result.push_str(&format!(", {} skipped", total_skipped));
        }
        if !total_time.is_empty() {
            result.push_str(&format!(
                " ({})",
                total_time.trim_start_matches("Total time: ")
            ));
        }
        return result;
    }

    // Has failures
    let mut result = String::new();
    result.push_str(&format!(
        "Maven test: {} passed, {} failed",
        passed, total_fail
    ));
    if total_skipped > 0 {
        result.push_str(&format!(", {} skipped", total_skipped));
    }
    result.push('\n');
    result.push_str("═══════════════════════════════════════\n");

    for (test_name, output_lines) in failed_tests.iter().take(10) {
        result.push_str(&format!("  ❌ {}\n", test_name));

        // Show relevant failure lines
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

/// Filter Maven dependency:tree output.
/// Compacts the tree, showing only direct dependencies.
fn filter_mvn_dependency_tree(output: &str) -> String {
    let mut direct_deps: Vec<String> = Vec::new();
    let mut in_tree = false;
    let mut project_name = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        let content = strip_mvn_prefix(trimmed);

        // Skip noise
        if content.starts_with("Downloading from")
            || content.starts_with("Downloaded from")
            || content.starts_with("Scanning for projects")
            || content.starts_with("Building ")
            || content.starts_with("Apache Maven")
            || content.starts_with("Maven home:")
            || content.starts_with("Java version:")
        {
            continue;
        }

        // Skip separator and empty content lines
        if content.chars().all(|c| c == '-' || c == '=') && content.len() > 3 {
            continue;
        }

        // Skip BUILD lines
        if content.starts_with("BUILD") || content.starts_with("Total time:") {
            continue;
        }

        // Detect tree header (project GAV)
        if content.starts_with("maven-dependency-plugin")
            || content.contains(":tree")
            || content.contains("Verbose not supported")
        {
            in_tree = true;
            continue;
        }

        // Detect project root in tree (no leading tree chars)
        if in_tree
            && !content.starts_with('+')
            && !content.starts_with('|')
            && !content.starts_with('\\')
            && !content.starts_with(' ')
            && content.contains(':')
            && !content.starts_with('[')
        {
            project_name = content.to_string();
            continue;
        }

        // Collect direct dependencies (first level: "+- " or "\- ")
        if in_tree && (content.starts_with("+- ") || content.starts_with("\\- ")) {
            let dep = content
                .trim_start_matches("+- ")
                .trim_start_matches("\\- ")
                .to_string();
            direct_deps.push(dep);
        }
    }

    if direct_deps.is_empty() {
        return "Maven dependencies: No dependencies found".to_string();
    }

    let mut result = String::new();
    if !project_name.is_empty() {
        result.push_str(&format!("Maven dependencies for {}:\n", project_name));
    } else {
        result.push_str("Maven dependencies:\n");
    }
    result.push_str(&format!("{} direct dependencies\n", direct_deps.len()));
    result.push_str("═══════════════════════════════════════\n");

    for dep in direct_deps.iter().take(30) {
        result.push_str(&format!("  {}\n", truncate(dep, 100)));
    }
    if direct_deps.len() > 30 {
        result.push_str(&format!("  ... +{} more\n", direct_deps.len() - 30));
    }

    result.trim().to_string()
}

/// Strip Maven log level prefix: "[INFO] ", "[WARNING] ", "[ERROR] "
fn strip_mvn_prefix(line: &str) -> &str {
    let trimmed = line.trim();
    if let Some(rest) = trimmed.strip_prefix("[INFO] ") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("[WARNING] ") {
        rest
    } else if let Some(rest) = trimmed.strip_prefix("[ERROR] ") {
        rest
    } else if trimmed == "[INFO]" || trimmed == "[WARNING]" || trimmed == "[ERROR]" {
        ""
    } else {
        trimmed
    }
}

/// Parse Surefire summary line: "Tests run: 15, Failures: 2, Errors: 0, Skipped: 1"
fn parse_surefire_summary(line: &str) -> Option<(usize, usize, usize, usize)> {
    let tests = extract_number_after(line, "Tests run:")?;
    let failures = extract_number_after(line, "Failures:").unwrap_or(0);
    let errors = extract_number_after(line, "Errors:").unwrap_or(0);
    let skipped = extract_number_after(line, "Skipped:").unwrap_or(0);
    Some((tests, failures, errors, skipped))
}

/// Extract a number after a label like "Tests run: 15,"
fn extract_number_after(line: &str, label: &str) -> Option<usize> {
    let after = line.split(label).nth(1)?;
    let num_str = after.trim().split(|c: char| !c.is_ascii_digit()).next()?;
    num_str.parse().ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_mvn_build_success() {
        let output = r#"[INFO] Scanning for projects...
[INFO]
[INFO] ----------------------< com.example:myapp >-----------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-resources-plugin:3.3.1:resources (default-resources) @ myapp ---
[INFO] Copying 1 resource from src/main/resources to target/classes
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Nothing to compile - all classes are up to date.
[INFO]
[INFO] --- maven-resources-plugin:3.3.1:testResources (default-testResources) @ myapp ---
[INFO] skip non existing resourceDirectory /home/user/myapp/src/test/resources
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:testCompile (default-testCompile) @ myapp ---
[INFO] No sources to compile.
[INFO]
[INFO] --- maven-jar-plugin:3.3.0:jar (default-jar) @ myapp ---
[INFO] Building jar: /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar
[INFO]
[INFO] --- maven-install-plugin:3.1.1:install (default-install) @ myapp ---
[INFO] Installing /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar to /home/user/.m2/repository/com/example/myapp/1.0-SNAPSHOT/myapp-1.0-SNAPSHOT.jar
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  2.345 s
[INFO] Finished at: 2026-02-28T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_build(output);
        assert!(result.contains("✓ Maven"));
        assert!(result.contains("BUILD SUCCESS"));
        assert!(result.contains("2.345 s"));

        let savings = 100.0 - (count_tokens(&result) as f64 / count_tokens(output) as f64 * 100.0);
        assert!(
            savings >= 80.0,
            "Expected ≥80% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_mvn_build_with_errors() {
        let output = r#"[INFO] Scanning for projects...
[INFO] ----------------------< com.example:myapp >-----------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[ERROR] /src/main/java/App.java:[10,5] error: cannot find symbol
[ERROR] /src/main/java/App.java:[15,2] error: incompatible types
[INFO] ------------------------------------------------------------------------
[INFO] BUILD FAILURE
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  1.234 s"#;

        let result = filter_mvn_build(output);
        assert!(result.contains("error"));
        assert!(result.contains("Maven build:"));
        assert!(!result.contains("Scanning for projects"));
    }

    #[test]
    fn test_filter_mvn_build_empty() {
        let result = filter_mvn_build("");
        assert!(result.contains("✓ Maven"));
    }

    #[test]
    fn test_filter_mvn_test_all_pass() {
        let output = r#"[INFO] Scanning for projects...
[INFO] ----------------------< com.example:myapp >-----------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Nothing to compile - all classes are up to date.
[INFO] --- maven-compiler-plugin:3.11.0:testCompile (default-testCompile) @ myapp ---
[INFO] Nothing to compile - all classes are up to date.
[INFO] --- maven-surefire-plugin:3.2.3:test (default-test) @ myapp ---
[INFO] Using auto detected provider org.apache.maven.surefire.junitplatform.JUnitPlatformProvider
[INFO]
[INFO] -------------------------------------------------------
[INFO]  T E S T S
[INFO] -------------------------------------------------------
[INFO] Running com.example.AppTest
[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.123 s -- in com.example.AppTest
[INFO] Running com.example.UtilsTest
[INFO] Tests run: 3, Failures: 0, Errors: 0, Skipped: 0, Time elapsed: 0.045 s -- in com.example.UtilsTest
[INFO]
[INFO] Results:
[INFO]
[INFO] Tests run: 8, Failures: 0, Errors: 0, Skipped: 0
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  4.567 s
[INFO] Finished at: 2026-02-28T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_test(output);
        assert!(result.contains("✓ Maven test"));
        assert!(result.contains("8 passed"));
        assert!(result.contains("4.567 s"));

        let savings = 100.0 - (count_tokens(&result) as f64 / count_tokens(output) as f64 * 100.0);
        assert!(
            savings >= 80.0,
            "Expected ≥80% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_filter_mvn_test_with_failures() {
        let output = r#"[INFO] -------------------------------------------------------
[INFO]  T E S T S
[INFO] -------------------------------------------------------
[INFO] Running com.example.AppTest
[ERROR] Tests run: 3, Failures: 1, Errors: 0, Skipped: 0, Time elapsed: 0.234 s <<< FAILURE! -- in com.example.AppTest
[ERROR] testBroken(com.example.AppTest)  Time elapsed: 0.012 s  <<< FAILURE!
org.opentest4j.AssertionFailedError: expected: <5> but was: <3>
	at org.junit.jupiter.api.AssertEquals.assertEquals(AssertEquals.java:166)
	at com.example.AppTest.testBroken(AppTest.java:25)

[INFO]
[INFO] Results:
[INFO]
[ERROR] Tests run: 3, Failures: 1, Errors: 0, Skipped: 0
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD FAILURE
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  3.456 s"#;

        let result = filter_mvn_test(output);
        assert!(result.contains("failed"));
        assert!(result.contains("testBroken"));
        assert!(result.contains("expected: <5> but was: <3>"));
    }

    #[test]
    fn test_filter_mvn_test_with_skipped() {
        let output = r#"[INFO] -------------------------------------------------------
[INFO]  T E S T S
[INFO] -------------------------------------------------------
[INFO] Running com.example.AppTest
[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 2, Time elapsed: 0.1 s -- in com.example.AppTest
[INFO]
[INFO] Results:
[INFO]
[INFO] Tests run: 5, Failures: 0, Errors: 0, Skipped: 2
[INFO]
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  2.0 s"#;

        let result = filter_mvn_test(output);
        assert!(result.contains("✓ Maven test"));
        assert!(result.contains("3 passed"));
        assert!(result.contains("2 skipped"));
    }

    #[test]
    fn test_filter_mvn_test_no_tests() {
        let output = r#"[INFO] Scanning for projects...
[INFO] ----------------------< com.example:myapp >-----------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] No tests to run.
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_test(output);
        assert!(result.contains("No tests found"));
    }

    #[test]
    fn test_filter_mvn_dependency_tree() {
        let output = r#"[INFO] Scanning for projects...
[INFO] ----------------------< com.example:myapp >-----------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-dependency-plugin:3.6.1:tree (default-cli) @ myapp ---
[INFO] com.example:myapp:jar:1.0-SNAPSHOT
[INFO] +- org.springframework.boot:spring-boot-starter-web:jar:3.2.0:compile
[INFO] |  +- org.springframework.boot:spring-boot-starter:jar:3.2.0:compile
[INFO] |  +- org.springframework.boot:spring-boot-starter-json:jar:3.2.0:compile
[INFO] |  \- org.springframework.boot:spring-boot-starter-tomcat:jar:3.2.0:compile
[INFO] +- com.google.guava:guava:jar:32.1.3-jre:compile
[INFO] |  +- com.google.guava:failureaccess:jar:1.0.1:compile
[INFO] |  \- com.google.guava:listenablefuture:jar:9999.0-empty-to-avoid-conflict-with-guava:compile
[INFO] \- org.projectlombok:lombok:jar:1.18.30:provided
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  1.234 s"#;

        let result = filter_mvn_dependency_tree(output);
        assert!(result.contains("Maven dependencies"));
        assert!(result.contains("3 direct dependencies"));
        assert!(result.contains("spring-boot-starter-web"));
        assert!(result.contains("guava"));
        assert!(result.contains("lombok"));
        // Should not include transitive deps
        assert!(!result.contains("failureaccess"));

        let savings = 100.0 - (count_tokens(&result) as f64 / count_tokens(output) as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "Expected ≥40% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_strip_mvn_prefix() {
        assert_eq!(strip_mvn_prefix("[INFO] BUILD SUCCESS"), "BUILD SUCCESS");
        assert_eq!(
            strip_mvn_prefix("[WARNING] Using deprecated API"),
            "Using deprecated API"
        );
        assert_eq!(
            strip_mvn_prefix("[ERROR] Compilation failed"),
            "Compilation failed"
        );
        assert_eq!(strip_mvn_prefix("[INFO]"), "");
        assert_eq!(strip_mvn_prefix("plain text"), "plain text");
    }

    #[test]
    fn test_parse_surefire_summary() {
        assert_eq!(
            parse_surefire_summary("Tests run: 15, Failures: 2, Errors: 1, Skipped: 3"),
            Some((15, 2, 1, 3))
        );
        assert_eq!(
            parse_surefire_summary("Tests run: 10, Failures: 0, Errors: 0, Skipped: 0"),
            Some((10, 0, 0, 0))
        );
    }

    #[test]
    fn test_extract_number_after() {
        assert_eq!(
            extract_number_after("Tests run: 15, Failures: 2", "Tests run:"),
            Some(15)
        );
        assert_eq!(
            extract_number_after("Tests run: 15, Failures: 2", "Failures:"),
            Some(2)
        );
        assert_eq!(extract_number_after("no match here", "Tests run:"), None);
    }

    #[test]
    fn test_mvn_build_token_savings() {
        let output = r#"[INFO] Scanning for projects...
[INFO]
[INFO] ----------------------< com.example:myapp >-----------------------
[INFO] Building myapp 1.0-SNAPSHOT
[INFO] --------------------------------[ jar ]---------------------------------
[INFO]
[INFO] --- maven-resources-plugin:3.3.1:resources (default-resources) @ myapp ---
[INFO] Copying 1 resource from src/main/resources to target/classes
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 15 source files to /home/user/myapp/target/classes
[INFO]
[INFO] --- maven-resources-plugin:3.3.1:testResources (default-testResources) @ myapp ---
[INFO] Copying 2 resources from src/test/resources to target/test-classes
[INFO]
[INFO] --- maven-compiler-plugin:3.11.0:testCompile (default-testCompile) @ myapp ---
[INFO] Compiling 8 source files to /home/user/myapp/target/test-classes
[INFO]
[INFO] --- maven-surefire-plugin:3.2.3:test (default-test) @ myapp ---
[INFO] Using auto detected provider org.apache.maven.surefire.junitplatform.JUnitPlatformProvider
[INFO]
[INFO] --- maven-jar-plugin:3.3.0:jar (default-jar) @ myapp ---
[INFO] Building jar: /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar
[INFO]
[INFO] --- maven-install-plugin:3.1.1:install (default-install) @ myapp ---
[INFO] Installing /home/user/myapp/pom.xml to /home/user/.m2/repository/com/example/myapp/1.0-SNAPSHOT/myapp-1.0-SNAPSHOT.pom
[INFO] Installing /home/user/myapp/target/myapp-1.0-SNAPSHOT.jar to /home/user/.m2/repository/com/example/myapp/1.0-SNAPSHOT/myapp-1.0-SNAPSHOT.jar
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  5.678 s
[INFO] Finished at: 2026-02-28T10:30:00Z
[INFO] ------------------------------------------------------------------------"#;

        let result = filter_mvn_build(output);
        let input_tokens = count_tokens(output);
        let output_tokens = count_tokens(&result);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 80.0,
            "Expected ≥80% savings on verbose Maven build, got {:.1}% (input: {}, output: {})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_mvn_build_with_warnings() {
        let output = r#"[INFO] Scanning for projects...
[INFO] Building myapp 1.0-SNAPSHOT
[WARNING] Using platform encoding (UTF-8 actually) to copy filtered resources
[WARNING] bootstrap class path not set in conjunction with -source 8
[INFO] --- maven-compiler-plugin:3.11.0:compile (default-compile) @ myapp ---
[INFO] Compiling 5 source files
[INFO] ------------------------------------------------------------------------
[INFO] BUILD SUCCESS
[INFO] ------------------------------------------------------------------------
[INFO] Total time:  1.5 s"#;

        let result = filter_mvn_build(output);
        assert!(result.contains("✓ Maven"));
        assert!(result.contains("2 warning"));
        assert!(result.contains("platform encoding"));
    }
}
