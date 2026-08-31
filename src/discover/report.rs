//! Data types for reporting which commands RTK can and cannot optimize.

use crate::hooks::constants::{
    CLAUDE_DIR, COPILOT_HOOK_FILE, CURSOR_DIR, GITHUB_DIR, HERMES_DIR, HERMES_PLUGINS_SUBDIR,
    HERMES_PLUGIN_MANIFEST_FILE, HERMES_PLUGIN_NAME, HOOKS_SUBDIR, REWRITE_HOOK_FILE,
};
use serde::Serialize;
use std::path::Path;

/// RTK support status for a command.
#[derive(Debug, Serialize, Clone, Copy, PartialEq, Eq, Hash)]
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
    pub claude_hook_installed: bool,
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
            claude_hook_installed: crate::hooks::hook_check::claude_hook_installed_in(
                &home.join(CLAUDE_DIR),
            ),
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
    pub command_source: &'static str,
    pub hook_rewrites_visible: bool,
    pub sessions_scanned: usize,
    pub total_commands: usize,
    pub already_rtk: usize,
    pub since_days: u64,
    pub supported: Vec<SupportedEntry>,
    pub unsupported: Vec<UnsupportedEntry>,
    pub parse_errors: usize,
    pub rtk_disabled_count: usize,
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

    out.push_str("RTK Discover -- Rewrite Candidates\n");
    out.push_str(&"=".repeat(52));
    out.push('\n');
    out.push_str(&format!(
        "Scanned: {} sessions (last {} days), {} Bash commands\n",
        report.sessions_scanned, report.since_days, report.total_commands
    ));
    out.push_str(&format!(
        "Explicit RTK commands in transcript: {} ({:.1}%)\n",
        report.already_rtk,
        if report.total_commands > 0 {
            report.already_rtk as f64 * 100.0 / report.total_commands as f64
        } else {
            0.0
        }
    ));
    out.push_str(
        "Note: Claude transcripts store the original pre-hook command. Candidate counts are\n\
         not confirmed misses when an RTK hook is active; use `rtk gain` for executed usage.\n",
    );

    if report.supported.is_empty() && report.unsupported.is_empty() {
        out.push_str("\nNo rewrite candidates found in the selected transcripts.\n");
        append_agent_notes(&mut out, report.agent_status);
        return out;
    }

    // Rewrite candidates. Transcripts do not expose whether the hook rewrote
    // these commands before execution.
    if !report.supported.is_empty() {
        out.push_str("\nREWRITE CANDIDATES -- Commands RTK already handles\n");
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
pub fn format_json(report: &DiscoverReport, limit: usize) -> String {
    let supported = report.supported.iter().take(limit).collect::<Vec<_>>();
    let unsupported = report.unsupported.iter().take(limit).collect::<Vec<_>>();
    serde_json::to_string_pretty(&serde_json::json!({
        "command_source": report.command_source,
        "hook_rewrites_visible": report.hook_rewrites_visible,
        "transcript_notice": "Candidate counts are not confirmed misses because Claude transcripts store pre-hook commands. Use rtk gain for executed RTK usage.",
        "sessions_scanned": report.sessions_scanned,
        "total_commands": report.total_commands,
        "explicit_rtk_commands": report.already_rtk,
        "since_days": report.since_days,
        "supported": supported,
        "supported_returned": supported.len(),
        "supported_total": report.supported.len(),
        "unsupported": unsupported,
        "unsupported_returned": unsupported.len(),
        "unsupported_total": report.unsupported.len(),
        "parse_errors": report.parse_errors,
        "rtk_disabled_count": report.rtk_disabled_count,
        "rtk_disabled_examples": report.rtk_disabled_examples,
        "agent_status": report.agent_status,
    }))
    .unwrap_or_else(|_| "{}".to_string())
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
            command_source: "claude_pre_hook_transcript",
            hook_rewrites_visible: false,
            sessions_scanned: 1,
            total_commands,
            already_rtk,
            since_days: 30,
            supported: vec![],
            unsupported: vec![],
            parse_errors: 0,
            rtk_disabled_count: 0,
            rtk_disabled_examples: vec![],
            agent_status: AgentIntegrationStatus::default(),
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
        assert!(output.contains("Explicit RTK commands in transcript: 0 (0.0%)"));
    }

    // Full percent: 1000/1000 = 100.0%
    #[test]
    fn test_already_rtk_percent_full() {
        let report = make_report(1000, 1000);
        let output = format_text(&report, 10, false);
        assert!(output.contains("100.0%"));
    }

    #[test]
    fn format_text_explains_pre_hook_transcript_semantics() {
        let report = make_report(10, 0);
        let output = format_text(&report, 10, false);
        assert!(output.contains("pre-hook command"));
        assert!(output.contains("not confirmed misses"));
        assert!(output.contains("`rtk gain`"));
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
    fn test_agent_status_detects_claude_binary_hook() {
        let temp_home = tempfile::tempdir().unwrap();
        let claude_dir = temp_home.path().join(CLAUDE_DIR);
        std::fs::create_dir_all(&claude_dir).unwrap();
        std::fs::write(
            claude_dir.join(crate::hooks::constants::SETTINGS_JSON),
            r#"{"hooks":{"PreToolUse":[{"hooks":[{"command":"rtk hook claude"}]}]}}"#,
        )
        .unwrap();

        let status = AgentIntegrationStatus::detect_from_home(temp_home.path());
        assert!(status.claude_hook_installed);
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
            claude_hook_installed: true,
            cursor_hook_installed: true,
            hermes_plugin_installed: true,
            copilot_hook_installed: true,
        };

        let output = format_json(&report, 10);
        let json: serde_json::Value = serde_json::from_str(&output).unwrap();

        assert_eq!(json["agent_status"]["claude_hook_installed"], true);
        assert_eq!(json["agent_status"]["cursor_hook_installed"], true);
        assert_eq!(json["agent_status"]["hermes_plugin_installed"], true);
        assert_eq!(json["agent_status"]["copilot_hook_installed"], true);
        assert_eq!(json["hook_rewrites_visible"], false);
        assert!(json["transcript_notice"]
            .as_str()
            .is_some_and(|notice| notice.contains("not confirmed misses")));
    }

    #[test]
    fn test_format_json_honors_entry_limit_without_losing_totals() {
        let mut report = make_report(10, 0);
        report.unsupported = (0..5)
            .map(|index| UnsupportedEntry {
                base_command: format!("cmd-{index}"),
                count: index + 1,
                example: format!("cmd-{index} --example"),
            })
            .collect();

        let output = format_json(&report, 2);
        let json: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(json["unsupported"].as_array().unwrap().len(), 2);
        assert_eq!(json["unsupported_returned"], 2);
        assert_eq!(json["unsupported_total"], 5);
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
