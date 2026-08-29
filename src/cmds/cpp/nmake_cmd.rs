#![allow(dead_code)]

//! Filters NMAKE build output — progress lines dropped, errors/warnings kept.
//!
//! Uses a buffered filter via `runner::run_filtered_with_exit`.  All output is
//! captured, classified line-by-line, then a compact summary is composed.

use super::diag;
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::collections::HashMap;

// ── Patterns ──

/// Check for `NMAKE : fatal error U1077:` or other U-prefixed fatal errors.
pub fn is_nmake_fatal(trimmed: &str) -> bool {
    diag::lazy_re!(r"^NMAKE : fatal error U\d+:").is_match(trimmed)
}

/// Check for the `Stop.` line that NMAKE emits after a fatal error.
pub fn is_nmake_stop(trimmed: &str) -> bool {
    trimmed == "Stop." || trimmed.starts_with("Stop.")
}

/// Check for cmake-driven progress lines: `[ NN%] Building/Linking...`.
pub fn is_cmake_progress(trimmed: &str) -> bool {
    diag::lazy_re!(r"^\[\s*\d+%\] (Building|Linking|Built target|Scanning)").is_match(trimmed)
}

/// Check for MSBuild-style progress (sometimes mixed in via cmake --build).
pub fn is_msbuild_progress(trimmed: &str) -> bool {
    trimmed.starts_with("  ")
        && (trimmed.contains("->") || trimmed.contains("Building"))
        && trimmed.contains(".vcxproj")
}

/// Check for cmake "Built target" lines.
pub fn is_built_target(trimmed: &str) -> bool {
    trimmed.starts_with("Built target ")
}

// ── Stats ──

/// Accumulated statistics from NMAKE output.
struct NmakeStats {
    /// Number of build edges completed.
    edges_built: usize,
    /// Total build edges (from progress lines).
    edges_total: usize,
    /// Number of errors detected.
    errors: usize,
    /// Warning flag → count.
    warning_counts: HashMap<String, usize>,
    /// Dedup: message body → count.
    seen_diagnostics: HashMap<String, usize>,
    /// Whether a fatal error was seen.
    has_fatal: bool,
    /// Error cascade levels (for summary).
    cascade_levels: Vec<String>,
}

impl NmakeStats {
    fn new() -> Self {
        Self {
            edges_built: 0,
            edges_total: 0,
            errors: 0,
            warning_counts: HashMap::new(),
            seen_diagnostics: HashMap::new(),
            has_fatal: false,
            cascade_levels: Vec::new(),
        }
    }
}

// ── Filter ──

/// Filter NMAKE output and compose a compact summary.
fn filter_nmake_output(input: &str, exit_code: i32) -> String {
    let mut stats = NmakeStats::new();
    let mut output_lines: Vec<String> = Vec::new();
    let mut in_error_block = false;
    let mut skip_until_blank = false;

    for raw_line in input.lines() {
        let normalized = diag::normalize(raw_line);
        let trimmed = normalized.trim();

        if trimmed.is_empty() {
            if in_error_block {
                in_error_block = false;
                output_lines.push(String::new());
            }
            skip_until_blank = false;
            continue;
        }

        // ── Progress lines ──
        if is_cmake_progress(trimmed) {
            stats.edges_built += 1;
            // Percentage alone doesn't give total; track as-is
            let _ = parse_progress_pct(trimmed);
            continue;
        }

        // ── Built target ──
        if is_built_target(trimmed) {
            continue;
        }

        // ── MSBuild-style progress ──
        if is_msbuild_progress(trimmed) {
            continue;
        }

        // ── NMAKE fatal error ──
        if is_nmake_fatal(trimmed) {
            stats.has_fatal = true;
            stats.errors += 1;
            stats.cascade_levels.push(trimmed.to_string());
            output_lines.push(trimmed.to_string());
            in_error_block = true;
            skip_until_blank = false;
            continue;
        }

        // ── Stop. line ──
        if is_nmake_stop(trimmed) {
            if in_error_block {
                in_error_block = false;
            }
            continue;
        }

        // ── Compiler diagnostic ──
        if diag::is_compiler_diag(trimmed) {
            let msg = diag::extract_diag_message(trimmed);
            let count = stats.seen_diagnostics.entry(msg).or_insert(0);
            *count += 1;
            if *count <= 3 {
                output_lines.push(trimmed.to_string());
            }
            // Track warnings
            if trimmed.to_lowercase().contains("warning") {
                if let Some(flag) = diag::extract_warning_flag(trimmed) {
                    *stats.warning_counts.entry(flag).or_insert(0) += 1;
                } else {
                    *stats.warning_counts.entry("other".to_string()).or_insert(0) += 1;
                }
            }
            if trimmed.to_lowercase().contains("error") {
                stats.errors += 1;
            }
            in_error_block = true;
            skip_until_blank = false;
            continue;
        }

        // ── Linker error ──
        if diag::is_linker_error(trimmed) {
            stats.errors += 1;
            output_lines.push(trimmed.to_string());
            in_error_block = true;
            skip_until_blank = false;
            continue;
        }

        // ── Diag continuation (inside error block) ──
        if in_error_block && diag::is_diag_continuation(trimmed) {
            output_lines.push(trimmed.to_string());
            continue;
        }

        // ── Recipe echoes (MSVC tool invocations) ──
        if skip_until_blank {
            continue;
        }
        if is_recipe_echo(trimmed) {
            skip_until_blank = true;
            continue;
        }

        // ── Fail-open: pass through unrecognized lines ──
        output_lines.push(trimmed.to_string());
    }

    // Compose output
    let mut result = String::new();

    // Emit collected output lines
    for line in &output_lines {
        result.push_str(line);
        result.push('\n');
    }

    // Summary
    if stats.has_fatal || exit_code != 0 {
        if stats.has_fatal {
            if stats.cascade_levels.is_empty() {
                result.push_str("nmake: fatal error\n");
            } else {
                result.push_str(&format!(
                    "nmake: fatal error ({})\n",
                    stats.cascade_levels.last().unwrap()
                ));
                for level in &stats.cascade_levels {
                    result.push_str(&format!("  {}\n", level));
                }
            }
        } else if stats.errors > 0 {
            result.push_str(&format!("nmake: {} error(s)\n", stats.errors));
        } else {
            result.push_str(&format!(
                "nmake: exited with code {} (no specific errors captured)\n",
                exit_code
            ));
        }
    } else {
        let total = if stats.edges_total > 0 {
            stats.edges_total
        } else {
            stats.edges_built
        };
        result.push_str(&format!("ok nmake: {} edges\n", total));
    }

    // Warning summary
    if !stats.warning_counts.is_empty() {
        let mut warnings: Vec<_> = stats.warning_counts.iter().collect();
        warnings.sort_by(|a, b| b.1.cmp(a.1));
        let warn_parts: Vec<String> = warnings
            .iter()
            .map(|(flag, count)| format!("{} ×{}", flag, count))
            .collect();
        result.push_str(&format!("  warnings: {}\n", warn_parts.join(", ")));
    }

    result
}

/// Parse total edges from cmake progress like `[ 42%] Building CXX object ...`.
/// Not strictly accurate for N edges but provides a rough count.
fn parse_progress_pct(trimmed: &str) -> Option<usize> {
    let start = trimmed.find('[')? + 1;
    let end = trimmed.find('%')?;
    trimmed[start..end].trim().parse::<usize>().ok()
}

/// Detect MSVC recipe echoes: `cl.exe`, `link.exe`, `rc.exe`, `mt.exe`, etc.
fn is_recipe_echo(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    // Starts with a quoted path
    if trimmed.starts_with('"') {
        return true;
    }
    let first = trimmed.split_whitespace().next().unwrap_or("");
    let known = [
        "cl", "cl.exe", "link", "link.exe", "rc", "rc.exe", "mt", "mt.exe", "lib", "lib.exe", "ml",
        "ml.exe", "ml64", "ml64.exe", "nmake", "midl", "midl.exe", "mc", "mc.exe",
    ];
    if let Some(fname) = std::path::Path::new(first).file_name() {
        if let Some(name) = fname.to_str() {
            if known.contains(&name) {
                return true;
            }
        }
    }
    // Path-like: contains backslash in first word
    if first.contains('\\') && first.len() > 2 {
        return true;
    }
    false
}

// ── Public API ──

/// Run NMAKE with filtered output.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("nmake: running nmake {}", args.join(" "));
    }

    let mut cmd = resolved_command("nmake");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_filtered_with_exit(
        cmd,
        "nmake",
        &args_str,
        filter_nmake_output,
        runner::RunOptions::with_tee("nmake"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    // ── Helper tests ──

    #[test]
    fn test_is_nmake_fatal_u1077() {
        assert!(is_nmake_fatal(
            "NMAKE : fatal error U1077: 'cl.exe' : return code '0x2'"
        ));
    }

    #[test]
    fn test_is_nmake_fatal_other() {
        assert!(is_nmake_fatal(
            "NMAKE : fatal error U1052: file 'foo' not found"
        ));
    }

    #[test]
    fn test_is_nmake_fatal_not() {
        assert!(!is_nmake_fatal("cl.exe /c file.cpp"));
        assert!(!is_nmake_fatal("file.cpp(42): error C2065: 'x'"));
    }

    #[test]
    fn test_is_nmake_stop() {
        assert!(is_nmake_stop("Stop."));
        assert!(!is_nmake_stop("Don't Stop."));
    }

    #[test]
    fn test_is_cmake_progress_building() {
        assert!(is_cmake_progress(
            "[ 42%] Building CXX object CMakeFiles/foo.dir/foo.cpp.obj"
        ));
    }

    #[test]
    fn test_is_cmake_progress_linking() {
        assert!(is_cmake_progress("[100%] Linking CXX executable myapp.exe"));
    }

    #[test]
    fn test_is_cmake_progress_scanning() {
        assert!(is_cmake_progress(
            "[  0%] Scanning dependencies of target foo"
        ));
    }

    #[test]
    fn test_is_cmake_progress_not() {
        assert!(!is_cmake_progress("cl.exe /c file.cpp"));
        assert!(!is_cmake_progress("error C2065: 'x'"));
    }

    // ── Success case ──

    #[test]
    fn test_nmake_success() {
        let input = "\
[  0%] Building CXX object CMakeFiles/app.dir/main.cpp.obj
[ 50%] Building CXX object CMakeFiles/app.dir/util.cpp.obj
[100%] Linking CXX executable app.exe
Built target app
";
        let result = filter_nmake_output(input, 0);
        assert!(result.contains("ok nmake: 3 edges"), "got: {}", result);
    }

    // ── Error cases ──

    #[test]
    fn test_nmake_single_error() {
        let input = "\
[  0%] Building CXX object CMakeFiles/app.dir/bad.cpp.obj
cl.exe /c bad.cpp /Fobad.cpp.obj
bad.cpp(5): error C2065: 'x': undeclared identifier
NMAKE : fatal error U1077: 'cl.exe' : return code '0x2'
Stop.
";
        let result = filter_nmake_output(input, 1);
        assert!(result.contains("fatal error"), "got: {}", result);
        assert!(result.contains("C2065"), "got: {}", result);
        assert!(
            !result.contains("cl.exe /c bad.cpp"),
            "recipe echo should be dropped, got: {}",
            result
        );
    }

    #[test]
    fn test_nmake_error_cascade() {
        let input = "\
[  0%] Building CXX object CMakeFiles/app.dir/a.cpp.obj
a.cpp(1): error C2065: 'a_err': undeclared identifier
NMAKE : fatal error U1077: 'cl.exe' : return code '0x2'
Stop.
[ 50%] Building CXX object CMakeFiles/app.dir/b.cpp.obj
b.cpp(1): error C2065: 'b_err': undeclared identifier
NMAKE : fatal error U1077: 'cl.exe' : return code '0x2'
Stop.
NMAKE : fatal error U1077: 'nmake.exe' : return code '0x2'
Stop.
";
        let result = filter_nmake_output(input, 1);
        assert!(result.contains("fatal error"), "got: {}", result);
        // Should show multiple errors
        assert!(
            result.matches("NMAKE : fatal error").count() >= 1,
            "got: {}",
            result
        );
    }

    #[test]
    fn test_nmake_recipe_echo_dropped() {
        let input = "\
[  0%] Building CXX object CMakeFiles/app.dir/main.cpp.obj
cl.exe /c /nologo /EHsc /O2 main.cpp /Fomain.cpp.obj
[100%] Linking CXX executable app.exe
Built target app
";
        let result = filter_nmake_output(input, 0);
        assert!(
            !result.contains("cl.exe /c"),
            "recipe echo should be dropped, got: {}",
            result
        );
        assert!(result.contains("ok nmake: 2 edges"), "got: {}", result);
    }

    // ── Edge cases ──

    #[test]
    fn test_nmake_ansi_stripped() {
        let input = "\x1b[31mNMAKE : fatal error U1077: 'cl.exe' : return code '0x2'\x1b[0m\n\
                      \x1b[31mStop.\x1b[0m\n";
        let result = filter_nmake_output(input, 1);
        assert!(
            !result.contains("\x1b["),
            "ANSI codes should be stripped, got: {}",
            result
        );
        assert!(result.contains("NMAKE : fatal error"), "got: {}", result);
    }

    #[test]
    fn test_nmake_empty_input() {
        let result = filter_nmake_output("", 0);
        assert!(
            result.contains("ok nmake: 0 edges"),
            "should have a summary, got: '{}'",
            result
        );
    }

    #[test]
    fn test_nmake_token_savings_above_70pct() {
        let mut input = String::new();
        for i in 1..=200 {
            input.push_str(&format!(
                "[{:3}%] Building CXX object CMakeFiles/app.dir/file{:03}.cpp.obj\n",
                (i * 100 / 200).min(99),
                i
            ));
        }
        input.push_str("[100%] Linking CXX executable app.exe\n");
        input.push_str("Built target app\n");

        let result = filter_nmake_output(&input, 0);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 70,
            "token savings: {}% (expected >=70%)",
            savings
        );
    }
}
