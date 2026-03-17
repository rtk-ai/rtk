pub mod detector;
pub mod report;

use crate::discover::provider::{ClaudeProvider, SessionProvider};
use anyhow::Result;
use detector::{deduplicate_corrections, find_corrections, CommandExecution};
use report::{format_console_report, write_rules_file, Sanitizer};

pub fn run(
    project: Option<String>,
    all: bool,
    since: u64,
    format: String,
    write_rules: bool,
    min_confidence: f64,
    min_occurrences: usize,
    sanitize: bool,
) -> Result<()> {
    let provider = ClaudeProvider;

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

    let sessions = provider.discover_sessions(project_filter.as_deref(), Some(since))?;

    if sessions.is_empty() {
        println!("No Claude Code sessions found in the last {} days.", since);
        return Ok(());
    }

    let mut all_commands: Vec<CommandExecution> = Vec::new();

    for session_path in &sessions {
        let extracted = match provider.extract_commands(session_path) {
            Ok(cmds) => cmds,
            Err(_) => continue,
        };

        for ext_cmd in extracted {
            if let Some(output) = ext_cmd.output_content {
                all_commands.push(CommandExecution {
                    command: ext_cmd.command,
                    is_error: ext_cmd.is_error,
                    output,
                });
            }
        }
    }

    let corrections = find_corrections(&all_commands);

    if corrections.is_empty() {
        println!(
            "No CLI corrections detected in {} sessions.",
            sessions.len()
        );
        return Ok(());
    }

    let filtered: Vec<_> = corrections
        .into_iter()
        .filter(|c| c.confidence >= min_confidence)
        .collect();

    let mut rules = deduplicate_corrections(filtered.clone());
    rules.retain(|r| r.occurrences >= min_occurrences);

    let sanitizer = Sanitizer::new(sanitize);

    match format.as_str() {
        "json" => {
            let json = serde_json::json!({
                "sessions_scanned": sessions.len(),
                "total_corrections": filtered.len(),
                "rules": rules.iter().map(|r| {
                    serde_json::json!({
                        "wrong": sanitizer.sanitize(&r.wrong_pattern).as_ref(),
                        "right": sanitizer.sanitize(&r.right_pattern).as_ref(),
                        "error_type": r.error_type.as_str(),
                        "occurrences": r.occurrences,
                        "base_command": r.base_command,
                    })
                }).collect::<Vec<_>>(),
            });
            println!("{}", serde_json::to_string_pretty(&json)?);
        }
        _ => {
            let report =
                format_console_report(&rules, filtered.len(), sessions.len(), since, &sanitizer);
            print!("{}", report);

            if write_rules && !rules.is_empty() {
                let rules_path = ".claude/rules/cli-corrections.md";
                write_rules_file(&rules, rules_path, &sanitizer)?;
                println!("\nWritten to: {}", rules_path);
            }
        }
    }

    Ok(())
}
