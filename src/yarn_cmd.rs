use crate::parser::{FormatMode, OutputParser, ParseResult, TokenFormatter};
use crate::tee;
use crate::tracking;
use crate::{lint_cmd, prettier_cmd, runner, tsc_cmd, vitest_cmd};
use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;

/// Filter yarn output - strip boilerplate, keep meaningful content.
///
/// Strips: YN-prefixed info lines, resolution/fetch/link progress,
/// empty lines, and yarn classic boilerplate (defensive).
/// Returns "ok checkmark" if all output was boilerplate.
pub(crate) fn filter_yarn_output(output: &str) -> String {
    lazy_static::lazy_static! {
        // YN0000-prefixed info lines from yarn berry (with or without arrow prefix)
        static ref YN_PREFIX: Regex = Regex::new(r"^[\u{27a4}\u{2794}]?\s*YN\d{4}:").unwrap();
        // Yarn classic version header: "yarn run v1.22.19"
        static ref CLASSIC_VERSION: Regex = Regex::new(r"^yarn run v\d").unwrap();
        // Yarn classic done message: "Done in 3.42s."
        static ref CLASSIC_DONE: Regex = Regex::new(r"^Done in \d").unwrap();
    }

    let mut result = Vec::new();

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip empty/whitespace-only lines
        if trimmed.is_empty() {
            continue;
        }

        // Skip yarn berry YN-prefixed info lines
        if YN_PREFIX.is_match(trimmed) {
            continue;
        }

        // Skip resolution/fetch/link progress from workspace-tools
        if trimmed.starts_with("Resolution step")
            || trimmed.starts_with("Fetch step")
            || trimmed.starts_with("Link step")
        {
            continue;
        }

        // Skip yarn classic boilerplate (defensive)
        if CLASSIC_VERSION.is_match(trimmed) {
            continue;
        }
        if CLASSIC_DONE.is_match(trimmed) {
            continue;
        }
        if trimmed.starts_with("info ") {
            continue;
        }

        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok \u{2713}".to_string()
    } else {
        result.join("\n")
    }
}

/// Which specialized filter to apply to captured yarn workspace output
enum FilterRoute {
    /// runner::extract_test_summary (generic test runner -- jest/cargo/pytest/go)
    TestRunner,
    /// vitest_cmd::VitestParser::parse (three-tier vitest parser)
    Vitest,
    /// lint_cmd::filter_generic_lint (text-based warning/error extraction)
    Lint,
    /// tsc_cmd::filter_tsc_output (TypeScript errors grouped by file/code)
    Tsc,
    /// prettier_cmd::filter_prettier_output (files needing formatting)
    Prettier,
}

/// Prefix exclusions -- compound scripts that should NOT match their prefix route.
/// These are explicitly excluded because their output format differs from what
/// the prefix route's filter expects (e.g., test:e2e is likely Playwright, not jest).
const PREFIX_EXCLUSIONS: &[&str] = &["test:e2e", "test:playwright", "test:cypress", "lint:fix"];

/// Route a yarn workspace script name to a specialized filter.
///
/// Priority: prefix exclusions > exact match > prefix match > None (passthrough).
/// Per locked decision: exact match only, hardcoded table, no fuzzy/alias matching.
fn route_script(script: &str) -> Option<FilterRoute> {
    // 1. Check prefix exclusions first (block known-bad prefix matches)
    if PREFIX_EXCLUSIONS.contains(&script) {
        return None;
    }

    // 2. Exact matches (highest priority)
    match script {
        "vitest" => return Some(FilterRoute::Vitest),
        "typecheck" | "tsc" => return Some(FilterRoute::Tsc),
        "prettier" | "format" => return Some(FilterRoute::Prettier),
        "lint" => return Some(FilterRoute::Lint),
        "test" => return Some(FilterRoute::TestRunner),
        _ => {}
    }

    // 3. Prefix matches (lower priority, after exclusions)
    if script.starts_with("test:") {
        return Some(FilterRoute::TestRunner);
    }
    if script.starts_with("lint:") {
        return Some(FilterRoute::Lint);
    }

    None
}

/// Apply a specialized filter to boilerplate-stripped yarn output.
///
/// Returns (filtered_output, tracking_label).
/// On filter panic or empty-output, falls back to clean_stdout with stderr warning (graceful degradation).
fn apply_filter(
    route: FilterRoute,
    clean_stdout: &str,
    package: &str,
    script: &str,
    verbose: u8,
) -> (String, String) {
    let clean_owned = clean_stdout.to_string();
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match route {
        FilterRoute::Vitest => {
            let mode = FormatMode::from_verbosity(verbose);
            let parse_result = vitest_cmd::VitestParser::parse(clean_stdout);
            let filtered = match parse_result {
                ParseResult::Full(data) => data.format(mode),
                ParseResult::Degraded(data, _warnings) => data.format(mode),
                ParseResult::Passthrough(raw) => raw,
            };
            (filtered, "vitest (via yarn workspace)".to_string())
        }
        FilterRoute::Tsc => {
            let filtered = tsc_cmd::filter_tsc_output(clean_stdout);
            (filtered, "tsc (via yarn workspace)".to_string())
        }
        FilterRoute::Lint => {
            let filtered = lint_cmd::filter_generic_lint(clean_stdout);
            (filtered, "lint (via yarn workspace)".to_string())
        }
        FilterRoute::Prettier => {
            let filtered = prettier_cmd::filter_prettier_output(clean_stdout);
            (filtered, "prettier (via yarn workspace)".to_string())
        }
        FilterRoute::TestRunner => {
            let cmd_hint = format!("yarn workspace {} run {}", package, script);
            let filtered = runner::extract_test_summary(clean_stdout, &cmd_hint);
            (filtered, "test (via yarn workspace)".to_string())
        }
    }));

    match result {
        Ok((filtered, label)) => {
            // Empty-output guard: if filter produced empty but input was non-empty,
            // the filter ate everything -- fall back to passthrough
            if filtered.is_empty() && !clean_owned.is_empty() {
                eprintln!(
                    "[RTK] Filter returned empty output for '{}', falling back to stripped output",
                    script
                );
                (clean_owned, "yarn workspace (passthrough)".to_string())
            } else {
                (filtered, label)
            }
        }
        Err(_) => {
            eprintln!(
                "[RTK] Filter panicked for '{}', falling back to stripped output",
                script
            );
            (clean_owned, "yarn workspace (passthrough)".to_string())
        }
    }
}

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<()> {
    // Expect: workspace <pkg> [run] <script> [extra_args...]
    if args.is_empty() || args[0] != "workspace" {
        anyhow::bail!("Usage: rtk yarn workspace <pkg> [run] <script> [args...]");
    }

    if args.len() < 3 {
        anyhow::bail!("Usage: rtk yarn workspace <pkg> [run] <script> [args...]");
    }

    let package = &args[1];

    // Determine script name and extra args
    // args[2] is either "run" (skip it, take script from args[3]) or the script name directly
    let (script, extra_args) = if args[2] == "run" {
        if args.len() < 4 {
            anyhow::bail!("Missing script name after 'run'");
        }
        (&args[3], args[4..].to_vec())
    } else {
        (&args[2], args[3..].to_vec())
    };

    let timer = tracking::TimedExecution::start();

    // Always use explicit "run" when constructing the command to avoid ambiguity
    let mut cmd = Command::new("yarn");
    cmd.arg("workspace").arg(package).arg("run").arg(script);
    for arg in &extra_args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    let extra_args_str = extra_args.join(" ");

    if verbose > 0 {
        eprintln!(
            "Running: yarn workspace {} run {} {}",
            package, script, extra_args_str
        );
    }

    let output = cmd
        .output()
        .context("Failed to run yarn workspace command")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output.status.code().unwrap_or(1);

    // Early exit on failure: skip filter routing, show raw output
    if !output.status.success() {
        if !stdout.is_empty() {
            print!("{}", stdout);
        }
        if !stderr.is_empty() {
            eprint!("{}", stderr);
        }

        // Tee raw output for recovery
        if let Some(hint) = tee::tee_and_hint(&raw, "yarn_workspace", exit_code) {
            println!("{}", hint);
        }

        // Track with failure label
        let original_cmd = format!(
            "yarn workspace {} run {} {}",
            package, script, extra_args_str
        );
        timer.track(&original_cmd, "rtk yarn workspace (failed)", &raw, &raw);

        std::process::exit(exit_code);
    }

    // Step 1: Strip yarn boilerplate (filters see clean output)
    let clean_stdout = filter_yarn_output(&stdout);

    // Step 2: Route to specialized filter or passthrough
    // Guard: if boilerplate stripping consumed everything, skip routing
    let (filtered, tracking_label) = if clean_stdout == "ok \u{2713}" {
        (
            clean_stdout.clone(),
            "yarn workspace (passthrough)".to_string(),
        )
    } else {
        match route_script(script) {
            Some(route) => apply_filter(route, &clean_stdout, package, script, verbose),
            None => (
                clean_stdout.clone(),
                "yarn workspace (passthrough)".to_string(),
            ),
        }
    };

    // Tee raw output for recovery on failure
    let final_output = if let Some(hint) = tee::tee_and_hint(&raw, "yarn_workspace", exit_code) {
        format!("{}\n{}", filtered, hint)
    } else {
        filtered.clone()
    };

    println!("{}", final_output);

    // Pass stderr through as-is to stderr
    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    // Track with routing-aware labels
    let original_cmd = format!(
        "yarn workspace {} run {} {}",
        package, script, extra_args_str
    );
    let rtk_cmd = format!("rtk yarn {}", tracking_label);
    timer.track(&original_cmd, &rtk_cmd, &raw, &final_output);

    // Preserve exit code -- must be AFTER tracking and printing
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

    #[test]
    fn test_filter_yarn_output_clean() {
        // Input with actual tool output (no boilerplate) -> output unchanged
        let input = "PASS src/auth.test.ts\n\
                      Test Suites: 1 passed, 1 total\n\
                      Tests:       5 passed, 5 total\n\
                      Snapshots:   0 total\n\
                      Time:        2.341 s";
        let result = filter_yarn_output(input);
        assert!(result.contains("PASS src/auth.test.ts"));
        assert!(result.contains("Tests:       5 passed, 5 total"));
        assert!(result.contains("Time:        2.341 s"));
    }

    #[test]
    fn test_filter_yarn_output_yn_prefix() {
        // Input with YN0000-prefixed lines mixed with real output -> YN lines stripped
        let input = "\u{27a4} YN0000: \u{2502} ESLint is enabled\n\
                      \u{27a4} YN0000: \u{2502} Browserslist: loading config\n\
                      Compiling TypeScript...\n\
                      \u{27a4} YN0000: Done\n\
                      Build succeeded with 0 errors";
        let result = filter_yarn_output(input);
        assert!(!result.contains("YN0000"));
        assert!(result.contains("Compiling TypeScript..."));
        assert!(result.contains("Build succeeded with 0 errors"));
    }

    #[test]
    fn test_filter_yarn_output_resolution_steps() {
        // Input with resolution/fetch/link progress -> stripped
        let input = "Resolution step 1/3\n\
                      Fetch step 2/3\n\
                      Link step 3/3\n\
                      All dependencies linked successfully";
        let result = filter_yarn_output(input);
        assert!(!result.contains("Resolution step"));
        assert!(!result.contains("Fetch step"));
        assert!(!result.contains("Link step"));
        assert!(result.contains("All dependencies linked successfully"));
    }

    #[test]
    fn test_filter_yarn_output_empty_after_filter() {
        // Input with only boilerplate and empty lines -> returns "ok checkmark"
        let input = "\u{27a4} YN0000: Done\n\
                      \n\
                      \n\
                      Resolution step 1/1\n\
                      ";
        let result = filter_yarn_output(input);
        assert_eq!(result, "ok \u{2713}");
    }

    #[test]
    fn test_filter_yarn_output_classic_boilerplate() {
        // Input with yarn classic patterns -> stripped
        let input = "yarn run v1.22.19\n\
                      info Visit https://yarnpkg.com/en/docs/cli/run\n\
                      $ vitest run\n\
                      PASS src/utils.test.ts\n\
                      Tests: 3 passed\n\
                      Done in 4.21s.";
        let result = filter_yarn_output(input);
        assert!(!result.contains("yarn run v1.22.19"));
        assert!(!result.contains("info Visit"));
        assert!(!result.contains("Done in 4.21s."));
        assert!(result.contains("$ vitest run"));
        assert!(result.contains("PASS src/utils.test.ts"));
        assert!(result.contains("Tests: 3 passed"));
    }

    #[test]
    fn test_filter_yarn_output_mixed() {
        // Complex input mixing boilerplate + real output + empty lines
        let input = "\u{27a4} YN0000: \u{2502} Resolution step\n\
                      Resolution step 1/2\n\
                      \n\
                      yarn run v1.22.19\n\
                      \n\
                      \u{27a4} YN0000: \u{2502} @server/api (vitest)\n\
                      \n\
                      PASS src/api/auth.test.ts\n\
                      PASS src/api/users.test.ts\n\
                      \n\
                      Test Suites: 2 passed, 2 total\n\
                      Tests:       12 passed, 12 total\n\
                      Snapshots:   0 total\n\
                      Time:        3.891 s, estimated 5 s\n\
                      \n\
                      \u{27a4} YN0000: Done in 5s 123ms\n\
                      Done in 5.12s.\n\
                      info Visit https://yarnpkg.com/en/docs/cli/run";
        let result = filter_yarn_output(input);
        // Should NOT contain any boilerplate
        assert!(!result.contains("YN0000"));
        assert!(!result.contains("Resolution step"));
        assert!(!result.contains("yarn run v1.22.19"));
        assert!(!result.contains("Done in 5.12s."));
        assert!(!result.contains("info Visit"));
        // Should contain all real test output
        assert!(result.contains("PASS src/api/auth.test.ts"));
        assert!(result.contains("PASS src/api/users.test.ts"));
        assert!(result.contains("Test Suites: 2 passed, 2 total"));
        assert!(result.contains("Tests:       12 passed, 12 total"));
        assert!(result.contains("Time:        3.891 s, estimated 5 s"));
    }

    #[test]
    fn test_filter_yarn_output_token_savings() {
        // Verify filter achieves measurable reduction on realistic mixed input
        let input = "\u{27a4} YN0000: \u{2502} resolution starting\n\
                      \u{27a4} YN0000: \u{2502} resolution completed\n\
                      Resolution step 1/3\n\
                      Fetch step 2/3\n\
                      Link step 3/3\n\
                      \n\
                      \u{27a4} YN0000: \u{2502} @myorg/server (vitest run)\n\
                      \n\
                      yarn run v1.22.19\n\
                      info Visit https://yarnpkg.com/en/docs/cli/run\n\
                      \n\
                      RUN  v1.2.0 src/\n\
                      \n\
                      PASS src/auth.test.ts (3 tests)\n\
                      PASS src/users.test.ts (5 tests)\n\
                      \n\
                      Test Files  2 passed (2)\n\
                      Tests       8 passed (8)\n\
                      Duration    1.23s\n\
                      \n\
                      \u{27a4} YN0000: Done in 3s 456ms\n\
                      Done in 3.46s.";
        let output = filter_yarn_output(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "Yarn filter: expected >=40% savings on mixed boilerplate, got {:.1}% (input={}, output={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // --- Routing tests ---

    #[test]
    fn test_route_script_exact_matches() {
        assert!(matches!(route_script("vitest"), Some(FilterRoute::Vitest)));
        assert!(matches!(route_script("typecheck"), Some(FilterRoute::Tsc)));
        assert!(matches!(route_script("tsc"), Some(FilterRoute::Tsc)));
        assert!(matches!(
            route_script("prettier"),
            Some(FilterRoute::Prettier)
        ));
        assert!(matches!(
            route_script("format"),
            Some(FilterRoute::Prettier)
        ));
        assert!(matches!(route_script("lint"), Some(FilterRoute::Lint)));
        assert!(matches!(
            route_script("test"),
            Some(FilterRoute::TestRunner)
        ));
    }

    #[test]
    fn test_route_script_unknown_returns_none() {
        assert!(route_script("build").is_none());
        assert!(route_script("dev").is_none());
        assert!(route_script("start").is_none());
        assert!(route_script("custom-script").is_none());
        assert!(route_script("deploy").is_none());
    }

    #[test]
    fn test_route_script_prefix_matching() {
        // test: prefix routes to TestRunner
        assert!(matches!(
            route_script("test:unit"),
            Some(FilterRoute::TestRunner)
        ));
        assert!(matches!(
            route_script("test:integration"),
            Some(FilterRoute::TestRunner)
        ));
        // lint: prefix routes to Lint
        assert!(matches!(
            route_script("lint:check"),
            Some(FilterRoute::Lint)
        ));
        assert!(matches!(route_script("lint:ci"), Some(FilterRoute::Lint)));
    }

    #[test]
    fn test_route_script_prefix_exclusions() {
        // These should NOT route despite matching a prefix
        assert!(route_script("test:e2e").is_none());
        assert!(route_script("test:playwright").is_none());
        assert!(route_script("test:cypress").is_none());
        assert!(route_script("lint:fix").is_none());
    }

    // --- apply_filter integration tests ---

    #[test]
    fn test_apply_filter_vitest_produces_label_and_output() {
        let vitest_text = " Test Files  2 passed (2)\n\
                            Tests  13 passed (13)\n\
                            Duration  1.23s";
        let (filtered, label) =
            apply_filter(FilterRoute::Vitest, vitest_text, "@server", "vitest", 0);
        assert_eq!(label, "vitest (via yarn workspace)");
        assert!(
            !filtered.is_empty(),
            "Filter should produce non-empty output"
        );
    }

    #[test]
    fn test_apply_filter_tsc_produces_label_and_output() {
        let tsc_text = "src/api.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.\n\
                         src/api.ts(20,5): error TS2345: Argument of type 'number' is not assignable.";
        let (filtered, label) = apply_filter(FilterRoute::Tsc, tsc_text, "@server", "typecheck", 0);
        assert_eq!(label, "tsc (via yarn workspace)");
        assert!(
            !filtered.is_empty(),
            "Filter should produce non-empty output"
        );
        // Filter groups by file and adds headers -- verify it processed the errors
        assert!(
            filtered.contains("TS2322") || filtered.contains("TypeScript"),
            "Filter should contain structured tsc output"
        );
    }

    #[test]
    fn test_apply_filter_lint_produces_label_and_output() {
        let lint_text = "src/utils.ts:10: warning - Unexpected any.\n\
                          src/api.ts:20: error - Missing return type.";
        let (filtered, label) = apply_filter(FilterRoute::Lint, lint_text, "@server", "lint", 0);
        assert_eq!(label, "lint (via yarn workspace)");
        assert!(
            !filtered.is_empty(),
            "Filter should produce non-empty output"
        );
    }

    #[test]
    fn test_apply_filter_prettier_produces_label_and_output() {
        let prettier_text = "Checking formatting...\n\
                              src/utils.ts\n\
                              src/api.ts\n\
                              Code style issues found.";
        let (filtered, label) = apply_filter(
            FilterRoute::Prettier,
            prettier_text,
            "@server",
            "prettier",
            0,
        );
        assert_eq!(label, "prettier (via yarn workspace)");
        assert!(
            !filtered.is_empty(),
            "Filter should produce non-empty output"
        );
    }

    #[test]
    fn test_apply_filter_test_runner_produces_label_and_output() {
        let jest_text = "PASS src/auth.test.ts\n\
                          FAIL src/users.test.ts\n\
                          Test Suites: 1 failed, 1 passed, 2 total\n\
                          Tests:       1 failed, 5 passed, 6 total";
        let (filtered, label) =
            apply_filter(FilterRoute::TestRunner, jest_text, "@server", "test", 0);
        assert_eq!(label, "test (via yarn workspace)");
        assert!(
            !filtered.is_empty(),
            "Filter should produce non-empty output"
        );
    }

    #[test]
    fn test_ok_checkmark_guard_skips_routing() {
        // When filter_yarn_output returns "ok checkmark" (all boilerplate),
        // routing should be skipped
        let all_boilerplate = "\u{27a4} YN0000: Done\n\
                                Resolution step 1/1";
        let clean = filter_yarn_output(all_boilerplate);
        assert_eq!(clean, "ok \u{2713}");
        // In the run() flow, this would skip route_script entirely
    }

    #[test]
    fn test_filter_then_route_integration() {
        // Simulate full flow: yarn boilerplate + vitest text output
        let yarn_vitest_output = "\u{27a4} YN0000: \u{2502} @server/api (vitest)\n\
                                   Resolution step 1/1\n\
                                   \n\
                                   Test Files  1 passed (1)\n\
                                   Tests  5 passed (5)\n\
                                   Duration  450ms\n\
                                   \n\
                                   \u{27a4} YN0000: Done in 2s";
        // Step 1: strip boilerplate
        let clean = filter_yarn_output(yarn_vitest_output);
        assert!(!clean.contains("YN0000"));
        assert!(clean.contains("Test Files"));

        // Step 2: route
        let route = route_script("vitest");
        assert!(matches!(route, Some(FilterRoute::Vitest)));

        // Step 3: apply filter -- assert on label and non-empty output
        let (filtered, label) = apply_filter(route.unwrap(), &clean, "@server", "vitest", 0);
        assert_eq!(label, "vitest (via yarn workspace)");
        assert!(
            !filtered.is_empty(),
            "Vitest filter should produce non-empty output from cleaned input"
        );
    }

    // --- Early-exit on failure tests ---

    #[test]
    fn test_failed_command_should_not_route_to_filter() {
        // Simulates what happens when yarn fails with "script not found"
        // The error text passes through filter_yarn_output (it's not boilerplate)
        // and then would be routed to tsc_cmd which returns misleading success
        let yarn_error = "Usage Error: Couldn't find a script named \"typecheck\"";
        let clean = filter_yarn_output(yarn_error);
        // The error message survives boilerplate stripping (correct)
        assert!(
            clean.contains("Couldn't find a script"),
            "Error message should survive boilerplate filtering"
        );
        assert_ne!(
            clean, "ok \u{2713}",
            "Error message should NOT become ok checkmark"
        );
        // This is why early-exit is needed: without it, this error text
        // would be sent to tsc_cmd::filter_tsc_output which returns
        // "TypeScript compilation completed" -- masking the error
        let misled = tsc_cmd::filter_tsc_output(&clean);
        assert!(
            misled.contains("TypeScript compilation completed") || misled.contains("TypeScript"),
            "Without early-exit, tsc filter masks yarn errors with success message"
        );
    }

    #[test]
    fn test_successful_command_still_routes_through_filters() {
        // Simulate successful tsc output wrapped in yarn boilerplate
        let yarn_tsc_output = "\u{27a4} YN0000: \u{2502} Starting tsc\n\
                               src/api.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.\n\
                               src/api.ts(20,3): error TS2345: Argument of type 'number' is not assignable.\n\
                               Found 2 errors in 1 file.\n\
                               \u{27a4} YN0000: Done in 2s";
        // Step 1: strip boilerplate
        let clean = filter_yarn_output(yarn_tsc_output);
        assert!(!clean.contains("YN0000"), "Boilerplate should be stripped");
        assert!(
            clean.contains("TS2322"),
            "TSC errors should survive stripping"
        );

        // Step 2: route
        let route = route_script("typecheck");
        assert!(matches!(route, Some(FilterRoute::Tsc)));

        // Step 3: apply filter (would only happen if command succeeded)
        let (filtered, label) = apply_filter(route.unwrap(), &clean, "@server", "typecheck", 0);
        assert_eq!(label, "tsc (via yarn workspace)");
        assert!(
            filtered.contains("TS2322"),
            "TSC filter should preserve error codes"
        );
    }
}
