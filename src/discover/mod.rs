//! Scans AI coding sessions to find commands that could benefit from RTK filtering.

pub mod lexer;
pub mod provider;
pub mod registry;
mod report;
pub mod rules;

use anyhow::Result;
use std::collections::HashMap;

use provider::{ClaudeProvider, SessionProvider};
use registry::{
    category_avg_tokens, classify_command, split_command_chain, strip_disabled_prefix,
    Classification,
};
use report::{DiscoverReport, SupportedEntry, UnsupportedEntry};

use crate::discover::registry::prefix_contains_rtk_disabled;

/// Aggregation bucket for supported commands.
struct SupportedBucket {
    rtk_equivalent: &'static str,
    category: &'static str,
    count: usize,
    /// Total estimated tokens *saved* (post-filter). Used for the "Est. Savings" column.
    total_output_tokens: usize,
    /// Total estimated tokens *before* filtering (raw output). Accumulated alongside
    /// `total_output_tokens` so the bucket's effective savings rate can be derived as
    /// `total_output_tokens / total_raw_output_tokens` — a weighted average across
    /// all sub-commands, regardless of which sub-command was seen first.
    total_raw_output_tokens: usize,
    // For display: the most common raw command
    command_counts: HashMap<String, usize>,
}

/// Aggregation bucket for unsupported commands.
struct UnsupportedBucket {
    count: usize,
    example: String,
}

/// Resolve the `-p`/`--project` value (or the default cwd) into the filter
/// string matched against Claude's sanitized project directory names.
///
/// A `-p` value containing a path separator is a filesystem path (e.g.
/// `/home/user/projects/my-app`), so it must go through the same
/// `encode_project_path` sanitization as the default cwd case -- Claude
/// stores transcripts under directory names with `/` (and other
/// non-alphanumeric characters) replaced by `-`, so an un-sanitized path
/// value can never substring-match the real directory name. A `-p` value
/// with no separator is treated as a plain name substring (e.g. `my-app`)
/// and passed through unchanged, since it already matches correctly today.
fn resolve_project_filter(project: Option<&str>, all: bool, cwd: &str) -> Option<String> {
    if all {
        return None;
    }
    match project {
        Some(p) if p.contains('/') || p.contains('\\') => {
            Some(ClaudeProvider::encode_project_path(p))
        }
        Some(p) => Some(p.to_string()),
        None => Some(ClaudeProvider::encode_project_path(cwd)),
    }
}

pub fn run(
    project: Option<&str>,
    all: bool,
    since_days: u64,
    limit: usize,
    format: &str,
    verbose: u8,
) -> Result<()> {
    let provider = ClaudeProvider;

    let cwd = std::env::current_dir()?;
    let cwd_str = cwd.to_string_lossy().to_string();
    let project_filter = resolve_project_filter(project, all, &cwd_str);

    let sessions = provider.discover_sessions(project_filter.as_deref(), Some(since_days))?;

    if verbose > 0 {
        eprintln!("Scanning {} session files...", sessions.len());
        for s in &sessions {
            eprintln!("  {}", s.display());
        }
    }

    let mut total_commands: usize = 0;
    let mut already_rtk: usize = 0;
    let mut parse_errors: usize = 0;
    let mut rtk_disabled_count: usize = 0;
    let mut rtk_disabled_cmds: HashMap<String, usize> = HashMap::new();
    let mut supported_map: HashMap<&'static str, SupportedBucket> = HashMap::new();
    let mut unsupported_map: HashMap<String, UnsupportedBucket> = HashMap::new();

    for session_path in &sessions {
        let extracted = match provider.extract_commands(session_path) {
            Ok(cmds) => cmds,
            Err(e) => {
                if verbose > 0 {
                    eprintln!("Warning: skipping {}: {}", session_path.display(), e);
                }
                parse_errors += 1;
                continue;
            }
        };

        for ext_cmd in &extracted {
            let parts = split_command_chain(&ext_cmd.command);
            for part in parts {
                total_commands += 1;

                // Detect RTK_DISABLED= bypass before classification
                let (env_prefix, actual_cmd) = strip_disabled_prefix(part);
                if prefix_contains_rtk_disabled(env_prefix) {
                    // Only count if the underlying command is one RTK supports
                    match classify_command(actual_cmd) {
                        Classification::Supported { .. } => {
                            rtk_disabled_count += 1;
                            let display = truncate_command(actual_cmd);
                            *rtk_disabled_cmds.entry(display).or_insert(0) += 1;
                        }
                        _ => {
                            // RTK_DISABLED on unsupported/ignored command — not interesting
                        }
                    }
                    continue;
                }

                match classify_command(part) {
                    Classification::Supported {
                        rtk_equivalent,
                        category,
                        estimated_savings_pct,
                        status,
                    } => {
                        let bucket = supported_map.entry(rtk_equivalent).or_insert_with(|| {
                            SupportedBucket {
                                rtk_equivalent,
                                category,
                                count: 0,
                                total_output_tokens: 0,
                                total_raw_output_tokens: 0,
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
                        // Accumulate pre-savings tokens so we can compute a weighted effective
                        // savings rate across all sub-commands in this bucket later.
                        bucket.total_raw_output_tokens += output_tokens;

                        // Track the display name with status
                        let display_name = truncate_command(part);
                        let entry = bucket
                            .command_counts
                            .entry(format!("{}:{:?}", display_name, status))
                            .or_insert(0);
                        *entry += 1;
                    }
                    Classification::Unsupported { base_command } => {
                        let bucket = unsupported_map.entry(base_command).or_insert_with(|| {
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
                            already_rtk += 1;
                        }
                        // Otherwise just skip
                    }
                }
            }
        }
    }

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

            // Derive the effective savings rate from accumulated totals rather than
            // using the first-seen sub-command's rate. This gives a weighted average
            // across all sub-commands that fell in this bucket.
            let effective_savings_pct = if bucket.total_raw_output_tokens > 0 {
                bucket.total_output_tokens as f64 * 100.0 / bucket.total_raw_output_tokens as f64
            } else {
                0.0
            };

            SupportedEntry {
                command: command_with_status,
                count: bucket.count,
                rtk_equivalent: bucket.rtk_equivalent,
                category: bucket.category,
                estimated_savings_tokens: bucket.total_output_tokens,
                estimated_savings_pct: effective_savings_pct,
                rtk_status: status,
            }
        })
        .collect();

    // Sort by estimated savings descending
    supported.sort_by_key(|b| std::cmp::Reverse(b.estimated_savings_tokens));

    let mut unsupported: Vec<UnsupportedEntry> = unsupported_map
        .into_iter()
        .map(|(base, bucket)| UnsupportedEntry {
            base_command: base,
            count: bucket.count,
            example: bucket.example,
        })
        .collect();

    // Sort by count descending
    unsupported.sort_by_key(|b| std::cmp::Reverse(b.count));

    // Build RTK_DISABLED examples sorted by frequency (top 5)
    let rtk_disabled_examples: Vec<String> = {
        let mut sorted: Vec<_> = rtk_disabled_cmds.into_iter().collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
        sorted
            .into_iter()
            .take(5)
            .map(|(cmd, count)| format!("{} ({}x)", cmd, count))
            .collect()
    };

    let report = DiscoverReport {
        sessions_scanned: sessions.len(),
        total_commands,
        already_rtk,
        since_days,
        supported,
        unsupported,
        parse_errors,
        rtk_disabled_count,
        rtk_disabled_examples,
        agent_status: report::AgentIntegrationStatus::detect(),
    };

    match format {
        "json" => println!("{}", report::format_json(&report)),
        _ => print!("{}", report::format_text(&report, limit, verbose > 0)),
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

#[cfg(test)]
mod resolve_project_filter_tests {
    use super::resolve_project_filter;

    #[test]
    fn all_flag_disables_filtering_regardless_of_project() {
        assert_eq!(
            resolve_project_filter(Some("my-app"), true, "/home/user/my-app"),
            None
        );
        assert_eq!(
            resolve_project_filter(None, true, "/home/user/my-app"),
            None
        );
    }

    #[test]
    fn absolute_unix_path_is_sanitized_like_the_cwd_default() {
        // Exact repro from the report: an absolute -p path must match the
        // same encoded form the default (cwd) branch already produces.
        let via_project_flag =
            resolve_project_filter(Some("/home/user/projects/my-app"), false, "/irrelevant");
        let via_cwd_default = resolve_project_filter(None, false, "/home/user/projects/my-app");
        assert_eq!(via_project_flag, via_cwd_default);
        assert_eq!(via_project_flag.unwrap(), "-home-user-projects-my-app");
    }

    #[test]
    fn windows_path_with_backslashes_is_sanitized() {
        let filter = resolve_project_filter(Some(r"C:\Users\foo\bar"), false, "/irrelevant");
        assert_eq!(filter.unwrap(), "C:-Users-foo-bar");
    }

    #[test]
    fn plain_name_substring_is_passed_through_unchanged() {
        // No path separator -> treated as a substring filter, unchanged,
        // matching the already-working "name substring" mode from the report.
        assert_eq!(
            resolve_project_filter(Some("my-app"), false, "/irrelevant"),
            Some("my-app".to_string())
        );
    }

    #[test]
    fn no_project_falls_back_to_encoded_cwd() {
        assert_eq!(
            resolve_project_filter(None, false, "/home/user/my-app"),
            Some("-home-user-my-app".to_string())
        );
    }
}
