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
    /// Commands that the Claude Code PreToolUse hook would rewrite — already handled (#1055).
    pub hook_rewritten_count: usize,
    /// Estimated tokens already saved by the hook (not double-counted as missed savings).
    pub hook_rewritten_tokens: usize,
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
        "Already using RTK: {} commands ({}%)\n",
        report.already_rtk,
        if report.total_commands > 0 {
            report.already_rtk * 100 / report.total_commands
        } else {
            0
        }
    ));

    // Show hook-rewritten commands separately so they don't pollute "missed savings" (#1055).
    if report.hook_rewritten_count > 0 {
        out.push_str(&format!(
            "Hook-handled:      {} commands — already rewritten by Claude Code hook (~{})\n",
            report.hook_rewritten_count,
            format_tokens(report.hook_rewritten_tokens),
        ));
    }

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

    fn empty_report() -> DiscoverReport {
        DiscoverReport {
            sessions_scanned: 1,
            total_commands: 100,
            already_rtk: 10,
            hook_rewritten_count: 0,
            hook_rewritten_tokens: 0,
            since_days: 7,
            supported: vec![],
            unsupported: vec![],
            parse_errors: 0,
            rtk_disabled_count: 0,
            rtk_disabled_examples: vec![],
        }
    }

    // Regression test for #1055: hook_rewritten_count is shown in report text when non-zero.
    #[test]
    fn test_hook_rewritten_count_shown_when_nonzero() {
        let mut report = empty_report();
        report.hook_rewritten_count = 42;
        report.hook_rewritten_tokens = 5000;

        let text = format_text(&report, 20, false);
        assert!(
            text.contains("Hook-handled"),
            "expected 'Hook-handled' line in report: {text}"
        );
        assert!(
            text.contains("42"),
            "expected hook count 42 in report: {text}"
        );
    }

    // When hook count is zero (no hook installed), the line must be absent.
    #[test]
    fn test_hook_rewritten_count_hidden_when_zero() {
        let report = empty_report();
        let text = format_text(&report, 20, false);
        assert!(
            !text.contains("Hook-handled"),
            "should not show Hook-handled when count is 0: {text}"
        );
    }
}
