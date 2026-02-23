pub mod detector;
pub mod report;

use crate::discover::provider::{ClaudeProvider, OpenCodeProvider, SessionProvider};
use crate::platform::{detect_platform, AgentPlatform};
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
    platform_filter: &str,
) -> Result<()> {
    // Determine which platforms to scan
    let platforms = match platform_filter {
        "claude" => vec![AgentPlatform::ClaudeCode],
        "opencode" => vec![AgentPlatform::OpenCode],
        "both" => vec![AgentPlatform::ClaudeCode, AgentPlatform::OpenCode],
        other => {
            anyhow::bail!(
                "Invalid platform filter '{}'. Use: claude, opencode, or both",
                other
            );
        }
    };

    // Aggregate commands across all platforms
    let mut all_commands: Vec<CommandExecution> = Vec::new();
    let mut total_sessions = 0;

    for platform in platforms {
        if let Err(_e) = scan_platform_learn(
            platform,
            project.as_ref(),
            all,
            since,
            &mut all_commands,
            &mut total_sessions,
        ) {
            // Continue scanning other platforms
            continue;
        }
    }

    if total_sessions == 0 {
        println!("No sessions found in the last {} days.", since);
        return Ok(());
    }

    // Find corrections
    let corrections = find_corrections(&all_commands);

    if corrections.is_empty() {
        println!(
            "No CLI corrections detected in {} sessions.",
            total_sessions
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
                "sessions_scanned": total_sessions,
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
            let report = format_console_report(&rules, filtered.len(), total_sessions, since);
            print!("{}", report);

            if write_rules && !rules.is_empty() {
                // Write to both platforms if scanning both
                let platforms_to_write = match platform_filter {
                    "claude" => vec![AgentPlatform::ClaudeCode],
                    "opencode" => vec![AgentPlatform::OpenCode],
                    "both" => vec![AgentPlatform::ClaudeCode, AgentPlatform::OpenCode],
                    _ => vec![],
                };

                for platform in platforms_to_write {
                    let rules_path = match platform {
                        AgentPlatform::ClaudeCode => ".claude/rules/cli-corrections.md",
                        AgentPlatform::OpenCode => ".opencode/rules/cli-corrections.md",
                    };
                    if let Ok(()) = write_rules_file(&rules, rules_path) {
                        println!("\nWritten to: {}", rules_path);
                    }
                }
            }
        }
    }

    Ok(())
}

fn scan_platform_learn(
    platform: AgentPlatform,
    project: Option<&String>,
    all: bool,
    since: u64,
    all_commands: &mut Vec<CommandExecution>,
    total_sessions: &mut usize,
) -> Result<()> {
    // Create the appropriate provider
    let provider: Box<dyn SessionProvider> = match platform {
        AgentPlatform::ClaudeCode => Box::new(ClaudeProvider),
        AgentPlatform::OpenCode => Box::new(OpenCodeProvider),
    };

    // Determine project filter (same logic as discover)
    let project_filter = if all {
        None
    } else if let Some(p) = project {
        Some(p.clone())
    } else {
        // Default: current working directory
        let cwd = std::env::current_dir()?;
        let cwd_str = cwd.to_string_lossy().to_string();
        match platform {
            AgentPlatform::ClaudeCode => Some(ClaudeProvider::encode_project_path(&cwd_str)),
            AgentPlatform::OpenCode => Some(cwd_str),
        }
    };

    // Discover sessions
    let sessions = provider.discover_sessions(project_filter.as_deref(), Some(since))?;
    *total_sessions += sessions.len();

    // Extract commands from all sessions
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

    Ok(())
}
