//! Scans AI coding sessions to find commands that could benefit from RTK filtering.

pub(crate) mod families;
pub mod fish_script;
pub mod provider;
pub mod registry;
mod report;
pub mod rules;
pub(crate) mod shell_wrapper;

use anyhow::Result;
use chrono::{DateTime, Utc};
use std::cell::OnceCell;
use std::collections::HashMap;

use provider::{ClaudeProvider, ExtractedCommand, SessionProvider};
use registry::{
    Call, ChainPart, Classification, ExcludePattern, category_avg_tokens, classify_command_at,
    strip_disabled_prefix_for_analytics,
};
use report::{DiscoverReport, SupportedEntry, UnsupportedEntry};

use crate::core::cmdline::lexer::{self, JoinedLine, tokenize, words};
use crate::core::cmdline::rtk::rtk_invocation;
use crate::core::cmdline::walker::Position;
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

/// Everything `hook_coverage` and the bypass counterfactual need to judge a command,
/// besides the command (and `tool_use_id`) itself — built once per `discover` run
/// and passed by reference to every per-command call.
///
/// Grouped into one struct rather than several individual positional params: with
/// `excluded`/`transparent_prefixes` both `Vec<String>`, a swapped argument order
/// at a call site would compile silently and misclassify RTK_DISABLED bypass
/// coverage (code-review finding on rtk-ai/rtk#3206's fixup round).
///
/// `exclude_patterns`/`normalized_transparent_prefixes` are the rewrite's exclude-pattern
/// regexes and normalized prefixes, precompiled once per scan here instead of once per
/// call that `registry::read_call` asks it about.
struct CoverageContext {
    hook_log: HashMap<String, HookDecisionRecord>,
    hook_installed: bool,
    rules: PermissionRules,
    exclude_patterns: Vec<ExcludePattern>,
    normalized_transparent_prefixes: Vec<String>,
}

/// Determine whether a command of a call was (or, absent a log entry, likely would have
/// been) routed through RTK by the installed PreToolUse hook.
///
/// Not for an `RTK_DISABLED=`-bypassed command: whether the bypass cost a saving is
/// `judge_disabled_segment`'s question (its doc says why a measured row cannot answer
/// it), and one that cost none ran raw, so `Tally::add` takes it as not covered.
///
/// The caller splits a chained command (`a && b`) into parts and calls this once
/// per part with the *same* `tool_use_id`, since Claude Code's PreToolUse hook
/// fires once per tool call for the whole raw command. `call` MUST hold the full,
/// unsplit command line, as the hook reads it, even though this is
/// being asked about one part of it — a chain-wide deny rule matching only `a` must
/// still keep the hook off `b`, and `permissions::check_command_with_rules` only
/// re-derives the chain from whatever string it's given. The real rewrite is
/// genuinely best-effort *per segment* though — it can rewrite just `a` and leave an
/// unsupported `b` untouched — so the estimate asks it: `reached` says whether the
/// rewrite of the whole call, read once ([`registry::read_call`]), edits this part. A
/// `hook_decisions` row is one decision for the whole call, with no per-segment
/// detail, so a measured answer can still be reported covered/missed based on a sibling
/// segment's rewrite fate; that residual imprecision is unavoidable without
/// per-segment hook logging. `Estimated` is the fallback for history that predates
/// `hook_decisions` logging — but note it does NOT simply fade out over time:
/// `hook_decisions` rows are pruned by the same `DEFAULT_HISTORY_DAYS` retention
/// window as the rest of the tracking DB (see `Tracker::cleanup_hook_decisions`),
/// so any `--since` reaching further back than that window permanently needs
/// `Estimated`, indefinitely, not just during an initial backfill period.
fn hook_coverage(
    call: &CallCheck<'_>,
    reached: bool,
    tool_use_id: &str,
    ctx: &CoverageContext,
) -> Coverage {
    if let Some(record) = ctx.hook_log.get(tool_use_id) {
        return Coverage::Measured(record.decision.is_covered());
    }
    Coverage::Estimated(reached && call.allows_rewrite(ctx))
}

/// A call as the hook's permission checks read it: its whole line, with what they decide
/// about it. That verdict is the call's, whichever of its commands the report asks about,
/// and reading it costs the length of the line, so it is worked out once, the first time a
/// command asks.
struct CallCheck<'a> {
    /// The exact full string the real hook would have seen — the whole raw command line,
    /// chain operators and any `RTK_DISABLED=` prefix intact — since that's what Claude
    /// Code's permission engine actually evaluates (`hooks::hook_cmd::decide_hook_action`
    /// never splits or strips it; see `hook_coverage`'s doc comment for why an isolated
    /// chain segment would blind the deny/ask check to sibling segments).
    line: &'a str,
    allows_rewrite: OnceCell<bool>,
}

impl<'a> CallCheck<'a> {
    fn new(line: &'a str) -> Self {
        Self {
            line,
            allows_rewrite: OnceCell::new(),
        }
    }

    /// Whether the hook's checks of the whole line leave it free to rewrite a command of
    /// it ([`allows_rewrite_with_verdict`]).
    ///
    /// Checks `hook_installed` before touching `rules`/the lexer: with no hook installed
    /// the answer is always "not covered", so there's no reason to pay for the
    /// permission-verdict lookup or the unattestable-construct scan, which on a history
    /// with no hook ever installed would be all wasted work.
    fn allows_rewrite(&self, ctx: &CoverageContext) -> bool {
        *self.allows_rewrite.get_or_init(|| {
            ctx.hook_installed && {
                let verdict = permissions::check_command_with_rules_and_prefixes(
                    self.line,
                    &ctx.rules.deny,
                    &ctx.rules.ask,
                    &ctx.rules.allow,
                    &ctx.normalized_transparent_prefixes,
                );
                allows_rewrite_with_verdict(self.line, verdict, ctx)
            }
        })
    }
}

/// Pure core of [`CallCheck::allows_rewrite`], taking the permission verdict directly so
/// it's testable without depending on real config files. Mirrors the real hook's own
/// decision order (`hooks::decision::decide`): a `Deny`-rule match or an
/// unattestable construct means the hook would never have rewritten this, regardless
/// of registry support.
fn allows_rewrite_with_verdict(
    permission_cmd: &str,
    verdict: PermissionVerdict,
    ctx: &CoverageContext,
) -> bool {
    ctx.hook_installed
        && !matches!(
            verdict,
            PermissionVerdict::Deny | PermissionVerdict::Defer | PermissionVerdict::TooComplex
        )
        && !lexer::line_is_unattestable(permission_cmd)
}

/// The segment `strip_disabled_prefix_for_analytics` split into `env_prefix` and
/// `actual_cmd`, with the words that set `RTK_DISABLED` removed, as the rewrite's refusal
/// reads them (`registry::sets_rtk_disabled`, so `RTK_DISABLED+=1` too), and every other
/// word kept verbatim, wrapper included. Words are split the way the rest of discover
/// splits them (`lexer::tokenize` + `lexer::words`), so a quoted or expanded value
/// (`RTK_DISABLED='1 2'`, `RTK_DISABLED=$CI`) goes as a whole.
fn without_rtk_disabled(env_prefix: &str, actual_cmd: &str) -> String {
    let words = words(env_prefix, &tokenize(env_prefix));
    let mut line = String::with_capacity(env_prefix.len() + actual_cmd.len() + 1);
    for ((i, word), sets) in words
        .iter()
        .enumerate()
        .zip(registry::sets_rtk_disabled(&words))
    {
        let end = words.get(i + 1).map_or(env_prefix.len(), |next| next.start);
        if !sets {
            line.push_str(&env_prefix[word.start..end]);
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

/// What a command of the call, carrying `RTK_DISABLED=` in front of it, contributes to
/// the report: would the call's hook have covered it if the bypass weren't there?
///
/// Never answered from a measured `hook_decisions` row: a bypassed command's logged
/// decision is always `Defer` (`get_rewritten` refuses an `RTK_DISABLED=` prefix, #345),
/// which only confirms the hook stepped aside and says nothing about the counterfactual
/// asked here. For the same reason `Tally::add` gives a bypassed command that is no bypass
/// `Coverage::Estimated(false)` and never asks `hook_coverage` about it: it ran raw.
///
/// `call` must hold the *full, unsplit* command line, as the hook reads it, prefix
/// and any sibling chain segments intact — what the real hook's permission check
/// would have seen (see `hook_coverage`'s doc comment for why an isolated segment would
/// blind the deny/ask check to siblings). `reached_without_bypass` is the rewrite's
/// own answer for the command, in its place in the call, once its bypass is read past
/// (`registry::Call::reached_without_bypass`): `docker ps` rewrites by default,
/// while `sudo docker ps` only does when `sudo` is in `hooks.transparent_prefixes`, and
/// a consumer at the end of a pipeline is left raw wherever the rewrite leaves it.
fn judge_disabled_segment(
    call: &CallCheck<'_>,
    reached_without_bypass: bool,
    env_prefix: &str,
    actual_cmd: &str,
    start: Position,
    ctx: &CoverageContext,
) -> DisabledSegment {
    if !matches!(
        classify_command_at(actual_cmd, start),
        Classification::Supported { .. }
    ) {
        return DisabledSegment::Unhandled;
    }
    if reached_without_bypass && call.allows_rewrite(ctx) {
        DisabledSegment::Bypass
    } else {
        DisabledSegment::Unbypassed(without_rtk_disabled(env_prefix, actual_cmd))
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
    /// because the call had no measured output, or given a share of one the call's other
    /// commands share.
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
/// `covered` alone cannot be trusted, because a `hook_decisions` row is one
/// decision for the whole tool call and knows nothing of where the command sat. `cat f.log`
/// becomes `rtk read` alone but not in `cat f.log | grep ERROR`, so the producer
/// of every such line would read as adoption RTK never had.
///
/// `reached` is the rewrite's answer for the command as it would be without an
/// `RTK_DISABLED=` bypass, if it carries one: a bypassed command the report did not
/// count as a bypass is judged like the same command without the assignment.
fn disposition(reached: bool, covered: bool, consumes_a_pipe: bool) -> Disposition {
    if reached && covered {
        return Disposition::AlreadyRtk;
    }
    if reached {
        return Disposition::Missed;
    }
    // A consumer the rewriter does not take (`tail` in `cargo test | tail -1`,
    // `grep -f patterns.txt` after any producer) stays raw because rewriting it
    // would change what the pipeline prints: no change to the command reaches
    // that saving.
    if consumes_a_pipe {
        return Disposition::Dropped;
    }
    // Anything else RTK has a rule for is an opportunity, whatever kept the
    // rewriter off it — feeding a pipe whose consumer throws the output away,
    // or being written in a form the rewriter does not match, like an absolute
    // path (#1053). Dropping those hides real savings and hides the
    // disagreements between classification and rewriting that cause them.
    Disposition::Opportunity
}

/// The category average that sizes `command`, a command of `category`.
fn average_tokens(category: &str, command: &str) -> usize {
    let head = head_words(command);
    category_avg_tokens(category, head.get(1).copied().unwrap_or(""))
}

/// One command of a call, read once for the output shares and for its row.
struct PartReading<'a> {
    /// The command as written.
    text: &'a str,
    /// The assignments in front of it, and the command after them.
    env_prefix: &'a str,
    command: &'a str,
    /// Whether it is a pipeline's stage after the first, reading the output of the one
    /// before it.
    consumes: bool,
    /// Where it starts: after a pipe, `time` is the program `time`.
    start: Position,
    /// What the part classifies as, as written.
    class: Classification,
    /// Whether it runs rtk, which the classifier leaves aside although rtk runs a tool.
    runs_rtk: bool,
}

impl<'a> PartReading<'a> {
    fn of(parts: &[ChainPart<'a>]) -> Vec<Self> {
        parts
            .iter()
            .map(|part| {
                let (env_prefix, command) = strip_disabled_prefix_for_analytics(part.text);
                let start = if part.consumes_pipe {
                    Position::STAGE
                } else {
                    Position::START
                };
                Self {
                    text: part.text,
                    env_prefix,
                    command,
                    consumes: part.consumes_pipe,
                    start,
                    class: classify_command_at(part.text, start),
                    runs_rtk: part.runs_rtk,
                }
            })
            .collect()
    }

    /// What weighs its share of the call's output: its category average when it has a
    /// rule, the average of any command when it runs rtk, [`NO_RULE_WEIGHT`] otherwise.
    fn weight(&self) -> usize {
        match &self.class {
            Classification::Supported { category, .. } => average_tokens(category, self.command),
            _ if self.runs_rtk => category_avg_tokens("", ""),
            _ => NO_RULE_WEIGHT,
        }
    }
}

/// The weight of a command that has no rule and does not run rtk, below every category
/// average. Such a command is most often a builtin or a small tool (`echo`, `cd`,
/// `export`, `mkdir`), so `git status && echo ok` leaves most of its output to
/// `git status`.
const NO_RULE_WEIGHT: usize = 10;

/// What a command's row counts of its call's output, in bytes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Share {
    /// The whole measurement: the call ran this command alone.
    Known(usize),
    /// Its part of a measurement the call's other commands share.
    Estimated(usize),
    /// There is no measurement for it: its category average stands in.
    Average,
}

/// What each part's row counts of a call's measured output.
///
/// A call has one measured output, and which of its commands wrote what of it cannot be told
/// apart: every command of the call takes a share in proportion to its category average,
/// marked estimated, and a call of one command takes it whole. A command that runs rtk weighs
/// the average of any command, and one with no rule [`NO_RULE_WEIGHT`]; neither share goes to
/// a row.
fn output_shares(parts: &[PartReading<'_>], output_len: Option<usize>) -> Vec<Share> {
    let Some(len) = output_len else {
        return vec![Share::Average; parts.len()];
    };
    if let [_] = parts {
        return vec![Share::Known(len)];
    }
    let weights: Vec<usize> = parts.iter().map(PartReading::weight).collect();
    let total: usize = weights.iter().sum();
    weights
        .iter()
        .map(|&weight| Share::Estimated((len as f64 * weight as f64 / total as f64) as usize))
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
        let line = JoinedLine::of(&ext_cmd.command);
        let command: &str = &line.text;
        // One answer per command, worked out once for the line: what the
        // rewriter does to one command depends on the ones around it, and a
        // report that counts a command the rewriter passes over promises a
        // saving the hook does not take.
        let Call {
            parts,
            reached,
            reached_without_bypass,
        } = registry::read_call(
            &line,
            &ctx.exclude_patterns,
            &ctx.normalized_transparent_prefixes,
        );
        let readings = PartReading::of(&parts);
        let shares = output_shares(&readings, ext_cmd.output_len);
        let call = CallCheck::new(command);
        for (index, reading) in readings.into_iter().enumerate() {
            let part = reading.text;
            let start = reading.start;
            self.total_commands += 1;

            // Detect RTK_DISABLED= bypass before classification
            let (env_prefix, actual_cmd) = (reading.env_prefix, reading.command);
            let bypassed = prefix_contains_rtk_disabled(env_prefix);
            // What the rewrite does with the command: as written, or without its bypass.
            let part_reached = if bypassed {
                reached_without_bypass[index]
            } else {
                reached[index]
            };
            let unbypassed;
            let part = if bypassed {
                // A bypass counts only if the hook would have covered the line
                // without it — otherwise RTK_DISABLED= bypassed nothing (hook not
                // installed / excluded / would defer / would deny / the wrapper is
                // not rewritable, or left raw where it sits in its pipeline), and
                // flagging it would be false advice. See `judge_disabled_segment`'s
                // doc comment for why this must never go through `hook_coverage`'s
                // measured-log path. A segment that bypassed nothing falls through
                // to the normal classification as typed without the assignment,
                // instead of vanishing from the report.
                match judge_disabled_segment(
                    &call,
                    part_reached,
                    env_prefix,
                    actual_cmd,
                    start,
                    ctx,
                ) {
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

            // An unbypassed line has lost its `RTK_DISABLED=`, which classification
            // reads past only in front (`sudo RTK_DISABLED=1 docker ps` is `sudo`).
            let class = if bypassed {
                classify_command_at(part, start)
            } else {
                reading.class
            };
            match class {
                Classification::Supported {
                    rtk_equivalent,
                    category,
                    estimated_savings_pct,
                    status,
                } => {
                    // A bypassed command ran raw, whatever a measured row says of the call.
                    let coverage = if bypassed {
                        Coverage::Estimated(false)
                    } else {
                        hook_coverage(&call, part_reached, &ext_cmd.tool_use_id, ctx)
                    };

                    let map =
                        match disposition(part_reached, coverage.is_covered(), reading.consumes) {
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

                    let (output_tokens, estimated) = match shares[index] {
                        Share::Known(len) => (len / 4, false),
                        Share::Estimated(len) => (len / 4, true),
                        Share::Average => (average_tokens(category, actual_cmd), true),
                    };
                    if estimated {
                        bucket.estimated_occurrences += 1;
                    }

                    let head = head_words(part);

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

    /// Every combination of the three facts, written out rather than derived, so
    /// that changing the rule means changing this table and saying why. Deriving
    /// the expected value would restate the rule and agree with it no matter
    /// what the rule became.
    #[test]
    fn test_every_disposition_is_pinned() {
        use Disposition::{AlreadyRtk, Dropped, Missed, Opportunity};
        // reached, covered, consumes_a_pipe
        let table = [
            ((false, false, false), Opportunity),
            ((false, false, true), Dropped),
            // Coverage says yes, but the rewriter never took this command —
            // that is the producer of `cat f.log | grep ERROR`, and calling it
            // adoption is what the `reached` gate exists to stop.
            ((false, true, false), Opportunity),
            ((false, true, true), Dropped),
            ((true, false, false), Missed),
            ((true, false, true), Missed),
            ((true, true, false), AlreadyRtk),
            ((true, true, true), AlreadyRtk),
        ];
        assert_eq!(table.len(), 8, "one row per combination of three facts");
        for ((reached, covered, consumes_a_pipe), expected) in table {
            assert_eq!(
                disposition(reached, covered, consumes_a_pipe),
                expected,
                "reached={reached} covered={covered} consumes_a_pipe={consumes_a_pipe}"
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

    /// What the estimate says of a command of a call whose line the permission rules gave
    /// `verdict`, when the rewrite has (`reached`) or has not edited it, without reading
    /// any config file.
    fn estimate_with_verdict(
        permission_cmd: &str,
        reached: bool,
        verdict: PermissionVerdict,
        ctx: &CoverageContext,
    ) -> bool {
        let call = CallCheck {
            line: permission_cmd,
            allows_rewrite: OnceCell::from(allows_rewrite_with_verdict(
                permission_cmd,
                verdict,
                ctx,
            )),
        };
        hook_coverage(&call, reached, "", ctx).is_covered()
    }

    /// What the estimate says of a command of the call `line`, the rewrite having `reached`
    /// it, with the permission rules of `ctx`.
    fn estimate(line: &str, reached: bool, ctx: &CoverageContext) -> bool {
        hook_coverage(&CallCheck::new(line), reached, "", ctx).is_covered()
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

    /// A call of one command takes its whole output, measured, whatever it redirects and
    /// whatever in it is no command of its own.
    #[test]
    fn test_a_call_of_one_command_takes_its_measured_output_whole() {
        for command in [
            "cargo test",
            "cargo test\n# done",
            "cargo test\n\n",
            "cargo test > test.log 2>&1",
            // What follows a subshell is no command.
            "(cargo test) > test.log 2>&1",
            "(cargo test)",
            // A heredoc or `$((` call is not cut into commands: it is one.
            "cargo fmt >/dev/null 2>&1; git commit -F - <<'EOF'\nmsg\nEOF",
            "cargo fmt >/dev/null 2>&1\ngit status\n: $((n + 1))",
        ] {
            assert_eq!(
                row_of_a_call(command, "rtk cargo"),
                (0, MEASURED),
                "{command:?}"
            );
        }
    }

    /// The commands of a call share its output by their category averages, each share an
    /// estimate. A command with no rule weighs less than any category, so after
    /// `cargo test` an `echo` takes the smaller share, and a redirect moves nothing.
    #[test]
    fn test_the_commands_of_a_call_share_its_output_by_category_average() {
        let cargo_avg = category_avg_tokens("Cargo", "test");
        let other = NO_RULE_WEIGHT;
        let read_avg = category_avg_tokens("Files", "");
        let git_avg = category_avg_tokens("Git", "status");
        // Even the lightest category outweighs a command with no rule.
        let (estimated, git) = row_of_a_call("git status && echo ok", "rtk git");
        assert_eq!(
            (estimated, git),
            (1, MEASURED * git_avg / (git_avg + other))
        );
        assert!(git > MEASURED / 2, "{git}");
        for command in [
            "cargo test; echo done",
            "cargo test\necho done",
            "cd x && cargo test",
            "X=$(date) && cargo test",
            "cargo test\nf() { :; }",
        ] {
            let (estimated, cargo) = row_of_a_call(command, "rtk cargo");
            assert_eq!(estimated, 1, "{command:?}");
            assert_eq!(
                cargo,
                MEASURED * cargo_avg / (cargo_avg + other),
                "{command:?}"
            );
            assert!(cargo > MEASURED / 2, "{command:?}: {cargo}");
        }
        // Where a command sends its output changes nothing, and what follows a subshell
        // is no command.
        for command in [
            "cargo test > test.log 2>&1; tail -20 test.log",
            "(cargo test) > test.log 2>&1; tail -20 test.log",
        ] {
            assert_eq!(
                row_of_a_call(command, "rtk cargo"),
                (1, MEASURED * cargo_avg / (cargo_avg + read_avg)),
                "{command:?}"
            );
            assert_eq!(
                row_of_a_call(command, "rtk read"),
                (1, MEASURED * read_avg / (cargo_avg + read_avg)),
                "{command:?}"
            );
        }
        // A group's closing word and the redirects after it are no command either: the
        // call is `{ cargo test`, which has no rule, and `tail`.
        assert_eq!(
            row_of_a_call(
                "{ cargo test; } > test.log 2>&1; tail -20 test.log",
                "rtk read"
            ),
            (1, MEASURED * read_avg / (other + read_avg))
        );
    }

    /// A stage after a pipe is classified where it stands, where `time` is the program
    /// `time`, also behind a group the pipe follows.
    #[test]
    fn test_a_stage_after_a_pipe_is_classified_where_it_stands() {
        for command in ["foo | time cargo test", "{ foo; } | time cargo test"] {
            let tally = tally_of(&[command], &test_ctx(false));
            assert!(
                tally.unsupported.contains_key("time cargo"),
                "{command:?}: {:?}",
                tally.unsupported.keys()
            );
        }
    }

    /// Every stage of a pipeline is a command of the call, and takes its share as any
    /// other does.
    #[test]
    fn test_pipeline_stages_share_the_measured_output() {
        let cargo_avg = category_avg_tokens("Cargo", "test");
        let other = NO_RULE_WEIGHT;
        let git_avg = category_avg_tokens("Git", "status");
        assert_eq!(
            row_of_a_call("cargo test | foo", "rtk cargo"),
            (1, MEASURED * cargo_avg / (cargo_avg + other))
        );
        assert_eq!(
            row_of_a_call("cargo test | foo\necho done", "rtk cargo"),
            (1, MEASURED * cargo_avg / (cargo_avg + 2 * other))
        );
        // After a pipe, `time` is the program `time`, which has no rule.
        for command in [
            "foo | time cargo test; git status",
            "git status; : | time echo x",
        ] {
            assert_eq!(
                row_of_a_call(command, "rtk git").1,
                MEASURED * git_avg / (git_avg + 2 * other),
                "{command:?}"
            );
        }
    }

    /// The commands of a block that a loop spreads over lines are counted once each:
    /// the loop's own words are no command.
    #[test]
    fn test_a_loops_reserved_words_are_no_commands() {
        let tally = tally_of(
            &["ls | while read c; do\n  echo a; rc=1\ndone; echo done"],
            &test_ctx(false),
        );
        assert_eq!(tally.total_commands, 5);
        assert!(
            tally.unsupported.is_empty(),
            "{:?}",
            tally.unsupported.keys()
        );
    }

    /// What a subshell's redirects follow is no command, and no row of the report.
    #[test]
    fn test_what_follows_a_subshell_is_no_row() {
        for command in [
            "(git status) 2>&1; git log",
            "(cd x && ls) > out; git status",
        ] {
            let tally = tally_of(&[command], &test_ctx(false));
            assert!(tally.unsupported.is_empty(), "{command:?}");
        }
        let tally = tally_of(&["(git status) 2>&1; git log"], &test_ctx(false));
        assert_eq!(tally.total_commands, 2);
    }

    /// A group a stage after a pipe opens is cut into its commands, which all read the
    /// pipe: no bracket ends up in a row, and a consumer inside the group is no
    /// opportunity, whichever command of the group it is.
    #[test]
    fn test_a_group_after_a_pipe_reads_it() {
        let tally = tally_of(
            &[
                "git log | (head -5; tail -5)",
                "git log | { head -5; tail -5; }",
            ],
            &test_ctx(false),
        );
        assert_eq!(tally.total_commands, 6);
        assert!(
            tally.unsupported.is_empty(),
            "{:?}",
            tally.unsupported.keys()
        );
        let opportunities: Vec<&str> = tally.opportunities.keys().copied().collect();
        assert_eq!(opportunities, ["rtk git"]);

        // So is a compound a reserved word opens.
        let tally = tally_of(
            &[
                "git log | while read l; do echo; tail -1; done",
                "ls | case x in a) cat;; b) tail -1;; esac",
            ],
            &test_ctx(false),
        );
        // `cat` and `tail -1` read the pipe: no `rtk read` saving is offered for them.
        assert!(
            !tally.opportunities.contains_key("rtk read"),
            "{:?}",
            tally.opportunities.keys()
        );

        let tally = tally_of(
            &[
                "cargo build 2>&1 | (grep error || true)",
                "cat file | (cd dir && git apply)",
                "git status | head -5 | (cat)",
                "(git log | head)",
            ],
            &test_ctx(false),
        );
        assert_eq!(tally.total_commands, 11);
        let mut unsupported: Vec<_> = tally.unsupported.keys().map(String::as_str).collect();
        unsupported.sort();
        assert_eq!(unsupported, ["cat", "git apply", "head"]);
    }

    /// A call of comments alone runs no command, and counts none.
    #[test]
    fn test_a_call_of_comments_counts_no_command() {
        let tally = tally_of(&["# just a note", "# a\n# b"], &test_ctx(false));
        assert_eq!(tally.total_commands, 0);
        let tally = tally_of(&["git status; # done"], &test_ctx(false));
        assert_eq!(tally.total_commands, 1);
    }

    /// A CRLF block is read line by line, as the rewrite reads it: a `\r` in a comment
    /// leaves the line's command whole.
    #[test]
    fn test_a_crlf_block_counts_each_line() {
        let tally = tally_of(&["git status # x\r\ngit log"], &test_ctx(false));
        assert_eq!(tally.total_commands, 2);
        assert_eq!(tally.supported["rtk git"].count, 2);
    }

    /// A backquote is one substitution, however many pipes it holds: the call is two
    /// commands.
    #[test]
    fn test_a_backquote_is_one_substitution() {
        let tally = tally_of(&["git log | grep `cat f | head -1`"], &test_ctx(false));
        assert_eq!(tally.total_commands, 2);
        assert!(
            tally.unsupported.is_empty(),
            "{:?}",
            tally.unsupported.keys()
        );
    }

    /// Without a measurement, a command is sized by its own category average, read past
    /// the assignments in front of it as its share would be.
    #[test]
    fn test_an_unmeasured_command_is_sized_past_its_assignments() {
        let tally = tally_of(&["FOO=1 cargo test"], &test_ctx(false));
        assert_eq!(
            tally.supported["rtk cargo"].total_raw_output_tokens,
            category_avg_tokens("Cargo", "test")
        );
    }

    /// A share too small to round above nothing is still an estimate.
    #[test]
    fn test_a_share_rounded_to_nothing_stays_an_estimate() {
        let mut tally = Tally::default();
        tally.add(
            &scanned("git status; cargo test", Some(12)),
            &test_ctx(false),
        );
        assert_eq!(tally.supported["rtk git"].estimated_occurrences, 1);
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

    /// A command with no row, or already through rtk, may have written it all. One that
    /// runs rtk weighs the average of any command, the others the weight of no rule.
    #[test]
    fn test_a_command_with_no_row_keeps_its_share() {
        let git_avg = category_avg_tokens("Git", "status");
        let through_rtk = category_avg_tokens("", "");
        for (command, other) in [
            ("git status && ./build.sh", NO_RULE_WEIGHT),
            ("git status\nrtk cargo test", through_rtk),
            ("git status\nrtk bypass cargo test", through_rtk),
            ("git status\nRTK_DISABLED=1 rtk cargo test", through_rtk),
            ("git status; env FOO=1 rtk cargo test", through_rtk),
            ("git status; echo rtk", NO_RULE_WEIGHT),
            ("git status\nbash ./build.sh", NO_RULE_WEIGHT),
            ("git status; sed -n 1,9p big.log", NO_RULE_WEIGHT),
            ("git status\npython3 -c 'print(1)'", NO_RULE_WEIGHT),
            ("git status; $(pwd)/run_tests.sh", NO_RULE_WEIGHT),
            ("git status; `pwd`/build.sh", NO_RULE_WEIGHT),
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

        // One too complex to join is refused by the rewrite, and stays one text here.
        let tally = tally_of(&["git status\na[1 # 2]=b\ngit log"], &test_ctx(false));
        assert_eq!(tally.total_commands, 1);
    }

    /// A block the rewrite refuses to read line by line is one command, as the rewrite
    /// leaves it whole: a line after a subshell or a group adds none.
    #[test]
    fn test_a_refused_block_is_one_command() {
        for command in [
            "( cargo build )\ncargo test\ngit log",
            "(\n  cd x\n  make\n)\ngit status",
            "if x; then y; fi\ngit status",
            "while x; do y; done\ngit log",
            "{ cargo build; }\ncargo test",
            "git log | (\nhead\n)\ngit status",
        ] {
            let tally = tally_of(&[command], &test_ctx(false));
            assert_eq!(tally.total_commands, 1, "{command:?}");
        }
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
        // in` and `echo 1` for the `case`, whose `esac` is no command, and two for
        // the chain.
        assert_eq!(tally.total_commands, 6);
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
        assert!(tally.opportunities.is_empty());
    }

    /// A consumer the rewriter does not take is kept raw whatever stands in front of it,
    /// whether its rule never runs at the end of a pipeline or this form of it is unsafe
    /// there (`grep -f`).
    #[test]
    fn test_a_consumer_the_rewriter_never_takes_is_in_neither_table() {
        for command in [
            "foo | tail -1",
            "make 2>&1 | head -20",
            "cargo test 2>&1 | sort | tail -5",
            "rtk cargo test 2>&1 | tail -1",
            // What stands in front of the pipe is a group's own syntax and no command,
            // and the stage after it still reads a pipe.
            "{ git status; } 2>&1 | tail -1",
            "for f in a; do git log; done | tail -5",
            "(git status) | tail -1",
        ] {
            let tally = tally_of(&[command], &test_ctx(false));
            assert!(!tally.opportunities.contains_key("rtk read"), "{command:?}");
        }
        for command in ["foo | git log", "foo | grep -f patterns.txt"] {
            let tally = tally_of(&[command], &test_ctx(false));
            assert!(tally.opportunities.is_empty(), "{command:?}");
            assert!(tally.supported.is_empty(), "{command:?}");
        }

        // Only a pipe makes a consumer: after `;` the command stands on its own.
        let tally = tally_of(&["git status; tail -f app.log"], &test_ctx(false));
        assert_eq!(counts(&tally.opportunities), vec![("rtk read", 1)]);
    }

    /// The other way round, a producer left in front of a consumer the rewriter
    /// takes is an opportunity: rewriting it is the change to make.
    #[test]
    fn test_a_producer_left_in_front_of_a_rewritten_consumer_is_an_opportunity() {
        let tally = tally_of(&["cargo test | grep FAILED"], &test_ctx(false));

        assert_eq!(counts(&tally.opportunities), vec![("rtk cargo", 1)]);
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

    /// A line's measured output is shared by the commands that wrote it. Handing
    /// it to every part multiplies the estimate by the number of parts.
    #[test]
    fn test_measured_output_is_shared_by_the_commands_that_wrote_it() {
        let ctx = test_ctx(false);
        let mut tally = Tally::default();
        tally.add(&scanned("cargo test | grep FAILED", Some(40_000)), &ctx);

        let grep = tally.supported.get("rtk grep").expect("grep is reported");
        let cargo = tally
            .opportunities
            .get("rtk cargo")
            .expect("cargo is reported");
        let shared = grep.total_raw_output_tokens + cargo.total_raw_output_tokens;
        assert!(
            (40_000 / 4 - 1..=40_000 / 4).contains(&shared),
            "the shares add up to the measured size: {shared}"
        );
        assert_eq!(
            (grep.estimated_occurrences, cargo.estimated_occurrences),
            (1, 1)
        );
    }

    /// A bypassed command the hook would have taken is a bypass, not an opportunity and
    /// not adoption, and it still counts once in the denominator.
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

    /// Only an assignment to `RTK_DISABLED` by that name is a bypass.
    #[test]
    fn test_other_variables_holding_the_text_are_no_bypass() {
        for line in [
            "XRTK_DISABLED=1 git status",
            "M=RTK_DISABLED=1 git status",
            "M='RTK_DISABLED=1' git status",
        ] {
            let tally = tally_of(&[line], &test_ctx(true));
            assert_eq!(tally.rtk_disabled_count, 0, "{line:?}");
        }
        assert_eq!(
            tally_of(&["RTK_DISABLED=1 git status"], &test_ctx(true)).rtk_disabled_count,
            1
        );
    }

    /// A bypass is counted when the hook would have taken the command where it sits in its
    /// call, without the assignment: a consumer at the end of a pipeline is left raw
    /// wherever the rewrite leaves it, and a producer is skipped when the rewrite takes the
    /// last stage, so neither lost a saving to the bypass. What the other commands of the
    /// call carry is read as written.
    #[test]
    fn test_a_bypass_counts_only_where_the_rewrite_would_take_the_command() {
        for (line, bypasses) in [
            ("RTK_DISABLED=1 git status", 1),
            ("RTK_DISABLED=1 git status && RTK_DISABLED=1 git log", 2),
            ("RTK_DISABLED=1 cargo test | tail -1", 1),
            ("cargo test | RTK_DISABLED=1 grep FAILED", 1),
            ("RTK_DISABLED=1 docker ps | RTK_DISABLED=1 grep web", 1),
            ("cargo test | RTK_DISABLED=1 tail -n 5 f.log", 0),
            ("RTK_DISABLED=1 cat f.log | grep ERROR", 0),
            ("RTK_DISABLED=1 cargo test | RTK_DISABLED=1 tail -1", 0),
            ("git status | RTK_DISABLED=1 cat | head -3", 0),
            ("RTK_DISABLED=1 git status | tee out.txt | head", 0),
        ] {
            let tally = tally_of(&[line], &test_ctx(true));
            assert_eq!(tally.rtk_disabled_count, bypasses, "{line:?}");
        }
        let tally = tally_of(
            &["RTK_DISABLED=1 docker ps | RTK_DISABLED=1 grep web"],
            &test_ctx(true),
        );
        assert_eq!(
            tally.rtk_disabled_cmds.keys().collect::<Vec<_>>(),
            ["grep web"]
        );
    }

    /// Whichever way the assignment is spelled, the command is the one the rewrite reads
    /// without it: the append spelling the rewrite refuses is a bypass as `=` is.
    #[test]
    fn test_every_spelling_the_rewrite_refuses_is_a_bypass() {
        for line in [
            "RTK_DISABLED=1 git status",
            "RTK_DISABLED+=1 git status",
            "FOO=1 RTK_DISABLED+=1 git status",
            "env RTK_DISABLED=1 git status",
            "RTK_DISABLED='1 2' git status",
            "RTK_DISABLED=$CI git status",
        ] {
            let tally = tally_of(&[line], &test_ctx(true));
            assert_eq!(
                (tally.rtk_disabled_count, tally.supported.len()),
                (1, 0),
                "{line:?}"
            );
        }
    }

    /// A bypassed command that is no bypass is judged like the same command without the
    /// assignment: a missed saving when the rewrite would take it where it sits and the
    /// hook's checks refuse, an opportunity or a dropped consumer when the rewrite would
    /// leave it, exactly as for the plain command.
    #[test]
    fn test_a_bypassed_command_that_is_no_bypass_is_judged_like_the_plain_one() {
        let mut excluded = test_ctx(true);
        excluded.exclude_patterns = registry::compile_exclude_patterns(&["git status".to_string()]);
        let mut denied = test_ctx(true);
        denied.rules.deny = vec!["cd /sensitive".to_string()];
        for (bypassed, plain, ctx) in [
            // Left raw where it sits: the producer of a pipeline whose last stage is taken.
            (
                "RTK_DISABLED=1 cat f.log | grep ERROR",
                "cat f.log | grep ERROR",
                test_ctx(true),
            ),
            // The user's exclusion.
            ("RTK_DISABLED=1 git status", "git status", excluded),
            // The hook's checks of the line refuse what the rewrite would take.
            (
                "cd /sensitive && RTK_DISABLED=1 git status",
                "cd /sensitive && git status",
                denied,
            ),
            // No hook installed.
            ("RTK_DISABLED=1 git status", "git status", test_ctx(false)),
        ] {
            let rows = |tally: &Tally| {
                (
                    counts(&tally.supported),
                    counts(&tally.opportunities),
                    tally.already_rtk,
                )
            };
            let tally = tally_of(&[bypassed], &ctx);
            assert_eq!(
                rows(&tally),
                rows(&tally_of(&[plain], &ctx)),
                "{bypassed:?} as {plain:?}"
            );
            assert_eq!(tally.rtk_disabled_count, 0, "{bypassed:?}");
        }
        // Neither is a missed saving. The consumer is dropped, and the producer next to it
        // is an opportunity, since a bypassed stage is no safe consumer and the hook leaves
        // the producer raw too.
        let tally = tally_of(&["RTK_DISABLED=1 cat f.log | grep ERROR"], &test_ctx(true));
        assert_eq!(counts(&tally.opportunities), vec![("rtk read", 1)]);
        assert!(tally.supported.is_empty());
        let tally = tally_of(
            &["cargo test | RTK_DISABLED=1 tail -n 5 f.log"],
            &test_ctx(true),
        );
        assert_eq!(counts(&tally.opportunities), vec![("rtk cargo", 1)]);
        assert!(tally.supported.is_empty());
    }

    /// A bypassed command ran raw, whatever the hook logged for the call: a measured row is
    /// one decision for the whole call, and it says nothing of the command the bypass kept
    /// out of the rewrite. Such a command that is no bypass is not covered, so it is a
    /// missed saving when the rewrite would take it, though the call's other commands were
    /// measured as covered.
    #[test]
    fn test_a_measured_row_never_covers_a_bypassed_command() {
        let tally_with_row = |line: &str, ctx: &mut CoverageContext| {
            ctx.hook_log
                .insert("toolu_1".to_string(), record(HookOutcome::Allow));
            let mut command = scanned(line, None);
            command.tool_use_id = "toolu_1".to_string();
            let mut tally = Tally::default();
            tally.add(&command, ctx);
            tally
        };
        let mut denied = test_ctx(true);
        denied.rules.deny = vec!["cd /sensitive".to_string()];
        for (line, mut ctx, adopted) in [
            // The hook is no longer installed, so no bypass was lost: a plain miss.
            ("RTK_DISABLED=1 git status", test_ctx(false), 0),
            (
                "RTK_DISABLED=1 git status && cargo test",
                test_ctx(false),
                1,
            ),
            // A deny rule keeps the hook off the line, as the line's checks say.
            ("cd /sensitive && RTK_DISABLED=1 git status", denied, 0),
        ] {
            let tally = tally_with_row(line, &mut ctx);
            assert_eq!(tally.already_rtk, adopted, "{line:?}");
            assert_eq!(counts(&tally.supported), vec![("rtk git", 1)], "{line:?}");
            assert_eq!(tally.rtk_disabled_count, 0, "{line:?}");
        }
        // A command that is not bypassed takes its measured row.
        let tally = tally_with_row("git status", &mut test_ctx(false));
        assert_eq!(tally.already_rtk, 1);
    }

    /// The estimate of a command is the rewrite's: one the rewrite of the call leaves
    /// alone is not covered, however the permission rules and the hook's install state
    /// stand.
    #[test]
    fn test_the_estimate_is_false_when_the_rewrite_leaves_the_command_alone() {
        let ctx = test_ctx(true);
        assert!(estimate("cargo test | tail -1", true, &ctx));
        assert!(!estimate("cargo test | tail -1", false, &ctx));
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
    fn test_the_estimate_false_when_hook_not_installed() {
        // No hook installed → the raw command genuinely ran unfiltered; still a miss.
        assert!(!estimate_with_verdict(
            "grep -n foo bar.py",
            true,
            PermissionVerdict::Default,
            &test_ctx(false),
        ));
    }

    #[test]
    fn test_the_estimate_false_for_deferred_wrapper() {
        assert!(!estimate_with_verdict(
            "env bash -c 'git push'",
            true,
            PermissionVerdict::Defer,
            &test_ctx(true),
        ));
        assert!(estimate_with_verdict(
            "env bash -c 'git push'",
            true,
            PermissionVerdict::Ask,
            &test_ctx(true),
        ));
    }

    #[test]
    fn test_the_estimate_true_for_rewritable_command() {
        let ctx = test_ctx(true);
        assert!(estimate_with_verdict(
            "grep -n foo bar.py",
            true,
            PermissionVerdict::Default,
            &ctx,
        ));
        assert!(estimate_with_verdict(
            "ls -la",
            true,
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
            ("RTK_DISABLED+=1 ", "docker ps"),
            ("FOO=1 RTK_DISABLED+=1 ", "FOO=1 docker ps"),
            ("env RTK_DISABLED+=1 ", "env RTK_DISABLED+=1 docker ps"),
        ] {
            assert_eq!(
                without_rtk_disabled(env_prefix, "docker ps"),
                expected,
                "{env_prefix:?}"
            );
        }
    }

    /// The report's judgement of `raw`, a call of one command carrying `RTK_DISABLED=`: what
    /// the rewrite of the call does with it once the bypass is read past.
    fn judge(raw: &str, ctx: &CoverageContext) -> DisabledSegment {
        let line = JoinedLine::of(raw);
        let call = registry::read_call(
            &line,
            &ctx.exclude_patterns,
            &ctx.normalized_transparent_prefixes,
        );
        let (env_prefix, actual_cmd) = strip_disabled_prefix_for_analytics(raw);
        judge_disabled_segment(
            &CallCheck::new(raw),
            call.reached_without_bypass[0],
            env_prefix,
            actual_cmd,
            Position::START,
            ctx,
        )
    }

    #[test]
    fn test_disabled_segment_is_judged_as_typed_without_the_bypass() {
        // The analytics peel strips `sudo` so the bypass can be *detected*, but the
        // counterfactual ("would the hook have rewritten this without the
        // RTK_DISABLED=") keeps the wrapper. `docker ps` always rewrites; `sudo
        // docker ps` does not (#146 / test_rewrite_sudo_passthrough), so a
        // sudo-wrapped bypass recovers no savings: it is judged as the line
        // without the assignment, not as a bypass.
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
        // Its row is the line without the assignment, not the line as typed.
        let tally = tally_of(&["sudo RTK_DISABLED=1 docker ps"], &ctx);
        let rows: Vec<&String> = tally.unsupported.keys().collect();
        assert_eq!(rows, vec!["sudo docker"]);
        // A wrapper's script too complex to read rewrites nothing of the line.
        let line = "git status; bash -c 'a[1 # 2]=b'";
        assert!(!estimate_with_verdict(
            line,
            true,
            PermissionVerdict::TooComplex,
            &ctx
        ));
        // A sudo-wrapped bypass is a real one once the user declares sudo transparent.
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
    fn test_the_estimate_false_for_unattestable_construct() {
        // The hook itself defers on unattestable constructs (substitutions, etc.),
        // so a command containing one was never actually rewritten — genuine miss.
        assert!(!estimate_with_verdict(
            "git status $(rm -rf /tmp/x)",
            true,
            PermissionVerdict::Default,
            &test_ctx(true),
        ));
        // Read as the hook reads it: bash joins this into a substitution.
        assert!(!estimate_with_verdict(
            "git status $\\\n(date)",
            true,
            PermissionVerdict::Default,
            &test_ctx(true),
        ));
    }

    #[test]
    fn test_the_estimate_false_when_excluded_by_config() {
        // Even with the hook installed, a command the user excluded via config
        // was never rewritten — the hook stepped aside, so it's a genuine miss.
        let mut ctx = test_ctx(true);
        assert_eq!(
            tally_of(&["grep -n foo bar.py"], &ctx).already_rtk,
            1,
            "the hook takes it unless excluded"
        );
        ctx.exclude_patterns = registry::compile_exclude_patterns(&["grep".to_string()]);
        let tally = tally_of(&["grep -n foo bar.py"], &ctx);
        assert_eq!(tally.already_rtk, 0);
        assert_eq!(counts(&tally.opportunities), vec![("rtk grep", 1)]);
    }

    #[test]
    fn test_the_estimate_false_when_denied() {
        // A command matching a permissions.deny rule is never auto-rewritten by the
        // real hook (it defers to Claude Code's native deny handling) — counting it
        // as covered would over-count savings for commands the hook actually denied.
        assert!(!estimate_with_verdict(
            "rm -rf /",
            true,
            PermissionVerdict::Deny,
            &test_ctx(true),
        ));
    }

    #[test]
    fn test_the_estimate_uses_full_raw_command_for_permission_check() {
        // Regression: the RTK_DISABLED= bypass path must check permission rules
        // against the *full* raw command (prefix intact) — matching what the real
        // hook's permission engine actually evaluates. An exact-match deny rule on
        // the literal prefixed command only matches the raw form, not the stripped one.
        let mut ctx = test_ctx(true);
        ctx.rules.deny = vec!["RTK_DISABLED=1 git status".to_string()];

        assert_ne!(
            judge("RTK_DISABLED=1 git status", &ctx),
            DisabledSegment::Bypass,
            "a deny rule matching the raw (prefixed) command must still apply"
        );

        // Sanity check: the same deny rule does NOT match the command without the
        // prefix — if the check ran against anything but the call's line, the
        // assertion above would come back true instead of false.
        assert!(estimate("git status", true, &ctx));
    }

    #[test]
    fn test_the_estimate_sees_chain_wide_deny_from_full_raw_command() {
        // Regression: check_command_with_rules re-derives the chain from whatever
        // string it's handed, so a deny rule matching only a *sibling* segment
        // (e.g. "cd /sensitive" in "cd /sensitive && grep -rn foo .") is invisible
        // if the isolated segment being evaluated ("grep -rn foo .") is what gets
        // passed as the permission-check command — exactly what the real hook
        // would NOT do (it always evaluates the whole raw line). Must pass the
        // full, unsplit command as the call's line, not the isolated segment.
        let mut ctx = test_ctx(true);
        ctx.rules.deny = vec!["cd /sensitive".to_string()];
        let full_chain = "cd /sensitive && grep -rn foo .";
        let isolated_segment = "grep -rn foo .";

        assert!(
            !estimate(full_chain, true, &ctx),
            "a deny rule matching a sibling chain segment must still deny the whole chain"
        );

        // Sanity check demonstrating the bug this guards against: checking
        // permissions against the isolated segment alone (instead of the full
        // chain) misses the sibling's deny match entirely.
        assert!(
            estimate(isolated_segment, true, &ctx),
            "checking the isolated segment alone should miss the sibling's deny rule \
             (demonstrates why the call's line must be the full chain, not a segment)"
        );
    }

    /// The report puts each command of a call to the hook's checks of the whole line: a
    /// deny rule on one command keeps the hook off the others, a bypass included.
    #[test]
    fn test_the_report_checks_the_whole_call_for_each_command() {
        let mut denied = test_ctx(true);
        denied.rules.deny = vec!["cd /sensitive".to_string()];
        let grep = "cd /sensitive && grep -rn foo .";
        let bypass = "cd /sensitive && RTK_DISABLED=1 git status";

        let tally = tally_of(&[grep, bypass], &denied);
        assert_eq!((tally.already_rtk, tally.rtk_disabled_count), (0, 0));
        assert_eq!(
            counts(&tally.supported),
            vec![("rtk git", 1), ("rtk grep", 1)]
        );

        // Without the rule, the hook takes the one and the other bypassed it.
        let tally = tally_of(&[grep, bypass], &test_ctx(true));
        assert_eq!((tally.already_rtk, tally.rtk_disabled_count), (1, 1));
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

        let coverage = hook_coverage(&CallCheck::new("git status"), true, "toolu_1", &ctx);
        assert!(matches!(coverage, Coverage::Measured(true)));
        assert!(coverage.is_covered());
        assert!(!coverage.is_estimated());
    }

    #[test]
    fn test_hook_coverage_measured_deny_is_a_genuine_miss() {
        let mut ctx = test_ctx(true);
        ctx.hook_log
            .insert("toolu_1".to_string(), record(HookOutcome::Deny));

        let coverage = hook_coverage(&CallCheck::new("rm -rf /"), true, "toolu_1", &ctx);
        assert!(matches!(coverage, Coverage::Measured(false)));
        assert!(!coverage.is_covered());
    }

    #[test]
    fn test_hook_coverage_falls_back_to_estimate_when_no_log_row() {
        // No log entry for this tool_use_id — predates logging (or non-Claude) —
        // falls back to the current-state heuristic, flagged as estimated.
        let coverage = hook_coverage(
            &CallCheck::new("ls -la"),
            true,
            "toolu_missing",
            &test_ctx(true),
        );
        assert!(coverage.is_estimated());
    }
}
