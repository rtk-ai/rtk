//! Scans AI coding sessions to find commands that could benefit from RTK filtering.

pub mod lexer;
pub mod provider;
pub mod registry;
mod report;
pub mod rules;

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use provider::{ClaudeProvider, SessionProvider};
use registry::{
    category_avg_tokens, classify_command, split_command_chain, strip_disabled_prefix,
    Classification, ExcludePattern,
};
use report::{DiscoverReport, SupportedEntry, UnsupportedEntry};

use crate::core::tracking::{HookDecisionRecord, Tracker};
use crate::discover::registry::prefix_contains_rtk_disabled;
use crate::hooks::hook_check::{status as hook_status, HookStatus};
use crate::hooks::permissions::{self, PermissionVerdict};

/// Whether a `Supported` command was actually routed through RTK, and how
/// confident we are in that answer.
///
/// `Measured` comes from a real `hook_decisions` log row, keyed by the
/// transcript's `tool_use_id` — ground truth about what the hook actually did
/// at the time. `Estimated` is the fallback for history that predates hook-decision
/// logging (or isn't Claude Code): a best-effort guess using *today's*
/// hook-install state, config, and registry, which may not reflect reality at
/// the time the command actually ran.
enum Coverage {
    Measured(bool),
    Estimated(bool),
}

impl Coverage {
    fn is_covered(&self) -> bool {
        match self {
            Coverage::Measured(covered) | Coverage::Estimated(covered) => *covered,
        }
    }

    fn is_estimated(&self) -> bool {
        matches!(self, Coverage::Estimated(_))
    }
}

/// Preloaded permission rules, read from disk once per `discover` run instead of
/// once per command. `check_command_for` re-reads every settings file on disk on
/// every call; over a large history (tens of thousands of commands) that dominated
/// runtime (see rtk-ai/rtk#3206 review: 80s vs 2.5s on the measured path).
struct PermissionRules {
    deny: Vec<String>,
    ask: Vec<String>,
    allow: Vec<String>,
}

/// Everything `hook_coverage`/`estimate_hook_coverage` need to judge a command,
/// besides the command (and `tool_use_id`) itself — built once per `discover` run
/// and passed by reference to every per-command call.
///
/// Grouped into one struct rather than several individual positional params: with
/// `excluded`/`transparent_prefixes` both `Vec<String>`, a swapped argument order
/// at a call site would compile silently and misclassify RTK_DISABLED bypass
/// coverage (code-review finding on rtk-ai/rtk#3206's fixup round).
///
/// `exclude_patterns`/`normalized_transparent_prefixes` are `registry::
/// rewrite_command`'s exclude-pattern regexes and normalized prefixes,
/// precompiled once here instead of inside `rewrite_command` on every call — that
/// recompilation is exactly the "recompute per command instead of once per run"
/// class of cost this PR's `PermissionRules`/`hook_status` caching already fixed
/// elsewhere, left as a hot-loop cost in `estimate_hook_coverage_with_verdict`'s
/// `registry::rewrite_command` call until now (code-review finding on rtk-ai/rtk#3206's
/// second fixup round).
struct CoverageContext {
    hook_log: HashMap<String, HookDecisionRecord>,
    hook_installed: bool,
    rules: PermissionRules,
    exclude_patterns: Vec<ExcludePattern>,
    normalized_transparent_prefixes: Vec<String>,
}

/// Determine whether `cmd` was (or, absent a log entry, likely would have been)
/// routed through RTK by the installed PreToolUse hook.
///
/// Do NOT call this for an `RTK_DISABLED=`-bypassed command — call
/// `would_be_covered_without_bypass` instead. `hook_coverage` trusts a measured
/// `hook_decisions` row as ground truth, but for a bypassed command the real logged
/// decision is always `Defer` (see `registry.rs` #345 / `get_rewritten`), which only
/// confirms the hook stepped aside — it says nothing about whether the command would
/// have been covered absent the bypass, which is the actual question the
/// RTK_DISABLED= bucket is answering. Consulting the measured log there made that
/// bucket silently collapse to (near) zero once real `hook_decisions` rows
/// accumulated (rtk-ai/rtk#3206 review) — the counterfactual can only ever be
/// answered by the estimate.
///
/// The caller splits a chained command (`a && b`) into parts and calls this once
/// per part with the *same* `tool_use_id`, since Claude Code's PreToolUse hook
/// fires once per tool call for the whole raw command. `raw_cmd` MUST be the full,
/// unsplit original command line (`ExtractedCommand::command`) even though this is
/// being asked about one `part` of it — passing an isolated segment here would
/// silently blind the `Deny`/`Ask` permission check to sibling segments (a
/// chain-wide deny rule matching only `a` would go undetected when checking `b` in
/// isolation, since `permissions::check_command_with_rules` only re-derives the
/// chain from whatever string it's given). The real rewrite (`registry::
/// rewrite_command`) genuinely is best-effort *per segment* though — it can
/// rewrite just `a` and leave an unsupported `b` untouched — so `rewrite_cmd` is
/// deliberately the isolated `part`, not `raw_cmd`. Neither `Measured` (one
/// `hook_decisions` row per `tool_use_id`, no per-segment detail) nor `Estimated`
/// re-derives the rewrite's per-segment split, so a segment can still be reported
/// covered/missed based on a sibling segment's rewrite fate — that residual
/// imprecision is unavoidable without per-segment hook logging and isn't worth
/// chasing further. `Estimated` is the fallback for history that predates
/// `hook_decisions` logging — but note it does NOT simply fade out over time:
/// `hook_decisions` rows are pruned by the same `DEFAULT_HISTORY_DAYS` retention
/// window as the rest of the tracking DB (see `Tracker::cleanup_hook_decisions`),
/// so any `--since` reaching further back than that window permanently needs
/// `Estimated`, indefinitely, not just during an initial backfill period.
fn hook_coverage(
    raw_cmd: &str,
    rewrite_cmd: &str,
    tool_use_id: &str,
    ctx: &CoverageContext,
) -> Coverage {
    if let Some(record) = ctx.hook_log.get(tool_use_id) {
        return Coverage::Measured(record.decision.is_covered());
    }
    Coverage::Estimated(estimate_hook_coverage(raw_cmd, rewrite_cmd, ctx))
}

/// Best-effort guess at whether the *currently* installed hook would rewrite this
/// command, used only when no `hook_decisions` log row exists for this exact
/// invocation.
///
/// Takes two forms of the command because callers generally need them to differ:
/// `permission_cmd` is checked against permission rules and scanned for
/// unattestable constructs, and MUST be the exact full string the real hook would
/// have seen — the whole raw command line, chain operators and any
/// `RTK_DISABLED=` prefix intact — since that's what Claude Code's permission
/// engine actually evaluates (`hooks::hook_cmd::decide_hook_action` never splits
/// or strips it; see `hook_coverage`'s doc comment for why an isolated chain
/// segment would blind the deny/ask check to sibling segments). `rewrite_cmd` is
/// passed to `registry::rewrite_command`, which is genuinely best-effort
/// *per segment* — pass the isolated segment being evaluated. For the RTK_DISABLED=
/// bypass call site specifically, `rewrite_cmd` must also have the prefix already
/// *stripped*, since `rewrite_command` itself detects and refuses a raw
/// `RTK_DISABLED=` prefix (registry.rs #345) — passing it unstripped there would
/// make every bypassed command register as never-covered regardless of whether
/// it's otherwise supported, defeating the point of the RTK_DISABLED bucket.
///
/// Checks `hook_installed` before touching `rules`/the lexer/the registry: with no
/// hook installed the answer is always "not covered", so there's no reason to pay
/// for the permission-verdict lookup, the unattestable-construct scan, or the
/// registry rewrite check on every single command in the scan (see
/// rtk-ai/rtk#3206 review: 100s of wasted work on histories with no hook ever
/// installed, since that case never had a chance to short-circuit before).
fn estimate_hook_coverage(permission_cmd: &str, rewrite_cmd: &str, ctx: &CoverageContext) -> bool {
    if !ctx.hook_installed {
        return false;
    }
    let verdict = permissions::check_command_with_rules(
        permission_cmd,
        &ctx.rules.deny,
        &ctx.rules.ask,
        &ctx.rules.allow,
    );
    estimate_hook_coverage_with_verdict(permission_cmd, rewrite_cmd, verdict, ctx)
}

/// Pure core of `estimate_hook_coverage`, taking the permission verdict directly so
/// it's testable without depending on real config files. Mirrors the real hook's own
/// decision order (`hooks::hook_cmd::decide_from_verdict`): a `Deny`-rule match or an
/// unattestable construct means the hook would never have rewritten this, regardless
/// of registry support. See `estimate_hook_coverage` for why `permission_cmd` and
/// `rewrite_cmd` can differ.
fn estimate_hook_coverage_with_verdict(
    permission_cmd: &str,
    rewrite_cmd: &str,
    verdict: PermissionVerdict,
    ctx: &CoverageContext,
) -> bool {
    ctx.hook_installed
        && verdict != PermissionVerdict::Deny
        && !lexer::contains_unattestable_construct(permission_cmd)
        && registry::rewrite_command_precompiled(
            rewrite_cmd,
            &ctx.exclude_patterns,
            &ctx.normalized_transparent_prefixes,
        )
        .is_some()
}

/// Would `raw_cmd` (still carrying its `RTK_DISABLED=` prefix) have been covered
/// by the hook if the bypass weren't there?
///
/// The *only* correct way to answer this question — deliberately named and kept
/// separate from `hook_coverage` so an RTK_DISABLED= call site reaches for this
/// instead of `hook_coverage` by construction, not just by doc comment. A prior
/// review round had to catch exactly this mixup once already: `hook_coverage`
/// trusts a measured `hook_decisions` row as ground truth, but a bypassed
/// command's real logged decision is always `Defer` (`get_rewritten` refuses an
/// `RTK_DISABLED=` prefix unconditionally — registry.rs #345), which only confirms
/// the hook stepped aside and says nothing about the counterfactual this function
/// answers. Consulting the measured log there made the RTK_DISABLED bucket
/// silently collapse to (near) zero once real `hook_decisions` rows accumulated
/// (rtk-ai/rtk#3206 review) — only the estimate can ever answer this.
///
/// `raw_cmd` must be the *full, unsplit* original command line
/// (`ExtractedCommand::command`), prefix and any sibling chain segments intact —
/// what the real hook's permission check would have seen (see `hook_coverage`'s
/// doc comment for why an isolated segment would blind the deny/ask check to
/// siblings). `stripped_cmd` is the isolated, prefix-*removed* segment being
/// evaluated, which is what `registry::rewrite_command` needs since it refuses a
/// raw `RTK_DISABLED=`-prefixed string on its own — see `estimate_hook_coverage`'s
/// doc comment for the full reasoning.
fn would_be_covered_without_bypass(
    raw_cmd: &str,
    stripped_cmd: &str,
    ctx: &CoverageContext,
) -> bool {
    estimate_hook_coverage(raw_cmd, stripped_cmd, ctx)
}

/// Whether an already-`rtk`-prefixed command counts as coverage. `rtk proxy <cmd>`
/// deliberately runs the raw command unfiltered, so it must not count — that would
/// let the audit flatter itself via its own escape hatch. This is ground truth read
/// directly from the transcript (the model really did invoke `rtk`), not a guess.
pub(crate) fn is_already_rtk(cmd: &str) -> bool {
    let trimmed = cmd.trim();
    trimmed.starts_with("rtk ") && !trimmed.starts_with("rtk proxy")
}

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
    // hook rewrites them. Prefer ground truth from the `hook_decisions` log (keyed by
    // `tool_use_id`, populated at the moment the hook actually ran) over guessing from
    // today's hook-install state; fall back to a heuristic only for history that
    // predates logging (or isn't Claude Code).
    let hook_installed = hook_status() != HookStatus::Missing;
    let (excluded, transparent_prefixes) = crate::core::config::hook_rewrite_params();
    // Compiled once here (see `CoverageContext`'s doc comment), not once per
    // command inside `registry::rewrite_command`.
    let exclude_patterns = registry::compile_exclude_patterns(&excluded);
    let normalized_transparent_prefixes =
        registry::normalize_transparent_prefixes(&transparent_prefixes);

    // Loaded once up front (see `PermissionRules`), not once per command.
    let (deny, ask, allow) = permissions::load_rules_for(permissions::Host::Claude);
    let rules = PermissionRules { deny, ask, allow };

    let cutoff = crate::core::utils::days_ago_cutoff(since_days);
    // Every other hook_decisions-touching path (record()/record_parse_failure()/
    // record_hook_decision() in tracking.rs) warns the user when a write fails
    // because a table is missing, pointing them at `rtk init` to self-heal. This
    // read path used to swallow the same class of error via unwrap_or_default()/
    // ok().flatten(), silently reporting "no hook-decision log yet" for what could
    // actually be a corrupted database — warn here too instead of going quiet.
    let (hook_log, measured_since): (HashMap<String, HookDecisionRecord>, Option<DateTime<Utc>>) =
        match Tracker::new() {
            Ok(t) => {
                let log = t.hook_decisions_since(cutoff).unwrap_or_else(|e| {
                    eprintln!(
                        "rtk: warning: failed to read hook_decisions log ({e}). \
                         Coverage will fall back to the current-state estimate. \
                         Run `rtk init` if the tracking database looks corrupted."
                    );
                    HashMap::new()
                });
                let measured_since = t.earliest_hook_decision_timestamp().unwrap_or_else(|e| {
                    eprintln!("rtk: warning: failed to read hook_decisions log timestamp ({e}).");
                    None
                });
                (log, measured_since)
            }
            Err(e) => {
                // A brand-new install with no tracking DB yet is the common,
                // benign case here — only surface this under --verbose so a fresh
                // `rtk discover` run doesn't alarm a first-time user.
                if verbose > 0 {
                    eprintln!(
                        "rtk: warning: failed to open tracking database ({e}). \
                         Coverage will fall back to the current-state estimate."
                    );
                }
                (HashMap::new(), None)
            }
        };

    let coverage_ctx = CoverageContext {
        hook_log,
        hook_installed,
        rules,
        exclude_patterns,
        normalized_transparent_prefixes,
    };

    let mut total_commands: usize = 0;
    let mut already_rtk: usize = 0;
    let mut already_rtk_estimated: usize = 0;
    let mut parse_errors: usize = 0;
    let mut rtk_disabled_count: usize = 0;
    let mut rtk_disabled_estimated: usize = 0;
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
                let part = if prefix_contains_rtk_disabled(env_prefix) {
                    match classify_command(actual_cmd) {
                        Classification::Supported { .. } => {
                            // Only count as a "bypass" if the hook would actually have
                            // covered it absent the bypass — otherwise RTK_DISABLED=
                            // bypassed nothing (hook wasn't installed / excluded / would
                            // defer / would deny), and flagging it as a "bypass" would be
                            // false advice. See `would_be_covered_without_bypass`'s doc
                            // comment for why this must never go through `hook_coverage`'s
                            // measured-log path.
                            if would_be_covered_without_bypass(
                                &ext_cmd.command,
                                actual_cmd,
                                &coverage_ctx,
                            ) {
                                rtk_disabled_count += 1;
                                rtk_disabled_estimated += 1;
                                let display = truncate_command(actual_cmd);
                                *rtk_disabled_cmds.entry(display).or_insert(0) += 1;
                                continue;
                            }
                            // Genuinely never had a chance regardless of the bypass (no
                            // hook installed / excluded by config / etc.) — a real
                            // missed-savings opportunity like any other command, not a
                            // "bypass" of anything. Fall through to the normal
                            // classification below (using the env-stripped command)
                            // instead of vanishing from the whole report — previously
                            // this case was counted only in `total_commands` and nowhere
                            // else (rtk-ai/rtk#3206 review).
                            actual_cmd
                        }
                        // Unsupported/Ignored under RTK_DISABLED= isn't interesting
                        // either way — rtk was never going to touch it regardless of
                        // the bypass.
                        _ => continue,
                    }
                } else {
                    part
                };

                match classify_command(part) {
                    Classification::Supported {
                        rtk_equivalent,
                        category,
                        estimated_savings_pct,
                        status,
                    } => {
                        let coverage = hook_coverage(
                            &ext_cmd.command,
                            part,
                            &ext_cmd.tool_use_id,
                            &coverage_ctx,
                        );

                        if coverage.is_covered() {
                            // Hook already routed this through RTK at runtime — it's
                            // coverage, not a missed opportunity.
                            already_rtk += 1;
                            if coverage.is_estimated() {
                                already_rtk_estimated += 1;
                            }
                            continue;
                        }

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
                        // Ground truth from the transcript itself — the model really
                        // did invoke `rtk` directly (excluding the deliberate
                        // unfiltered `rtk proxy` escape hatch).
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
        already_rtk_estimated,
        measured_since: measured_since.map(|ts| ts.format("%Y-%m-%d").to_string()),
        since_days,
        supported,
        unsupported,
        parse_errors,
        rtk_disabled_count,
        rtk_disabled_estimated,
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
mod tests {
    use super::*;
    use crate::core::tracking::HookOutcome;

    // rtk-ai/rtk#3148: hook-rewritten commands were counted as missed savings
    // because transcripts record the pre-rewrite (raw) form of the command.
    // Real hook decisions are logged (`hook_decisions`, keyed by `tool_use_id`)
    // now so coverage is measured, not guessed; `estimate_hook_coverage`/
    // `Coverage::Estimated` — partially modeled on #3164's heuristic, with its
    // permission-deny gap fixed — remain only as the fallback for history that
    // predates that log.

    fn record(decision: HookOutcome) -> HookDecisionRecord {
        HookDecisionRecord { decision }
    }

    fn empty_rules() -> PermissionRules {
        PermissionRules {
            deny: vec![],
            ask: vec![],
            allow: vec![],
        }
    }

    /// Build a `CoverageContext` for tests, with an empty `hook_log` (so every
    /// call falls through to the estimate) and no permission/exclusion rules
    /// unless overridden by the caller after construction.
    fn test_ctx(hook_installed: bool) -> CoverageContext {
        CoverageContext {
            hook_log: HashMap::new(),
            hook_installed,
            rules: empty_rules(),
            exclude_patterns: vec![],
            normalized_transparent_prefixes: vec![],
        }
    }

    #[test]
    fn test_estimate_hook_coverage_false_when_hook_not_installed() {
        // No hook installed → the raw command genuinely ran unfiltered; still a miss.
        assert!(!estimate_hook_coverage_with_verdict(
            "grep -n foo bar.py",
            "grep -n foo bar.py",
            PermissionVerdict::Default,
            &test_ctx(false),
        ));
    }

    #[test]
    fn test_estimate_hook_coverage_true_for_rewritable_command() {
        let ctx = test_ctx(true);
        assert!(estimate_hook_coverage_with_verdict(
            "grep -n foo bar.py",
            "grep -n foo bar.py",
            PermissionVerdict::Default,
            &ctx,
        ));
        assert!(estimate_hook_coverage_with_verdict(
            "ls -la",
            "ls -la",
            PermissionVerdict::Allow,
            &ctx,
        ));
    }

    #[test]
    fn test_estimate_hook_coverage_false_for_unattestable_construct() {
        // The hook itself defers on unattestable constructs (substitutions, etc.),
        // so a command containing one was never actually rewritten — genuine miss.
        assert!(!estimate_hook_coverage_with_verdict(
            "git status $(rm -rf /tmp/x)",
            "git status $(rm -rf /tmp/x)",
            PermissionVerdict::Default,
            &test_ctx(true),
        ));
    }

    #[test]
    fn test_estimate_hook_coverage_false_when_excluded_by_config() {
        // Even with the hook installed, a command the user excluded via config
        // was never rewritten — the hook stepped aside, so it's a genuine miss.
        let mut ctx = test_ctx(true);
        ctx.exclude_patterns = registry::compile_exclude_patterns(&["grep".to_string()]);
        assert!(!estimate_hook_coverage_with_verdict(
            "grep -n foo bar.py",
            "grep -n foo bar.py",
            PermissionVerdict::Default,
            &ctx,
        ));
    }

    #[test]
    fn test_estimate_hook_coverage_false_when_denied() {
        // A command matching a permissions.deny rule is never auto-rewritten by the
        // real hook (it defers to Claude Code's native deny handling) — counting it
        // as covered would over-count savings for commands the hook actually denied.
        assert!(!estimate_hook_coverage_with_verdict(
            "rm -rf /",
            "rm -rf /",
            PermissionVerdict::Deny,
            &test_ctx(true),
        ));
    }

    #[test]
    fn test_estimate_hook_coverage_uses_full_raw_command_for_permission_check() {
        // Regression: the RTK_DISABLED= bypass path must check permission rules
        // against the *full* raw command (prefix intact) — matching what the real
        // hook's permission engine actually evaluates — even though the rewrite
        // check needs the stripped form. An exact-match deny rule on the literal
        // prefixed command only matches the raw form, not the stripped one.
        let mut ctx = test_ctx(true);
        ctx.rules.deny = vec!["RTK_DISABLED=1 git status".to_string()];

        assert!(
            !would_be_covered_without_bypass("RTK_DISABLED=1 git status", "git status", &ctx),
            "a deny rule matching the raw (prefixed) command must still apply"
        );

        // Sanity check: the same deny rule does NOT match the stripped form alone —
        // if `estimate_hook_coverage` regressed to checking permissions against
        // `stripped_cmd` instead of `raw_cmd`, this assertion would start failing
        // (the bug would make it come back true instead of false above).
        assert!(estimate_hook_coverage("git status", "git status", &ctx));
    }

    #[test]
    fn test_estimate_hook_coverage_sees_chain_wide_deny_from_full_raw_command() {
        // Regression: check_command_with_rules re-derives the chain from whatever
        // string it's handed, so a deny rule matching only a *sibling* segment
        // (e.g. "cd /sensitive" in "cd /sensitive && grep -rn foo .") is invisible
        // if the isolated segment being evaluated ("grep -rn foo .") is what gets
        // passed as the permission-check command — exactly what the real hook
        // would NOT do (it always evaluates the whole raw line). Must pass the
        // full, unsplit command as `raw_cmd`, not the isolated `rewrite_cmd`.
        let mut ctx = test_ctx(true);
        ctx.rules.deny = vec!["cd /sensitive".to_string()];
        let full_chain = "cd /sensitive && grep -rn foo .";
        let isolated_segment = "grep -rn foo .";

        assert!(
            !estimate_hook_coverage(full_chain, isolated_segment, &ctx),
            "a deny rule matching a sibling chain segment must still deny the whole chain"
        );

        // Sanity check demonstrating the bug this guards against: checking
        // permissions against the isolated segment alone (instead of the full
        // chain) misses the sibling's deny match entirely.
        assert!(
            estimate_hook_coverage(isolated_segment, isolated_segment, &ctx),
            "checking the isolated segment alone should miss the sibling's deny rule \
             (demonstrates why raw_cmd must be the full chain, not a segment)"
        );
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

    #[test]
    fn test_hook_coverage_prefers_measured_log_over_estimate() {
        // Even if the current hook state would estimate "not covered" (hook
        // uninstalled today), a real log row is ground truth and wins.
        let mut ctx = test_ctx(false);
        ctx.hook_log
            .insert("toolu_1".to_string(), record(HookOutcome::Allow));

        let coverage = hook_coverage("git status", "git status", "toolu_1", &ctx);
        assert!(matches!(coverage, Coverage::Measured(true)));
        assert!(coverage.is_covered());
        assert!(!coverage.is_estimated());
    }

    #[test]
    fn test_hook_coverage_measured_deny_is_a_genuine_miss() {
        let mut ctx = test_ctx(true);
        ctx.hook_log
            .insert("toolu_1".to_string(), record(HookOutcome::Deny));

        let coverage = hook_coverage("rm -rf /", "rm -rf /", "toolu_1", &ctx);
        assert!(matches!(coverage, Coverage::Measured(false)));
        assert!(!coverage.is_covered());
    }

    #[test]
    fn test_hook_coverage_falls_back_to_estimate_when_no_log_row() {
        // No log entry for this tool_use_id — predates logging (or non-Claude) —
        // falls back to the current-state heuristic, flagged as estimated.
        let coverage = hook_coverage("ls -la", "ls -la", "toolu_missing", &test_ctx(true));
        assert!(coverage.is_estimated());
    }
}
