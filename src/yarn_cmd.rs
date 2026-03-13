use crate::tracking;
use anyhow::{Context, Result};
use std::process::Command;

/// Run a yarn script with intelligent routing to specialized filters.
///
/// Known scripts (test, build, lint, typecheck) are delegated to existing
/// optimized filters. Unknown scripts fall back to generic boilerplate stripping.
pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("yarn requires a script argument");
    }

    // Strip optional "run" prefix (yarn run test → yarn test)
    let effective_args = if args[0] == "run" && args.len() > 1 {
        &args[1..]
    } else {
        args
    };

    let script = effective_args[0].as_str();
    let script_args = &effective_args[1..];

    if verbose > 0 {
        eprintln!("yarn: routing '{}' (args: {:?})", script, script_args);
    }

    // Intelligent routing: delegate to specialized filters
    match script {
        "test" | "test:unit" | "test:e2e" | "test:integration" => {
            run_via_yarn_with_filter(effective_args, verbose, skip_env, "test", |raw| {
                filter_test_output(raw)
            })
        }
        "build" => {
            // Next.js build detection: delegate to next_cmd if next is involved
            run_via_yarn_with_filter(effective_args, verbose, skip_env, "build", |raw| {
                filter_build_output(raw)
            })
        }
        "lint" | "lint:fix" => {
            // Try eslint/biome filter
            run_via_yarn_with_filter(effective_args, verbose, skip_env, "lint", |raw| {
                filter_lint_output(raw)
            })
        }
        "typecheck" | "type-check" | "tsc" | "check-types" => {
            // TypeScript type checking — run via yarn, filter with tsc filter
            run_via_yarn_with_filter(effective_args, verbose, skip_env, "typecheck", |raw| {
                crate::tsc_cmd::filter_tsc_output(raw)
            })
        }
        _ => {
            // Generic passthrough with yarn boilerplate stripping
            run_generic(effective_args, verbose, skip_env)
        }
    }
}

/// Execute a yarn script, capture output, apply a filter function, and track savings.
fn run_via_yarn_with_filter(
    args: &[String],
    _verbose: u8,
    skip_env: bool,
    label: &str,
    filter_fn: fn(&str) -> String,
) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("yarn");
    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    let output = cmd.output().context("Failed to run yarn")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_fn(&raw);

    let exit_code = output.status.code().unwrap_or(1);
    if let Some(hint) = crate::tee::tee_and_hint(&raw, label, exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    let args_str = args.join(" ");
    timer.track(
        &format!("yarn {}", args_str),
        &format!("rtk yarn {}", args_str),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/// Generic passthrough: strip yarn boilerplate, track savings.
fn run_generic(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("yarn");
    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    if verbose > 0 {
        eprintln!("Running: yarn {}", args.join(" "));
    }

    let output = cmd.output().context("Failed to run yarn")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_yarn_boilerplate(&raw);
    println!("{}", filtered);

    let args_str = args.join(" ");
    timer.track(
        &format!("yarn {}", args_str),
        &format!("rtk yarn {}", args_str),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Strip yarn boilerplate lines (lifecycle scripts, warnings, empty lines).
fn filter_yarn_boilerplate(output: &str) -> String {
    lazy_static::lazy_static! {
        static ref YARN_LIFECYCLE: regex::Regex =
            regex::Regex::new(r"^\$\s+.+$").unwrap();
    }

    let mut result = Vec::new();

    for line in output.lines() {
        // Skip yarn v1 lifecycle prefix: "$ next build"
        if YARN_LIFECYCLE.is_match(line) {
            continue;
        }
        // Skip yarn v1 info lines
        if line.starts_with("yarn run v") || line.starts_with("info Visit") {
            continue;
        }
        // Skip "Done in X.XXs."
        if line.starts_with("Done in ") {
            continue;
        }
        // Skip yarn warnings
        if line.contains("warning") && line.contains("yarn") {
            continue;
        }
        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        result.push(line);
    }

    if result.is_empty() {
        "ok ✓".to_string()
    } else {
        result.join("\n")
    }
}

/// Filter test output: show failures + summary only.
fn filter_test_output(output: &str) -> String {
    let cleaned = filter_yarn_boilerplate(output);

    // Detect vitest output
    if cleaned.contains("PASS") && cleaned.contains("Tests")
        || cleaned.contains("✓")
            && (cleaned.contains("test") || cleaned.contains("Tests"))
            && cleaned.contains("ms")
    {
        return filter_vitest_style(&cleaned);
    }

    // Detect jest output
    if cleaned.contains("Test Suites:") || cleaned.contains("Tests:") {
        return filter_jest_style(&cleaned);
    }

    // Fallback: show last lines + any FAIL lines
    filter_test_fallback(&cleaned)
}

/// Filter vitest-style output: show only failures + summary line.
fn filter_vitest_style(output: &str) -> String {
    let mut failures = Vec::new();
    let mut summary_lines = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.contains("FAIL") || trimmed.contains("✕") || trimmed.contains("×") {
            failures.push(line.to_string());
        }
        // Summary lines: "Tests  X passed", "Test Files", "Duration"
        if trimmed.starts_with("Tests")
            || trimmed.starts_with("Test Files")
            || trimmed.starts_with("Duration")
            || trimmed.starts_with("Start at")
        {
            summary_lines.push(line.to_string());
        }
    }

    let mut result = Vec::new();
    if !failures.is_empty() {
        result.push("❌ FAILURES:".to_string());
        for f in failures.iter().take(20) {
            result.push(f.clone());
        }
        if failures.len() > 20 {
            result.push(format!("  ... +{} more", failures.len() - 20));
        }
        result.push(String::new());
    }
    for s in &summary_lines {
        result.push(s.clone());
    }

    if result.is_empty() {
        "✅ all tests passed".to_string()
    } else {
        result.join("\n")
    }
}

/// Filter jest-style output: show only failures + summary.
fn filter_jest_style(output: &str) -> String {
    let mut failures = Vec::new();
    let mut summary_lines = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();
        if trimmed.contains("FAIL") || trimmed.contains("✕") {
            failures.push(line.to_string());
        }
        if trimmed.starts_with("Test Suites:") || trimmed.starts_with("Tests:") {
            summary_lines.push(line.to_string());
        }
    }

    let mut result = Vec::new();
    if !failures.is_empty() {
        result.push("❌ FAILURES:".to_string());
        for f in failures.iter().take(20) {
            result.push(f.clone());
        }
        if failures.len() > 20 {
            result.push(format!("  ... +{} more", failures.len() - 20));
        }
        result.push(String::new());
    }
    for s in &summary_lines {
        result.push(s.clone());
    }

    if result.is_empty() {
        "✅ all tests passed".to_string()
    } else {
        result.join("\n")
    }
}

/// Fallback test filter: show failures + last 5 non-empty lines.
fn filter_test_fallback(output: &str) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut failures = Vec::new();

    for line in &lines {
        let trimmed = line.trim();
        if trimmed.contains("FAIL") || trimmed.contains("FAILED") || trimmed.contains("✕") {
            failures.push(line.to_string());
        }
    }

    let mut result = Vec::new();
    if !failures.is_empty() {
        result.push("❌ FAILURES:".to_string());
        for f in failures.iter().take(10) {
            result.push(f.clone());
        }
        result.push(String::new());
    }

    // Show last 5 non-empty lines as summary
    let tail: Vec<&str> = lines
        .iter()
        .filter(|l| !l.trim().is_empty())
        .copied()
        .rev()
        .take(5)
        .collect();
    for line in tail.into_iter().rev() {
        result.push(line.to_string());
    }

    if result.is_empty() {
        "✅ all tests passed".to_string()
    } else {
        result.join("\n")
    }
}

/// Filter build output: strip progress bars, show errors + summary.
fn filter_build_output(output: &str) -> String {
    let cleaned = filter_yarn_boilerplate(output);

    let mut errors = Vec::new();
    let mut summary = Vec::new();
    let mut route_section = false;

    for line in cleaned.lines() {
        let trimmed = line.trim();

        // Collect errors
        if trimmed.contains("Error:")
            || trimmed.contains("error TS")
            || trimmed.starts_with("Failed")
            || trimmed.starts_with("Error")
        {
            errors.push(line);
            continue;
        }

        // Next.js route summary
        if trimmed.starts_with("Route") || trimmed.starts_with("○") || trimmed.starts_with("●")
        {
            route_section = true;
            summary.push(line);
            continue;
        }
        if route_section
            && (trimmed.starts_with("├")
                || trimmed.starts_with("└")
                || trimmed.starts_with("│")
                || trimmed.starts_with("ƒ")
                || trimmed.starts_with("+"))
        {
            summary.push(line);
            continue;
        }
        if route_section && trimmed.is_empty() {
            route_section = false;
        }

        // Build completion markers
        if trimmed.contains("compiled")
            || trimmed.contains("Build complete")
            || trimmed.contains("✓")
            || trimmed.contains("Compiled successfully")
        {
            summary.push(line);
        }
    }

    let mut result = Vec::new();
    if !errors.is_empty() {
        result.push("❌ ERRORS:".to_string());
        for e in errors.iter().take(20) {
            result.push(format!("  {}", e));
        }
        if errors.len() > 20 {
            result.push(format!("  ... +{} more", errors.len() - 20));
        }
        result.push(String::new());
    }
    for s in &summary {
        result.push(s.to_string());
    }

    if result.is_empty() {
        // Fallback: show last 5 non-empty lines
        let lines: Vec<&str> = cleaned.lines().filter(|l| !l.trim().is_empty()).collect();
        let start = lines.len().saturating_sub(5);
        for line in &lines[start..] {
            result.push(line.to_string());
        }
    }

    if result.is_empty() {
        "ok ✓ build complete".to_string()
    } else {
        result.join("\n")
    }
}

/// Filter lint output: strip boilerplate, show errors + summary.
fn filter_lint_output(output: &str) -> String {
    let cleaned = filter_yarn_boilerplate(output);

    // If no issues, just show success
    if cleaned.trim().is_empty() {
        return "ok ✓ no lint issues".to_string();
    }

    // Pass through — eslint/biome output is already useful post-boilerplate stripping
    cleaned
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_yarn_boilerplate() {
        let output = r#"yarn run v1.22.22
$ next build
   Creating an optimized production build...
   ✓ Build completed
info Visit https://yarnpkg.com/en/docs/cli/run for documentation.
Done in 12.34s.
"#;
        let result = filter_yarn_boilerplate(output);
        assert!(!result.contains("yarn run v"));
        assert!(!result.contains("$ next build"));
        assert!(!result.contains("info Visit"));
        assert!(!result.contains("Done in"));
        assert!(result.contains("Build completed"));
    }

    #[test]
    fn test_filter_yarn_boilerplate_empty() {
        let output = "yarn run v1.22.22\n$ echo hello\nDone in 0.01s.\n";
        let result = filter_yarn_boilerplate(output);
        // Only "echo hello" output remains, but it was the lifecycle line
        // The actual output after stripping is "ok ✓" since nothing remains
        assert!(!result.contains("yarn run"));
    }

    #[test]
    fn test_filter_yarn_boilerplate_preserves_content() {
        let output = "src/index.ts:5:3 - error TS2304: Cannot find name 'foo'\nFound 1 error.\n";
        let result = filter_yarn_boilerplate(output);
        assert!(result.contains("error TS2304"));
        assert!(result.contains("Found 1 error"));
    }

    #[test]
    fn test_filter_test_output_jest() {
        let output = r#"Test Suites: 2 passed, 2 total
Tests:       15 passed, 15 total
Snapshots:   0 total
Time:        3.456s
"#;
        let result = filter_test_output(output);
        assert!(result.contains("Tests:"));
        assert!(result.contains("15 passed"));
    }

    #[test]
    fn test_filter_test_output_jest_with_failures() {
        let output = r#"FAIL src/utils.test.ts
  ✕ should validate input (5ms)

Test Suites: 1 failed, 1 passed, 2 total
Tests:       1 failed, 14 passed, 15 total
"#;
        let result = filter_test_output(output);
        assert!(result.contains("FAIL"));
        assert!(result.contains("Tests:"));
    }

    #[test]
    fn test_filter_test_output_vitest() {
        let output = r#" ✓ src/utils.test.ts (3 tests) 12ms
 ✓ src/api.test.ts (5 tests) 45ms

Tests  8 passed (8)
Duration  1.23s
"#;
        let result = filter_test_output(output);
        assert!(result.contains("Tests"));
        assert!(result.contains("passed"));
    }

    #[test]
    fn test_filter_build_output_nextjs() {
        let output = r#"Creating an optimized production build...
Compiled successfully

Route (app)                    Size     First Load JS
○ /                            5.2 kB   89.1 kB
○ /about                       1.1 kB   85.0 kB
├ ○ /api/health                0 B      0 B
└ ○ /dashboard                 12.3 kB  96.2 kB

+ First Load JS shared by all  83.9 kB
"#;
        let result = filter_build_output(output);
        assert!(result.contains("Compiled successfully"));
        assert!(result.contains("Route"));
        assert!(result.contains("/about"));
    }

    #[test]
    fn test_filter_build_output_with_errors() {
        let output = r#"Creating an optimized production build...
Failed to compile.

Error: ./src/index.ts
Module not found: Can't resolve './missing'
"#;
        let result = filter_build_output(output);
        assert!(result.contains("ERRORS"));
        assert!(result.contains("Failed to compile"));
    }

    #[test]
    fn test_filter_lint_output_clean() {
        let output = "";
        let result = filter_lint_output(output);
        assert!(result.contains("ok ✓"));
    }

    #[test]
    fn test_filter_lint_output_with_issues() {
        let output = "src/index.ts:5:3 - warning: Unexpected console statement (no-console)\n";
        let result = filter_lint_output(output);
        assert!(result.contains("no-console"));
    }

    #[test]
    fn test_token_savings_test_output() {
        let input = r#"yarn run v1.22.22
$ vitest run
 ✓ src/utils.test.ts (3 tests) 12ms
 ✓ src/api.test.ts (5 tests) 45ms
 ✓ src/auth.test.ts (8 tests) 23ms
 ✓ src/db.test.ts (12 tests) 89ms
 ✓ src/routes.test.ts (7 tests) 34ms
 ✓ src/middleware.test.ts (4 tests) 15ms
 ✓ src/validators.test.ts (6 tests) 28ms
 ✓ src/helpers.test.ts (3 tests) 11ms
 ✓ src/config.test.ts (2 tests) 8ms
 ✓ src/logger.test.ts (1 test) 5ms

Tests  51 passed (51)
Test Files  10 passed (10)
Duration  2.34s
Start at  14:32:15

info Visit https://yarnpkg.com/en/docs/cli/run for documentation.
Done in 3.45s.
"#;
        let output = filter_test_output(&filter_yarn_boilerplate(input));
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "yarn test filter: expected ≥60% savings, got {:.1}% (input={}, output={})",
            savings,
            input_tokens,
            output_tokens,
        );
    }

    #[test]
    fn test_token_savings_build_output() {
        let input = r#"yarn run v1.22.22
$ next build
info  - Loaded env from /app/.env.local
info  - Linting and checking validity of types...
info  - Creating an optimized production build...
info  - Compiled successfully
info  - Collecting page data...
info  - Generating static pages (0/10)
info  - Generating static pages (2/10)
info  - Generating static pages (5/10)
info  - Generating static pages (8/10)
info  - Generating static pages (10/10)
info  - Finalizing page optimization...

Route (app)                    Size     First Load JS
○ /                            5.2 kB   89.1 kB
○ /about                       1.1 kB   85.0 kB
├ ○ /api/health                0 B      0 B
└ ○ /dashboard                 12.3 kB  96.2 kB

+ First Load JS shared by all  83.9 kB
  chunks/main-abc123.js        45.2 kB
  chunks/pages/_app-def456.js  38.7 kB

info Visit https://yarnpkg.com/en/docs/cli/run for documentation.
Done in 25.67s.
"#;
        let output = filter_build_output(&filter_yarn_boilerplate(input));
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "yarn build filter: expected ≥60% savings, got {:.1}% (input={}, output={})",
            savings,
            input_tokens,
            output_tokens,
        );
    }

    #[test]
    fn test_strip_run_prefix() {
        // "yarn run test" → effective_args = ["test"]
        let args: Vec<String> = vec!["run".into(), "test".into()];
        let effective = if args[0] == "run" && args.len() > 1 {
            &args[1..]
        } else {
            &args[..]
        };
        assert_eq!(effective, &["test"]);

        // "yarn test" → effective_args = ["test"]
        let args2: Vec<String> = vec!["test".into()];
        let effective2 = if args2[0] == "run" && args2.len() > 1 {
            &args2[1..]
        } else {
            &args2[..]
        };
        assert_eq!(effective2, &["test"]);
    }
}
