//! Filters buck2 build output — progress lines counted/stripped, errors kept.
#![allow(dead_code)]

use super::diag;
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

/// Parsed statistics from a buck2 build run.
struct Buck2Stats {
    build_id: Option<String>,
    jobs_completed: usize,
    time_elapsed: String,
    cache_hits: String,
    commands_count: usize,
    errors: Vec<String>,
    warnings: Vec<String>,
    kept_lines: Vec<String>,
    action_lines: usize,
    action_details: Vec<String>,
    has_failed: bool,
}

impl Buck2Stats {
    fn new() -> Self {
        Self {
            build_id: None,
            jobs_completed: 0,
            time_elapsed: String::new(),
            cache_hits: String::new(),
            commands_count: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
            kept_lines: Vec::new(),
            action_lines: 0,
            action_details: Vec::new(),
            has_failed: false,
        }
    }
}

// ── Line classification ──

/// Extract "Build ID: <uuid>" from a trimmed line.
fn extract_build_id(trimmed: &str) -> Option<&str> {
    if let Some(rest) = trimmed.strip_prefix("Build ID: ") {
        if !rest.is_empty() {
            return Some(rest);
        }
    }
    None
}

/// Parse "Jobs completed: N. Time elapsed: Xs. Cache hits: Y%. Commands: Z"
fn parse_jobs_line(trimmed: &str) -> Option<(usize, &str, &str, usize)> {
    let rest = trimmed.strip_prefix("Jobs completed: ")?;
    let (jobs_str, rest) = rest.split_once(". Time elapsed: ")?;
    let jobs = jobs_str.parse::<usize>().ok()?;
    let (time_str, rest) = rest.split_once(". Cache hits: ")?;
    let (cache_str, rest) = rest.split_once(". Commands: ")?;
    let cmds = rest.trim_end_matches('.').parse::<usize>().ok()?;
    Some((jobs, time_str, cache_str, cmds))
}

/// "BUILD SUCCEEDED" → Some(true), "BUILD FAILED" → Some(false)
fn is_verdict(trimmed: &str) -> Option<bool> {
    if trimmed == "BUILD SUCCEEDED" {
        Some(true)
    } else if trimmed == "BUILD FAILED" {
        Some(false)
    } else {
        None
    }
}

/// "Running action: <digest>"
fn is_running_action(trimmed: &str) -> bool {
    trimmed.starts_with("Running action: ")
}

/// "Error: " prefix or starts with "error" and contains '"'
fn is_buck2_error(trimmed: &str) -> bool {
    trimmed.starts_with("Error: ")
}

/// "Warning: " prefix
fn is_buck2_warning(trimmed: &str) -> bool {
    trimmed.starts_with("Warning: ")
}

// ── Filter ──

fn filter_buck2_output(input: &str, exit_code: i32, verbose: u8) -> String {
    let mut stats = Buck2Stats::new();

    for line in input.lines() {
        let normalized = diag::normalize(line);
        let trimmed = normalized.trim();

        // Blank → skip
        if trimmed.is_empty() {
            continue;
        }

        // Build ID → capture
        if let Some(id) = extract_build_id(trimmed) {
            stats.build_id = Some(id.to_string());
            continue;
        }

        // Jobs completed → parse
        if let Some((jobs, time, cache, cmds)) = parse_jobs_line(trimmed) {
            stats.jobs_completed = jobs;
            stats.time_elapsed = time.to_string();
            stats.cache_hits = cache.to_string();
            stats.commands_count = cmds;
            continue;
        }

        // Running action → count (show only in verbose)
        if is_running_action(trimmed) {
            stats.action_lines += 1;
            if verbose > 0 {
                stats.action_details.push(normalized);
            }
            continue;
        }

        // Verdict
        if let Some(success) = is_verdict(trimmed) {
            if !success {
                stats.has_failed = true;
            }
            continue;
        }

        // Buck2 errors / warnings
        if is_buck2_error(trimmed) {
            stats.errors.push(normalized);
            stats.has_failed = true;
            continue;
        }

        if is_buck2_warning(trimmed) {
            stats.warnings.push(normalized);
            continue;
        }

        // Compiler diagnostics → errors
        if diag::is_compiler_diag(trimmed) {
            if trimmed.to_lowercase().contains("error") {
                stats.has_failed = true;
            }
            stats.errors.push(normalized);
            continue;
        }

        // Linker errors
        if diag::is_linker_error(trimmed) {
            stats.errors.push(normalized);
            stats.has_failed = true;
            continue;
        }

        // Everything else → keep (fail-open)
        stats.kept_lines.push(normalized);
    }

    // Exit code override
    if exit_code != 0 && !stats.has_failed {
        stats.has_failed = true;
    }

    compose_output(&stats)
}

fn compose_output(stats: &Buck2Stats) -> String {
    if stats.has_failed {
        let mut out = String::from("buck2: BUILD FAILED\n");
        for err in &stats.errors {
            out.push_str(&format!("  {}\n", err));
        }
        if !stats.warnings.is_empty() {
            for w in &stats.warnings {
                out.push_str(&format!("  {}\n", w));
            }
        }
        if !stats.kept_lines.is_empty() {
            for line in &stats.kept_lines {
                out.push_str(&format!("  {}\n", line));
            }
        }
        out
    } else {
        let mut out = format!(
            "ok buck2: {} jobs, {}, {} cache, {} commands",
            stats.jobs_completed, stats.time_elapsed, stats.cache_hits, stats.commands_count
        );
        if let Some(ref id) = stats.build_id {
            out.push_str(&format!("\n  Build ID: {}", id));
        }
        if stats.action_lines > 0 {
            out.push_str(&format!("\n  {} actions", stats.action_lines));
        }
        for detail in &stats.action_details {
            out.push_str(&format!("\n  {}", detail));
        }
        if !stats.warnings.is_empty() {
            for w in &stats.warnings {
                out.push_str(&format!("\n  {}", w));
            }
        }
        if !stats.kept_lines.is_empty() {
            for line in &stats.kept_lines {
                out.push_str(&format!("\n  {}", line));
            }
        }
        out.push('\n');
        out
    }
}

// ── Public API ──

/// Run buck2 with output filtering.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("buck2: running buck2 {}", args.join(" "));
    }

    let mut cmd = resolved_command("buck2");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_filtered_with_exit(
        cmd,
        "buck2",
        &args_str,
        move |input, exit_code| filter_buck2_output(input, exit_code, verbose),
        runner::RunOptions::with_tee("buck2"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    fn filter_buck2(input: &str, exit_code: i32) -> String {
        filter_buck2_output(input, exit_code, 0)
    }

    fn filter_buck2_verbose(input: &str, exit_code: i32) -> String {
        filter_buck2_output(input, exit_code, 1)
    }

    // ── Helper tests ──

    #[test]
    fn test_extract_build_id() {
        assert_eq!(
            extract_build_id("Build ID: 550bdd5a-52a6-424c-a082-50522ce800cc"),
            Some("550bdd5a-52a6-424c-a082-50522ce800cc")
        );
        assert_eq!(extract_build_id("Build ID: "), None);
        assert_eq!(extract_build_id("other"), None);
    }

    #[test]
    fn test_parse_jobs_line() {
        let result = parse_jobs_line(
            "Jobs completed: 42. Time elapsed: 12.3s. Cache hits: 85%. Commands: 156.",
        );
        assert!(result.is_some());
        let (jobs, time, cache, cmds) = result.unwrap();
        assert_eq!(jobs, 42);
        assert_eq!(time, "12.3s");
        assert_eq!(cache, "85%");
        assert_eq!(cmds, 156);
    }

    #[test]
    fn test_is_verdict() {
        assert_eq!(is_verdict("BUILD SUCCEEDED"), Some(true));
        assert_eq!(is_verdict("BUILD FAILED"), Some(false));
        assert_eq!(is_verdict("other"), None);
    }

    // ── Success case ──

    #[test]
    fn test_buck2_success() {
        let input = "\
Build ID: 550bdd5a-52a6-424c-a082-50522ce800cc
Jobs completed: 42. Time elapsed: 12.3s. Cache hits: 85%. Commands: 156.
Running action: abc123def456
Running action: fed789cba321
BUILD SUCCEEDED
";
        let result = filter_buck2(input, 0);
        assert!(result.contains("ok buck2:"), "got: {}", result);
        assert!(result.contains("42 jobs"), "got: {}", result);
        assert!(result.contains("12.3s"), "got: {}", result);
        assert!(result.contains("85%"), "got: {}", result);
        assert!(result.contains("156 commands"), "got: {}", result);
        assert!(result.contains("Build ID:"), "got: {}", result);
        // Running actions should not appear in default output
        assert!(!result.contains("Running action"), "got: {}", result);
    }

    #[test]
    fn test_buck2_success_verbose() {
        let input = "\
Build ID: 550bdd5a-52a6-424c-a082-50522ce800cc
Jobs completed: 3. Time elapsed: 1.5s. Cache hits: 100%. Commands: 10.
Running action: abc123
Running action: def456
BUILD SUCCEEDED
";
        let result = filter_buck2_verbose(input, 0);
        assert!(result.contains("ok buck2:"), "got: {}", result);
        // In verbose mode (verbose=1), running actions should appear
        assert!(result.contains("Running action"), "got: {}", result);
        assert!(result.contains("abc123"), "got: {}", result);
    }

    // ── Failure case ──

    #[test]
    fn test_buck2_failure() {
        let input = "\
Build ID: 550bdd5a-52a6-424c-a082-50522ce800cc
Running action: abc123
Error: failed to compile 'foo.cpp'
BUILD FAILED
";
        let result = filter_buck2(input, 1);
        assert!(result.contains("buck2: BUILD FAILED"), "got: {}", result);
        assert!(result.contains("failed to compile"), "got: {}", result);
    }

    // ── Empty input ──

    #[test]
    fn test_buck2_empty_input() {
        let result = filter_buck2("", 0);
        assert!(result.contains("ok buck2:"), "got: {}", result);
        assert!(result.contains("0 jobs"), "got: {}", result);
    }

    // ── Token savings ──

    #[test]
    fn test_buck2_token_savings() {
        let mut input = String::new();
        input.push_str("Build ID: 550bdd5a-52a6-424c-a082-50522ce800cc\n");
        for i in 1..=100 {
            input.push_str(&format!("Running action: action_{:08x}_{}\n", i, i));
        }
        input.push_str(
            "Jobs completed: 100. Time elapsed: 45.2s. Cache hits: 80%. Commands: 500.\n",
        );
        input.push_str("BUILD SUCCEEDED\n");

        let result = filter_buck2(&input, 0);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        // Default output is terse, but savings may be modest since the input
        // already has a small summary. Still expect some savings from 100 action lines.
        assert!(savings > 0, "token savings: {}% (expected > 0%)", savings);
    }
}
