//! Data types for reporting which commands RTK can and cannot optimize.

use crate::hooks::constants::{
    COPILOT_HOOK_FILE, CURSOR_DIR, GITHUB_DIR, HERMES_DIR, HERMES_PLUGIN_MANIFEST_FILE,
    HERMES_PLUGIN_NAME, HERMES_PLUGINS_SUBDIR, HOOKS_SUBDIR, REWRITE_HOOK_FILE,
};
use serde::Serialize;
use std::path::Path;

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

#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Default)]
pub struct AgentIntegrationStatus {
    pub cursor_hook_installed: bool,
    pub hermes_plugin_installed: bool,
    pub copilot_hook_installed: bool,
}

impl AgentIntegrationStatus {
    pub fn detect() -> Self {
        let mut status = dirs::home_dir()
            .map(|home| Self::detect_from_home(&home))
            .unwrap_or_default();
        // Copilot is project-scoped (.github/hooks/), unlike the home-based agents.
        status.copilot_hook_installed = std::env::current_dir()
            .map(|cwd| Self::copilot_hook_installed_in(&cwd))
            .unwrap_or(false);
        status
    }

    fn detect_from_home(home: &Path) -> Self {
        Self {
            cursor_hook_installed: home
                .join(CURSOR_DIR)
                .join(HOOKS_SUBDIR)
                .join(REWRITE_HOOK_FILE)
                .exists(),
            hermes_plugin_installed: home
                .join(HERMES_DIR)
                .join(HERMES_PLUGINS_SUBDIR)
                .join(HERMES_PLUGIN_NAME)
                .join(HERMES_PLUGIN_MANIFEST_FILE)
                .is_file(),
            copilot_hook_installed: false,
        }
    }

    fn copilot_hook_installed_in(dir: &Path) -> bool {
        dir.join(GITHUB_DIR)
            .join(HOOKS_SUBDIR)
            .join(COPILOT_HOOK_FILE)
            .exists()
    }
}

/// Full discover report.
#[derive(Debug, Serialize)]
pub struct DiscoverReport {
    pub sessions_scanned: usize,
    pub total_commands: usize,
    pub already_rtk: usize,
    /// Subset of `already_rtk` that came from the current-state heuristic fallback
    /// rather than a measured `hook_decisions` log entry — i.e. history that
    /// predates hook-decision logging (or isn't Claude Code).
    pub already_rtk_estimated: usize,
    /// Date (`YYYY-MM-DD`) of the *oldest currently-retained* `hook_decisions` log
    /// entry, if any exist. `None` means no measured data at all — every coverage
    /// number in this report is an estimate.
    ///
    /// NOT an install date: `hook_decisions` rows are pruned by the same
    /// `DEFAULT_HISTORY_DAYS` retention window as the rest of the tracking DB
    /// (`Tracker::cleanup_hook_decisions`), so this date is a rolling boundary —
    /// on a machine with months of real usage it still reads as "~90 days ago",
    /// not the date logging actually began.
    pub measured_since: Option<String>,
    pub since_days: u64,
    pub supported: Vec<SupportedEntry>,
    pub unsupported: Vec<UnsupportedEntry>,
    pub parse_errors: usize,
    pub rtk_disabled_count: usize,
    /// Always equal to `rtk_disabled_count`: unlike `already_rtk_estimated`, this can
    /// never be a *strict* subset. An `RTK_DISABLED=` bypass makes the hook's own
    /// logged decision `Defer` unconditionally (see `registry.rs` #345 / `get_rewritten`),
    /// so a real `hook_decisions` row for a bypassed command never says "this would
    /// have been covered" — it only ever confirms the hook stepped aside, which is
    /// already known from the bypass itself. So `rtk_disabled_count` is always computed
    /// from the current-state estimate, never the measured log, and this field mirrors
    /// it 1:1. Kept as its own field (rather than folded away) so the report's JSON/text
    /// output keeps disclosing "this bucket is an estimate" the same way
    /// `already_rtk_estimated` does, instead of silently going quiet about it.
    pub rtk_disabled_estimated: usize,
    pub rtk_disabled_examples: Vec<String>,
    pub agent_status: AgentIntegrationStatus,
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
    if report.already_rtk_estimated > 0 {
        match &report.measured_since {
            Some(date) => out.push_str(&format!(
                "  includes ~{} estimated from current hook/config state (measured data covers {date} onward; older history is estimated and may not reflect what was actually installed at the time)\n",
                report.already_rtk_estimated
            )),
            None => out.push_str(&format!(
                "  all {} estimated from current hook/config state -- no hook-decision log yet, coverage may not reflect historical reality\n",
                report.already_rtk_estimated
            )),
        }
    }

    // The RTK_DISABLED bypass section below is unconditional on `rtk_disabled_count`
    // alone (it doesn't touch `supported`/`unsupported`), so this early return must
    // also check it — otherwise a user whose *every* RTK-supported command ran as
    // `RTK_DISABLED=1 <cmd>` never populates `supported`/`unsupported` at all, and
    // this would print "RTK usage looks good!" while hiding that 100% of their
    // commands ran unfiltered.
    if report.supported.is_empty()
        && report.unsupported.is_empty()
        && report.rtk_disabled_count == 0
    {
        out.push_str("\nNo missed savings found. RTK usage looks good!\n");
        append_agent_notes(&mut out, report.agent_status);
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
        if report.rtk_disabled_estimated > 0 {
            out.push_str(&format!(
                "  includes ~{} estimated from current hook/config state\n",
                report.rtk_disabled_estimated
            ));
        }
        out.push_str("-> Remove RTK_DISABLED=1 to recover token savings\n");
    }

    out.push_str("\n~estimated from tool_result output sizes\n");

    append_agent_notes(&mut out, report.agent_status);

    if verbose && report.parse_errors > 0 {
        out.push_str(&format!("Parse errors skipped: {}\n", report.parse_errors));
    }

    out
}

fn append_agent_notes(out: &mut String, status: AgentIntegrationStatus) {
    if status.cursor_hook_installed {
        out.push_str("\nNote: Cursor sessions are tracked via `rtk gain` (discover scans Claude Code only)\n");
    }

    if status.hermes_plugin_installed {
        out.push_str("\nNote: Hermes plugin is installed; Hermes sessions are tracked via `rtk gain` (discover scans Claude Code only)\n");
    }

    if status.copilot_hook_installed {
        out.push_str("\nNote: GitHub Copilot sessions are tracked via `rtk gain` (discover scans Claude Code only)\n");
    }
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
            already_rtk_estimated: 0,
            measured_since: None,
            since_days: 30,
            supported: vec![],
            unsupported: vec![],
            parse_errors: 0,
            rtk_disabled_count: 0,
            rtk_disabled_estimated: 0,
            rtk_disabled_examples: vec![],
            agent_status: AgentIntegrationStatus::default(),
        }
    }

    #[test]
    fn test_format_text_omits_estimate_caveat_when_fully_measured() {
        let report = make_report(100, 10);
        let output = format_text(&report, 10, false);
        assert!(!output.contains("estimated from current hook/config state"));
    }

    #[test]
    fn test_format_text_shows_estimate_caveat_with_measured_boundary() {
        let mut report = make_report(100, 10);
        report.already_rtk_estimated = 4;
        report.measured_since = Some("2026-07-20".to_string());

        let output = format_text(&report, 10, false);
        assert!(output.contains("includes ~4 estimated from current hook/config state"));
        assert!(output.contains("measured data covers 2026-07-20 onward"));
    }

    #[test]
    fn test_format_text_shows_fully_estimated_caveat_when_no_log_yet() {
        let mut report = make_report(100, 10);
        report.already_rtk_estimated = 10;
        report.measured_since = None;

        let output = format_text(&report, 10, false);
        assert!(output.contains("all 10 estimated from current hook/config state"));
        assert!(output.contains("no hook-decision log yet"));
    }

    fn dummy_supported_entry() -> SupportedEntry {
        SupportedEntry {
            command: "git status".to_string(),
            count: 1,
            rtk_equivalent: "rtk git",
            category: "Git",
            estimated_savings_tokens: 10,
            estimated_savings_pct: 50.0,
            rtk_status: RtkStatus::Existing,
        }
    }

    #[test]
    fn test_format_text_shows_rtk_disabled_estimate_caveat() {
        let mut report = make_report(100, 10);
        report.supported = vec![dummy_supported_entry()];
        report.rtk_disabled_count = 3;
        report.rtk_disabled_estimated = 2;

        let output = format_text(&report, 10, false);
        assert!(output.contains("RTK_DISABLED BYPASS -- 3 commands"));
        assert!(output.contains("includes ~2 estimated from current hook/config state"));
    }

    #[test]
    fn test_format_text_rtk_disabled_section_survives_empty_supported_and_unsupported() {
        // Regression: a user whose *every* RTK-supported command ran as
        // `RTK_DISABLED=1 <cmd>` never populates supported/unsupported at all — the
        // early "no missed savings" return must not fire just because those two are
        // empty when rtk_disabled_count says otherwise, or the report would falsely
        // claim "RTK usage looks good!" while hiding that every command ran
        // unfiltered.
        let mut report = make_report(100, 10);
        report.rtk_disabled_count = 5;
        report.rtk_disabled_estimated = 5;

        let output = format_text(&report, 10, false);
        assert!(
            !output.contains("No missed savings found"),
            "must not claim things look good when rtk_disabled_count > 0: {output}"
        );
        assert!(output.contains("RTK_DISABLED BYPASS -- 5 commands"));
    }

    #[test]
    fn test_format_text_omits_rtk_disabled_estimate_caveat_when_zero() {
        // `format_text` renders whatever it's given and doesn't enforce the
        // rtk_disabled_estimated == rtk_disabled_count invariant `discover::run`
        // now always produces (see the field doc on `DiscoverReport::
        // rtk_disabled_estimated`: a bypassed command's hook decision is always
        // `Defer`, so this bucket is never measured — `run` sets both counters
        // together, never a count > 0 / estimated == 0 combination). This test is
        // just exercising the renderer's zero-suppression on its own terms.
        let mut report = make_report(100, 10);
        report.supported = vec![dummy_supported_entry()];
        report.rtk_disabled_count = 0;
        report.rtk_disabled_estimated = 0;

        let output = format_text(&report, 10, false);
        assert!(!output.contains("RTK_DISABLED BYPASS"));
        assert!(!output.contains("includes ~0 estimated"));
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

    #[test]
    fn test_agent_status_detects_hermes_plugin_manifest() {
        let temp_home = tempfile::tempdir().unwrap();
        let manifest = temp_home
            .path()
            .join(HERMES_DIR)
            .join(HERMES_PLUGINS_SUBDIR)
            .join(HERMES_PLUGIN_NAME)
            .join(HERMES_PLUGIN_MANIFEST_FILE);
        std::fs::create_dir_all(manifest.parent().unwrap()).unwrap();
        std::fs::write(&manifest, "name: rtk-rewrite\n").unwrap();

        let status = AgentIntegrationStatus::detect_from_home(temp_home.path());

        assert!(status.hermes_plugin_installed);
        assert!(!status.cursor_hook_installed);
    }

    #[test]
    fn test_agent_status_ignores_hermes_plugin_dir_without_manifest() {
        let temp_home = tempfile::tempdir().unwrap();
        let plugin_dir = temp_home
            .path()
            .join(HERMES_DIR)
            .join(HERMES_PLUGINS_SUBDIR)
            .join(HERMES_PLUGIN_NAME);
        std::fs::create_dir_all(plugin_dir).unwrap();

        let status = AgentIntegrationStatus::detect_from_home(temp_home.path());

        assert!(!status.hermes_plugin_installed);
    }

    #[test]
    fn test_format_text_reports_hermes_plugin_detected() {
        let mut report = make_report(0, 0);
        report.agent_status = AgentIntegrationStatus {
            hermes_plugin_installed: true,
            ..AgentIntegrationStatus::default()
        };

        let output = format_text(&report, 10, false);

        assert!(
            output.contains("Hermes plugin is installed"),
            "Expected Hermes installed note in output but got:\n{}",
            output
        );
    }

    #[test]
    fn test_format_json_includes_agent_status() {
        let mut report = make_report(0, 0);
        report.agent_status = AgentIntegrationStatus {
            cursor_hook_installed: true,
            hermes_plugin_installed: true,
            copilot_hook_installed: true,
        };

        let output = format_json(&report);
        let json: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(json["agent_status"]["cursor_hook_installed"], true);
        assert_eq!(json["agent_status"]["hermes_plugin_installed"], true);
        assert_eq!(json["agent_status"]["copilot_hook_installed"], true);
    }

    #[test]
    fn test_agent_status_detects_copilot_hook_in_project() {
        let temp = tempfile::tempdir().unwrap();
        let hook = temp
            .path()
            .join(GITHUB_DIR)
            .join(HOOKS_SUBDIR)
            .join(COPILOT_HOOK_FILE);
        std::fs::create_dir_all(hook.parent().unwrap()).unwrap();
        std::fs::write(&hook, "{}").unwrap();

        assert!(AgentIntegrationStatus::copilot_hook_installed_in(
            temp.path()
        ));
        assert!(!AgentIntegrationStatus::copilot_hook_installed_in(
            tempfile::tempdir().unwrap().path()
        ));
    }

    #[test]
    fn test_format_text_reports_copilot_detected() {
        let mut report = make_report(0, 0);
        report.agent_status = AgentIntegrationStatus {
            copilot_hook_installed: true,
            ..AgentIntegrationStatus::default()
        };

        let output = format_text(&report, 10, false);

        assert!(
            output.contains("GitHub Copilot sessions are tracked via `rtk gain`"),
            "Expected Copilot note in output but got:\n{}",
            output
        );
    }
}
