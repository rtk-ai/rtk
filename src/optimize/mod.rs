//! Personalized optimization engine: analyzes session history and generates suggestions.

pub mod applier;
pub mod config_tuner;
pub mod corrections;
pub mod report;
pub mod suggestions;
pub mod toml_generator;
pub mod uncovered;

use anyhow::{Context, Result};

use crate::core::config::Config;
use crate::core::tracking::Tracker;
use crate::discover::provider::{ClaudeProvider, ExtractedCommand, SessionProvider};
use crate::discover::registry::{classify_command, split_command_chain, Classification};
use crate::learn::detector::CommandExecution;
use suggestions::{OptimizeReport, Suggestion};

/// Main entry point for `rtk optimize`.
#[allow(clippy::too_many_arguments)]
pub fn run(
    project: Option<String>,
    all: bool,
    since: u64,
    sessions_limit: usize,
    format: String,
    apply: bool,
    dry_run: bool,
    min_frequency: usize,
    min_savings: f64,
    verbose: u8,
) -> Result<i32> {
    let provider = ClaudeProvider;

    // Resolve project filter (same pattern as discover/learn)
    let project_filter = if all {
        None
    } else if let Some(p) = project {
        Some(p)
    } else {
        let cwd = std::env::current_dir()?;
        let cwd_str = cwd.to_string_lossy().to_string();
        let encoded = ClaudeProvider::encode_project_path(&cwd_str);
        Some(encoded)
    };

    // Discover sessions
    let mut sessions = provider
        .discover_sessions(project_filter.as_deref(), Some(since))
        .context("Failed to discover sessions")?;

    if sessions.is_empty() {
        println!("No Claude Code sessions found in the last {} days.", since);
        return Ok(0);
    }

    // Cap sessions
    sessions.truncate(sessions_limit);

    if verbose > 0 {
        eprintln!("rtk optimize: analyzing {} sessions...", sessions.len());
    }

    // Extract commands from all sessions
    let mut all_extracted: Vec<ExtractedCommand> = Vec::new();
    let mut all_executions: Vec<CommandExecution> = Vec::new();

    for session_path in &sessions {
        let extracted = match provider.extract_commands(session_path) {
            Ok(cmds) => cmds,
            Err(_) => continue,
        };

        for cmd in extracted {
            // Build CommandExecution for corrections analyzer
            if let Some(ref output) = cmd.output_content {
                all_executions.push(CommandExecution {
                    command: cmd.command.clone(),
                    is_error: cmd.is_error,
                    output: output.clone(),
                });
            }
            all_extracted.push(cmd);
        }
    }

    let commands_analyzed = all_extracted.len();

    if verbose > 0 {
        eprintln!(
            "rtk optimize: {} commands extracted from {} sessions",
            commands_analyzed,
            sessions.len()
        );
    }

    // Run analyzers
    let mut all_suggestions: Vec<Suggestion> = Vec::new();

    // 1. Uncovered command analyzer
    let uncovered_suggestions =
        uncovered::analyze_uncovered(&all_extracted, min_frequency, min_savings, since);
    if verbose > 0 {
        eprintln!(
            "rtk optimize: {} uncovered command suggestions",
            uncovered_suggestions.len()
        );
    }
    all_suggestions.extend(uncovered_suggestions);

    // 2. Config tuner (optional — needs tracker)
    if let Ok(tracker) = Tracker::new() {
        let config = Config::load().unwrap_or_default();
        if let Ok(config_suggestions) = config_tuner::analyze_config(&tracker, &config) {
            if verbose > 0 {
                eprintln!(
                    "rtk optimize: {} config suggestions",
                    config_suggestions.len()
                );
            }
            all_suggestions.extend(config_suggestions);
        }
    }

    // 3. Corrections analyzer
    let correction_suggestions = corrections::analyze_corrections(&all_executions, 0.6, 1);
    if verbose > 0 {
        eprintln!(
            "rtk optimize: {} correction suggestions",
            correction_suggestions.len()
        );
    }
    all_suggestions.extend(correction_suggestions);

    // Sort by impact descending
    all_suggestions.sort_by(|a, b| b.impact_score.cmp(&a.impact_score));

    // Compute coverage
    let (current_coverage, projected_coverage) = compute_coverage(&all_extracted, &all_suggestions);

    // Build report
    let total_monthly_savings: u64 = all_suggestions
        .iter()
        .map(|s| s.estimated_tokens_saved)
        .sum();

    let report = OptimizeReport {
        sessions_analyzed: sessions.len(),
        commands_analyzed,
        days_covered: since,
        suggestions: all_suggestions,
        total_estimated_monthly_savings: total_monthly_savings,
        current_coverage_pct: current_coverage,
        projected_coverage_pct: projected_coverage,
    };

    // Handle --apply and --dry-run
    if apply {
        let results = applier::apply_all(&report.suggestions)?;
        print!("{}", applier::format_applied(&results));
        return Ok(0);
    }

    if dry_run {
        print!("{}", applier::format_dry_run(&report.suggestions));
        return Ok(0);
    }

    // Output report
    match format.as_str() {
        "json" => {
            let json = report::format_json(&report)?;
            println!("{}", json);
        }
        _ => {
            print!("{}", report::format_text(&report));
        }
    }

    Ok(0)
}

/// Compute current and projected RTK coverage percentages.
fn compute_coverage(commands: &[ExtractedCommand], suggestions: &[Suggestion]) -> (f64, f64) {
    let mut total = 0usize;
    let mut covered = 0usize;

    for cmd in commands {
        let parts = split_command_chain(&cmd.command);
        for part in parts {
            let classification = classify_command(part);
            total += 1;
            if matches!(classification, Classification::Supported { .. }) {
                covered += 1;
            }
        }
    }

    if total == 0 {
        return (100.0, 100.0);
    }

    let current = covered as f64 / total as f64 * 100.0;

    // Projected: count how many uncovered commands would be covered by new TOML filters
    let new_filter_count = suggestions
        .iter()
        .filter(|s| {
            matches!(
                s.kind,
                suggestions::SuggestionKind::GenerateTomlFilter { .. }
            )
        })
        .count();

    let uncovered = total - covered;
    let newly_covered = new_filter_count.min(uncovered);
    let projected = (covered + newly_covered) as f64 / total as f64 * 100.0;

    (current, projected)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_compute_coverage_empty() {
        let (current, projected) = compute_coverage(&[], &[]);
        assert_eq!(current, 100.0);
        assert_eq!(projected, 100.0);
    }

    #[test]
    fn test_compute_coverage_all_supported() {
        let commands = vec![ExtractedCommand {
            command: "git status".to_string(),
            output_len: Some(100),
            session_id: "test".to_string(),
            output_content: None,
            is_error: false,
            sequence_index: 0,
        }];
        let (current, _projected) = compute_coverage(&commands, &[]);
        assert!(current > 50.0); // git status is supported
    }
}
