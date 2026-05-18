//! Filters `node` invocations:
//!   - `node --test [files…]`     compact TAP summary (failures shown in full)
//!   - `node --check <file>`      silent on success, full error on syntax fail
//!   - everything else            passthrough
//!
//! Designed for the agent-driven workflow where `node --check FILE` is used
//! as a syntax sanity check and `node --test` runs the built-in test runner.

use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    let mode = classify(args);
    match mode {
        NodeMode::Test => run_test(args, verbose, skip_env),
        NodeMode::Check => run_check(args, verbose, skip_env),
        NodeMode::Other => run_passthrough(args, verbose, skip_env),
    }
}

#[derive(Debug, PartialEq, Eq)]
enum NodeMode {
    Test,
    Check,
    Other,
}

/// Looks at the leading flags to decide which filter to apply. Flags are
/// position-sensitive in `node` (they must come before the script path), so
/// scanning the early args is sufficient.
fn classify(args: &[String]) -> NodeMode {
    for arg in args {
        if arg == "--" {
            break;
        }
        // Stop once we hit something that looks like a script path.
        if !arg.starts_with('-') {
            break;
        }
        if arg == "--test" || arg.starts_with("--test=") {
            return NodeMode::Test;
        }
        if arg == "--check" || arg == "-c" {
            return NodeMode::Check;
        }
    }
    NodeMode::Other
}

fn run_test(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    // Force the TAP reporter for predictable parsing. Drop any user-provided
    // reporter flag so our injection wins — the user can still rely on the
    // exit code for pass/fail signalling.
    let mut effective: Vec<String> = Vec::with_capacity(args.len() + 1);
    let mut injected_reporter = false;
    for arg in args {
        if arg.starts_with("--test-reporter") {
            continue;
        }
        effective.push(arg.clone());
        if !injected_reporter && (arg == "--test" || arg.starts_with("--test=")) {
            effective.push("--test-reporter=tap".to_string());
            injected_reporter = true;
        }
    }
    if !injected_reporter {
        effective.insert(0, "--test-reporter=tap".to_string());
    }

    invoke(
        "node",
        &effective,
        verbose,
        skip_env,
        filter_node_test_output,
    )
}

fn run_check(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    invoke("node", args, verbose, skip_env, filter_node_check_output)
}

fn run_passthrough(args: &[String], verbose: u8, skip_env: bool) -> Result<i32> {
    invoke("node", args, verbose, skip_env, passthrough)
}

fn passthrough(output: &str) -> String {
    output.to_string()
}

fn invoke(
    name: &str,
    args: &[String],
    verbose: u8,
    skip_env: bool,
    filter: fn(&str) -> String,
) -> Result<i32> {
    let mut cmd = resolved_command(name);
    for arg in args {
        cmd.arg(arg);
    }
    if skip_env {
        cmd.env("SKIP_ENV_VALIDATION", "1");
    }

    let args_display = args.join(" ");
    if verbose > 0 {
        eprintln!("Running: {} {}", name, args_display);
    }

    runner::run_filtered(
        cmd,
        name,
        &args_display,
        filter,
        runner::RunOptions::default(),
    )
}

/// `node --check FILE`:
///   - On success the binary prints nothing → we emit "ok".
///   - On syntax error the binary prints the offending source line + a
///     `SyntaxError: …` stack. We keep it as-is (already compact, and the
///     full content is what the agent needs to fix the error).
fn filter_node_check_output(output: &str) -> String {
    let trimmed = output.trim();
    if trimmed.is_empty() {
        "ok".to_string()
    } else {
        trimmed.to_string()
    }
}

/// `node --test --test-reporter=tap`:
/// Parse the TAP 13 stream into a compact summary. Failures are surfaced in
/// full (test name + diagnostic block) since that's the actionable signal.
fn filter_node_test_output(output: &str) -> String {
    let mut total = 0usize;
    let mut passed = 0usize;
    let mut failed = 0usize;
    let mut skipped = 0usize;
    let mut duration_ms: Option<u64> = None;
    let mut failures: Vec<String> = Vec::new();

    let mut current_failure: Option<Vec<String>> = None;
    let mut saw_summary = false;

    for line in output.lines() {
        let trimmed = line.trim_start();

        // Summary lines come at the end of TAP output.
        if let Some(rest) = trimmed.strip_prefix("# tests ") {
            total = rest.trim().parse().unwrap_or(0);
            saw_summary = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# pass ") {
            passed = rest.trim().parse().unwrap_or(0);
            saw_summary = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# fail ") {
            failed = rest.trim().parse().unwrap_or(0);
            saw_summary = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# skipped ") {
            skipped = rest.trim().parse().unwrap_or(0);
            saw_summary = true;
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("# duration_ms ") {
            if let Ok(v) = rest.trim().parse::<f64>() {
                duration_ms = Some(v as u64);
            }
            saw_summary = true;
            continue;
        }

        // Start of a failing test.
        if trimmed.starts_with("not ok ") {
            if let Some(prev) = current_failure.take() {
                failures.push(prev.join("\n"));
            }
            current_failure = Some(vec![trimmed.to_string()]);
            continue;
        }

        // Start of a passing test — flush any pending failure block.
        if trimmed.starts_with("ok ") {
            if let Some(prev) = current_failure.take() {
                failures.push(prev.join("\n"));
            }
            continue;
        }

        // If we're collecting a failure's YAML diagnostic block, keep the
        // indented lines verbatim.
        if let Some(ref mut acc) = current_failure {
            if line.starts_with("  ") || line.starts_with('\t') {
                acc.push(line.to_string());
            }
        }
    }
    if let Some(prev) = current_failure.take() {
        failures.push(prev.join("\n"));
    }

    if !saw_summary {
        // Output didn't look like TAP at all — pass through.
        return output.to_string();
    }

    let duration_str = duration_ms
        .map(|d| format!(" in {}ms", d))
        .unwrap_or_default();
    let skipped_str = if skipped > 0 {
        format!(" ({} skipped)", skipped)
    } else {
        String::new()
    };

    if failed > 0 {
        let mut out = vec![format!(
            "node --test: {}/{} passed, {} failed{}{}",
            passed, total, failed, skipped_str, duration_str
        )];
        for failure in &failures {
            out.push(String::new());
            out.push(failure.clone());
        }
        out.join("\n")
    } else if total > 0 {
        format!(
            "node --test: {} passed{}{}",
            passed, skipped_str, duration_str
        )
    } else {
        "node --test: no tests found".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_classify_test_mode() {
        assert_eq!(classify(&args(&["--test"])), NodeMode::Test);
        assert_eq!(classify(&args(&["--test", "tests/foo.js"])), NodeMode::Test);
        assert_eq!(classify(&args(&["--test=concurrency=1"])), NodeMode::Test);
    }

    #[test]
    fn test_classify_check_mode() {
        assert_eq!(classify(&args(&["--check", "file.js"])), NodeMode::Check);
        assert_eq!(classify(&args(&["-c", "file.js"])), NodeMode::Check);
    }

    #[test]
    fn test_classify_other_mode() {
        assert_eq!(classify(&args(&["script.js"])), NodeMode::Other);
        assert_eq!(classify(&args(&["-e", "console.log(1)"])), NodeMode::Other);
        assert_eq!(classify(&args(&[])), NodeMode::Other);
    }

    #[test]
    fn test_classify_stops_at_script_path() {
        // Once a positional script path appears, subsequent --test is the
        // script's own argv, not node's.
        assert_eq!(classify(&args(&["script.js", "--test"])), NodeMode::Other);
    }

    #[test]
    fn test_check_silent_on_success() {
        assert_eq!(filter_node_check_output(""), "ok");
        assert_eq!(filter_node_check_output("\n\n\n"), "ok");
        assert_eq!(filter_node_check_output("   \t  "), "ok");
    }

    #[test]
    fn test_check_preserves_syntax_errors() {
        let stderr = "/tmp/foo.js:5\n  if (x { }\n       ^\n\nSyntaxError: Unexpected token '{'";
        let result = filter_node_check_output(stderr);
        assert!(result.contains("SyntaxError"));
        assert!(result.contains("foo.js"));
    }

    #[test]
    fn test_filter_test_all_passing() {
        let tap = "TAP version 13\n\
                   ok 1 - test one\n\
                     ---\n\
                     duration_ms: 1.5\n\
                     ...\n\
                   ok 2 - test two\n\
                   1..2\n\
                   # tests 2\n\
                   # pass 2\n\
                   # fail 0\n\
                   # duration_ms 100.0\n";
        let result = filter_node_test_output(tap);
        assert!(result.contains("2 passed"), "got: {}", result);
        assert!(!result.contains("failed"), "got: {}", result);
        assert!(result.contains("100ms"), "got: {}", result);
    }

    #[test]
    fn test_filter_test_with_failure() {
        let tap = "TAP version 13\n\
                   not ok 1 - failing test\n\
                     ---\n\
                     error: 'expected 1 to equal 2'\n\
                     stack: |-\n\
                       at TestContext.<anonymous>\n\
                     ...\n\
                   ok 2 - passing test\n\
                   1..2\n\
                   # tests 2\n\
                   # pass 1\n\
                   # fail 1\n\
                   # duration_ms 50.0\n";
        let result = filter_node_test_output(tap);
        assert!(result.contains("1/2 passed"), "got: {}", result);
        assert!(result.contains("1 failed"), "got: {}", result);
        assert!(result.contains("failing test"), "got: {}", result);
        assert!(result.contains("expected 1 to equal 2"), "got: {}", result);
    }

    #[test]
    fn test_filter_test_skipped() {
        let tap = "TAP version 13\n\
                   ok 1 - test one\n\
                   1..1\n\
                   # tests 2\n\
                   # pass 1\n\
                   # fail 0\n\
                   # skipped 1\n\
                   # duration_ms 5.0\n";
        let result = filter_node_test_output(tap);
        assert!(result.contains("1 skipped"), "got: {}", result);
    }

    #[test]
    fn test_filter_test_no_summary_passes_through() {
        // If the output doesn't include the TAP summary block, we can't
        // safely compact it — return it verbatim.
        let raw = "totally unparseable output\nwith multiple lines\n";
        let result = filter_node_test_output(raw);
        assert_eq!(result, raw);
    }

    #[test]
    fn test_filter_test_zero_tests() {
        let tap = "TAP version 13\n\
                   1..0\n\
                   # tests 0\n\
                   # pass 0\n\
                   # fail 0\n";
        let result = filter_node_test_output(tap);
        assert!(result.contains("no tests found"), "got: {}", result);
    }
}
