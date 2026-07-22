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
use crate::hooks::hook_check::{status as hook_status, HookStatus};

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

pub fn run(
    project: Option<&str>,
    all: bool,
    since_days: u64,
    limit: usize,
    format: &str,
    verbose: u8,
) -> Result<()> {
    let provider = ClaudeProvider;

    // Determine project filter
    let project_filter = if all {
        None
    } else if let Some(p) = project {
        Some(p.to_string())
    } else {
        // Default: current working directory
        let cwd = std::env::current_dir()?;
        let cwd_str = cwd.to_string_lossy().to_string();
        let encoded = ClaudeProvider::encode_project_path(&cwd_str);
        Some(encoded)
    };

    let sessions = provider.discover_sessions(project_filter.as_deref(), Some(since_days))?;

    if verbose > 0 {
        eprintln!("Scanning {} session files...", sessions.len());
        for s in &sessions {
            eprintln!("  {}", s.display());
        }
    }

    // Transcripts record commands as the model emitted them, before the PreToolUse
    // hook rewrites them. If the hook is installed, a command it would rewrite was
    // already routed through RTK at runtime — it's coverage, not a missed opportunity.
    // Uses the same rewrite engine the hook itself calls (`rtk hook check`), so a
    // command excluded via config or containing an unattestable construct (the hook
    // would defer on it) still counts as a genuine miss below.
    let hook_installed = hook_status() != HookStatus::Missing;
    let (excluded, transparent_prefixes) = crate::core::config::Config::load()
        .map(|c| (c.hooks.exclude_commands, c.hooks.transparent_prefixes))
        .unwrap_or_default();

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
                    Classification::Supported { .. }
                        if covered_by_hook(
                            part,
                            hook_installed,
                            &excluded,
                            &transparent_prefixes,
                        ) =>
                    {
                        // Hook is installed and would rewrite this exact command —
                        // it already ran through RTK, not a missed opportunity.
                        already_rtk += 1;
                    }
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
                        if is_already_rtk(part) {
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

/// Whether a `Supported` command was already routed through RTK by an installed
/// PreToolUse hook, using the same rewrite engine the hook itself calls (`rtk hook
/// check`). A transcript records the command as the model emitted it — before any
/// hook rewrite — so this re-derives whether the hook would have intercepted this
/// exact instance, respecting excludes/transparent-prefixes and the same
/// unattestable-construct defer the hook applies (heredocs, substitutions, etc.).
fn covered_by_hook(
    cmd: &str,
    hook_installed: bool,
    excluded: &[String],
    transparent_prefixes: &[String],
) -> bool {
    hook_installed
        && !lexer::contains_unattestable_construct(cmd)
        && registry::rewrite_command(cmd, excluded, transparent_prefixes).is_some()
}

/// Whether an already-`rtk`-prefixed command counts as coverage. `rtk proxy <cmd>`
/// deliberately runs the raw command unfiltered, so it must not count — that would
/// let the audit flatter itself via its own escape hatch.
fn is_already_rtk(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    trimmed.starts_with("rtk ") && !trimmed.starts_with("rtk proxy")
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
mod tests {
    use super::*;

    // rtk-ai/rtk#3148: hook-rewritten commands were counted as missed savings
    // because transcripts record the pre-rewrite (raw) form of the command.

    #[test]
    fn test_covered_by_hook_false_when_hook_not_installed() {
        // No hook installed → the raw command genuinely ran unfiltered; still a miss.
        assert!(!covered_by_hook("grep -n foo bar.py", false, &[], &[]));
    }

    #[test]
    fn test_covered_by_hook_true_for_rewritable_command() {
        // Hook installed and this command has a rewrite → already covered at runtime.
        assert!(covered_by_hook("grep -n foo bar.py", true, &[], &[]));
        assert!(covered_by_hook("ls -la", true, &[], &[]));
    }

    #[test]
    fn test_covered_by_hook_false_for_unattestable_construct() {
        // The hook itself defers on unattestable constructs (substitutions, etc.),
        // so a command containing one was never actually rewritten — genuine miss.
        assert!(!covered_by_hook(
            "git status $(rm -rf /tmp/x)",
            true,
            &[],
            &[]
        ));
    }

    #[test]
    fn test_covered_by_hook_false_when_excluded_by_config() {
        // Even with the hook installed, a command the user excluded via config
        // was never rewritten — the hook stepped aside, so it's a genuine miss.
        let excluded = vec!["grep".to_string()];
        assert!(!covered_by_hook("grep -n foo bar.py", true, &excluded, &[]));
    }

    #[test]
    fn test_is_already_rtk_plain_rewrite() {
        assert!(is_already_rtk("rtk grep -n foo bar.py"));
    }

    #[test]
    fn test_is_already_rtk_excludes_proxy_escape_hatch() {
        // rtk#3148 secondary finding: `rtk proxy` deliberately bypasses filtering,
        // so it must not count as coverage.
        assert!(!is_already_rtk("rtk proxy grep -n foo bar.py"));
    }

    #[test]
    fn test_is_already_rtk_false_for_unrelated_command() {
        assert!(!is_already_rtk("grep -n foo bar.py"));
    }
}
