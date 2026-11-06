//! Scans AI coding sessions to find commands that could benefit from RTK filtering.

pub mod fish_script;
pub mod provider;
pub mod registry;
mod report;
pub mod rules;
pub(crate) mod shell_wrapper;

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::collections::HashMap;

use provider::{ClaudeProvider, ExtractedCommand, SessionProvider};
use registry::{
    ChainPart, Classification, ExcludePattern, category_avg_tokens, classify_command_at,
    split_command_chain_parts, strip_disabled_prefix_for_analytics,
};
use report::{DiscoverReport, SupportedEntry, UnsupportedEntry};

use crate::core::cmdline::lexer::{
    self, CommandStart, TokenKind, joined_or_as_written, tokenize, words,
};
use crate::core::cmdline::rtk::rtk_invocation;
use crate::core::tracking::{HookDecisionRecord, Tracker};
use crate::core::user_dirs;
use crate::discover::registry::prefix_contains_rtk_disabled;
use crate::hooks::hook_check::{HookStatus, status as hook_status};
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
/// unsplit command line, as the hook reads it, even though this is
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
/// bypass call site specifically, `rewrite_cmd` must also have its `RTK_DISABLED=`
/// words removed (`without_rtk_disabled` does it, keeping any wrapper),
/// since `rewrite_command` itself detects and refuses a raw
/// `RTK_DISABLED=` prefix (registry.rs #345) — passing it unremoved there would
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
    let verdict = permissions::check_command_with_rules_and_prefixes(
        permission_cmd,
        &ctx.rules.deny,
        &ctx.rules.ask,
        &ctx.rules.allow,
        &ctx.normalized_transparent_prefixes,
    );
    estimate_hook_coverage_with_verdict(permission_cmd, rewrite_cmd, verdict, ctx)
}

/// Pure core of `estimate_hook_coverage`, taking the permission verdict directly so
/// it's testable without depending on real config files. Mirrors the real hook's own
/// decision order (`hooks::decision::decide`): a `Deny`-rule match or an
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
        && !matches!(
            verdict,
            PermissionVerdict::Deny | PermissionVerdict::Defer | PermissionVerdict::TooComplex
        )
        && !lexer::line_is_unattestable(permission_cmd)
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
/// `raw_cmd` must be the *full, unsplit* command line, as the hook reads it, prefix
/// and any sibling chain segments intact —
/// what the real hook's permission check would have seen (see `hook_coverage`'s
/// doc comment for why an isolated segment would blind the deny/ask check to
/// siblings). `unbypassed_cmd` is the segment as [`without_rtk_disabled`]
/// rebuilds it: `docker ps` rewrites by default, while `sudo docker ps` only does when
/// `sudo` is in `hooks.transparent_prefixes`, so asking about the peeled command
/// alone would report savings the hook never offered.
fn would_be_covered_without_bypass(
    raw_cmd: &str,
    unbypassed_cmd: &str,
    ctx: &CoverageContext,
) -> bool {
    estimate_hook_coverage(raw_cmd, unbypassed_cmd, ctx)
}

/// The segment `strip_disabled_prefix_for_analytics` split into `env_prefix` and
/// `actual_cmd`, with its `RTK_DISABLED=` words removed and every other word kept
/// verbatim, wrapper included. Words are split the way the rest of discover splits
/// them (`lexer::tokenize` + `lexer::words`), so a quoted or expanded value
/// (`RTK_DISABLED='1 2'`, `RTK_DISABLED=$CI`) goes as a whole.
fn without_rtk_disabled(env_prefix: &str, actual_cmd: &str) -> String {
    let starts: Vec<usize> = words(env_prefix, &tokenize(env_prefix))
        .iter()
        .map(|word| word.start)
        .collect();
    let mut line = String::with_capacity(env_prefix.len() + actual_cmd.len() + 1);
    for (i, &start) in starts.iter().enumerate() {
        let end = starts.get(i + 1).copied().unwrap_or(env_prefix.len());
        let word = &env_prefix[start..end];
        if !word.starts_with("RTK_DISABLED=") {
            line.push_str(word);
        }
    }
    if !line.is_empty() && !line.ends_with(char::is_whitespace) {
        line.push(' ');
    }
    line.push_str(actual_cmd);
    line
}

/// What a segment carrying `RTK_DISABLED=` contributes to the report.
#[derive(Debug, PartialEq)]
enum DisabledSegment {
    /// The hook would have rewritten the line without the assignment: lost savings.
    Bypass,
    /// The assignment cost nothing. The line as typed without it is classified like
    /// any other command, so `sudo RTK_DISABLED=1 docker ps` is an unhandled `sudo`
    /// line rather than hook coverage of `docker ps`.
    Unbypassed(String),
    /// `classify_command_at` does not recognise the peeled command; the segment is left
    /// out of the report.
    Unhandled,
}

fn judge_disabled_segment(
    raw_cmd: &str,
    env_prefix: &str,
    actual_cmd: &str,
    start: CommandStart,
    ctx: &CoverageContext,
) -> DisabledSegment {
    if !matches!(
        classify_command_at(actual_cmd, start),
        Classification::Supported { .. }
    ) {
        return DisabledSegment::Unhandled;
    }
    let line = without_rtk_disabled(env_prefix, actual_cmd);
    if would_be_covered_without_bypass(raw_cmd, &line, ctx) {
        DisabledSegment::Bypass
    } else {
        DisabledSegment::Unbypassed(line)
    }
}

/// Whether a command that runs rtk counts as coverage. `rtk bypass <cmd>` (or its
/// deprecated alias `rtk proxy`) deliberately runs the raw command unfiltered, so it
/// must not count — that would let the audit flatter itself via its own escape
/// hatch. This is ground truth read directly from the transcript (the model really
/// did invoke `rtk`), not a guess.
pub(crate) fn is_already_rtk(cmd: &str) -> bool {
    rtk_invocation(&words(cmd, &tokenize(cmd))).is_some_and(|rtk| !rtk.bypass)
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
    /// How many of `count` were estimated from category averages: sized by their own,
    /// because what the line measured was somebody else's output, or given a share of a
    /// number other commands wrote to as well. A command that feeds a pipe is always one
    /// of these.
    estimated_occurrences: usize,
    // For display: the most common raw command
    command_counts: HashMap<String, usize>,
}

/// What the report does with one command of a line.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum Disposition {
    /// The hook routed it through RTK.
    AlreadyRtk,
    /// RTK would have filtered it where it ran, and did not.
    Missed,
    /// RTK handles it, but not as it was run — the saving is real and reachable
    /// only by changing the command.
    Opportunity,
    /// Nothing RTK would do with it.
    Dropped,
}

/// Decide which of those a command is.
///
/// Kept apart from the scanning loop so a test can reach it: the loop needs a
/// real session directory and a real tracking database.
///
/// `covered` alone cannot be trusted, because neither source behind it knows
/// where the command sat: a `hook_decisions` row is one decision for the whole
/// tool call, and the estimate asks about the command on its own. `cat f.log`
/// becomes `rtk read` alone but not in `cat f.log | grep ERROR`, so the producer
/// of every such line would read as adoption RTK never had.
fn disposition(bypassed: bool, reached: bool, covered: bool, consumes_a_pipe: bool) -> Disposition {
    if reached && covered {
        return Disposition::AlreadyRtk;
    }
    // A bypassed command belongs in the main table: the rewriter refused it for
    // its prefix, not for its position, and the bypass bucket has already had
    // its say about that.
    if bypassed || reached {
        return Disposition::Missed;
    }
    // A command reading another's output wrote what actually came back, and
    // `output_len` already measures that. Counting it would claim a saving on
    // the one line `tail` printed on top of everything `cargo` wrote.
    //
    // Anything else RTK has a rule for is an opportunity, whatever kept the
    // rewriter off it — feeding a pipe whose consumer throws the output away,
    // or being written in a form the rewriter does not match, like an absolute
    // path (#1053). Dropping those hides real savings and hides the
    // disagreements between classification and rewriting that cause them.
    if consumes_a_pipe {
        return Disposition::Dropped;
    }
    Disposition::Opportunity
}

/// A command reads another's output when the one before it feeds a pipe.
fn consumes_a_pipe(parts: &[ChainPart<'_>], index: usize) -> bool {
    index
        .checked_sub(1)
        .and_then(|prev| parts.get(prev))
        .is_some_and(|prev| prev.feeds_pipe)
}

/// After a pipe, `time` is the program `time`.
fn command_start(consumes_a_pipe: bool) -> CommandStart {
    if consumes_a_pipe {
        CommandStart::PipeStage
    } else {
        CommandStart::Pipeline
    }
}

/// Whether a redirect sends `command`'s own output somewhere other than the tool result.
fn sends_its_output_elsewhere(command: &str) -> bool {
    tokenize(command).iter().any(|token| {
        token.kind == TokenKind::Redirect && {
            let operator = token.value.strip_prefix('1').unwrap_or(token.value);
            // `>&2` copies it to the other stream, which comes back too. A bare `>&` takes a
            // file for its word.
            operator == ">&"
                || (operator.starts_with('>') && !operator.starts_with(">&"))
                || operator.starts_with("&>")
        }
    })
}

/// Commands whose output is next to nothing beside what they run with.
const WRITES_NEXT_TO_NOTHING: &[&str] = &[
    "cd", "echo", "printf", "pwd", "true", "false", "export", "set", "unset", "mkdir", "mv", "cp",
    "rm", "chmod", "chown", "touch", "sleep", "wait", "kill", "fi", "done",
];

fn writes_next_to_nothing(command: &str) -> bool {
    // A comment or a bare assignment leaves no command at all.
    command.is_empty()
        || head_words(command)
            .first()
            .is_some_and(|word| WRITES_NEXT_TO_NOTHING.contains(word))
}

/// What each part's share of a call's measured output is, `None` where it gets none and
/// its category average stands in.
///
/// The one number is shared by the commands that wrote to it, in proportion to their
/// category averages, and a command alone takes it whole. A command that fed a pipe never
/// reached it, and one that redirected its output is known to have added nothing. A trailing
/// `echo done` or `cd x` is left out. Any other command with no row, such as `bash x.sh` or
/// `rtk cargo test`, may have written it all, so it takes a share that no row shows.
fn output_shares(parts: &[ChainPart<'_>], output_len: Option<usize>) -> Vec<Option<usize>> {
    let Some(output_len) = output_len else {
        return vec![None; parts.len()];
    };
    let weights: Vec<Option<usize>> = (0..parts.len())
        .map(|index| {
            let start = command_start(consumes_a_pipe(parts, index));
            let (_, command) = strip_disabled_prefix_for_analytics(parts[index].text);
            if parts[index].feeds_pipe {
                return None;
            }
            if sends_its_output_elsewhere(command) {
                return Some(0);
            }
            match classify_command_at(command, start) {
                Classification::Supported { category, .. } => {
                    let head = head_words(command);
                    let subcmd = head.get(1).copied().unwrap_or("");
                    Some(category_avg_tokens(category, subcmd))
                }
                _ if writes_next_to_nothing(command) => None,
                _ => Some(category_avg_tokens("", "")),
            }
        })
        .collect();
    let total: usize = weights.iter().flatten().sum();
    weights
        .iter()
        .map(|weight| {
            let weight = (*weight)?;
            Some(if weight == 0 {
                0
            } else {
                (output_len as f64 * weight as f64 / total as f64) as usize
            })
        })
        .collect()
}

/// Everything one scan counts, kept apart from the session files and the
/// tracking database `run()` reads them from.
///
/// Counting lives here rather than inline in `run()`, which needs a real session
/// directory and a real tracking database, so a test can hand it commands
/// directly and assert on what it counted.
#[derive(Default)]
struct Tally {
    total_commands: usize,
    already_rtk: usize,
    already_rtk_estimated: usize,
    rtk_disabled_count: usize,
    rtk_disabled_estimated: usize,
    rtk_disabled_cmds: HashMap<String, usize>,
    supported: HashMap<&'static str, SupportedBucket>,
    /// Commands RTK handles that the rewriter passed over because of where they
    /// ran — reachable by changing the command, not by the hook.
    opportunities: HashMap<&'static str, SupportedBucket>,
    unsupported: HashMap<String, UnsupportedBucket>,
}

impl Tally {
    fn take_rows(&mut self) -> ReportRows {
        ReportRows {
            supported: into_entries(std::mem::take(&mut self.supported)),
            opportunities: into_entries(std::mem::take(&mut self.opportunities)),
        }
    }

    fn add(&mut self, ext_cmd: &ExtractedCommand, ctx: &CoverageContext) {
        // The line as the hook reads it, so a `\⏎` is no word of the report's. One too
        // complex to join stays as written.
        let joined = joined_or_as_written(&ext_cmd.command);
        let command: &str = &joined;
        let parts = split_command_chain_parts(command);
        // One answer per command, worked out once for the line: what the
        // rewriter does to one command depends on the ones around it, and a
        // report that counts a command the rewriter passes over promises a
        // saving the hook does not take.
        let reached = registry::stages_the_rewriter_reaches(
            command,
            &ctx.exclude_patterns,
            &ctx.normalized_transparent_prefixes,
        );
        let shares = output_shares(&parts, ext_cmd.output_len);
        let is_split = shares.iter().flatten().filter(|share| **share > 0).count() > 1;
        for (index, chain_part) in parts.iter().enumerate() {
            let part = chain_part.text;
            let consumes_a_pipe = consumes_a_pipe(&parts, index);
            let start = command_start(consumes_a_pipe);
            let output_len = shares[index];
            self.total_commands += 1;

            // Detect RTK_DISABLED= bypass before classification
            let (env_prefix, actual_cmd) = strip_disabled_prefix_for_analytics(part);
            let bypassed = prefix_contains_rtk_disabled(env_prefix);
            let unbypassed;
            let part = if bypassed {
                // A bypass counts only if the hook would have covered the line
                // without it — otherwise RTK_DISABLED= bypassed nothing (hook not
                // installed / excluded / would defer / would deny / the wrapper is
                // not rewritable), and flagging it would be false advice. See
                // `would_be_covered_without_bypass`'s doc comment for why this must
                // never go through `hook_coverage`'s measured-log path. A segment
                // that bypassed nothing falls through to the normal classification
                // as typed without the assignment, instead of vanishing from the
                // report (rtk-ai/rtk#3206 review).
                match judge_disabled_segment(command, env_prefix, actual_cmd, start, ctx) {
                    DisabledSegment::Bypass => {
                        self.rtk_disabled_count += 1;
                        self.rtk_disabled_estimated += 1;
                        let display = head_words(actual_cmd).join(" ");
                        *self.rtk_disabled_cmds.entry(display).or_insert(0) += 1;
                        continue;
                    }
                    DisabledSegment::Unbypassed(line) => {
                        unbypassed = line;
                        unbypassed.as_str()
                    }
                    DisabledSegment::Unhandled => continue,
                }
            } else {
                part
            };

            match classify_command_at(part, start) {
                Classification::Supported {
                    rtk_equivalent,
                    category,
                    estimated_savings_pct,
                    status,
                } => {
                    let coverage = hook_coverage(command, part, &ext_cmd.tool_use_id, ctx);

                    let map = match disposition(
                        bypassed,
                        reached.get(index).copied().unwrap_or(false),
                        coverage.is_covered(),
                        consumes_a_pipe,
                    ) {
                        Disposition::AlreadyRtk => {
                            self.already_rtk += 1;
                            if coverage.is_estimated() {
                                self.already_rtk_estimated += 1;
                            }
                            continue;
                        }
                        Disposition::Dropped => continue,
                        Disposition::Missed => &mut self.supported,
                        Disposition::Opportunity => &mut self.opportunities,
                    };

                    let bucket = map
                        .entry(rtk_equivalent)
                        .or_insert_with(|| SupportedBucket {
                            rtk_equivalent,
                            category,
                            count: 0,
                            total_output_tokens: 0,
                            total_raw_output_tokens: 0,
                            estimated_occurrences: 0,
                            command_counts: HashMap::new(),
                        });

                    bucket.count += 1;

                    if output_len.is_none() || is_split {
                        bucket.estimated_occurrences += 1;
                    }

                    let head = head_words(part);
                    // Estimate tokens for this command
                    let output_tokens = if let Some(len) = output_len {
                        // Real: from tool_result content length
                        len / 4
                    } else {
                        // Fallback: category average
                        let subcmd = head.get(1).copied().unwrap_or("");
                        category_avg_tokens(category, subcmd)
                    };

                    let savings = (output_tokens as f64 * estimated_savings_pct / 100.0) as usize;
                    bucket.total_output_tokens += savings;
                    // Accumulate pre-savings tokens so we can compute a weighted effective
                    // savings rate across all sub-commands in this bucket later.
                    bucket.total_raw_output_tokens += output_tokens;

                    // Track the display name with status
                    let display_name = head.join(" ");
                    let entry = bucket
                        .command_counts
                        .entry(format!("{}:{:?}", display_name, status))
                        .or_insert(0);
                    *entry += 1;
                }
                Classification::Unsupported { base_command } => {
                    let bucket =
                        self.unsupported
                            .entry(base_command)
                            .or_insert_with(|| UnsupportedBucket {
                                count: 0,
                                example: part.to_string(),
                            });
                    bucket.count += 1;
                }
                Classification::Ignored => {
                    // Ground truth from the transcript itself — the model really
                    // did invoke `rtk` directly (excluding the deliberate
                    // unfiltered `rtk bypass` escape hatch).
                    if is_already_rtk(part) {
                        self.already_rtk += 1;
                    }
                    // Otherwise just skip
                }
            }
        }
    }
}

/// The two tables of the report, once the buckets behind them are rows.
///
/// They hold the same type, so crossing them compiles and prints each table
/// under the other's heading — which is why the assignment lives in
/// [`Tally::take_rows`] where a test can check it, rather than inline in `run()`.
struct ReportRows {
    supported: Vec<SupportedEntry>,
    opportunities: Vec<SupportedEntry>,
}

/// Collapse aggregated buckets into report rows, highest estimated saving first.
fn into_entries(map: HashMap<&'static str, SupportedBucket>) -> Vec<SupportedEntry> {
    let mut entries: Vec<SupportedEntry> = map
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
                estimated_occurrences: bucket.estimated_occurrences,
                rtk_equivalent: bucket.rtk_equivalent,
                category: bucket.category,
                estimated_savings_tokens: bucket.total_output_tokens,
                estimated_savings_pct: effective_savings_pct,
                rtk_status: status,
            }
        })
        .collect();
    entries.sort_by_key(|b| std::cmp::Reverse(b.estimated_savings_tokens));
    entries
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
    let scope = if all {
        report::ScanScope::AllProjects
    } else if let Some(p) = project {
        report::ScanScope::ProjectFilter(p.to_string())
    } else {
        // Default: current working directory
        let cwd = user_dirs::current_dir()?;
        let cwd_str = cwd.to_string_lossy().to_string();
        report::ScanScope::CurrentProject(ClaudeProvider::encode_project_path(&cwd_str))
    };
    let project_filter = match &scope {
        report::ScanScope::AllProjects => None,
        report::ScanScope::ProjectFilter(f) | report::ScanScope::CurrentProject(f) => {
            Some(f.as_str())
        }
    };

    let sessions = provider.discover_sessions(project_filter, Some(since_days))?;

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

    let mut parse_errors: usize = 0;
    let mut tally = Tally::default();

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
            tally.add(ext_cmd, &coverage_ctx);
        }
    }

    // Build report
    let ReportRows {
        supported,
        opportunities,
    } = tally.take_rows();

    let Tally {
        total_commands,
        already_rtk,
        already_rtk_estimated,
        rtk_disabled_count,
        rtk_disabled_estimated,
        rtk_disabled_cmds,
        unsupported: unsupported_map,
        ..
    } = tally;

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
        scope,
        total_commands,
        already_rtk,
        already_rtk_estimated,
        measured_since: measured_since.map(|ts| ts.format("%Y-%m-%d").to_string()),
        since_days,
        supported,
        opportunities,
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

/// A command's first two words as written, the program and its subcommand, as
/// the lexer ends them: a quoted or escaped blank stays inside its word. The
/// report files a command under the second one and shows the two joined by one
/// space.
fn head_words(cmd: &str) -> Vec<&str> {
    words(cmd, &tokenize(cmd))
        .into_iter()
        .take(2)
        .map(|word| word.text)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::HookOutcome;

    /// Every combination of the four facts, written out rather than derived, so
    /// that changing the rule means changing this table and saying why. Deriving
    /// the expected value would restate the rule and agree with it no matter
    /// what the rule became.
    #[test]
    fn test_every_disposition_is_pinned() {
        use Disposition::{AlreadyRtk, Dropped, Missed, Opportunity};
        // bypassed, reached, covered, consumes_a_pipe
        let table = [
            ((false, false, false, false), Opportunity),
            ((false, false, false, true), Dropped),
            // Coverage says yes, but the rewriter never took this command —
            // that is the producer of `cat f.log | grep ERROR`, and calling it
            // adoption is what the `reached` gate exists to stop.
            ((false, false, true, false), Opportunity),
            ((false, false, true, true), Dropped),
            ((false, true, false, false), Missed),
            ((false, true, false, true), Missed),
            ((false, true, true, false), AlreadyRtk),
            ((false, true, true, true), AlreadyRtk),
            // A bypassed command is a missed saving wherever it sits: the
            // rewriter refused it for its prefix, not for its position.
            ((true, false, false, false), Missed),
            ((true, false, false, true), Missed),
            ((true, false, true, false), Missed),
            ((true, false, true, true), Missed),
            ((true, true, false, false), Missed),
            ((true, true, false, true), Missed),
            ((true, true, true, false), AlreadyRtk),
            ((true, true, true, true), AlreadyRtk),
        ];
        assert_eq!(table.len(), 16, "one row per combination of four facts");
        for ((bypassed, reached, covered, consumes_a_pipe), expected) in table {
            assert_eq!(
                disposition(bypassed, reached, covered, consumes_a_pipe),
                expected,
                "bypassed={bypassed} reached={reached} covered={covered} \
                 consumes_a_pipe={consumes_a_pipe}"
            );
        }
    }

    fn empty_rules() -> PermissionRules {
        PermissionRules {
            deny: vec![],
            ask: vec![],
            allow: vec![],
        }
    }

    fn record(decision: HookOutcome) -> HookDecisionRecord {
        HookDecisionRecord { decision }
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

    fn scanned(command: &str, output_len: Option<usize>) -> ExtractedCommand {
        ExtractedCommand {
            command: command.to_string(),
            output_len,
            session_id: "s".to_string(),
            tool_use_id: String::new(),
            output_content: None,
            is_error: false,
            sequence_index: 0,
        }
    }

    fn tally_of(commands: &[&str], ctx: &CoverageContext) -> Tally {
        let mut tally = Tally::default();
        for c in commands {
            tally.add(&scanned(c, None), ctx);
        }
        tally
    }

    /// Tokens of 400 KB of output, as a row reads them.
    const MEASURED: usize = 100_000;

    fn row_of_a_call(command: &str, rule: &str) -> (usize, usize) {
        let mut tally = Tally::default();
        tally.add(&scanned(command, Some(4 * MEASURED)), &test_ctx(false));
        [&tally.supported, &tally.opportunities]
            .iter()
            .find_map(|map| map.get(rule))
            .map(|bucket| (bucket.estimated_occurrences, bucket.total_raw_output_tokens))
            .unwrap_or_else(|| panic!("{command:?}: no {rule} row"))
    }

    /// A command alone takes the call's whole output, whatever the report ignores after it.
    #[test]
    fn test_a_calls_only_writer_takes_its_measured_output() {
        for command in [
            "cargo test",
            "cargo test\necho done",
            "cargo test; echo done",
            "cargo test\ncd x",
            "cargo test\n# done",
            "cargo test; # done",
            "cargo test\nprintf done",
            "cd x\ncargo test",
        ] {
            assert_eq!(
                row_of_a_call(command, "rtk cargo"),
                (0, MEASURED),
                "{command:?}"
            );
        }
        // Where `time` after a pipe is the program, a stage the report does not size.
        assert_eq!(
            row_of_a_call("foo | time cargo test; git status", "rtk git").1,
            MEASURED * 40 / 190
        );
    }

    /// What fed a pipe never reached the output, and its size is not known.
    #[test]
    fn test_a_command_feeding_a_pipe_keeps_its_average() {
        let average = (1, category_avg_tokens("Cargo", "test"));
        for command in ["cargo test | foo", "cargo test | foo\necho done"] {
            assert_eq!(row_of_a_call(command, "rtk cargo"), average, "{command:?}");
        }
    }

    /// What went to a file added nothing to the output, and that is known.
    #[test]
    fn test_a_redirected_command_is_measured_at_nothing() {
        for command in [
            "cargo test > out.log\necho done",
            "cargo test >> out.log; echo done",
            "cargo test 1>out.log\necho done",
            "cargo test &> out.log\necho done",
            "cargo test >& out.log\necho done",
            "cargo test >/dev/null; echo $?",
            "cargo test > out.log",
        ] {
            assert_eq!(row_of_a_call(command, "rtk cargo"), (0, 0), "{command:?}");
        }
        assert_eq!(
            row_of_a_call("cargo test > out.log; git status", "rtk git"),
            (0, MEASURED)
        );
        // Its errors and a copy to the other stream still reach it.
        for command in ["cargo test 2>err.log", "cargo test >&2", "cargo test 2>&1"] {
            assert_eq!(
                row_of_a_call(command, "rtk cargo"),
                (0, MEASURED),
                "{command:?}"
            );
        }
    }

    /// Commands that wrote to the output share it by their averages, and add up to it.
    #[test]
    fn test_writers_share_the_measured_output_by_their_averages() {
        let cargo_avg = category_avg_tokens("Cargo", "test");
        let git_avg = category_avg_tokens("Git", "status");
        let total = cargo_avg + git_avg;
        for command in ["cargo test\ngit status", "cargo test; git status"] {
            let (cargo_estimated, cargo) = row_of_a_call(command, "rtk cargo");
            let (git_estimated, git) = row_of_a_call(command, "rtk git");
            assert_eq!((cargo_estimated, git_estimated), (1, 1), "{command:?}");
            assert_eq!(cargo, MEASURED * cargo_avg / total, "{command:?}");
            assert_eq!(git, MEASURED * git_avg / total, "{command:?}");
        }
        // The prefix is no word of the command, so `test` is still what sizes it.
        assert_eq!(
            row_of_a_call("RTK_DISABLED=1 cargo test\ngit status", "rtk git").1,
            MEASURED * git_avg / total
        );
    }

    /// A command with no row, or already through rtk, may have written it all.
    #[test]
    fn test_a_command_with_no_row_keeps_its_share() {
        let other = category_avg_tokens("", "");
        let git_avg = category_avg_tokens("Git", "status");
        for command in [
            "git status && ./build.sh",
            "git status\npython gen.py",
            "git status\nrtk cargo test",
            "git status\nrtk bypass cargo test",
            "git status\nbash ./build.sh",
            "git status; sed -n 1,9p big.log",
            "git status\npython3 -c 'print(1)'",
        ] {
            assert_eq!(
                row_of_a_call(command, "rtk git"),
                (1, MEASURED * git_avg / (git_avg + other)),
                "{command:?}"
            );
        }
    }

    /// Every line of a multi-line call is a command the agent ran.
    #[test]
    fn test_the_report_counts_every_line_of_a_call() {
        let tally = tally_of(
            &[
                "git status\ncargo test",
                "cd x\ngit log",
                "foo bar\ngit diff",
            ],
            &test_ctx(false),
        );

        assert_eq!(tally.total_commands, 6);
        assert_eq!(counts(&tally.supported), [("rtk cargo", 1), ("rtk git", 3)]);
    }

    /// A `\⏎` is no word: the report reads a line as the hook does.
    #[test]
    fn test_the_report_reads_a_line_with_its_continuations_joined() {
        let ctx = test_ctx(false);
        let mut tally = tally_of(
            &[
                "cargo test 2>&1 | tail \\\n -1",
                "git status; ls \\\n -la",
                "frobnicate \\\n x",
            ],
            &ctx,
        );
        let unsupported: Vec<_> = tally.unsupported.keys().cloned().collect();
        assert_eq!(unsupported, ["frobnicate x"]);
        let mut shown: Vec<_> = tally
            .take_rows()
            .supported
            .into_iter()
            .map(|row| row.command)
            .collect();
        shown.sort();
        assert_eq!(shown, ["cargo test", "git status", "ls -la"]);

        let mut tally = tally_of(&["git \\\n status"], &ctx);
        assert!(tally.unsupported.is_empty());
        let rows = tally.take_rows().supported;
        assert_eq!(rows[0].command, "git status");
    }

    fn counts(map: &HashMap<&'static str, SupportedBucket>) -> Vec<(&'static str, usize)> {
        let mut rows: Vec<_> = map.iter().map(|(k, b)| (*k, b.count)).collect();
        rows.sort();
        rows
    }

    /// A transcript whose Bash calls ran `commands`, one tool call each.
    fn transcript(commands: &[&str]) -> tempfile::NamedTempFile {
        use std::io::Write;
        let mut file = tempfile::NamedTempFile::new().expect("temp transcript");
        for (i, command) in commands.iter().enumerate() {
            let call = serde_json::json!({
                "type": "assistant",
                "message": {"role": "assistant", "content": [{
                    "type": "tool_use",
                    "id": format!("toolu_{i}"),
                    "name": "Bash",
                    "input": {"command": command},
                }]},
            });
            writeln!(file, "{call}").expect("write transcript");
        }
        file.flush().expect("flush transcript");
        file
    }

    /// Nothing inside a `[[ ]]` expression, an arithmetic command or a `case`
    /// pattern runs, so none of it is a missed command: the report splits a
    /// line where the rewriter does.
    #[test]
    fn test_expressions_and_patterns_hold_no_missed_command() {
        let file = transcript(&[
            "[[ -f a || ls ]]",
            "(( a || ls ))",
            "case $x in ls) echo 1;; esac",
            "ls && git status",
        ]);
        let commands = ClaudeProvider
            .extract_commands(file.path())
            .expect("read transcript");
        assert_eq!(commands.len(), 4);
        let mut tally = Tally::default();
        for command in &commands {
            tally.add(command, &test_ctx(false));
        }

        // One command each for the expression and the arithmetic, `case $x
        // in`, `echo 1` and `esac` for the `case`, and two for the chain.
        assert_eq!(tally.total_commands, 7);
        assert_eq!(
            counts(&tally.supported),
            vec![("rtk git", 1), ("rtk ls", 1)]
        );
        assert!(
            tally.opportunities.is_empty(),
            "{:?}",
            counts(&tally.opportunities)
        );
    }

    /// The whole point of the split between the two tables: `grep` is rewritten
    /// where it sits, `cargo test` only would be if it ran on its own, and each
    /// lands on its own side. Driving the real scan is the only way to catch the
    /// arguments being handed to `disposition` in the wrong order — every one of
    /// them is a `bool`, so a transposition compiles and the pinned table still
    /// passes.
    #[test]
    fn test_a_pipeline_splits_across_the_two_tables() {
        let tally = tally_of(&["cargo test | grep FAILED"], &test_ctx(false));

        assert_eq!(tally.total_commands, 2);
        assert_eq!(counts(&tally.supported), vec![("rtk grep", 1)]);
        assert_eq!(counts(&tally.opportunities), vec![("rtk cargo", 1)]);
        assert_eq!(tally.already_rtk, 0, "no hook installed, so no adoption");
    }

    /// The producer is taken here and the consumer is not, which is the reverse
    /// of the case above and the reason position has to decide rather than the
    /// command's own name.
    #[test]
    fn test_a_consumer_the_rewriter_declines_is_in_neither_table() {
        let tally = tally_of(&["cargo test 2>&1 | tail -1"], &test_ctx(false));

        assert_eq!(tally.total_commands, 2);
        assert_eq!(counts(&tally.supported), vec![("rtk cargo", 1)]);
        assert!(
            tally.opportunities.is_empty(),
            "`tail` wrote what came back; counting it would bill the line twice"
        );
    }

    /// With a hook installed and nothing in the log, the estimate carries the
    /// answer — and it must not carry it to a command the rewriter passed over.
    #[test]
    fn test_coverage_does_not_reach_a_command_the_rewriter_passed_over() {
        let tally = tally_of(&["cat f.log | grep ERROR"], &test_ctx(true));

        assert_eq!(
            tally.already_rtk, 1,
            "the `grep` the hook rewrites is adoption"
        );
        assert_eq!(
            counts(&tally.opportunities),
            vec![("rtk read", 1)],
            "`cat` is rewritable alone, so it is an opportunity, not adoption"
        );
        assert!(tally.supported.is_empty());
    }

    /// A line's measured output was written by its last command. Handing it to
    /// every part multiplies the estimate by the number of parts.
    #[test]
    fn test_measured_output_is_credited_to_the_command_that_wrote_it() {
        let ctx = test_ctx(false);
        let mut tally = Tally::default();
        tally.add(&scanned("cargo test | grep FAILED", Some(40_000)), &ctx);

        let grep = tally.supported.get("rtk grep").expect("grep is reported");
        let cargo = tally
            .opportunities
            .get("rtk cargo")
            .expect("cargo is reported");
        assert_eq!(
            grep.total_raw_output_tokens,
            40_000 / 4,
            "the last command gets the measured size"
        );
        assert_ne!(
            cargo.total_raw_output_tokens,
            40_000 / 4,
            "an earlier command falls back to the category average"
        );
    }

    /// A bypassed command is a missed saving, not an opportunity and not
    /// adoption, and it still counts once in the denominator.
    #[test]
    fn test_a_bypassed_command_is_counted_as_a_bypass() {
        let tally = tally_of(&["RTK_DISABLED=1 git status"], &test_ctx(true));

        assert_eq!(tally.total_commands, 1);
        assert_eq!(tally.rtk_disabled_count, 1);
        assert!(tally.supported.is_empty());
        assert!(tally.opportunities.is_empty());
    }

    /// Whatever else it decides, every command a session ran reaches the
    /// denominator exactly once — the buckets are a partition of it, not a
    /// filter on it.
    #[test]
    fn test_every_command_is_counted_once() {
        for (line, parts) in [
            ("git status", 1),
            ("cargo test | grep FAILED", 2),
            ("a | b | c", 3),
            ("git status && cargo build", 2),
            ("frobnicate | wibble", 2),
            ("RTK_DISABLED=1 git status", 1),
            ("cat <<EOF | wc -l", 1),
        ] {
            let tally = tally_of(&[line], &test_ctx(true));
            assert_eq!(tally.total_commands, parts, "wrong part count for {line:?}");
        }
    }

    /// A command that runs rtk counts as adoption however its first word is
    /// spelled, and `rtk bypass` (or its deprecated alias `rtk proxy`) does
    /// not, as in `rtk session`.
    #[test]
    fn test_every_spelling_of_rtk_counts_as_adoption() {
        let tally = tally_of(
            &[
                "rtk git status",
                "rtk\tgit status",
                "'rtk' git status",
                "\\rtk git status",
                "rtk bypass git status",
                "rtk proxy git status",
            ],
            &test_ctx(false),
        );

        assert_eq!(tally.total_commands, 6);
        assert_eq!(tally.already_rtk, 4);
        assert!(tally.unsupported.is_empty());
    }

    /// Classification normalises an absolute path and the rewriter does not
    /// match one, so the hook leaves `/usr/bin/git status` alone. The saving is
    /// still there for anyone who writes `git status`, and dropping the command
    /// would hide both the saving and the disagreement behind it (#1053).
    #[test]
    fn test_a_command_the_rewriter_never_matches_is_still_an_opportunity() {
        let tally = tally_of(&["/usr/bin/git status"], &test_ctx(true));

        assert_eq!(tally.total_commands, 1);
        assert_eq!(counts(&tally.opportunities), vec![("rtk git", 1)]);
        assert!(tally.supported.is_empty());
        assert_eq!(tally.already_rtk, 0);
    }

    /// The two tables are filled from two buckets, and nothing downstream can
    /// tell them apart once they are rows — so the assembly has to keep them
    /// straight.
    #[test]
    fn test_each_bucket_reaches_its_own_table() {
        let mut tally = Tally::default();
        tally.add(&scanned("cargo test | grep FAILED", None), &test_ctx(false));

        let rows = tally.take_rows();

        assert_eq!(
            rows.supported
                .iter()
                .map(|e| e.rtk_equivalent)
                .collect::<Vec<_>>(),
            vec!["rtk grep"],
            "what the hook would take"
        );
        assert_eq!(
            rows.opportunities
                .iter()
                .map(|e| e.rtk_equivalent)
                .collect::<Vec<_>>(),
            vec!["rtk cargo"],
            "what it would take a changed command to collect"
        );
    }

    /// Rows are ranked by what they would save, so the first line of the report
    /// is the one worth acting on.
    #[test]
    fn test_entries_are_ranked_by_saving() {
        let mut map: HashMap<&'static str, SupportedBucket> = HashMap::new();
        for (rtk_cmd, tokens) in [("rtk git", 10), ("rtk cargo", 900), ("rtk ls", 300)] {
            map.insert(
                rtk_cmd,
                SupportedBucket {
                    rtk_equivalent: rtk_cmd,
                    category: "build",
                    count: 1,
                    total_output_tokens: tokens,
                    total_raw_output_tokens: tokens * 2,
                    estimated_occurrences: 0,
                    command_counts: HashMap::new(),
                },
            );
        }
        let ranked: Vec<&str> = into_entries(map).iter().map(|e| e.rtk_equivalent).collect();
        assert_eq!(ranked, vec!["rtk cargo", "rtk ls", "rtk git"]);
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
    fn test_estimate_hook_coverage_false_for_deferred_wrapper() {
        assert!(!estimate_hook_coverage_with_verdict(
            "env bash -c 'git push'",
            "env bash -c 'git push'",
            PermissionVerdict::Defer,
            &test_ctx(true),
        ));
        assert!(estimate_hook_coverage_with_verdict(
            "env bash -c 'git push'",
            "env bash -c 'git push'",
            PermissionVerdict::Ask,
            &test_ctx(true),
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
    fn test_without_rtk_disabled_keeps_every_other_word() {
        for (env_prefix, expected) in [
            ("RTK_DISABLED=1 ", "docker ps"),
            ("sudo RTK_DISABLED=1 ", "sudo docker ps"),
            ("RTK_DISABLED=1 sudo -E ", "sudo -E docker ps"),
            ("FOO=1 RTK_DISABLED=true ", "FOO=1 docker ps"),
            ("FOO='a  b' RTK_DISABLED=1 ", "FOO='a  b' docker ps"),
            ("RTK_DISABLED='1 2' ", "docker ps"),
            ("RTK_DISABLED=$CI ", "docker ps"),
            ("sudo", "sudo docker ps"),
        ] {
            assert_eq!(
                without_rtk_disabled(env_prefix, "docker ps"),
                expected,
                "{env_prefix:?}"
            );
        }
    }

    #[test]
    fn test_disabled_segment_is_judged_as_typed_without_the_bypass() {
        // The analytics peel strips `sudo` so the bypass can be *detected*, but the
        // counterfactual ("would the hook have rewritten this without the
        // RTK_DISABLED=") keeps the wrapper. `docker ps` always rewrites; `sudo
        // docker ps` does not (#146 / test_rewrite_sudo_passthrough), so a
        // sudo-wrapped bypass recovers no savings: it is judged as the line
        // without the assignment, not as a bypass.
        fn judge(raw: &str, ctx: &CoverageContext) -> DisabledSegment {
            let (env_prefix, actual_cmd) = strip_disabled_prefix_for_analytics(raw);
            judge_disabled_segment(raw, env_prefix, actual_cmd, CommandStart::Pipeline, ctx)
        }
        let unbypassed = |line: &str| DisabledSegment::Unbypassed(line.to_string());
        let ctx = test_ctx(true);
        assert_eq!(
            judge("sudo RTK_DISABLED=1 docker ps", &ctx),
            unbypassed("sudo docker ps")
        );
        assert_eq!(
            judge("RTK_DISABLED=1 env -i git status", &ctx),
            unbypassed("env -i git status")
        );
        // A wrapper's script too complex to read rewrites nothing of the line.
        let line = "git status; bash -c 'a[1 # 2]=b'";
        assert!(!estimate_hook_coverage_with_verdict(
            line,
            line,
            PermissionVerdict::TooComplex,
            &ctx
        ));
        // The stripped segment alone would say yes.
        assert!(estimate_hook_coverage(
            "sudo RTK_DISABLED=1 docker ps",
            "docker ps",
            &ctx
        ));
        // …and it becomes a real bypass once the user declares sudo transparent.
        let mut ctx_sudo = test_ctx(true);
        ctx_sudo.normalized_transparent_prefixes = vec!["sudo".to_string()];
        assert_eq!(
            judge("sudo RTK_DISABLED=1 docker ps", &ctx_sudo),
            DisabledSegment::Bypass
        );
        assert_eq!(
            judge("RTK_DISABLED=1 sudo docker ps", &ctx_sudo),
            DisabledSegment::Bypass
        );
        // Only the RTK_DISABLED= word is removed, whole: a bare bypass, one next
        // to another assignment, and a quoted or expanded value are all bypasses.
        for raw in [
            "RTK_DISABLED=1 docker ps",
            "FOO=1 RTK_DISABLED=true cargo test",
            "RTK_DISABLED='1 2' docker ps",
            "RTK_DISABLED=$CI docker ps",
        ] {
            assert_eq!(judge(raw, &ctx), DisabledSegment::Bypass, "{raw}");
        }
        // No hook installed: nothing was bypassed; the command is a plain miss.
        assert_eq!(
            judge("RTK_DISABLED=1 docker ps", &test_ctx(false)),
            unbypassed("docker ps")
        );
        assert_eq!(
            judge("RTK_DISABLED=1 frobnicate x", &ctx),
            DisabledSegment::Unhandled
        );
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
        // Read as the hook reads it: bash joins this into a substitution.
        assert!(!estimate_hook_coverage_with_verdict(
            "git status $\\\n(date)",
            "git status $\\\n(date)",
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
        // `rewrite_cmd` instead of `permission_cmd`, this assertion would start failing
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
    fn test_is_already_rtk_excludes_bypass_escape_hatch() {
        // rtk#3148 secondary finding: `rtk bypass` deliberately bypasses filtering,
        // so it must not count as coverage.
        assert!(!is_already_rtk("rtk bypass grep -n foo bar.py"));
    }

    #[test]
    fn test_is_already_rtk_false_for_unrelated_command() {
        assert!(!is_already_rtk("grep -n foo bar.py"));
    }

    /// The first word decides, as bash ends it (at a space, a tab or a newline)
    /// and removes its quoting.
    #[test]
    fn test_is_already_rtk_reads_the_first_word() {
        for cmd in [
            "rtk\tgit status",
            "  rtk  ls",
            "rtk\nls",
            "rtk",
            "rtk proxyfoo x",
            "'rtk' git status",
            "\\rtk git status",
        ] {
            assert!(is_already_rtk(cmd), "{cmd:?}");
        }
        for cmd in [
            "rtk\tproxy git status",
            "rtk  proxy ls",
            "'rtk' 'proxy' ls",
            "rtkx ls",
            "rtk\r ls",
            "xrtk ls",
        ] {
            assert!(!is_already_rtk(cmd), "{cmd:?}");
        }
    }

    #[test]
    fn test_head_words_are_the_first_two_words_as_written() {
        for (cmd, expected) in [
            ("git status -s", &["git", "status"][..]),
            ("git  status", &["git", "status"]),
            ("git\tstatus\t-s", &["git", "status"]),
            ("  git \t status", &["git", "status"]),
            ("  git  ", &["git"]),
            ("grep 'a b' src", &["grep", "'a b'"]),
            ("cat f\\ g h", &["cat", "f\\ g"]),
            ("", &[]),
        ] {
            assert_eq!(head_words(cmd), expected, "{cmd:?}");
        }
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
