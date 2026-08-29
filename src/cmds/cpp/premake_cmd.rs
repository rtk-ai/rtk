//! Filters premake5 output — probe lines stripped, errors and summary kept.
#![allow(dead_code)]

use super::diag;
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

/// Parsed statistics from a premake5 run.
struct PremakeStats {
    action: Option<String>,
    files_generated: usize,
    elapsed_ms: Option<u64>,
    errors: Vec<String>,
    unrecognized: Vec<String>,
    backend: Option<String>,
}

impl PremakeStats {
    fn new() -> Self {
        Self {
            action: None,
            files_generated: 0,
            elapsed_ms: None,
            errors: Vec::new(),
            unrecognized: Vec::new(),
            backend: None,
        }
    }
}

// ── Backend detection ──

fn detect_backend(action: &str) -> &str {
    match action {
        "gmake" | "gmake2" => "make",
        "vs2022" | "vs2019" | "vs2017" => "msbuild",
        "ninja" => "ninja",
        "xcode4" => "xcode",
        _ => "unknown",
    }
}

// ── Line classification ──

/// Extract action from "Running action 'gmake2'..."
fn extract_action(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("Running action '")?;
    let end_quote = rest.find('\'')?;
    Some(&rest[..end_quote])
}

/// "Generated <path>..."
fn is_generated(trimmed: &str) -> bool {
    trimmed.starts_with("Generated ")
}

/// Extract ms from "Done (Nms)."
fn extract_done_ms(trimmed: &str) -> Option<u64> {
    let rest = trimmed.strip_prefix("Done (")?;
    let end_paren = rest.find("ms)")?;
    rest[..end_paren].parse::<u64>().ok()
}

/// "Error: <Lua error>"
fn is_premake_error(trimmed: &str) -> bool {
    trimmed.starts_with("Error: ")
}

// ── Filter ──

fn filter_premake_output(input: &str, exit_code: i32) -> String {
    let mut stats = PremakeStats::new();

    for line in input.lines() {
        let normalized = diag::normalize(line);
        let trimmed = normalized.trim();

        // Blank → skip
        if trimmed.is_empty() {
            continue;
        }

        // Running action → capture + detect backend
        if let Some(action) = extract_action(trimmed) {
            stats.action = Some(action.to_string());
            stats.backend = Some(detect_backend(action).to_string());
            continue;
        }

        // Generated → count
        if is_generated(trimmed) {
            stats.files_generated += 1;
            continue;
        }

        // Done (Nms) → capture
        if let Some(ms) = extract_done_ms(trimmed) {
            stats.elapsed_ms = Some(ms);
            continue;
        }

        // Error → errors
        if is_premake_error(trimmed) {
            stats.errors.push(normalized);
            continue;
        }

        // Compiler diagnostics → errors
        if diag::is_compiler_diag(trimmed) {
            stats.errors.push(normalized);
            continue;
        }

        // Everything else → keep (fail-open)
        stats.unrecognized.push(normalized);
    }

    compose_output(&stats, exit_code)
}

fn compose_output(stats: &PremakeStats, exit_code: i32) -> String {
    if !stats.errors.is_empty() && stats.action.is_none() {
        // Premake failed before generating anything
        let mut out = String::from("premake: FAILED\n");
        for err in &stats.errors {
            out.push_str(&format!("  {}\n", err));
        }
        if !stats.unrecognized.is_empty() {
            for line in &stats.unrecognized {
                out.push_str(&format!("  {}\n", line));
            }
        }
        out
    } else if !stats.errors.is_empty() {
        // Had errors but possibly partial output
        let mut out = String::from("premake: completed with errors\n");
        for err in &stats.errors {
            out.push_str(&format!("  {}\n", err));
        }
        if !stats.unrecognized.is_empty() {
            for line in &stats.unrecognized {
                out.push_str(&format!("  {}\n", line));
            }
        }
        out
    } else if exit_code != 0 {
        let mut out = format!("premake: failed (exit code {})\n", exit_code);
        if !stats.unrecognized.is_empty() {
            for line in &stats.unrecognized {
                out.push_str(&format!("  {}\n", line));
            }
        }
        out
    } else {
        let action = stats.action.as_deref().unwrap_or("?");
        let elapsed = stats
            .elapsed_ms
            .map(|ms| format!(" ({}ms)", ms))
            .unwrap_or_default();
        let backend = stats.backend.as_deref().unwrap_or("unknown");

        let mut out = format!(
            "ok premake: {} → {} files{}\n  backend: {}\n",
            action, stats.files_generated, elapsed, backend
        );
        if !stats.unrecognized.is_empty() {
            for line in &stats.unrecognized {
                out.push_str(&format!("  {}\n", line));
            }
        }
        out
    }
}

// ── Public API ──

/// Run premake5 with output filtering.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("premake: running premake5 {}", args.join(" "));
    }

    let mut cmd = resolved_command("premake5");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_filtered_with_exit(
        cmd,
        "premake5",
        &args_str,
        filter_premake_output,
        runner::RunOptions::with_tee("premake5"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    fn filter_premake(input: &str, exit_code: i32) -> String {
        filter_premake_output(input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_extract_action() {
        assert_eq!(extract_action("Running action 'gmake2'..."), Some("gmake2"));
        assert_eq!(extract_action("Running action 'vs2022'..."), Some("vs2022"));
        assert_eq!(extract_action("other"), None);
    }

    #[test]
    fn test_extract_done_ms() {
        assert_eq!(extract_done_ms("Done (1234ms)."), Some(1234));
        assert_eq!(extract_done_ms("Done (0ms)."), Some(0));
        assert_eq!(extract_done_ms("Done (abc)."), None);
    }

    #[test]
    fn test_detect_backend() {
        assert_eq!(detect_backend("gmake"), "make");
        assert_eq!(detect_backend("gmake2"), "make");
        assert_eq!(detect_backend("vs2022"), "msbuild");
        assert_eq!(detect_backend("vs2019"), "msbuild");
        assert_eq!(detect_backend("vs2017"), "msbuild");
        assert_eq!(detect_backend("ninja"), "ninja");
        assert_eq!(detect_backend("xcode4"), "xcode");
        assert_eq!(detect_backend("unknown-action"), "unknown");
    }

    // ── Success cases ──

    #[test]
    fn test_premake_success_gmake2() {
        let input = "\
Running action 'gmake2'...
Generated Makefile
Generated project1.make
Generated project2.make
Done (1234ms).
";
        let result = filter_premake(input, 0);
        assert!(result.contains("ok premake:"), "got: {}", result);
        assert!(result.contains("gmake2"), "got: {}", result);
        assert!(result.contains("→ 3 files"), "got: {}", result);
        assert!(result.contains("1234ms"), "got: {}", result);
        assert!(result.contains("backend: make"), "got: {}", result);
    }

    #[test]
    fn test_premake_success_vs2022() {
        let input = "\
Running action 'vs2022'...
Generated Project.sln
Generated Project.vcxproj
Generated Project.vcxproj.filters
Generated Project.vcxproj.user
Done (5678ms).
";
        let result = filter_premake(input, 0);
        assert!(result.contains("ok premake:"), "got: {}", result);
        assert!(result.contains("vs2022"), "got: {}", result);
        assert!(result.contains("→ 4 files"), "got: {}", result);
        assert!(result.contains("backend: msbuild"), "got: {}", result);
    }

    // ── Error case ──

    #[test]
    fn test_premake_lua_error() {
        let input = "\
Error: premake5.lua:42: attempt to index a nil value (global 'foo')
";
        let result = filter_premake(input, 1);
        assert!(result.contains("premake: FAILED"), "got: {}", result);
        assert!(
            result.contains("attempt to index a nil value"),
            "got: {}",
            result
        );
    }

    // ── Empty input ──

    #[test]
    fn test_premake_empty_input() {
        let result = filter_premake("", 0);
        assert!(result.contains("premake"), "got: {}", result);
        assert!(result.contains("0 files"), "got: {}", result);
    }

    // ── Token savings ──

    #[test]
    fn test_premake_token_savings() {
        let mut input = String::new();
        input.push_str("Running action 'gmake2'...\n");
        for i in 1..=200 {
            input.push_str(&format!("Generated output/file_{:04}.make\n", i));
        }
        input.push_str("Done (9876ms).\n");

        let result = filter_premake(&input, 0);
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
