pub mod detector;
pub mod report;

use crate::discover::provider::{ClaudeProvider, SessionProvider};
use anyhow::Result;
use detector::{deduplicate_corrections, find_corrections, CommandExecution};
use report::{format_console_report, write_rules_file};

pub fn run(
    project: Option<String>,
    all: bool,
    since: u64,
    format: String,
    write_rules: bool,
    min_confidence: f64,
    min_occurrences: usize,
) -> Result<()> {
    let provider = ClaudeProvider;

    // Determine project filter (same logic as discover)
    let project_filter = if all {
        None
    } else if let Some(p) = project {
        Some(p)
    } else {
        // Default: current working directory
        let cwd = std::env::current_dir()?;
        let cwd_str = cwd.to_string_lossy().to_string();
        let encoded = ClaudeProvider::encode_project_path(&cwd_str);
        Some(encoded)
    };

    // Discover sessions
    let sessions = provider.discover_sessions(project_filter.as_deref(), Some(since))?;

    if sessions.is_empty() {
        println!("No Claude Code sessions found in the last {} days.", since);
        return Ok(());
    }

    // Extract commands from all sessions
    let mut all_commands: Vec<CommandExecution> = Vec::new();

    for session_path in &sessions {
        let extracted = match provider.extract_commands(session_path) {
            Ok(cmds) => cmds,
            Err(_) => continue, // Skip malformed sessions
        };

        for ext_cmd in extracted {
            // Only process commands with output content
            if let Some(output) = ext_cmd.output_content {
                all_commands.push(CommandExecution {
                    command: ext_cmd.command,
                    is_error: ext_cmd.is_error,
                    output,
                });
            }
        }
    }

    // Sort by sequence index to maintain chronological order
    // (already sorted by extraction order within each session)

    // Find corrections
    let corrections = find_corrections(&all_commands);

    if corrections.is_empty() {
        println!(
            "No CLI corrections detected in {} sessions.",
            sessions.len()
        );
        return Ok(());
    }

    // Filter by confidence
    let filtered: Vec<_> = corrections
        .into_iter()
        .filter(|c| c.confidence >= min_confidence)
        .collect();

    // Deduplicate
    let mut rules = deduplicate_corrections(filtered.clone());

    // Filter by occurrences
    rules.retain(|r| r.occurrences >= min_occurrences);

    // Output
    match format.as_str() {
        "json" => {
            // JSON output
            let json = serde_json::json!({
                "sessions_scanned": sessions.len(),
                "total_corrections": filtered.len(),
                "rules": rules.iter().map(|r| serde_json::json!({
                    "wrong": r.wrong_pattern,
                    "right": r.right_pattern,
                    "error_type": r.error_type.as_str(),
                    "occurrences": r.occurrences,
                    "base_command": r.base_command,
                })).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        _ => {
            // Text output
            let report = format_console_report(&rules, filtered.len(), sessions.len(), since);
            print!("{}", report);

            if write_rules && !rules.is_empty() {
                let rules_path = ".claude/rules/cli-corrections.md";
                write_rules_file(&rules, rules_path)?;
                println!("\nWritten to: {}", rules_path);
            }
        }
    }

    Ok(())
}
