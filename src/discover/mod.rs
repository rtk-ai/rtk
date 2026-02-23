pub mod provider;
pub mod registry;
mod report;

use anyhow::Result;
use std::collections::HashMap;

use crate::platform::{detect_platform, AgentPlatform};
use provider::{ClaudeProvider, OpenCodeProvider, SessionProvider};
use registry::{category_avg_tokens, classify_command, split_command_chain, Classification};
use report::{DiscoverReport, SupportedEntry, UnsupportedEntry};

/// Aggregation bucket for supported commands.
struct SupportedBucket {
    rtk_equivalent: &'static str,
    category: &'static str,
    count: usize,
    total_output_tokens: usize,
    savings_pct: f64,
    // For display: the most common raw command
    command_counts: HashMap<String, usize>,
}

/// Aggregation bucket for unsupported commands.
struct UnsupportedBucket {
    count: usize,
    example: String,
}

pub fn run(
    project: Option<&str>,
    all: bool,
    since_days: u64,
    limit: usize,
    format: &str,
    platform_filter: &str,
    verbose: u8,
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

    // Aggregate results across all platforms
    let mut all_supported_buckets: HashMap<&'static str, SupportedBucket> = HashMap::new();
    let mut all_unsupported_buckets: HashMap<String, UnsupportedBucket> = HashMap::new();
    let mut total_sessions = 0;
    let mut total_commands = 0;
    let mut total_rtk_commands = 0;

    for platform in platforms {
        if let Err(e) = scan_platform(
            platform,
            project,
            all,
            since_days,
            &mut all_supported_buckets,
            &mut all_unsupported_buckets,
            &mut total_sessions,
            &mut total_commands,
            &mut total_rtk_commands,
        ) {
            if verbose > 0 {
                eprintln!("Warning: Failed to scan {}: {}", platform.name(), e);
            }
            // Continue scanning other platforms
        }
    }

    // Generate report from aggregated results
    generate_report(
        all_supported_buckets,
        all_unsupported_buckets,
        total_sessions,
        total_commands,
        total_rtk_commands,
        limit,
        format,
    )
}

fn scan_platform(
    platform: AgentPlatform,
    project: Option<&str>,
    all: bool,
    since_days: u64,
    supported_buckets: &mut HashMap<&'static str, SupportedBucket>,
    unsupported_buckets: &mut HashMap<String, UnsupportedBucket>,
    total_sessions: &mut usize,
    total_commands: &mut usize,
    total_rtk_commands: &mut usize,
) -> Result<()> {
    // Create the appropriate provider
    let provider: Box<dyn SessionProvider> = match platform {
        AgentPlatform::ClaudeCode => Box::new(ClaudeProvider),
        AgentPlatform::OpenCode => Box::new(OpenCodeProvider),
    };

    // Determine project filter
    let project_filter = if all {
        None
    } else if let Some(p) = project {
        Some(p.to_string())
    } else {
        // Default: current working directory
        let cwd = std::env::current_dir()?;
        let cwd_str = cwd.to_string_lossy().to_string();
        match platform {
            // Claude Code encodes paths: /Users/foo/bar → -Users-foo-bar
            AgentPlatform::ClaudeCode => Some(ClaudeProvider::encode_project_path(&cwd_str)),
            // OpenCode uses raw directory paths in session.directory
            AgentPlatform::OpenCode => Some(cwd_str),
        }
    };

    let sessions = provider.discover_sessions(project_filter.as_deref(), Some(since_days))?;
    *total_sessions += sessions.len();

    for session_path in &sessions {
        let extracted = match provider.extract_commands(session_path) {
            Ok(cmds) => cmds,
            Err(_e) => {
                // Skip this session on error, continue with others
                continue;
            }
        };

        for ext_cmd in &extracted {
            let parts = split_command_chain(&ext_cmd.command);
            for part in parts {
                *total_commands += 1;

                match classify_command(part) {
                    Classification::Supported {
                        rtk_equivalent,
                        category,
                        estimated_savings_pct,
                        status,
                    } => {
                        let bucket = supported_buckets.entry(rtk_equivalent).or_insert_with(|| {
                            SupportedBucket {
                                rtk_equivalent,
                                category,
                                count: 0,
                                total_output_tokens: 0,
                                savings_pct: estimated_savings_pct,
                                command_counts: HashMap::new(),
                            }
                        });

                        bucket.count += 1;

                        // Estimate tokens for this command
                        let output_tokens = if let Some(len) = ext_cmd.output_len {
                            // Real: from tool_result content length
                            len / 4
                        } else {
                            // Fallback: category average
                            let subcmd = extract_subcmd(part);
                            category_avg_tokens(category, subcmd)
                        };

                        let savings =
                            (output_tokens as f64 * estimated_savings_pct / 100.0) as usize;
                        bucket.total_output_tokens += savings;

                        // Track the display name with status
                        let display_name = truncate_command(part);
                        let entry = bucket
                            .command_counts
                            .entry(format!("{}:{:?}", display_name, status))
                            .or_insert(0);
                        *entry += 1;
                    }
                    Classification::Unsupported { base_command } => {
                        let bucket = unsupported_buckets.entry(base_command).or_insert_with(|| {
                            UnsupportedBucket {
                                count: 0,
                                example: part.to_string(),
                            }
                        });
                        bucket.count += 1;
                    }
                    Classification::Ignored => {
                        // Check if it starts with "rtk "
                        if part.trim().starts_with("rtk ") {
                            *total_rtk_commands += 1;
                        }
                        // Otherwise just skip
                    }
                }
            }
        }
    }

    Ok(())
}

fn generate_report(
    supported_map: HashMap<&'static str, SupportedBucket>,
    unsupported_map: HashMap<String, UnsupportedBucket>,
    total_sessions: usize,
    total_commands: usize,
    already_rtk: usize,
    limit: usize,
    format: &str,
) -> Result<()> {
    // Build report
    let mut supported: Vec<SupportedEntry> = supported_map
        .into_values()
        .map(|bucket| {
            // Pick the most common command as the display name
            let (command_with_status, status) = bucket
                .command_counts
                .into_iter()
                .max_by_key(|(_, c)| *c)
                .map(|(name, _)| {
                    // Extract status from "command:Status" format
                    if let Some(colon_pos) = name.rfind(':') {
                        let cmd = name[..colon_pos].to_string();
                        let status_str = &name[colon_pos + 1..];
                        let status = match status_str {
                            "Passthrough" => report::RtkStatus::Passthrough,
                            "NotSupported" => report::RtkStatus::NotSupported,
                            _ => report::RtkStatus::Existing,
                        };
                        (cmd, status)
                    } else {
                        (name, report::RtkStatus::Existing)
                    }
                })
                .unwrap_or_else(|| (String::new(), report::RtkStatus::Existing));

            SupportedEntry {
                command: command_with_status,
                count: bucket.count,
                rtk_equivalent: bucket.rtk_equivalent,
                category: bucket.category,
                estimated_savings_tokens: bucket.total_output_tokens,
                estimated_savings_pct: bucket.savings_pct,
                rtk_status: status,
            }
        })
        .collect();

    // Sort by estimated savings descending
    supported.sort_by(|a, b| b.estimated_savings_tokens.cmp(&a.estimated_savings_tokens));

    let mut unsupported: Vec<UnsupportedEntry> = unsupported_map
        .into_iter()
        .map(|(base, bucket)| UnsupportedEntry {
            base_command: base,
            count: bucket.count,
            example: bucket.example,
        })
        .collect();

    // Sort by count descending
    unsupported.sort_by(|a, b| b.count.cmp(&a.count));

    let report = DiscoverReport {
        sessions_scanned: total_sessions,
        total_commands,
        already_rtk,
        since_days: 0, // We don't track this in aggregated results
        supported,
        unsupported,
        parse_errors: 0, // We don't track this in aggregated results
    };

    match format {
        "json" => println!("{}", report::format_json(&report)),
        _ => print!("{}", report::format_text(&report, limit, false)),
    }

    Ok(())
}

/// Extract the subcommand from a command string (second word).
fn extract_subcmd(cmd: &str) -> &str {
    let parts: Vec<&str> = cmd.trim().splitn(3, char::is_whitespace).collect();
    if parts.len() >= 2 {
        parts[1]
    } else {
        ""
    }
}

/// Truncate a command for display (keep first meaningful portion).
fn truncate_command(cmd: &str) -> String {
    let trimmed = cmd.trim();
    // Keep first two words for display
    let parts: Vec<&str> = trimmed.splitn(3, char::is_whitespace).collect();
    match parts.len() {
        0 => String::new(),
        1 => parts[0].to_string(),
        _ => format!("{} {}", parts[0], parts[1]),
    }
}
