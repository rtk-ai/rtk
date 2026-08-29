//! Filters build2 output — action lines counted/stripped, errors/warnings kept.
#![allow(dead_code)]

use super::diag;
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

/// Parsed statistics from a build2 run.
struct Build2Stats {
    edges: usize,
    errors: Vec<String>,
    warnings: Vec<String>,
    kept_lines: Vec<String>,
    update_stats: Vec<String>,
    has_failed: bool,
}

impl Build2Stats {
    fn new() -> Self {
        Self {
            edges: 0,
            errors: Vec::new(),
            warnings: Vec::new(),
            kept_lines: Vec::new(),
            update_stats: Vec::new(),
            has_failed: false,
        }
    }
}

/// Action tools that build2 shows: c++, cc, ld, ar, as, install, cp, ln, rm, mkdir, etc.
const ACTION_TOOLS: &[&str] = &[
    "c++", "cc", "ld", "ar", "as", "install", "cp", "ln", "rm", "mkdir", "rc", "windres", "mt",
    "ranlib", "strip", "objcopy", "objdump",
];

// ── Line classification ──

/// Starts with an action tool name followed by space and contains '@' (build2 target notation).
pub fn is_action_line(trimmed: &str) -> bool {
    for tool in ACTION_TOOLS {
        if trimmed.starts_with(tool) && trimmed.len() > tool.len() {
            let after = &trimmed[tool.len()..];
            if after.starts_with(' ') && after.contains('@') {
                return true;
            }
        }
    }
    false
}

/// "error: " or "warning: " prefix → returns the severity ("error" or "warning").
fn is_build2_diag(trimmed: &str) -> Option<&str> {
    if trimmed.starts_with("error: ") {
        Some("error")
    } else if trimmed.starts_with("warning: ") {
        Some("warning")
    } else {
        None
    }
}

/// "updated N/M targets" or "cleaned N/M targets"
fn is_update_stat(trimmed: &str) -> bool {
    trimmed.starts_with("updated ") || trimmed.starts_with("cleaned ")
}

// ── Filter ──

fn filter_build2_output(input: &str, exit_code: i32) -> String {
    let mut stats = Build2Stats::new();

    for line in input.lines() {
        let normalized = diag::normalize(line);
        let trimmed = normalized.trim();

        // Blank → skip
        if trimmed.is_empty() {
            continue;
        }

        // Action lines → count + drop
        if is_action_line(trimmed) {
            stats.edges += 1;
            continue;
        }

        // Update stats → collect + drop
        if is_update_stat(trimmed) {
            stats.update_stats.push(normalized);
            continue;
        }

        // Build2 error/warning diags → errors/warnings
        if let Some(severity) = is_build2_diag(trimmed) {
            if severity == "error" {
                stats.errors.push(normalized);
                stats.has_failed = true;
            } else {
                stats.warnings.push(normalized);
            }
            continue;
        }

        // Compiler diagnostics → errors
        if diag::is_compiler_diag(trimmed) {
            if trimmed.to_lowercase().contains("error") {
                stats.errors.push(normalized);
                stats.has_failed = true;
            } else if trimmed.to_lowercase().contains("warning") {
                stats.warnings.push(normalized);
            } else {
                stats.errors.push(normalized);
            }
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

fn compose_output(stats: &Build2Stats) -> String {
    if stats.has_failed {
        let mut out = String::from("build2: build failed\n");
        for err in &stats.errors {
            out.push_str(&format!("  {}\n", err));
        }
        if !stats.warnings.is_empty() {
            for warn in &stats.warnings {
                out.push_str(&format!("  {}\n", warn));
            }
        }
        if !stats.kept_lines.is_empty() {
            for line in &stats.kept_lines {
                out.push_str(&format!("  {}\n", line));
            }
        }
        out
    } else {
        let mut out = format!("ok build2: {} actions\n", stats.edges);
        for stat in &stats.update_stats {
            out.push_str(&format!("  {}\n", stat));
        }
        if !stats.warnings.is_empty() {
            for warn in &stats.warnings {
                out.push_str(&format!("  {}\n", warn));
            }
        }
        if !stats.kept_lines.is_empty() {
            for line in &stats.kept_lines {
                out.push_str(&format!("  {}\n", line));
            }
        }
        out
    }
}

// ── Public API ──

/// Run build2 with output filtering.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("build2: running b {}", args.join(" "));
    }

    let mut cmd = resolved_command("b");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_filtered_with_exit(
        cmd,
        "build2",
        &args_str,
        filter_build2_output,
        runner::RunOptions::with_tee("build2"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    fn filter_build2(input: &str, exit_code: i32) -> String {
        filter_build2_output(input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_action_line_cxx() {
        assert!(is_action_line(
            "c++ hello-0.16.0/libhello-0.16.0.so@/tmp/b/stage/lib/"
        ));
        assert!(is_action_line("c++ libhello-0.16.0.a@/tmp/b/stage/lib/"));
    }

    #[test]
    fn test_is_action_line_ld() {
        assert!(is_action_line(
            "ld hello-0.16.0/exe/hello@/tmp/b/stage/bin/"
        ));
    }

    #[test]
    fn test_is_action_line_ar() {
        assert!(is_action_line("ar libhello-0.16.0.a@/tmp/b/stage/lib/"));
    }

    #[test]
    fn test_is_action_line_not() {
        assert!(!is_action_line("error: something went wrong"));
        assert!(!is_action_line("updated 5/10 targets"));
        assert!(!is_action_line(""));
        assert!(!is_action_line("custom@tool"));
    }

    #[test]
    fn test_is_build2_diag() {
        assert_eq!(is_build2_diag("error: something went wrong"), Some("error"));
        assert_eq!(
            is_build2_diag("warning: deprecated feature"),
            Some("warning")
        );
        assert_eq!(is_build2_diag("not a diag"), None);
    }

    // ── Success case ──

    #[test]
    fn test_build2_success() {
        let input = "\
c++ hello-0.16.0/libhello-0.16.0.so@/tmp/b/stage/lib/
c++ libhello-0.16.0.a@/tmp/b/stage/lib/
ld hello-0.16.0/exe/hello@/tmp/b/stage/bin/
updated 3/3 targets
";
        let result = filter_build2(input, 0);
        assert!(result.contains("ok build2:"), "got: {}", result);
        assert!(result.contains("3 actions"), "got: {}", result);
        assert!(result.contains("updated 3/3 targets"), "got: {}", result);
        // Action lines should be stripped
        assert!(!result.contains("libhello-0.16.0.so"), "got: {}", result);
    }

    // ── Error case ──

    #[test]
    fn test_build2_error() {
        let input = "\
c++ hello-0.16.0/libhello-0.16.0.so@/tmp/b/stage/lib/
c++ hello-0.16.0/exe/hello/hello.cxx@/tmp/b/stage/
error: 'foo' was not declared in this scope
  in file hello.cxx line 42
";
        let result = filter_build2(input, 1);
        assert!(result.contains("build2: build failed"), "got: {}", result);
        assert!(result.contains("'foo' was not declared"), "got: {}", result);
    }

    // ── Warning case ──

    #[test]
    fn test_build2_warning() {
        let input = "\
c++ hello-0.16.0/libhello-0.16.0.so@/tmp/b/stage/lib/
warning: unused variable 'x'
updated 1/1 targets
";
        let result = filter_build2(input, 0);
        assert!(result.contains("ok build2:"), "got: {}", result);
        assert!(
            result.contains("warning: unused variable"),
            "got: {}",
            result
        );
    }

    // ── Verbose ──

    #[test]
    fn test_build2_verbose_many_actions() {
        let mut input = String::new();
        for i in 1..=50 {
            input.push_str(&format!("c++ project-1.0/file_{}.o@/build/stage/\n", i));
        }
        input.push_str("updated 50/50 targets\n");

        let result = filter_build2(&input, 0);
        assert!(result.contains("50 actions"), "got: {}", result);
        // Action lines should all be stripped
        assert!(!result.contains("project-1.0/file_"), "got: {}", result);
    }

    // ── Empty input ──

    #[test]
    fn test_build2_empty_input() {
        let result = filter_build2("", 0);
        assert!(result.contains("ok build2:"), "got: {}", result);
        assert!(result.contains("0 actions"), "got: {}", result);
    }

    // ── Token savings ──

    #[test]
    fn test_build2_token_savings() {
        let mut input = String::new();
        for i in 1..=200 {
            input.push_str(&format!("c++ project-1.0/file_{:04}.o@/build/stage/\n", i));
        }
        input.push_str("updated 200/200 targets\n");

        let result = filter_build2(&input, 0);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 80,
            "token savings: {}% (expected >=80%)",
            savings
        );
    }
}
