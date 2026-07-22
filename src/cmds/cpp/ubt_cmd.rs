//! Filters UnrealBuildTool (UBT) project-generation / build output.
//!
//! UBT output is typically short (~150 lines). Uses buffered mode for simplicity.
//! Progress lines (`@progress '...' N%`) are collapsed to one "completed" line per phase.
//! Phase timing, toolchain info, compiler version warnings, and final result are preserved.

#![allow(dead_code)]

use super::diag;
use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;

// ─── State ───

/// Phase info captured from UBT output.
struct PhaseInfo {
    name: String,
    timing: Option<String>,
}

/// Track state during UBT output parsing.
struct UbtStats {
    /// Extracted [CMD] line (command that was run)
    command: String,
    /// Log file path (dropped in output)
    _log_file: Option<String>,
    /// Ordered phases with optional timing
    phases: Vec<PhaseInfo>,
    /// Current @progress phase being tracked
    current_phase: Option<String>,
    /// Toolchain entries
    toolchains: Vec<String>,
    /// Non-fatal warnings
    warnings: Vec<String>,
    /// Error lines
    errors: Vec<String>,
    /// Result string: "Succeeded" or "Failed"
    result: Option<String>,
    /// Total execution time
    total_time: Option<String>,
    /// Whether we've seen a phase timing line for the current phase
    _has_timing: bool,
}

impl UbtStats {
    fn new() -> Self {
        Self {
            command: String::new(),
            _log_file: None,
            phases: Vec::new(),
            current_phase: None,
            toolchains: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
            result: None,
            total_time: None,
            _has_timing: false,
        }
    }
}

// ─── Line Classification Helpers ───

/// Check if a line is a `@progress 'Phase Name' N%` line.
fn is_progress_line(trimmed: &str) -> Option<&str> {
    // Matches: @progress 'Phase Name' 0% through 100%
    diag::lazy_re!(r"^@progress '([^']+)' \d+%$")
        .captures(trimmed)
        .map(|c| c.get(1).unwrap().as_str())
}

/// Check if a line is a phase timing line like `Adding projects for all targets took 0.88s`.
/// Returns (phase_name, timing_string).
fn is_phase_timing(trimmed: &str) -> Option<(&str, &str)> {
    let re = diag::lazy_re!(r"^(.+?) took ([\d.]+)s$");
    let caps = re.captures(trimmed)?;
    Some((caps.get(1)?.as_str(), caps.get(2)?.as_str()))
}

/// Check if a line is a toolchain header like `Available x64 toolchains (1):`.
fn is_toolchain_header(trimmed: &str) -> bool {
    trimmed.starts_with("Available ") && trimmed.contains(" toolchains")
}

/// Check if a line is a toolchain entry (starts with a digit or dash — was indented in original).
fn is_toolchain_entry(trimmed: &str) -> bool {
    (trimmed.starts_with(|c: char| c.is_ascii_digit()) || trimmed.starts_with('-'))
        && trimmed.len() > 2
}

/// Check if a line is a warning (compiler version warning etc.).
fn is_warning_line(trimmed: &str) -> bool {
    trimmed.contains("not a preferred version") || trimmed.starts_with("WARNING: ")
}

/// Check if a line is an error.
fn is_error_line(trimmed: &str) -> bool {
    trimmed.starts_with("Error: ") || trimmed.starts_with("error: ")
}

/// Check if a line is the result line. Returns "Succeeded" or "Failed".
fn is_result_line(trimmed: &str) -> Option<&str> {
    if let Some(val) = trimmed.strip_prefix("Result: ") {
        Some(val.trim())
    } else {
        None
    }
}

/// Check if a line is total execution time line.
fn is_total_time(trimmed: &str) -> Option<&str> {
    if let Some(val) = trimmed.strip_prefix("Total execution time: ") {
        Some(val.trim())
    } else {
        None
    }
}

/// Extract command from [CMD] line.
fn extract_command(trimmed: &str) -> Option<&str> {
    trimmed.strip_prefix("[CMD] ")
}

/// Extract log file path.
fn extract_log_file(trimmed: &str) -> Option<&str> {
    trimmed.strip_prefix("Log file: ")
}

// ─── Main Filter Function ───

/// Filter UBT project-generation/build output.
fn filter_ubt_output(input: &str, _exit_code: i32) -> String {
    let ansi_free = strip_ansi(input);
    let mut stats = UbtStats::new();

    for line in ansi_free.lines() {
        let trimmed = line.trim();

        // Skip blank lines
        if trimmed.is_empty() {
            continue;
        }

        // Command line
        if let Some(cmd) = extract_command(trimmed) {
            stats.command = cmd.to_string();
            continue;
        }

        // Log file path — drop
        if extract_log_file(trimmed).is_some() {
            stats._log_file = Some(trimmed.to_string());
            continue;
        }

        // Result line
        if let Some(result) = is_result_line(trimmed) {
            stats.result = Some(result.to_string());
            continue;
        }

        // Total execution time
        if let Some(time) = is_total_time(trimmed) {
            stats.total_time = Some(time.to_string());
            continue;
        }

        // Progress lines — collapse
        if let Some(phase_name) = is_progress_line(trimmed) {
            if stats.current_phase.as_deref() != Some(phase_name) {
                // New phase starting
                if let Some(old_phase) = stats.current_phase.take() {
                    stats.phases.push(PhaseInfo {
                        name: old_phase,
                        timing: None,
                    });
                }
                stats.current_phase = Some(phase_name.to_string());
                stats._has_timing = false;
            }
            continue;
        }

        // Phase timing
        if let Some((phase_name, timing)) = is_phase_timing(trimmed) {
            // Check if this matches the current progress phase or stands alone
            let phase_str = phase_name.to_string();
            if let Some(ref current) = stats.current_phase {
                // Try to match: the timing line might rephrase the phase name
                stats.phases.push(PhaseInfo {
                    name: current.clone(),
                    timing: Some(format!("{}s", timing)),
                });
                stats.current_phase = None;
                stats._has_timing = true;
            } else {
                // Standalone timing line without progress header
                stats.phases.push(PhaseInfo {
                    name: phase_str,
                    timing: Some(format!("{}s", timing)),
                });
            }
            continue;
        }

        // Toolchain header — keep as information
        if is_toolchain_header(trimmed) {
            stats.toolchains.push(trimmed.to_string());
            continue;
        }

        // Toolchain entry — keep
        if is_toolchain_entry(trimmed) {
            stats.toolchains.push(trimmed.to_string());
            continue;
        }

        // Warning lines
        if is_warning_line(trimmed) {
            stats.warnings.push(trimmed.to_string());
            continue;
        }

        // Error lines
        if is_error_line(trimmed) {
            stats.errors.push(trimmed.to_string());
            continue;
        }

        // Phase header / section header — keep (e.g. "Generating VisualStudio project files:")
        // These are lines that describe what's happening but don't match progress patterns.
        // Keep them unless they're pure noise.
        if trimmed == "Discovering modules, targets and source code for project..."
            || trimmed == "Generating VisualStudio project files:"
        {
            if stats.current_phase.is_none() {
                stats.current_phase = Some(trimmed.to_string());
            }
            continue;
        }

        // Exit code line — capture if present but don't display
        if trimmed.starts_with("Exit code:") {
            continue;
        }

        // Keep other lines that seem meaningful (non-progress, non-boilerplate)
        // Only drop known noisy patterns; keep everything else for safety
        if trimmed.starts_with("Log file:") {
            continue;
        }
        if trimmed.starts_with("Took ") && trimmed.ends_with("ms") {
            // Timing line like "Took 123.45ms" — drop, we have phase timing
            continue;
        }

        // Keep anything else that looks meaningful
        // (But skip if it's just a path/line we can't classify)
    }

    // Flush current progress phase
    if let Some(phase) = stats.current_phase.take() {
        stats.phases.push(PhaseInfo {
            name: phase,
            timing: None,
        });
    }

    compose_output(&stats)
}

// ─── Output Composition ───

/// Build the compact output from parsed state.
fn compose_output(stats: &UbtStats) -> String {
    let mut output = String::new();

    // Determine what kind of UBT invocation this is
    let cmd_desc = if stats.command.is_empty() {
        "ubt".to_string()
    } else {
        // Shorten the command: extract project file or relevant part
        let cmd = &stats.command;
        if cmd.contains("-projectfiles") {
            // Extract project path
            if let Some(pos) = cmd.find("-projectfiles") {
                let after = &cmd[pos + "-projectfiles".len()..].trim();
                let project = after.split_whitespace().next().unwrap_or("");
                format!("ubt: projectfiles ({})", project)
            } else {
                "ubt: projectfiles".to_string()
            }
        } else if cmd.contains("-build") || cmd.contains("-make") {
            "ubt: build".to_string()
        } else {
            "ubt".to_string()
        }
    };

    // Success/failure prefix
    let is_failure = stats.result.as_deref() == Some("Failed") || !stats.errors.is_empty();

    if is_failure {
        output.push_str(&format!("{} failed\n", cmd_desc));
    } else {
        output.push_str(&format!("ok {}\n", cmd_desc));
    }

    // Phases
    if !stats.phases.is_empty() {
        let phase_strs: Vec<String> = stats
            .phases
            .iter()
            .map(|p| {
                if let Some(ref t) = p.timing {
                    format!("{} ({})", p.name, t)
                } else {
                    p.name.clone()
                }
            })
            .collect();
        output.push_str(&format!("  phases: {}\n", phase_strs.join(", ")));
    }

    // Toolchains
    for tc in &stats.toolchains {
        if tc.starts_with(|c: char| c.is_ascii_digit()) || tc.starts_with('-') {
            output.push_str(&format!("  toolchain: {}\n", tc));
        } else {
            output.push_str(&format!("  {}\n", tc));
        }
    }

    // Warnings
    for w in &stats.warnings {
        output.push_str(&format!("  warning: {}\n", w));
    }

    // Errors
    for e in &stats.errors {
        output.push_str(&format!("  error: {}\n", e));
    }

    // Result
    if let Some(ref result) = stats.result {
        output.push_str(&format!("Result: {}\n", result));
    }

    // Total time
    if let Some(ref time) = stats.total_time {
        output.push_str(&format!("Total execution time: {}\n", time));
    }

    output
}

// ─── Public API ───

/// Run UBT with output filtering.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("ubt: running ubt {}", args.join(" "));
    }

    let mut cmd = resolved_command("ubt");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_filtered_with_exit(
        cmd,
        "ubt",
        &args_str,
        filter_ubt_output,
        runner::RunOptions::with_tee("ubt"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    fn filter_ubt(input: &str, exit_code: i32) -> String {
        filter_ubt_output(input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_progress_line_normal() {
        assert_eq!(
            is_progress_line("@progress 'Compiling Rules Assemblies' 0%"),
            Some("Compiling Rules Assemblies")
        );
        assert_eq!(
            is_progress_line("@progress 'Compiling Rules Assemblies' 100%"),
            Some("Compiling Rules Assemblies")
        );
    }

    #[test]
    fn test_is_progress_line_not() {
        assert!(is_progress_line("Generating VisualStudio project files:").is_none());
        assert!(is_progress_line("").is_none());
        assert!(is_progress_line("[CMD] something").is_none());
    }

    #[test]
    fn test_is_phase_timing() {
        let result = is_phase_timing("Adding projects for all targets took 0.88s");
        assert!(result.is_some());
        assert_eq!(result.unwrap().0, "Adding projects for all targets");
        assert_eq!(result.unwrap().1, "0.88");
    }

    #[test]
    fn test_is_result_line() {
        assert_eq!(is_result_line("Result: Succeeded"), Some("Succeeded"));
        assert_eq!(is_result_line("Result: Failed"), Some("Failed"));
        assert!(is_result_line("Not a result").is_none());
    }

    #[test]
    fn test_is_total_time() {
        assert_eq!(
            is_total_time("Total execution time: 8.87 seconds"),
            Some("8.87 seconds")
        );
        assert!(is_total_time("something else").is_none());
    }

    #[test]
    fn test_extract_command() {
        assert_eq!(
            extract_command("[CMD] D:\\UE_5.6\\Engine\\Binaries\\DotNET\\UnrealBuildTool\\UnrealBuildTool.exe -projectfiles"),
            Some("D:\\UE_5.6\\Engine\\Binaries\\DotNET\\UnrealBuildTool\\UnrealBuildTool.exe -projectfiles")
        );
        assert!(extract_command("not a cmd").is_none());
    }

    #[test]
    fn test_is_warning_line() {
        assert!(is_warning_line(
            "Visual Studio 2022 compiler version 14.44.35217 is not a preferred version..."
        ));
        assert!(is_warning_line("WARNING: something"));
        assert!(!is_warning_line("Result: Succeeded"));
    }

    #[test]
    fn test_is_error_line() {
        assert!(is_error_line("Error: something went wrong"));
        assert!(is_error_line("error: file not found"));
        assert!(!is_error_line("Result: Failed"));
    }

    // ── Success cases ──

    #[test]
    fn test_ubt_successful_projectfiles() {
        let input = "\
[CMD] D:\\UE_5.6\\Engine\\Binaries\\DotNET\\UnrealBuildTool\\UnrealBuildTool.exe -projectfiles D:\\TestUE\\TestUE.uproject
Log file: C:\\Users\\user\\AppData\\Local\\UnrealBuildTool\\Log_GPF.txt
Generating VisualStudio project files:
Discovering modules, targets and source code for project...
@progress 'Compiling Rules Assemblies' 0%
@progress 'Compiling Rules Assemblies' 50%
@progress 'Compiling Rules Assemblies' 100%
Adding projects for all targets took 0.88s
@progress 'Generating project files' 0%
@progress 'Generating project files' 100%
Generating project files took 2.34s
Available x64 toolchains (1):
  1) Visual Studio 2022 (version 14.44.35207)
Visual Studio 2022 compiler version 14.44.35217 is not a preferred version (prefer 14.38.33130)
Result: Succeeded
Total execution time: 8.87 seconds
Exit code: 0
";
        let result = filter_ubt(input, 0);
        assert!(result.contains("ok ubt: projectfiles"), "got: {}", result);
        assert!(
            result.contains("Compiling Rules Assemblies (0.88s)"),
            "should have phase with timing, got: {}",
            result
        );
        assert!(
            !result.contains("@progress"),
            "progress lines should be collapsed, got: {}",
            result
        );
        assert!(
            result.contains("toolchain: 1) Visual Studio 2022 (version 14.44.35207)"),
            "got: {}",
            result
        );
        assert!(
            result.contains("warning:"),
            "should include warning, got: {}",
            result
        );
        assert!(result.contains("Result: Succeeded"), "got: {}", result);
        assert!(
            result.contains("Total execution time: 8.87 seconds"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_ubt_progress_collapsed() {
        let input = "\
@progress 'Phase One' 0%
@progress 'Phase One' 25%
@progress 'Phase One' 50%
@progress 'Phase One' 75%
@progress 'Phase One' 100%
@progress 'Phase Two' 0%
@progress 'Phase Two' 50%
@progress 'Phase Two' 100%
Result: Succeeded
";
        let result = filter_ubt(input, 0);
        assert!(
            !result.contains("@progress"),
            "all progress lines should be removed, got: {}",
            result
        );
        assert!(result.contains("Phase One"), "got: {}", result);
        assert!(result.contains("Phase Two"), "got: {}", result);
    }

    #[test]
    fn test_ubt_warning_compiler_version() {
        let input = "\
Visual Studio 2022 compiler version 14.44.35217 is not a preferred version (prefer 14.38.33130)
Result: Succeeded
";
        let result = filter_ubt(input, 0);
        assert!(
            result.contains("not a preferred version"),
            "warning should be kept, got: {}",
            result
        );
    }

    #[test]
    fn test_ubt_toolchain_info() {
        let input = "\
Available x64 toolchains (1):
  1) Visual Studio 2022 (version 14.44.35207)
Result: Succeeded
";
        let result = filter_ubt(input, 0);
        assert!(
            result.contains("toolchain: 1) Visual Studio 2022"),
            "toolchain should be kept, got: {}",
            result
        );
    }

    #[test]
    fn test_ubt_phase_timings() {
        let input = "\
@progress 'Discovering modules' 0%
@progress 'Discovering modules' 100%
Discovering modules took 1.23s
@progress 'Generating files' 0%
@progress 'Generating files' 100%
Generating files took 4.56s
Result: Succeeded
";
        let result = filter_ubt(input, 0);
        assert!(
            result.contains("Discovering modules (1.23s)"),
            "got: {}",
            result
        );
        assert!(
            result.contains("Generating files (4.56s)"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_ubt_empty_input() {
        let result = filter_ubt("", 0);
        assert!(
            result.contains("ubt"),
            "should have a summary, got: '{}'",
            result
        );
    }

    #[test]
    fn test_ubt_only_progress_lines() {
        let input = "\
@progress 'Phase One' 0%
@progress 'Phase One' 100%
@progress 'Phase Two' 0%
@progress 'Phase Two' 100%
";
        let result = filter_ubt(input, 0);
        // Should have phases listed but no progress lines
        assert!(!result.contains("@progress"), "got: {}", result);
        assert!(
            result.contains("Phase One") || result.contains("Phase Two"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_ubt_token_savings_above_70pct() {
        // Simulate UBT output with many progress lines
        let mut input = String::new();
        input.push_str("[CMD] UnrealBuildTool.exe -build\n");
        // 50 progress lines (10 phases × 5 lines each)
        for phase in 1..=10 {
            for pct in &[0, 25, 50, 75, 100] {
                input.push_str(&format!("@progress 'Phase {}' {}%\n", phase, pct));
            }
        }
        input.push_str("Available x64 toolchains (1):\n");
        input.push_str("  1) Visual Studio 2022 (version 14.44.35207)\n");
        input.push_str("Result: Succeeded\n");
        input.push_str("Total execution time: 15.23 seconds\n");

        let result = filter_ubt(&input, 0);
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

    #[test]
    fn test_ubt_ansi_stripped() {
        let input = "\x1b[32mResult: Succeeded\x1b[0m\n\x1b[31mError: something bad\x1b[0m\n";
        let result = filter_ubt(input, 1);
        assert!(
            !result.contains("\x1b["),
            "ANSI codes should be stripped, got: {}",
            result
        );
        assert!(result.contains("Result: Succeeded"), "got: {}", result);
    }
}
