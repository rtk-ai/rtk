//! Filters npm output and auto-injects the "run" subcommand when appropriate.

use crate::core::runner;
use crate::core::shell::display_args;
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::io::IsTerminal;
use std::sync::LazyLock;

static TEST_SUMMARY: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:Test Suites:|Test Files\s+|\d+ passing \()").unwrap());
static PASSING_TEST: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:PASS\s+\S|[✓✔√]\s+\S)").unwrap());
static TEST_FAILURE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\s*(?:FAIL(?:\s|$)|[×✗❯●]\s|\d+\)\s|(?:AssertionError|Error):)").unwrap()
});

/// Known npm subcommands that should NOT get "run" injected.
/// Shared between production code and tests to avoid drift.
///
/// A tool with a list like this also belongs in `core::tracking::SUBCOMMAND_ROUTERS`,
/// which keeps the subcommand in its telemetry label.
const NPM_SUBCOMMANDS: &[&str] = &[
    "install",
    "i",
    "ci",
    "uninstall",
    "remove",
    "rm",
    "update",
    "up",
    "list",
    "ls",
    "outdated",
    "init",
    "create",
    "publish",
    "pack",
    "link",
    "audit",
    "fund",
    "exec",
    "explain",
    "why",
    "search",
    "view",
    "info",
    "show",
    "config",
    "set",
    "get",
    "cache",
    "prune",
    "dedupe",
    "doctor",
    "help",
    "version",
    "prefix",
    "root",
    "bin",
    "bugs",
    "docs",
    "home",
    "repo",
    "ping",
    "whoami",
    "token",
    "profile",
    "team",
    "access",
    "owner",
    "deprecate",
    "dist-tag",
    "star",
    "stars",
    "login",
    "logout",
    "adduser",
    "unpublish",
    "pkg",
    "diff",
    "rebuild",
    "test",
    "t",
    "tst",
    "start",
    "stop",
    "restart",
    "completion",
    "edit",
    "explore",
    "find-dupes",
    "help-search",
    "hook",
    "install-ci-test",
    "install-test",
    "ll",
    "org",
    "query",
    "run-script",
    "sbom",
    "shrinkwrap",
    "unstar",
];

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    // Determine if this is "npm run <script>" or another npm subcommand (install, list, etc.)
    // Only inject "run" when args look like a script name, not a known npm subcommand.
    let first_arg = args.first().map(|s| s.as_str());
    let is_run_explicit = first_arg == Some("run");
    let is_npm_subcommand = first_arg
        .map(|a| NPM_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
        .unwrap_or(false);

    let mut effective_args: Vec<String> = Vec::with_capacity(args.len() + 1);
    if is_run_explicit || is_npm_subcommand {
        effective_args.extend_from_slice(args);
    } else {
        // "rtk npm build" → "npm run build" (assume script name)
        effective_args.push("run".to_string());
        effective_args.extend_from_slice(args);
    }

    run_filtered("npm", &effective_args, verbose, skip_env, false)
}

/// Run an npx tool through the same filtered pipeline as `npm`.
///
/// Used for unrouted tools in the `Commands::Npx` fallback so that
/// `rtk npx cowsay hello` dispatches to `npx`, not `npm`. Honors `--skip-env`
/// the same way `run` does.
pub fn exec(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered("npx", args, verbose, skip_env, false)
}

/// Run an arbitrary package runner (`npx`, `bunx`) under the npm output filter.
///
/// This is the light line filter, not the errors-only one: these runners host
/// tools whose entire value is their stdout, so dropping non-error lines would
/// return nothing.
pub fn exec_with(runner: &str, args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    run_filtered(runner, args, verbose, skip_env, true)
}

/// Shared command-execution path for `run` (npm) and `exec` (npx).
///
/// Builds the resolved command, appends args, applies `SKIP_ENV_VALIDATION`,
/// emits the verbose log line, and routes through `runner::run_filtered` with
/// the npm output filter.
/// `forward_piped_stdin` sends the caller's stdin to the child when it is a
/// pipe rather than a terminal. The filtered path buffers output until exit, so
/// inheriting a terminal would let an interactive tool prompt invisibly and wait
/// forever; a pipe already holds its data and cannot do that.
///
/// `bunx` opts in, `npx` does not: npx's behaviour here predates this path and
/// is tracked on its own as rtk-ai/rtk#2431, which covers every filtered entry
/// point rather than this one.
fn run_filtered(
    name: &str,
    args: &[String],
    verbose: u8,
    skip_env: bool,
    forward_piped_stdin: bool,
) -> Result<i32> {
    let mut cmd = resolved_command(name);
    for arg in args {
        cmd.arg(arg);
    }

    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    let args_display = display_args(args);
    if verbose > 0 {
        eprintln!("Running: {} {}", name, args_display);
    }

    let mut opts = runner::RunOptions::default();
    if forward_piped_stdin && !std::io::stdin().is_terminal() {
        opts = opts.inherit_stdin();
    }

    // Extra arguments may request verbose output or a custom reporter. Keep
    // that output under the existing light filter instead of removing passes.
    let filter =
        if name == "npm" && args.len() == 1 && matches!(args[0].as_str(), "test" | "t" | "tst") {
            filter_npm_test_output
        } else {
            filter_npm_output
        };
    runner::run_filtered(cmd, name, &args_display, filter, opts)
}

/// Keep the actual npm lifecycle (including pretest/posttest) and all failure
/// details. A recognizable footer allows dropping only passing test rows;
/// arbitrary scripts and incomplete/crashed reporters retain their output.
fn filter_npm_test_output(output: &str) -> String {
    let clean = crate::core::utils::strip_ansi(output);
    if !TEST_SUMMARY.is_match(&clean) {
        return filter_npm_output(output);
    }
    let filtered = filter_npm_output(&clean);
    let mut in_failure = false;
    filtered
        .lines()
        .filter(|line| {
            // Assertion diffs and captured logs may themselves look like a
            // passing test. Preserve everything from the first failure on.
            in_failure |= TEST_FAILURE.is_match(line);
            in_failure || !PASSING_TEST.is_match(line)
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Filter npm run output - strip boilerplate, progress bars, npm WARN
fn filter_npm_output(output: &str) -> String {
    let mut result = Vec::new();

    for line in output.lines() {
        // Skip npm boilerplate
        if line.starts_with('>') && line.contains('@') {
            continue;
        }
        // Skip npm lifecycle scripts
        if line.trim_start().starts_with("npm WARN") {
            continue;
        }
        if line.trim_start().starts_with("npm notice") {
            continue;
        }
        // Skip progress indicators
        if line.contains("⸩") || line.contains("⸨") || line.contains("...") && line.len() < 10 {
            continue;
        }
        // Skip empty lines
        if line.trim().is_empty() {
            continue;
        }

        result.push(line.to_string());
    }

    if result.is_empty() {
        "ok".to_string()
    } else {
        result.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_npm_output() {
        let output = r#"
> project@1.0.0 build
> next build

npm WARN deprecated inflight@1.0.6: This module is not supported
npm notice

   Creating an optimized production build...
   ✓ Build completed
"#;
        let result = filter_npm_output(output);
        assert!(!result.contains("npm WARN"));
        assert!(!result.contains("npm notice"));
        assert!(!result.contains("> project@"));
        assert!(result.contains("Build completed"));
    }

    #[test]
    fn test_npm_subcommand_routing() {
        // Uses the shared NPM_SUBCOMMANDS constant — no drift between prod and test
        fn needs_run_injection(args: &[&str]) -> bool {
            let first = args.first().copied();
            let is_run_explicit = first == Some("run");
            let is_subcommand = first
                .map(|a| NPM_SUBCOMMANDS.contains(&a) || a.starts_with('-'))
                .unwrap_or(false);
            !is_run_explicit && !is_subcommand
        }

        // Known subcommands should NOT get "run" injected
        for subcmd in NPM_SUBCOMMANDS {
            assert!(
                !needs_run_injection(&[subcmd]),
                "'npm {}' should NOT inject 'run'",
                subcmd
            );
        }

        // Script names SHOULD get "run" injected
        for script in &["build", "dev", "lint", "typecheck", "deploy"] {
            assert!(
                needs_run_injection(&[script]),
                "'npm {}' SHOULD inject 'run'",
                script
            );
        }

        // Flags should NOT get "run" injected
        assert!(!needs_run_injection(&["--version"]));
        assert!(!needs_run_injection(&["-h"]));

        // Explicit "run" should NOT inject another "run"
        assert!(!needs_run_injection(&["run", "build"]));
    }

    #[test]
    fn test_extended_official_subcommands_do_not_inject_run() {
        let subcommands = [
            "completion",
            "edit",
            "explore",
            "find-dupes",
            "help-search",
            "hook",
            "install-ci-test",
            "install-test",
            "ll",
            "org",
            "query",
            "run-script",
            "sbom",
            "shrinkwrap",
            "unstar",
            "test",
            "t",
            "tst",
        ];

        for subcommand in subcommands {
            assert!(
                NPM_SUBCOMMANDS.contains(&subcommand),
                "npm {subcommand} must be routed as a native subcommand"
            );
        }
    }

    #[test]
    fn test_filter_npm_output_empty() {
        let output = "\n\n\n";
        let result = filter_npm_output(output);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_npm_test_preserves_failures_and_summaries() {
        for (passed, summary) in [
            (
                "PASS tests/pass.test.js",
                "Test Suites: 1 failed, 1 passed, 2 total",
            ),
            (
                " ✓ tests/pass.test.js (2 tests) 3ms",
                " Test Files  1 failed | 1 passed (2)",
            ),
            ("    ✔ passes (2ms)", "  1 passing (12ms)"),
        ] {
            let raw = format!(
                "> app@1.0.0 test\n{passed}\nFAIL tests/fail.test.js\n  expected 1 to equal 2\n  at test.js:3:4\n{summary}\nposttest cleanup\n"
            );
            let filtered = filter_npm_test_output(&raw);
            assert!(!filtered.contains(passed));
            assert!(!filtered.contains("app@1.0.0"));
            for evidence in [
                "FAIL tests/fail.test.js",
                "expected 1 to equal 2",
                "at test.js:3:4",
                summary,
                "posttest cleanup",
            ] {
                assert!(filtered.contains(evidence), "lost {evidence}: {filtered}");
            }
        }
    }

    #[test]
    fn test_npm_test_keeps_unknown_and_incomplete_reporters() {
        for raw in [
            "PASS is application output\nError: test runner crashed",
            "TAP version 13\nok 1 - passes\nnot ok 2 - fails\n1..2",
            r#"{"numPassedTests":1,"numFailedTests":0}"#,
        ] {
            assert_eq!(filter_npm_test_output(raw), raw);
        }
    }

    #[test]
    fn test_npm_test_keeps_pass_markers_inside_failure_details() {
        for marker in [
            "FAIL tests/fail.test.js",
            " ❯ tests/fail.test.js (1 test | 1 failed)",
            "  1) fails with captured output:",
        ] {
            let raw = format!(
                "PASS tests/pass.test.js\n{marker}\n  ✔ expected log line\nPASS actual log line\nTest Suites: 1 failed, 1 passed, 2 total"
            );
            let filtered = filter_npm_test_output(&raw);
            assert!(!filtered.contains("PASS tests/pass.test.js"));
            assert!(filtered.contains("✔ expected log line"));
            assert!(filtered.contains("PASS actual log line"));
        }
    }

    #[test]
    fn test_npm_test_real_mocha_lifecycle_and_savings() {
        // Captured from npm test with Mocha's default reporter and lifecycle
        // scripts; only the machine-specific executable path was normalized.
        let raw = include_str!("../../../tests/fixtures/npm_test_mocha.txt");
        let filtered = filter_npm_test_output(raw);
        assert!(filtered.contains("8 passing"));
        assert!(filtered.contains("pretest fixture preparation"));
        assert!(filtered.contains("posttest fixture cleanup"));
        assert!(!filtered.contains("adds 1 twice"));
        assert!(filtered.len() * 5 <= raw.len() * 4);
    }
}
