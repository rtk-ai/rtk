use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::ffi::OsString;
use std::process::Command;

#[derive(Debug, Clone)]
pub enum CargoCommand {
    Build,
    Test,
    Clippy,
    Check,
    Install,
}

pub fn run(cmd: CargoCommand, args: &[String], verbose: u8) -> Result<()> {
    match cmd {
        CargoCommand::Build => run_build(args, verbose),
        CargoCommand::Test => run_test(args, verbose),
        CargoCommand::Clippy => run_clippy(args, verbose),
        CargoCommand::Check => run_check(args, verbose),
        CargoCommand::Install => run_install(args, verbose),
    }
}

/// Generic cargo command runner with filtering
fn run_cargo_filtered<F>(subcommand: &str, args: &[String], verbose: u8, filter_fn: F) -> Result<()>
where
    F: Fn(&str) -> String,
{
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("cargo");
    cmd.arg(subcommand);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: cargo {} {}", subcommand, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run cargo {}", subcommand))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_fn(&raw);
    println!("{}", filtered);

    timer.track(
        &format!("cargo {} {}", subcommand, args.join(" ")),
        &format!("rtk cargo {} {}", subcommand, args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

fn run_build(args: &[String], verbose: u8) -> Result<()> {
    run_cargo_filtered("build", args, verbose, filter_cargo_build)
}

fn run_test(args: &[String], verbose: u8) -> Result<()> {
    run_cargo_filtered("test", args, verbose, filter_cargo_test)
}

fn run_clippy(args: &[String], verbose: u8) -> Result<()> {
    run_cargo_filtered("clippy", args, verbose, filter_cargo_clippy)
}

fn run_check(args: &[String], verbose: u8) -> Result<()> {
    run_cargo_filtered("check", args, verbose, filter_cargo_build)
}

fn run_install(args: &[String], verbose: u8) -> Result<()> {
    run_cargo_filtered("install", args, verbose, filter_cargo_install)
}

/// Format crate name + version into a display string
fn format_crate_info(name: &str, version: &str, fallback: &str) -> String {
    if name.is_empty() {
        fallback.to_string()
    } else if version.is_empty() {
        name.to_string()
    } else {
        format!("{} {}", name, version)
    }
}

/// Filter cargo install output - strip dep compilation, keep installed/replaced/errors
fn filter_cargo_install(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut error_count = 0;
    let mut compiled = 0;
    let mut in_error = false;
    let mut current_error = Vec::new();
    let mut installed_crate = String::new();
    let mut installed_version = String::new();
    let mut replaced_lines: Vec<String> = Vec::new();
    let mut already_installed = false;
    let mut ignored_line = String::new();

    for line in output.lines() {
        let trimmed = line.trim_start();

        // Strip noise: dep compilation, downloading, locking, etc.
        if trimmed.starts_with("Compiling") {
            compiled += 1;
            continue;
        }
        if trimmed.starts_with("Downloading")
            || trimmed.starts_with("Downloaded")
            || trimmed.starts_with("Locking")
            || trimmed.starts_with("Updating")
            || trimmed.starts_with("Adding")
            || trimmed.starts_with("Finished")
            || trimmed.starts_with("Blocking waiting for file lock")
        {
            continue;
        }

        // Keep: Installing line (extract crate name + version)
        if trimmed.starts_with("Installing") {
            let rest = trimmed.strip_prefix("Installing").unwrap_or("").trim();
            if !rest.is_empty() && !rest.starts_with('/') {
                if let Some((name, version)) = rest.split_once(' ') {
                    installed_crate = name.to_string();
                    installed_version = version.to_string();
                } else {
                    installed_crate = rest.to_string();
                }
            }
            continue;
        }

        // Keep: Installed line (extract crate + version if not already set)
        if trimmed.starts_with("Installed") {
            let rest = trimmed.strip_prefix("Installed").unwrap_or("").trim();
            if !rest.is_empty() && installed_crate.is_empty() {
                let mut parts = rest.split_whitespace();
                if let (Some(name), Some(version)) = (parts.next(), parts.next()) {
                    installed_crate = name.to_string();
                    installed_version = version.to_string();
                }
            }
            continue;
        }

        // Keep: Replacing/Replaced lines
        if trimmed.starts_with("Replacing") || trimmed.starts_with("Replaced") {
            replaced_lines.push(trimmed.to_string());
            continue;
        }

        // Keep: "Ignored package" (already up to date)
        if trimmed.starts_with("Ignored package") {
            already_installed = true;
            ignored_line = trimmed.to_string();
            continue;
        }

        // Keep: actionable warnings (e.g., "be sure to add `/path` to your PATH")
        // Skip summary lines like "warning: `crate` generated N warnings"
        if line.starts_with("warning:") {
            if !(line.contains("generated") && line.contains("warning")) {
                replaced_lines.push(line.to_string());
            }
            continue;
        }

        // Detect error blocks
        if line.starts_with("error[") || line.starts_with("error:") {
            if line.contains("aborting due to") || line.contains("could not compile") {
                continue;
            }
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            error_count += 1;
            in_error = true;
            current_error.push(line.to_string());
        } else if in_error {
            if line.trim().is_empty() && current_error.len() > 3 {
                errors.push(current_error.join("\n"));
                current_error.clear();
                in_error = false;
            } else {
                current_error.push(line.to_string());
            }
        }
    }

    if !current_error.is_empty() {
        errors.push(current_error.join("\n"));
    }

    // Already installed / up to date
    if already_installed {
        let info = ignored_line.split('`').nth(1).unwrap_or(&ignored_line);
        return format!("✓ cargo install: {} already installed", info);
    }

    // Errors
    if error_count > 0 {
        let crate_info = format_crate_info(&installed_crate, &installed_version, "");
        let deps_info = if compiled > 0 {
            format!(", {} deps compiled", compiled)
        } else {
            String::new()
        };

        let mut result = String::new();
        if crate_info.is_empty() {
            result.push_str(&format!(
                "cargo install: {} error{}{}\n",
                error_count,
                if error_count > 1 { "s" } else { "" },
                deps_info
            ));
        } else {
            result.push_str(&format!(
                "cargo install: {} error{} ({}{})\n",
                error_count,
                if error_count > 1 { "s" } else { "" },
                crate_info,
                deps_info
            ));
        }
        result.push_str("═══════════════════════════════════════\n");

        for (i, err) in errors.iter().enumerate().take(15) {
            result.push_str(err);
            result.push('\n');
            if i < errors.len() - 1 {
                result.push('\n');
            }
        }

        if errors.len() > 15 {
            result.push_str(&format!("\n... +{} more issues\n", errors.len() - 15));
        }

        return result.trim().to_string();
    }

    // Success
    let crate_info = format_crate_info(&installed_crate, &installed_version, "package");

    let mut result = format!(
        "✓ cargo install ({}, {} deps compiled)",
        crate_info, compiled
    );

    for line in &replaced_lines {
        result.push_str(&format!("\n  {}", line));
    }

    result
}

/// Filter cargo build/check output - strip "Compiling"/"Checking" lines, keep errors + summary
fn filter_cargo_build(output: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings = 0;
    let mut error_count = 0;
    let mut compiled = 0;
    let mut in_error = false;
    let mut current_error = Vec::new();

    for line in output.lines() {
        if line.trim_start().starts_with("Compiling") || line.trim_start().starts_with("Checking") {
            compiled += 1;
            continue;
        }
        if line.trim_start().starts_with("Downloading")
            || line.trim_start().starts_with("Downloaded")
        {
            continue;
        }
        if line.trim_start().starts_with("Finished") {
            continue;
        }

        // Detect error/warning blocks
        if line.starts_with("error[") || line.starts_with("error:") {
            // Skip "error: aborting due to" summary lines
            if line.contains("aborting due to") || line.contains("could not compile") {
                continue;
            }
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            error_count += 1;
            in_error = true;
            current_error.push(line.to_string());
        } else if line.starts_with("warning:")
            && line.contains("generated")
            && line.contains("warning")
        {
            // "warning: `crate` generated N warnings" summary line
            continue;
        } else if line.starts_with("warning:") || line.starts_with("warning[") {
            if in_error && !current_error.is_empty() {
                errors.push(current_error.join("\n"));
                current_error.clear();
            }
            warnings += 1;
            in_error = true;
            current_error.push(line.to_string());
        } else if in_error {
            if line.trim().is_empty() && current_error.len() > 3 {
                errors.push(current_error.join("\n"));
                current_error.clear();
                in_error = false;
            } else {
                current_error.push(line.to_string());
            }
        }
    }

    if !current_error.is_empty() {
        errors.push(current_error.join("\n"));
    }

    if error_count == 0 && warnings == 0 {
        return format!("✓ cargo build ({} crates compiled)", compiled);
    }

    let mut result = String::new();
    result.push_str(&format!(
        "cargo build: {} errors, {} warnings ({} crates)\n",
        error_count, warnings, compiled
    ));
    result.push_str("═══════════════════════════════════════\n");

    for (i, err) in errors.iter().enumerate().take(15) {
        result.push_str(err);
        result.push('\n');
        if i < errors.len() - 1 {
            result.push('\n');
        }
    }

    if errors.len() > 15 {
        result.push_str(&format!("\n... +{} more issues\n", errors.len() - 15));
    }

    result.trim().to_string()
}

/// Filter cargo test output - show failures + summary only
fn filter_cargo_test(output: &str) -> String {
    let mut failures: Vec<String> = Vec::new();
    let mut summary_lines: Vec<String> = Vec::new();
    let mut in_failure_section = false;
    let mut current_failure = Vec::new();

    for line in output.lines() {
        // Skip compilation lines
        if line.trim_start().starts_with("Compiling")
            || line.trim_start().starts_with("Downloading")
            || line.trim_start().starts_with("Downloaded")
            || line.trim_start().starts_with("Finished")
        {
            continue;
        }

        // Skip "running N tests" and individual "test ... ok" lines
        if line.starts_with("running ") || (line.starts_with("test ") && line.ends_with("... ok")) {
            continue;
        }

        // Detect failures section
        if line == "failures:" {
            in_failure_section = true;
            continue;
        }

        if in_failure_section {
            if line.starts_with("test result:") {
                in_failure_section = false;
                summary_lines.push(line.to_string());
            } else if line.starts_with("    ") || line.starts_with("---- ") {
                current_failure.push(line.to_string());
            } else if line.trim().is_empty() && !current_failure.is_empty() {
                failures.push(current_failure.join("\n"));
                current_failure.clear();
            } else if !line.trim().is_empty() {
                current_failure.push(line.to_string());
            }
        }

        // Capture test result summary
        if !in_failure_section && line.starts_with("test result:") {
            summary_lines.push(line.to_string());
        }
    }

    if !current_failure.is_empty() {
        failures.push(current_failure.join("\n"));
    }

    let mut result = String::new();

    if failures.is_empty() && !summary_lines.is_empty() {
        // All passed
        for line in &summary_lines {
            result.push_str(&format!("✓ {}\n", line));
        }
        return result.trim().to_string();
    }

    if !failures.is_empty() {
        result.push_str(&format!("FAILURES ({}):\n", failures.len()));
        result.push_str("═══════════════════════════════════════\n");
        for (i, failure) in failures.iter().enumerate().take(10) {
            result.push_str(&format!("{}. {}\n", i + 1, truncate(failure, 200)));
        }
        if failures.len() > 10 {
            result.push_str(&format!("\n... +{} more failures\n", failures.len() - 10));
        }
        result.push('\n');
    }

    for line in &summary_lines {
        result.push_str(&format!("{}\n", line));
    }

    if result.trim().is_empty() {
        // Fallback: show last meaningful lines
        let meaningful: Vec<&str> = output
            .lines()
            .filter(|l| !l.trim().is_empty() && !l.trim_start().starts_with("Compiling"))
            .collect();
        for line in meaningful.iter().rev().take(5).rev() {
            result.push_str(&format!("{}\n", line));
        }
    }

    result.trim().to_string()
}

/// Filter cargo clippy output - group warnings by lint rule
fn filter_cargo_clippy(output: &str) -> String {
    let mut by_rule: HashMap<String, Vec<String>> = HashMap::new();
    let mut error_count = 0;
    let mut warning_count = 0;

    // Parse clippy output lines
    // Format: "warning: description\n  --> file:line:col\n  |\n  | code\n"
    let mut current_rule = String::new();

    for line in output.lines() {
        // Skip compilation lines
        if line.trim_start().starts_with("Compiling")
            || line.trim_start().starts_with("Checking")
            || line.trim_start().starts_with("Downloading")
            || line.trim_start().starts_with("Downloaded")
            || line.trim_start().starts_with("Finished")
        {
            continue;
        }

        // "warning: unused variable [unused_variables]" or "warning: description [clippy::rule_name]"
        if (line.starts_with("warning:") || line.starts_with("warning["))
            || (line.starts_with("error:") || line.starts_with("error["))
        {
            // Skip summary lines: "warning: `rtk` (bin) generated 5 warnings"
            if line.contains("generated") && line.contains("warning") {
                continue;
            }
            // Skip "error: aborting" / "error: could not compile"
            if line.contains("aborting due to") || line.contains("could not compile") {
                continue;
            }

            let is_error = line.starts_with("error");
            if is_error {
                error_count += 1;
            } else {
                warning_count += 1;
            }

            // Extract rule name from brackets
            current_rule = if let Some(bracket_start) = line.rfind('[') {
                if let Some(bracket_end) = line.rfind(']') {
                    line[bracket_start + 1..bracket_end].to_string()
                } else {
                    line.to_string()
                }
            } else {
                // No bracket: use the message itself as the rule
                let prefix = if is_error { "error: " } else { "warning: " };
                line.strip_prefix(prefix).unwrap_or(line).to_string()
            };
        } else if line.trim_start().starts_with("--> ") {
            let location = line.trim_start().trim_start_matches("--> ").to_string();
            if !current_rule.is_empty() {
                by_rule
                    .entry(current_rule.clone())
                    .or_default()
                    .push(location);
            }
        }
    }

    if error_count == 0 && warning_count == 0 {
        return "✓ cargo clippy: No issues found".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!(
        "cargo clippy: {} errors, {} warnings\n",
        error_count, warning_count
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Sort rules by frequency
    let mut rule_counts: Vec<_> = by_rule.iter().collect();
    rule_counts.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (rule, locations) in rule_counts.iter().take(15) {
        result.push_str(&format!("  {} ({}x)\n", rule, locations.len()));
        for loc in locations.iter().take(3) {
            result.push_str(&format!("    {}\n", loc));
        }
        if locations.len() > 3 {
            result.push_str(&format!("    ... +{} more\n", locations.len() - 3));
        }
    }

    if by_rule.len() > 15 {
        result.push_str(&format!("\n... +{} more rules\n", by_rule.len() - 15));
    }

    result.trim().to_string()
}

/// Runs an unsupported cargo subcommand by passing it through directly
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("cargo passthrough: {:?}", args);
    }
    let status = Command::new("cargo")
        .args(args)
        .status()
        .context("Failed to run cargo")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("cargo {}", args_str),
        &format!("rtk cargo {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_cargo_build_success() {
        let output = r#"   Compiling libc v0.2.153
   Compiling cfg-if v1.0.0
   Compiling rtk v0.5.0
    Finished dev [unoptimized + debuginfo] target(s) in 15.23s
"#;
        let result = filter_cargo_build(output);
        assert!(result.contains("✓ cargo build"));
        assert!(result.contains("3 crates compiled"));
    }

    #[test]
    fn test_filter_cargo_build_errors() {
        let output = r#"   Compiling rtk v0.5.0
error[E0308]: mismatched types
 --> src/main.rs:10:5
  |
10|     "hello"
  |     ^^^^^^^ expected `i32`, found `&str`

error: aborting due to 1 previous error
"#;
        let result = filter_cargo_build(output);
        assert!(result.contains("1 errors"));
        assert!(result.contains("E0308"));
        assert!(result.contains("mismatched types"));
    }

    #[test]
    fn test_filter_cargo_test_all_pass() {
        let output = r#"   Compiling rtk v0.5.0
    Finished test [unoptimized + debuginfo] target(s) in 2.53s
     Running target/debug/deps/rtk-abc123

running 15 tests
test utils::tests::test_truncate_short_string ... ok
test utils::tests::test_truncate_long_string ... ok
test utils::tests::test_strip_ansi_simple ... ok

test result: ok. 15 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.01s
"#;
        let result = filter_cargo_test(output);
        assert!(result.contains("✓ test result: ok. 15 passed"));
        assert!(!result.contains("Compiling"));
        assert!(!result.contains("test utils"));
    }

    #[test]
    fn test_filter_cargo_test_failures() {
        let output = r#"running 5 tests
test foo::test_a ... ok
test foo::test_b ... FAILED
test foo::test_c ... ok

failures:

---- foo::test_b stdout ----
thread 'foo::test_b' panicked at 'assert_eq!(1, 2)'

failures:
    foo::test_b

test result: FAILED. 4 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out
"#;
        let result = filter_cargo_test(output);
        assert!(result.contains("FAILURES"));
        assert!(result.contains("test_b"));
        assert!(result.contains("test result:"));
    }

    #[test]
    fn test_filter_cargo_clippy_clean() {
        let output = r#"    Checking rtk v0.5.0
    Finished dev [unoptimized + debuginfo] target(s) in 1.53s
"#;
        let result = filter_cargo_clippy(output);
        assert!(result.contains("✓ cargo clippy: No issues found"));
    }

    #[test]
    fn test_filter_cargo_clippy_warnings() {
        let output = r#"    Checking rtk v0.5.0
warning: unused variable: `x` [unused_variables]
 --> src/main.rs:10:9
  |
10|     let x = 5;
  |         ^ help: if this is intentional, prefix it with an underscore: `_x`

warning: this function has too many arguments [clippy::too_many_arguments]
 --> src/git.rs:16:1
  |
16| pub fn run(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32, g: i32, h: i32) {}
  |

warning: `rtk` (bin) generated 2 warnings
    Finished dev [unoptimized + debuginfo] target(s) in 1.53s
"#;
        let result = filter_cargo_clippy(output);
        assert!(result.contains("0 errors, 2 warnings"));
        assert!(result.contains("unused_variables"));
        assert!(result.contains("clippy::too_many_arguments"));
    }

    #[test]
    fn test_filter_cargo_install_success() {
        let output = r#"  Installing rtk v0.11.0
  Downloading crates ...
  Downloaded anyhow v1.0.80
  Downloaded clap v4.5.0
   Compiling libc v0.2.153
   Compiling cfg-if v1.0.0
   Compiling anyhow v1.0.80
   Compiling clap v4.5.0
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 45.23s
  Replacing /Users/user/.cargo/bin/rtk
   Replaced package `rtk v0.9.4` with `rtk v0.11.0` (/Users/user/.cargo/bin/rtk)
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(result.contains("rtk v0.11.0"), "got: {}", result);
        assert!(result.contains("5 deps compiled"), "got: {}", result);
        assert!(result.contains("Replaced"), "got: {}", result);
        assert!(!result.contains("Compiling"), "got: {}", result);
        assert!(!result.contains("Downloading"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_replace() {
        let output = r#"  Installing rtk v0.11.0
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 10.0s
  Replacing /Users/user/.cargo/bin/rtk
   Replaced package `rtk v0.9.4` with `rtk v0.11.0` (/Users/user/.cargo/bin/rtk)
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(result.contains("Replacing"), "got: {}", result);
        assert!(result.contains("Replaced"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_error() {
        let output = r#"  Installing rtk v0.11.0
   Compiling rtk v0.11.0
error[E0308]: mismatched types
 --> src/main.rs:10:5
  |
10|     "hello"
  |     ^^^^^^^ expected `i32`, found `&str`

error: aborting due to 1 previous error
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("cargo install: 1 error"), "got: {}", result);
        assert!(result.contains("E0308"), "got: {}", result);
        assert!(result.contains("mismatched types"), "got: {}", result);
        assert!(!result.contains("aborting"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_already_installed() {
        let output = r#"  Ignored package `rtk v0.11.0`, is already installed
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("already installed"), "got: {}", result);
        assert!(result.contains("rtk v0.11.0"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_up_to_date() {
        let output = r#"  Ignored package `cargo-deb v2.1.0 (/Users/user/cargo-deb)`, is already installed
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("already installed"), "got: {}", result);
        assert!(result.contains("cargo-deb v2.1.0"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_empty_output() {
        let result = filter_cargo_install("");
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(result.contains("0 deps compiled"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_path_warning() {
        let output = r#"  Installing rtk v0.11.0
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 10.0s
  Replacing /Users/user/.cargo/bin/rtk
   Replaced package `rtk v0.9.4` with `rtk v0.11.0` (/Users/user/.cargo/bin/rtk)
warning: be sure to add `/Users/user/.cargo/bin` to your PATH
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(
            result.contains("be sure to add"),
            "PATH warning should be kept: {}",
            result
        );
        assert!(result.contains("Replaced"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_multiple_errors() {
        let output = r#"  Installing rtk v0.11.0
   Compiling rtk v0.11.0
error[E0308]: mismatched types
 --> src/main.rs:10:5
  |
10|     "hello"
  |     ^^^^^^^ expected `i32`, found `&str`

error[E0425]: cannot find value `foo`
 --> src/lib.rs:20:9
  |
20|     foo
  |     ^^^ not found in this scope

error: aborting due to 2 previous errors
"#;
        let result = filter_cargo_install(output);
        assert!(
            result.contains("2 errors"),
            "should show 2 errors: {}",
            result
        );
        assert!(result.contains("E0308"), "got: {}", result);
        assert!(result.contains("E0425"), "got: {}", result);
        assert!(!result.contains("aborting"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_locking_and_blocking() {
        let output = r#"  Locking 45 packages to latest compatible versions
  Blocking waiting for file lock on package cache
  Downloading crates ...
  Downloaded serde v1.0.200
   Compiling serde v1.0.200
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 30.0s
  Installing rtk v0.11.0
"#;
        let result = filter_cargo_install(output);
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(!result.contains("Locking"), "got: {}", result);
        assert!(!result.contains("Blocking"), "got: {}", result);
        assert!(!result.contains("Downloading"), "got: {}", result);
    }

    #[test]
    fn test_filter_cargo_install_from_path() {
        let output = r#"  Installing /Users/user/projects/rtk
   Compiling rtk v0.11.0
    Finished `release` profile [optimized] target(s) in 10.0s
"#;
        let result = filter_cargo_install(output);
        // Path-based install: crate info not extracted from path
        assert!(result.contains("✓ cargo install"), "got: {}", result);
        assert!(result.contains("1 deps compiled"), "got: {}", result);
    }

    #[test]
    fn test_format_crate_info() {
        assert_eq!(format_crate_info("rtk", "v0.11.0", ""), "rtk v0.11.0");
        assert_eq!(format_crate_info("rtk", "", ""), "rtk");
        assert_eq!(format_crate_info("", "", "package"), "package");
        assert_eq!(format_crate_info("", "v0.1.0", "fallback"), "fallback");
    }
}
