//! Data types for reporting which commands RTK can and cannot optimize.

use crate::hooks::constants::{HOOKS_SUBDIR, REWRITE_HOOK_FILE};
use serde::Serialize;

/// RTK support status for a command.
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq)]
pub enum RtkStatus {
    /// Dedicated handler with filtering (e.g., git status → git.rs:run_status())
    Existing,
    /// Works via external_subcommand passthrough, no filtering (e.g., cargo fmt → Other)
    Passthrough,
    /// RTK doesn't handle this command at all
    NotSupported,
}

impl RtkStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            RtkStatus::Existing => "existing",
            RtkStatus::Passthrough => "passthrough",
            RtkStatus::NotSupported => "not-supported",
        }
    }
}

/// A supported command that RTK already handles.
#[derive(Debug, Serialize)]
pub struct SupportedEntry {
    pub command: String,
    pub count: usize,
    pub rtk_equivalent: &'static str,
    pub category: &'static str,
    pub estimated_savings_tokens: usize,
    pub estimated_savings_pct: f64,
    pub rtk_status: RtkStatus,
}

/// An unsupported command not yet handled by RTK.
#[derive(Debug, Serialize)]
pub struct UnsupportedEntry {
    pub base_command: String,
    pub count: usize,
    pub example: String,
}

/// Full discover report.
#[derive(Debug, Serialize)]
pub struct DiscoverReport {
    pub sessions_scanned: usize,
    pub total_commands: usize,
    pub already_rtk: usize,
    pub since_days: u64,
    pub supported: Vec<SupportedEntry>,
    pub unsupported: Vec<UnsupportedEntry>,
    pub parse_errors: usize,
    pub rtk_disabled_count: usize,
    pub rtk_disabled_examples: Vec<String>,
}

impl DiscoverReport {
    pub fn total_saveable_tokens(&self) -> usize {
        self.supported
            .iter()
            .map(|s| s.estimated_savings_tokens)
            .sum()
    }

    pub fn total_supported_count(&self) -> usize {
        self.supported.iter().map(|s| s.count).sum()
    }
}

/// Format report as text.
pub fn format_text(report: &DiscoverReport, limit: usize, verbose: bool) -> String {
    let mut out = String::with_capacity(2048);

    out.push_str("RTK Discover -- Savings Opportunities\n");
    out.push_str(&"=".repeat(52));
    out.push('\n');
    out.push_str(&format!(
        "Scanned: {} sessions (last {} days), {} Bash commands\n",
        report.sessions_scanned, report.since_days, report.total_commands
    ));
    out.push_str(&format!(
        "Already using RTK: {} commands ({:.1}%)\n",
        report.already_rtk,
        if report.total_commands > 0 {
            report.already_rtk as f64 * 100.0 / report.total_commands as f64
        } else {
            0.0
        }
    ));

    if report.supported.is_empty() && report.unsupported.is_empty() {
        out.push_str("\nNo missed savings found. RTK usage looks good!\n");
        return out;
    }

    // Missed savings
    if !report.supported.is_empty() {
        out.push_str("\nMISSED SAVINGS -- Commands RTK already handles\n");
        out.push_str(&"-".repeat(72));
        out.push('\n');
        out.push_str(&format!(
            "{:<24} {:>5}    {:<18} {:<13} {:>12}\n",
            "Command", "Count", "RTK Equivalent", "Status", "Est. Savings"
        ));

        for entry in report.supported.iter().take(limit) {
            out.push_str(&format!(
                "{:<24} {:>5}    {:<18} {:<13} ~{}\n",
                truncate_str(&entry.command, 23),
                entry.count,
                entry.rtk_equivalent,
                entry.rtk_status.as_str(),
                format_tokens(entry.estimated_savings_tokens),
            ));
        }

        out.push_str(&"-".repeat(72));
        out.push('\n');
        out.push_str(&format!(
            "Total: {} commands -> ~{} saveable\n",
            report.total_supported_count(),
            format_tokens(report.total_saveable_tokens()),
        ));
    }

    // Unhandled
    if !report.unsupported.is_empty() {
        out.push_str("\nTOP UNHANDLED COMMANDS -- open an issue?\n");
        out.push_str(&"-".repeat(52));
        out.push('\n');
        out.push_str(&format!(
            "{:<24} {:>5}    {}\n",
            "Command", "Count", "Example"
        ));

        for entry in report.unsupported.iter().take(limit) {
            out.push_str(&format!(
                "{:<24} {:>5}    {}\n",
                truncate_str(&entry.base_command, 23),
                entry.count,
                truncate_str(&entry.example, 40),
            ));
        }

        out.push_str(&"-".repeat(52));
        out.push('\n');
        out.push_str("-> github.com/rtk-ai/rtk/issues\n");
    }

    // RTK_DISABLED bypass warning
    if report.rtk_disabled_count > 0 {
        out.push_str(&format!(
            "\nRTK_DISABLED BYPASS -- {} commands ran without filtering\n",
            report.rtk_disabled_count
        ));
        out.push_str(&"-".repeat(72));
        out.push('\n');
        out.push_str("These commands used RTK_DISABLED=1 unnecessarily:\n");
        if !report.rtk_disabled_examples.is_empty() {
            out.push_str(&format!("  {}\n", report.rtk_disabled_examples.join(", ")));
        }
        out.push_str("-> Remove RTK_DISABLED=1 to recover token savings\n");
    }

    out.push_str("\n~estimated from tool_result output sizes\n");

    // Cursor note: check if Cursor hooks are installed
    if let Some(home) = dirs::home_dir() {
        let cursor_hook = home
            .join(".cursor")
            .join(HOOKS_SUBDIR)
            .join(REWRITE_HOOK_FILE);
        if cursor_hook.exists() {
            out.push_str("\nNote: Cursor sessions are tracked via `rtk gain` (discover scans Claude Code only)\n");
        }
    }

    if verbose && report.parse_errors > 0 {
        out.push_str(&format!("Parse errors skipped: {}\n", report.parse_errors));
    }

    out
}

/// Format report as JSON.
pub fn format_json(report: &DiscoverReport) -> String {
    serde_json::to_string_pretty(report).unwrap_or_else(|_| "{}".to_string())
}

fn format_tokens(tokens: usize) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M tokens", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K tokens", tokens as f64 / 1_000.0)
    } else {
        format!("{} tokens", tokens)
    }
}

fn truncate_str(s: &str, max: usize) -> String {
    if s.len() <= max {
        s.to_string()
    } else {
        // UTF-8 safe truncation: collect chars up to max-2, then add ".."
        let truncated: String = s
            .char_indices()
            .take_while(|(i, _)| *i < max.saturating_sub(2))
            .map(|(_, c)| c)
            .collect();
        format!("{}..", truncated)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_report(total_commands: usize, already_rtk: usize) -> DiscoverReport {
        DiscoverReport {
            sessions_scanned: 1,
            total_commands,
            already_rtk,
            since_days: 30,
            supported: vec![],
            unsupported: vec![],
            parse_errors: 0,
            rtk_disabled_count: 0,
            rtk_disabled_examples: vec![],
        }
    }

    // B6 regression: integer division truncated small percentages to 0%.
    // Example: 3/1000 = 0% (old bug), should be "0.3%".
    #[test]
    fn test_already_rtk_percent_shows_decimal() {
        let report = make_report(1000, 3);
        let output = format_text(&report, 10, false);
        // "0.3%" must appear; old code would print "0%"
        assert!(
            output.contains("0.3%"),
            "Expected '0.3%' in output but got:\n{}",
            output
        );
        assert!(
            !output.contains("(0%)"),
            "Output must not contain '(0%)' — integer division bug still present:\n{}",
            output
        );
    }

    // Edge case: 0/0 must not divide-by-zero.
    #[test]
    fn test_already_rtk_percent_zero_total() {
        let report = make_report(0, 0);
        let output = format_text(&report, 10, false);
        assert!(output.contains("0 commands (0.0%)"));
    }

    // Full percent: 1000/1000 = 100.0%
    #[test]
    fn test_already_rtk_percent_full() {
        let report = make_report(1000, 1000);
        let output = format_text(&report, 10, false);
        assert!(output.contains("100.0%"));
    }
}
