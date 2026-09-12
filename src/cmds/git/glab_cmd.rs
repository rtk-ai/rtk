//! GitLab CLI (glab) command output compression.
//!
//! Provides token-optimized alternatives to verbose `glab` commands.
//! Mirrors gh_cmd.rs patterns, adapted for glab-specific differences:
//! - MR notation: `!42` (not `#42`)
//! - States: `opened`/`merged`/`closed` (lowercase, not UPPER)
//! - Author: `author.username` (not `author.login`)
//! - URL: `web_url` (not `url`)
//! - Description: `description` (not `body`)
//! - Merge status: `merge_status` ("can_be_merged") (not `mergeable`)
//! - Pipeline: `head_pipeline.status` (not `statusCheckRollup`)

use super::git;
use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::args_utils;
use crate::core::runner::{self, RunOptions};
use crate::core::truncate::{CAP_LIST, CAP_WARNINGS};
use crate::core::utils::{ok_confirmation, resolved_command, strip_ansi, truncate};
use anyhow::Result;
use regex::Regex;
use serde_json::Value;
use std::process::Command;
use std::sync::LazyLock;

static HTML_COMMENT_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<!--.*?-->").unwrap());
static BADGE_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*\[!\[[^\]]*\]\([^)]*\)\]\([^)]*\)\s*$").unwrap());
static IMAGE_ONLY_LINE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*!\[[^\]]*\]\([^)]*\)\s*$").unwrap());
static HORIZONTAL_RULE_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"(?m)^\s*(?:---+|\*\*\*+|___+)\s*$").unwrap());
static MULTI_BLANK_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n{3,}").unwrap());
static MR_URL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"/-/merge_requests/(\d+)").unwrap());
/// Match GitLab CI section markers: section_start/end:timestamp:name[0K
static SECTION_MARKER_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"section_(?:start|end):\d+:[a-z0-9_]+(?:\x1b\[0K|\[0K)*").unwrap()
});
/// Match bare bracket ANSI-like codes without ESC prefix: [0K, [0;m, [36;1m, etc.
static BARE_ANSI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[[\d;]+[A-Za-z]").unwrap());

/// Filter markdown body to remove noise while preserving meaningful content.
/// Removes HTML comments, badge lines, image-only lines, horizontal rules,
/// and collapses excessive blank lines. Preserves code blocks untouched.
fn filter_markdown_body(body: &str) -> String {
    if body.is_empty() {
        return String::new();
    }

    let mut result = String::new();
    let mut remaining = body;

    loop {
        let fence_pos = remaining
            .find("```")
            .or_else(|| remaining.find("~~~"))
            .map(|pos| {
                let fence = if remaining[pos..].starts_with("```") {
                    "```"
                } else {
                    "~~~"
                };
                (pos, fence)
            });

        match fence_pos {
            Some((start, fence)) => {
                let before = &remaining[..start];
                result.push_str(&filter_markdown_segment(before));

                let after_open = start + fence.len();
                let code_start = remaining[after_open..]
                    .find('\n')
                    .map(|p| after_open + p + 1)
                    .unwrap_or(remaining.len());

                let close_pos = remaining[code_start..]
                    .find(fence)
                    .map(|p| code_start + p + fence.len());

                match close_pos {
                    Some(end) => {
                        result.push_str(&remaining[start..end]);
                        let after_close = remaining[end..]
                            .find('\n')
                            .map(|p| end + p + 1)
                            .unwrap_or(remaining.len());
                        result.push_str(&remaining[end..after_close]);
                        remaining = &remaining[after_close..];
                    }
                    None => {
                        result.push_str(&remaining[start..]);
                        remaining = "";
                    }
                }
            }
            None => {
                result.push_str(&filter_markdown_segment(remaining));
                break;
            }
        }
    }

    result.trim().to_string()
}

/// Filter a markdown segment that is NOT inside a code block.
fn filter_markdown_segment(text: &str) -> String {
    let mut s = HTML_COMMENT_RE.replace_all(text, "").to_string();
    s = BADGE_LINE_RE.replace_all(&s, "").to_string();
    s = IMAGE_ONLY_LINE_RE.replace_all(&s, "").to_string();
    s = HORIZONTAL_RULE_RE.replace_all(&s, "").to_string();
    s = MULTI_BLANK_RE.replace_all(&s, "\n\n").to_string();
    s
}

/// State icon for MR/issue states (glab uses lowercase).
fn state_icon(state: &str, ultra_compact: bool) -> &'static str {
    if ultra_compact {
        match state {
            "opened" => "O",
            "merged" => "M",
            "closed" => "C",
            _ => "?",
        }
    } else {
        match state {
            "opened" => "[open]",
            "merged" => "[merged]",
            "closed" => "[closed]",
            _ => "?",
        }
    }
}

/// Pipeline status icon. Non-compact mode uses text tags for parity with
/// `gh_cmd.rs` (avoids multi-byte terminal rendering quirks; aligns with the
/// rest of the codebase). Ultra-compact keeps single-char density.
fn pipeline_icon(status: &str, ultra_compact: bool) -> &'static str {
    if ultra_compact {
        match status {
            "success" => "+",
            "failed" => "x",
            "canceled" | "cancelled" => "X",
            "running" | "pending" => "~",
            "skipped" => "-",
            _ => "?",
        }
    } else {
        match status {
            "success" => "[ok]",
            "failed" => "[fail]",
            "canceled" | "cancelled" => "[cancel]",
            "running" => "[run]",
            "pending" => "[pend]",
            "skipped" => "[skip]",
            _ => "?",
        }
    }
}

/// Extract MR number from glab output URL or text.
fn extract_mr_number(text: &str) -> Option<String> {
    MR_URL_RE
        .captures(text)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// pflag reads a literal `--` as the value of a value-taking flag rather than as the
/// end-of-options boundary: `glab mr view --page -- 5` rejects `--` as the page number.
fn glab_value() -> ValueSpec {
    ValueSpec::value().claiming_dash_dash()
}

/// glab's inherited flags, available on every subcommand. `--group` is only glab's own on the
/// list/search family, but rtk's `Commands::Glab` injects it into any subcommand's args, so it
/// has to be classified everywhere or its value is read as the identifier.
fn inherited_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match kind {
        TokenKind::Long => matches!(name, "repo" | "group").then(glab_value),
        TokenKind::Short => matches!(name, "R" | "g").then(glab_value),
        _ => None,
    }
}

/// `glab mr view` and `glab issue view` share one grammar — their value-taking flag sets are
/// identical on both glab 1.36 and 1.117. The table is the union over those two: 1.117 added
/// `--jq` and `-F/--output` to the `-p/-P/-R` set 1.36 had.
fn view_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let own = match kind {
        TokenKind::Long => matches!(name, "jq" | "output" | "page" | "per-page").then(glab_value),
        TokenKind::Short => matches!(name, "F" | "p" | "P").then(glab_value),
        _ => None,
    };
    own.or_else(|| inherited_takes_value(kind, name))
}

/// `glab mr merge`: `-s` is the boolean `--squash` here but the value-taking `--sha` on
/// `glab mr approve`, so the two cannot share a table.
fn mr_merge_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let own = match kind {
        TokenKind::Long => matches!(name, "message" | "sha" | "squash-message").then(glab_value),
        TokenKind::Short => (name == "m").then(glab_value),
        _ => None,
    };
    own.or_else(|| inherited_takes_value(kind, name))
}

/// `glab mr approve`: `-s/--sha` is its only own value-taking flag.
fn mr_approve_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let own = match kind {
        TokenKind::Long => (name == "sha").then(glab_value),
        TokenKind::Short => (name == "s").then(glab_value),
        _ => None,
    };
    own.or_else(|| inherited_takes_value(kind, name))
}

/// `glab mr note`: `-m/--message` carries the comment body. 1.117 moved it onto the
/// `note create` sub-subcommand; 1.36 still takes it here, so the table keeps both.
fn mr_note_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let own = match kind {
        TokenKind::Long => (name == "message").then(glab_value),
        TokenKind::Short => (name == "m").then(glab_value),
        _ => None,
    };
    own.or_else(|| inherited_takes_value(kind, name))
}

/// `glab mr update`: nearly every flag takes a value, so a missing entry turns a label, a
/// title or a username into the MR number.
fn mr_update_takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    let own = match kind {
        TokenKind::Long => matches!(
            name,
            "assignee"
                | "attach"
                | "description"
                | "description-file"
                | "label"
                | "milestone"
                | "reviewer"
                | "target-branch"
                | "title"
                | "unlabel"
        )
        .then(glab_value),
        TokenKind::Short => matches!(name, "a" | "d" | "l" | "m" | "t" | "u").then(glab_value),
        _ => None,
    };
    own.or_else(|| inherited_takes_value(kind, name))
}

/// Splits `args` into the MR/issue identifier — the first free positional under `takes_value` —
/// and everything else, verbatim and in order. glab keeps reading positionals past `--`
/// (`glab mr view -- 42` views MR 42), so the search reaches past the boundary, but only while
/// the boundary escapes a lone token that is not itself flag-shaped.
fn split_identifier(
    args: &[String],
    takes_value: &dyn Fn(TokenKind, &str) -> Option<ValueSpec>,
) -> (Option<String>, Vec<String>) {
    let tokens = arg_tokenizer::tokenize_grammar(args, takes_value, Dialect::Posix);

    // Re-emitting the identifier in front of the `--` unescapes it along with whatever else
    // trailed it, so reach past the boundary only where that is a no-op: one token, still read
    // as a positional once unescaped. Anything else glab rejects on arity whatever rtk sends.
    let searchable = match arg_tokenizer::dashdash_index(&tokens) {
        Some(index) if !escapes_a_bare_positional(args, &tokens[index + 1..]) => &tokens[..index],
        _ => &tokens[..],
    };

    let id_index = searchable
        .iter()
        .find(|t| t.is_free_positional())
        .map(|t| t.source_index);
    let extra = args
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != id_index)
        .map(|(_, arg)| arg.clone())
        .collect();
    (id_index.map(|i| args[i].clone()), extra)
}

/// Whether `escaped`, the tokens behind a `--`, is a single argument that stays a positional
/// once the boundary no longer shields it.
fn escapes_a_bare_positional(args: &[String], escaped: &[arg_tokenizer::Token<'_>]) -> bool {
    let [only] = escaped else { return false };
    let i = only.source_index;
    arg_tokenizer::tokenize(&args[i..=i])
        .first()
        .is_some_and(|t| t.kind == TokenKind::Positional)
}

/// Check if user explicitly requested JSON/custom output format.
/// When present, passthrough to avoid double JSON injection. `--jq` counts: its result is the
/// user's own projection, and reformatting it as an MR would print a summary of fields the
/// projection does not have.
fn has_output_flag(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--output" || a == "-F" || a == "--json" || a == "--jq")
}

/// Check if view subcommand should passthrough (--web, --comments, etc.).
fn should_passthrough_view(extra_args: &[String]) -> bool {
    extra_args
        .iter()
        .any(|a| a == "--web" || a == "--comments" || a == "--output" || a == "-F")
}

/// Run a glab command that emits JSON and filter through `filter_fn`.
/// On JSON parse failure (glab returns plain text for empty results),
/// fall back to the raw stdout.
fn run_glab_json<F>(cmd: Command, label: &str, filter_fn: F) -> Result<i32>
where
    F: Fn(&Value) -> String,
{
    runner::run_filtered(
        cmd,
        "glab",
        label,
        |stdout| match serde_json::from_str::<Value>(stdout) {
            Ok(json) => filter_fn(&json),
            Err(_) => stdout.to_string(),
        },
        RunOptions::stdout_only()
            .early_exit_on_failure()
            .no_trailing_newline(),
    )
}

/// Run a glab command with token-optimized output. `repo` and `group` are rtk's own `-R`/`-g`
/// options, which it forwards to glab.
pub fn run(
    subcommand: &str,
    args: &[String],
    repo: Option<&str>,
    group: Option<&str>,
    verbose: u8,
    ultra_compact: bool,
) -> Result<i32> {
    // clap carves `subcommand` out of the same trailing region as `args`, so the stripped `--`
    // has to be restored over the whole region — over `args` alone it lands one token off.
    let mut region: Vec<String> = Vec::with_capacity(args.len() + 1);
    region.push(subcommand.to_string());
    region.extend_from_slice(args);
    let mut region = args_utils::restore_double_dash(&region);

    // Only rtk's own terminator reaches the head of that region — trailing_var_arg keeps every
    // later `--` — and glab never saw it: `glab mr -- view 42` prints the `mr` help instead of
    // dispatching, and `glab api -- projects/1 --paginate` is `Accepts 1 arg(s), received 2`.
    let rtk_terminator = arg_tokenizer::tokenize(&region)
        .into_iter()
        .find(|t| t.kind == TokenKind::DashDash && t.source_index <= 1)
        .map(|t| t.source_index);
    if let Some(index) = rtk_terminator {
        region.remove(index);
    }

    // glab reads everything past `--` as a positional, so -R/-g go in ahead of the boundary.
    let mut injected: Vec<String> = Vec::new();
    if let Some(r) = repo {
        injected.extend(["-R".to_string(), r.to_string()]);
    }
    if let Some(g) = group {
        injected.extend(["-g".to_string(), g.to_string()]);
    }
    if !injected.is_empty() {
        // Grammar-less: the sub-subcommand that picks a `takes_value` table is only known after
        // the split below, so a `--` that is some flag's own value reads as the boundary here.
        let at = arg_tokenizer::injection_point(&arg_tokenizer::tokenize(&region), region.len());
        region.splice(at..at, injected);
    }

    // Nothing to dispatch on — forward the region verbatim and let glab answer.
    let Some((subcommand, args)) = split_glab_region(&region) else {
        return run_passthrough_with_extra("glab", &[], &region);
    };

    // If the user explicitly requests a specific output format, passthrough unchanged.
    if has_output_flag(args) {
        return run_passthrough("glab", subcommand, args);
    }

    match subcommand {
        "mr" => run_mr(args, verbose, ultra_compact),
        "issue" => run_issue(args, verbose, ultra_compact),
        "ci" | "pipeline" => run_ci(args, verbose, ultra_compact),
        "release" => run_release(args, verbose, ultra_compact),
        "api" => run_api(args, verbose),
        _ => run_passthrough("glab", subcommand, args),
    }
}

/// Splits a restored `glab` region back into subcommand and remainder, the way clap did before
/// the `--` came back: a leading token that is neither a flag nor the boundary.
fn split_glab_region(region: &[String]) -> Option<(&str, &[String])> {
    let tokens = arg_tokenizer::tokenize(region);
    match tokens.first() {
        Some(token) if token.kind == TokenKind::Positional => {
            Some((region[0].as_str(), &region[1..]))
        }
        _ => None,
    }
}

// ── MR subcommands ──────────────────────────────────────────────────────

fn run_mr(args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return run_passthrough("glab", "mr", args);
    }

    match args[0].as_str() {
        "list" => mr_list(&args[1..], verbose, ultra_compact),
        "view" => mr_view(&args[1..], verbose, ultra_compact),
        "create" => mr_create(&args[1..], verbose),
        "merge" => mr_action("merge", "merged", &args[1..], verbose, &mr_merge_takes_value),
        "approve" => mr_action(
            "approve",
            "approved",
            &args[1..],
            verbose,
            &mr_approve_takes_value,
        ),
        "diff" => mr_diff(&args[1..], verbose),
        "note" => mr_action("note", "noted", &args[1..], verbose, &mr_note_takes_value),
        "update" => mr_action(
            "update",
            "updated",
            &args[1..],
            verbose,
            &mr_update_takes_value,
        ),
        _ => run_passthrough("glab", "mr", args),
    }
}

/// Format MR list JSON into compact output (pure function, testable).
fn format_mr_list(json: &Value, ultra_compact: bool) -> String {
    let mrs = match json.as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };
    if mrs.is_empty() {
        return if ultra_compact {
            "No MRs\n".to_string()
        } else {
            "No Merge Requests\n".to_string()
        };
    }

    let mut filtered = String::new();
    filtered.push_str(if ultra_compact {
        "MRs\n"
    } else {
        "Merge Requests\n"
    });

    let all_lines: Vec<String> = mrs
        .iter()
        .map(|mr| {
            let iid = mr["iid"].as_i64().unwrap_or(0);
            let title = mr["title"].as_str().unwrap_or("???");
            let state = mr["state"].as_str().unwrap_or("???");
            let author = mr["author"]["username"].as_str().unwrap_or("???");
            let icon = state_icon(state, ultra_compact);
            format!("  {} !{} {} ({})", icon, iid, truncate(title, 60), author)
        })
        .collect();
    const MAX_LIST: usize = CAP_LIST;
    for line in all_lines.iter().take(MAX_LIST) {
        filtered.push_str(&format!("{}\n", line));
    }
    if all_lines.len() > MAX_LIST {
        filtered.push_str(&format!("  … +{} more\n", all_lines.len() - MAX_LIST));
        let all_text = all_lines.join("\n");
        if let Some(hint) = crate::core::tee::force_tee_tail_hint(&all_text, "glab-mrs", MAX_LIST + 1) {
            filtered.push_str(&format!("  {}\n", hint));
        }
    }

    filtered
}

fn mr_list(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["mr", "list", "-F", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_glab_json(cmd, "mr list", |json| format_mr_list(json, ultra_compact))
}

/// Format MR view JSON into compact output (pure function, testable).
fn format_mr_view(json: &Value, ultra_compact: bool) -> String {
    let iid = json["iid"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["username"].as_str().unwrap_or("???");
    let web_url = json["web_url"].as_str().unwrap_or("");
    let merge_status = json["merge_status"].as_str().unwrap_or("unknown");
    let source_branch = json["source_branch"].as_str().unwrap_or("???");
    let target_branch = json["target_branch"].as_str().unwrap_or("???");

    let icon = state_icon(state, ultra_compact);

    let mut filtered = String::new();
    filtered.push_str(&format!("{} MR !{}: {}\n", icon, iid, title));
    filtered.push_str(&format!("  {}\n", author));

    let mergeable_str = match merge_status {
        "can_be_merged" => "[ok]",
        "cannot_be_merged" => "[conflict]",
        _ => "[?]",
    };
    filtered.push_str(&format!("  {} | {}\n", state, mergeable_str));
    filtered.push_str(&format!("  {} -> {}\n", source_branch, target_branch));

    if let Some(labels) = json["labels"].as_array() {
        let joined: Vec<&str> = labels.iter().filter_map(|v| v.as_str()).collect();
        if !joined.is_empty() {
            filtered.push_str(&format!("  Labels: {}\n", joined.join(", ")));
        }
    }

    if let Some(reviewers) = json["reviewers"].as_array() {
        let names: Vec<String> = reviewers
            .iter()
            .filter_map(|r| r["username"].as_str())
            .map(|u| format!("@{}", u))
            .collect();
        if !names.is_empty() {
            filtered.push_str(&format!("  Reviewers: {}\n", names.join(", ")));
        }
    }

    if let Some(pipeline) = json.get("head_pipeline") {
        if !pipeline.is_null() {
            let pipeline_status = pipeline["status"].as_str().unwrap_or("unknown");
            let p_icon = pipeline_icon(pipeline_status, ultra_compact);
            filtered.push_str(&format!("  Pipeline: {} {}\n", p_icon, pipeline_status));
        }
    }

    filtered.push_str(&format!("  {}\n", web_url));

    if let Some(desc) = json["description"].as_str() {
        if !desc.is_empty() {
            let desc_filtered = filter_markdown_body(desc);
            if !desc_filtered.is_empty() {
                filtered.push('\n');
                for line in desc_filtered.lines() {
                    filtered.push_str(&format!("  {}\n", line));
                }
            }
        }
    }

    filtered
}

fn mr_view(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    // `glab mr view` without an identifier defaults to the MR for the current branch.
    let (mr_number_opt, extra_args) = split_identifier(args, &view_takes_value);

    // Passthrough for --web, --comments, or explicit output format
    if should_passthrough_view(&extra_args) {
        let mut base: Vec<&str> = vec!["mr", "view"];
        if let Some(id) = mr_number_opt.as_deref() {
            base.push(id);
        }
        return run_passthrough_with_extra("glab", &base, &extra_args);
    }

    let mut cmd = resolved_command("glab");
    cmd.args(["mr", "view"]);
    if let Some(id) = mr_number_opt.as_deref() {
        cmd.arg(id);
    }
    cmd.args(["-F", "json"]);
    for arg in &extra_args {
        cmd.arg(arg);
    }
    let label = match mr_number_opt.as_deref() {
        Some(id) => format!("mr view {}", id),
        None => "mr view".to_string(),
    };
    run_glab_json(cmd, &label, |json| format_mr_view(json, ultra_compact))
}

fn mr_create(args: &[String], _verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["mr", "create"]);
    for arg in args {
        cmd.arg(arg);
    }
    runner::run_filtered(
        cmd,
        "glab",
        "mr create",
        |stdout| {
            // glab mr create outputs the URL on success
            let url = stdout.trim();
            let mr_num = extract_mr_number(url).unwrap_or_default();
            let detail = if !mr_num.is_empty() {
                format!("!{} {}", mr_num, url)
            } else {
                url.to_string()
            };
            ok_confirmation("created", &detail)
        },
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

fn mr_diff(args: &[String], _verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["mr", "diff"]);
    for arg in args {
        cmd.arg(arg);
    }
    runner::run_filtered(
        cmd,
        "glab",
        "mr diff",
        |stdout| {
            if stdout.trim().is_empty() {
                "No diff\n".to_string()
            } else {
                git::compact_diff(stdout, 500)
            }
        },
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

/// Generic MR action handler for merge/approve/note/update. Each takes its own `takes_value`
/// table: the MR number is only reported, but reading it out of a flag's value would put a
/// commit message or a label in the confirmation line.
fn mr_action(
    subcmd: &str,
    label: &str,
    args: &[String],
    _verbose: u8,
    takes_value: &dyn Fn(TokenKind, &str) -> Option<ValueSpec>,
) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["mr", subcmd]);
    for arg in args {
        cmd.arg(arg);
    }

    let mr_num = split_identifier(args, takes_value)
        .0
        .map(|id| format!("!{}", id))
        .unwrap_or_default();
    let label = label.to_string();
    runner::run_filtered(
        cmd,
        "glab",
        &format!("mr {}", subcmd),
        move |_stdout| ok_confirmation(&label, &mr_num),
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

// ── Issue subcommands ───────────────────────────────────────────────────

fn run_issue(args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return run_passthrough("glab", "issue", args);
    }

    match args[0].as_str() {
        "list" => issue_list(&args[1..], verbose, ultra_compact),
        "view" => issue_view(&args[1..], verbose),
        _ => run_passthrough("glab", "issue", args),
    }
}

/// Format issue list JSON into compact output (pure function, testable).
fn format_issue_list(json: &Value, ultra_compact: bool) -> String {
    let issues = match json.as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };
    if issues.is_empty() {
        return "No Issues\n".to_string();
    }

    let mut filtered = String::new();
    filtered.push_str("Issues\n");

    let all_lines: Vec<String> = issues
        .iter()
        .map(|issue| {
            let iid = issue["iid"].as_i64().unwrap_or(0);
            let title = issue["title"].as_str().unwrap_or("???");
            let state = issue["state"].as_str().unwrap_or("???");
            let icon = if ultra_compact {
                if state == "opened" { "O" } else { "C" }
            } else if state == "opened" {
                "[open]"
            } else {
                "[closed]"
            };
            format!("  {} #{} {}", icon, iid, truncate(title, 60))
        })
        .collect();
    const MAX_LIST: usize = CAP_LIST;
    for line in all_lines.iter().take(MAX_LIST) {
        filtered.push_str(&format!("{}\n", line));
    }
    if all_lines.len() > MAX_LIST {
        filtered.push_str(&format!("  … +{} more\n", all_lines.len() - MAX_LIST));
        let all_text = all_lines.join("\n");
        if let Some(hint) = crate::core::tee::force_tee_tail_hint(&all_text, "glab-issues", MAX_LIST + 1) {
            filtered.push_str(&format!("  {}\n", hint));
        }
    }

    filtered
}

fn issue_list(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["issue", "list", "-F", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_glab_json(cmd, "issue list", |json| {
        format_issue_list(json, ultra_compact)
    })
}

/// Format issue view JSON into compact output (pure function, testable).
fn format_issue_view(json: &Value) -> String {
    let iid = json["iid"].as_i64().unwrap_or(0);
    let title = json["title"].as_str().unwrap_or("???");
    let state = json["state"].as_str().unwrap_or("???");
    let author = json["author"]["username"].as_str().unwrap_or("???");
    let web_url = json["web_url"].as_str().unwrap_or("");

    let icon = if state == "opened" {
        "[open]"
    } else {
        "[closed]"
    };

    let mut filtered = String::new();
    filtered.push_str(&format!("{} Issue #{}: {}\n", icon, iid, title));
    filtered.push_str(&format!("  Author: @{}\n", author));
    filtered.push_str(&format!("  Status: {}\n", state));
    filtered.push_str(&format!("  URL: {}\n", web_url));

    if let Some(desc) = json["description"].as_str() {
        if !desc.is_empty() {
            let desc_filtered = filter_markdown_body(desc);
            if !desc_filtered.is_empty() {
                filtered.push_str("\n  Description:\n");
                for line in desc_filtered.lines() {
                    filtered.push_str(&format!("    {}\n", line));
                }
            }
        }
    }

    filtered
}

fn issue_view(args: &[String], _verbose: u8) -> Result<i32> {
    // Let glab emit its own error message when the identifier is missing rather than pre-rejecting.
    let (issue_number_opt, extra_args) = split_identifier(args, &view_takes_value);

    if should_passthrough_view(&extra_args) {
        let mut base: Vec<&str> = vec!["issue", "view"];
        if let Some(id) = issue_number_opt.as_deref() {
            base.push(id);
        }
        return run_passthrough_with_extra("glab", &base, &extra_args);
    }

    let mut cmd = resolved_command("glab");
    cmd.args(["issue", "view"]);
    if let Some(id) = issue_number_opt.as_deref() {
        cmd.arg(id);
    }
    cmd.args(["-F", "json"]);
    for arg in &extra_args {
        cmd.arg(arg);
    }
    let label = match issue_number_opt.as_deref() {
        Some(id) => format!("issue view {}", id),
        None => "issue view".to_string(),
    };
    run_glab_json(cmd, &label, format_issue_view)
}

// ── CI/Pipeline subcommands ─────────────────────────────────────────────

fn run_ci(args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return run_passthrough("glab", "ci", args);
    }

    match args[0].as_str() {
        "list" => ci_list(&args[1..], verbose, ultra_compact),
        "status" => ci_status(&args[1..], verbose, ultra_compact),
        "trace" => ci_trace(&args[1..]),
        // "ci view" is an interactive TUI (tcell) — must run with inherited stdio
        _ => run_passthrough("glab", "ci", args),
    }
}

/// Format CI list JSON into compact output (pure function, testable).
fn format_ci_list(json: &Value, ultra_compact: bool) -> String {
    let pipelines = match json.as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };
    if pipelines.is_empty() {
        return "No Pipelines\n".to_string();
    }

    let mut filtered = String::new();
    filtered.push_str("Pipelines\n");
    let all_lines: Vec<String> = pipelines
        .iter()
        .map(|pipeline| {
            let id = pipeline["id"].as_i64().unwrap_or(0);
            let status = pipeline["status"].as_str().unwrap_or("???");
            let ref_name = pipeline["ref"].as_str().unwrap_or("???");
            let icon = pipeline_icon(status, ultra_compact);
            format!("  {} #{} {} ({})", icon, id, status, ref_name)
        })
        .collect();
    const MAX_CI_LIST: usize = CAP_WARNINGS;
    for line in all_lines.iter().take(MAX_CI_LIST) {
        filtered.push_str(&format!("{}\n", line));
    }
    if all_lines.len() > MAX_CI_LIST {
        filtered.push_str(&format!("  … +{} more\n", all_lines.len() - MAX_CI_LIST));
        let all_text = all_lines.join("\n");
        if let Some(hint) =
            crate::core::tee::force_tee_tail_hint(&all_text, "glab-pipelines", MAX_CI_LIST + 1)
        {
            filtered.push_str(&format!("  {}\n", hint));
        }
    }
    filtered
}

fn ci_list(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["ci", "list", "-F", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_glab_json(cmd, "ci list", |json| format_ci_list(json, ultra_compact))
}

/// Format `glab ci status` text output (English keyword parsing, raw fallback).
/// Returns the raw input when no status keyword is recognized on any line
/// (e.g. non-English locale).
fn format_ci_status(raw: &str, ultra_compact: bool) -> String {
    let mut filtered = String::new();
    let mut any_keyword_matched = false;
    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let icon = if trimmed.contains("passed") || trimmed.contains("success") {
            pipeline_icon("success", ultra_compact)
        } else if trimmed.contains("failed") {
            pipeline_icon("failed", ultra_compact)
        } else if trimmed.contains("running") {
            pipeline_icon("running", ultra_compact)
        } else if trimmed.contains("pending") {
            pipeline_icon("pending", ultra_compact)
        } else if trimmed.contains("canceled") || trimmed.contains("cancelled") {
            pipeline_icon("canceled", ultra_compact)
        } else {
            ""
        };

        if !icon.is_empty() {
            any_keyword_matched = true;
            filtered.push_str(&format!("{} {}\n", icon, trimmed));
        } else {
            filtered.push_str(&format!("  {}\n", trimmed));
        }
    }

    if !any_keyword_matched {
        // Non-English locale or unrecognized format — preserve raw output verbatim.
        raw.to_string()
    } else {
        filtered
    }
}

fn ci_status(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    // glab ci status does not support -F json — text parsing with raw fallback
    let mut cmd = resolved_command("glab");
    cmd.args(["ci", "status"]);
    for arg in args {
        cmd.arg(arg);
    }
    runner::run_filtered(
        cmd,
        "glab",
        "ci status",
        |stdout| format_ci_status(stdout, ultra_compact),
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

fn ci_trace(args: &[String]) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["ci", "trace"]);
    for arg in args {
        cmd.arg(arg);
    }
    runner::run_filtered(
        cmd,
        "glab",
        "ci trace",
        filter_ci_trace,
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

/// Filter CI job trace output: strip ANSI codes, section markers, and runner
/// boilerplate. Keep warnings, errors, and build output.
fn filter_ci_trace(raw: &str) -> String {
    let cleaned = strip_ansi(raw);
    let cleaned = BARE_ANSI_RE.replace_all(&cleaned, "");
    let cleaned = SECTION_MARKER_RE.replace_all(&cleaned, "");

    let mut filtered = String::new();

    for line in cleaned.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }

        // Skip runner boilerplate
        if trimmed.starts_with("Running with gitlab-runner")
            || (trimmed.starts_with("on ") && trimmed.contains("system ID:"))
            || trimmed.starts_with("Using Docker executor")
            || trimmed.starts_with("Using Shell")
            || trimmed.starts_with("Running on runner-")
            || trimmed.starts_with("Running on ")
            || trimmed.starts_with("Preparing the")
            || trimmed.starts_with("Preparing environment")
            || trimmed.starts_with("Getting source from")
            || trimmed.starts_with("Resolving secrets")
            || trimmed.starts_with("Cleaning up")
            || trimmed.starts_with("Uploading artifacts")
            || trimmed.starts_with("Downloading artifacts")
            || trimmed.starts_with("Runtime platform")
        {
            continue;
        }

        // Skip git fetch / checkout boilerplate
        if trimmed.starts_with("Fetching changes with git")
            || trimmed.starts_with("Initialized empty Git")
            || trimmed.starts_with("Created fresh repository")
            || trimmed.starts_with("Checking out ")
            || trimmed.starts_with("Skipping Git submodules")
        {
            continue;
        }

        filtered.push_str(trimmed);
        filtered.push('\n');
    }

    filtered
}

// ── Release subcommands ──────────────────────────────────────────────────

fn run_release(args: &[String], _verbose: u8, _ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return run_passthrough("glab", "release", args);
    }

    match args[0].as_str() {
        "list" => release_list(&args[1..]),
        "view" => release_view(&args[1..]),
        _ => run_passthrough("glab", "release", args),
    }
}

/// Format `glab release list` tab-separated output into compact form.
/// Input format: "Name\tTag\tCreated\n" header + data rows.
fn format_release_list(raw: &str) -> Option<String> {
    let mut lines = raw.lines().peekable();
    let mut filtered = String::new();

    // Skip "Showing N releases..." preamble and blank lines
    while let Some(line) = lines.peek() {
        let trimmed = line.trim();
        if trimmed.starts_with("Name\t") || trimmed.starts_with("NAME\t") {
            lines.next(); // consume header
            break;
        }
        lines.next();
    }

    filtered.push_str("Releases\n");

    let mut count = 0;
    for line in lines {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let parts: Vec<&str> = trimmed.split('\t').collect();
        if parts.len() < 3 {
            continue;
        }

        let name = parts[0].trim();
        let tag = parts[1].trim();
        let created = parts[2].trim();

        if name == tag {
            filtered.push_str(&format!("  {} ({})\n", name, created));
        } else {
            filtered.push_str(&format!("  {} [{}] ({})\n", name, tag, created));
        }

        count += 1;
        if count >= 20 {
            break;
        }
    }

    if count == 0 {
        return None;
    }

    Some(filtered)
}

fn release_list(args: &[String]) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["release", "list"]);
    for arg in args {
        cmd.arg(arg);
    }
    runner::run_filtered(
        cmd,
        "glab",
        "release list",
        |stdout| format_release_list(stdout).unwrap_or_else(|| stdout.to_string()),
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

fn release_view(args: &[String]) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["release", "view"]);
    for arg in args {
        cmd.arg(arg);
    }
    runner::run_filtered(
        cmd,
        "glab",
        "release view",
        filter_release_view,
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

/// Filter release view output: strip SOURCES block, image lines, HTML comments,
/// horizontal rules, and collapse blank lines.
fn filter_release_view(raw: &str) -> String {
    let mut filtered = String::new();
    let mut in_sources = false;

    for line in raw.lines() {
        let trimmed = line.trim();

        // Skip SOURCES section (archive download URLs)
        if trimmed == "SOURCES" {
            in_sources = true;
            continue;
        }
        if in_sources {
            if trimmed.starts_with("http://") || trimmed.starts_with("https://") {
                continue;
            }
            in_sources = false;
        }

        // Strip image-only lines
        if trimmed.starts_with("![") && trimmed.ends_with(')') && trimmed.contains("](") {
            continue;
        }
        // Strip glab's "Image: name → url" rendering
        if trimmed.starts_with("Image:") && trimmed.contains('→') {
            continue;
        }

        // Strip HTML comments
        if trimmed.starts_with("<!--") && trimmed.ends_with("-->") {
            continue;
        }

        // Strip horizontal rules (--- rendered as --------)
        if trimmed.chars().all(|c| c == '-') && trimmed.len() >= 3 {
            continue;
        }

        filtered.push_str(line);
        filtered.push('\n');
    }

    // Collapse multiple blank lines
    MULTI_BLANK_RE.replace_all(&filtered, "\n\n").to_string()
}

// ── API subcommand ──────────────────────────────────────────────────────

fn run_api(args: &[String], _verbose: u8) -> Result<i32> {
    // glab api is an explicit/advanced command — the user knows what they asked for.
    // Converting JSON to a schema destroys all values and forces Claude to re-fetch.
    // Passthrough preserves the full response and tracks metrics at 0% savings.
    run_passthrough("glab", "api", args)
}

// ── Passthrough ─────────────────────────────────────────────────────────

fn run_passthrough(cmd: &str, subcommand: &str, args: &[String]) -> Result<i32> {
    let mut os_args: Vec<std::ffi::OsString> = vec![std::ffi::OsString::from(subcommand)];
    os_args.extend(args.iter().map(std::ffi::OsString::from));
    runner::run_passthrough(cmd, &os_args, 0)
}

fn run_passthrough_with_extra(cmd: &str, base_args: &[&str], extra_args: &[String]) -> Result<i32> {
    let mut os_args: Vec<std::ffi::OsString> =
        base_args.iter().map(std::ffi::OsString::from).collect();
    os_args.extend(extra_args.iter().map(std::ffi::OsString::from));
    runner::run_passthrough(cmd, &os_args, 0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_icon_opened() {
        assert_eq!(state_icon("opened", false), "[open]");
        assert_eq!(state_icon("opened", true), "O");
    }

    #[test]
    fn test_state_icon_merged() {
        assert_eq!(state_icon("merged", false), "[merged]");
        assert_eq!(state_icon("merged", true), "M");
    }

    #[test]
    fn test_state_icon_closed() {
        assert_eq!(state_icon("closed", false), "[closed]");
        assert_eq!(state_icon("closed", true), "C");
    }

    #[test]
    fn test_pipeline_icon_success() {
        assert_eq!(pipeline_icon("success", false), "[ok]");
        assert_eq!(pipeline_icon("success", true), "+");
    }

    #[test]
    fn test_pipeline_icon_failed() {
        assert_eq!(pipeline_icon("failed", false), "[fail]");
        assert_eq!(pipeline_icon("failed", true), "x");
    }

    #[test]
    fn test_pipeline_icon_running() {
        assert_eq!(pipeline_icon("running", false), "[run]");
        assert_eq!(pipeline_icon("running", true), "~");
    }

    #[test]
    fn test_extract_mr_number_from_url() {
        let url = "https://gitlab.example.com/group/project/-/merge_requests/42";
        assert_eq!(extract_mr_number(url), Some("42".to_string()));
    }

    #[test]
    fn test_extract_mr_number_no_match() {
        assert_eq!(extract_mr_number("not a url"), None);
    }

    #[test]
    fn test_filter_markdown_body_empty() {
        assert_eq!(filter_markdown_body(""), "");
    }

    #[test]
    fn test_filter_markdown_body_html_comments() {
        let input = "Hello\n<!-- comment -->\nWorld";
        let result = filter_markdown_body(input);
        assert!(!result.contains("<!--"));
        assert!(result.contains("Hello"));
        assert!(result.contains("World"));
    }

    #[test]
    fn test_filter_markdown_body_code_block_preserved() {
        let input = "Text\n```\n<!-- not stripped -->\n```\nAfter";
        let result = filter_markdown_body(input);
        assert!(result.contains("<!-- not stripped -->"));
        assert!(result.contains("Text"));
        assert!(result.contains("After"));
    }

    #[test]
    fn test_filter_markdown_body_blank_lines_collapse() {
        let input = "Line 1\n\n\n\n\nLine 2";
        let result = filter_markdown_body(input);
        assert!(!result.contains("\n\n\n"));
        assert!(result.contains("Line 1"));
        assert!(result.contains("Line 2"));
    }

    #[test]
    fn test_filter_markdown_body_badges_removed() {
        let input =
            "# Title\n[![CI](https://img.shields.io/badge.svg)](https://github.com/actions)\nText";
        let result = filter_markdown_body(input);
        assert!(!result.contains("shields.io"));
        assert!(result.contains("# Title"));
        assert!(result.contains("Text"));
    }

    #[test]
    fn test_filter_markdown_body_meaningful_content_preserved() {
        let input = "## Summary\n- Item 1\n- Item 2\n\n[Link](https://example.com)";
        let result = filter_markdown_body(input);
        assert!(result.contains("## Summary"));
        assert!(result.contains("- Item 1"));
        assert!(result.contains("[Link](https://example.com)"));
    }

    #[test]
    fn test_ok_confirmation_mr_create() {
        let result = ok_confirmation(
            "created",
            "!42 https://gitlab.example.com/-/merge_requests/42",
        );
        assert!(result.contains("ok created"));
        assert!(result.contains("!42"));
    }

    #[test]
    fn test_ok_confirmation_mr_merge() {
        let result = ok_confirmation("merged", "!42");
        assert_eq!(result, "ok merged !42");
    }

    #[test]
    fn test_ok_confirmation_mr_approve() {
        let result = ok_confirmation("approved", "!42");
        assert_eq!(result, "ok approved !42");
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    fn parse_fixture(raw: &str) -> Value {
        serde_json::from_str(raw).expect("valid JSON fixture")
    }

    #[test]
    fn test_mr_list_token_savings() {
        let input = include_str!("../../../tests/fixtures/glab_mr_list_raw.json");
        let output = format_mr_list(&parse_fixture(input), false);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "MR list: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_mr_list_format() {
        let input = include_str!("../../../tests/fixtures/glab_mr_list_raw.json");
        let output = format_mr_list(&parse_fixture(input), false);
        assert!(output.contains("Merge Requests"));
        assert!(output.contains("!314"));
        assert!(output.contains("[open]")); // opened
        assert!(output.contains("[merged]")); // merged
        assert!(output.contains("[closed]")); // closed
    }

    #[test]
    fn test_mr_list_ultra_compact() {
        let input = include_str!("../../../tests/fixtures/glab_mr_list_raw.json");
        let output = format_mr_list(&parse_fixture(input), true);
        assert!(output.starts_with("MRs\n"));
        assert!(output.contains("O ")); // opened
        assert!(output.contains("M ")); // merged
        assert!(output.contains("C ")); // closed
    }

    #[test]
    fn test_issue_list_token_savings() {
        let input = include_str!("../../../tests/fixtures/glab_issue_list_raw.json");
        let output = format_issue_list(&parse_fixture(input), false);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Issue list: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_issue_list_format() {
        let input = include_str!("../../../tests/fixtures/glab_issue_list_raw.json");
        let output = format_issue_list(&parse_fixture(input), false);
        assert!(output.contains("Issues"));
        assert!(output.contains("#156"));
        assert!(output.contains("[open]")); // opened
        assert!(output.contains("[closed]")); // closed
    }

    #[test]
    fn test_format_mr_list_non_array_returns_empty() {
        // Non-array JSON (e.g. error object) returns empty — run_glab_json then
        // falls back to raw stdout through its JSON parse branch.
        let output = format_mr_list(&Value::Object(Default::default()), false);
        assert!(output.is_empty());
    }

    #[test]
    fn test_format_issue_list_non_array_returns_empty() {
        let output = format_issue_list(&Value::Object(Default::default()), false);
        assert!(output.is_empty());
    }

    #[test]
    fn test_split_identifier_simple() {
        let args: Vec<String> = vec!["42".into()];
        let (id, extra) = split_identifier(&args, &view_takes_value);
        assert_eq!(id.as_deref(), Some("42"));
        assert!(extra.is_empty());
    }

    #[test]
    fn test_split_identifier_with_repo_flag_before() {
        // glab mr view -R group/project 42
        let args: Vec<String> = vec!["-R".into(), "group/project".into(), "42".into()];
        let (id, extra) = split_identifier(&args, &view_takes_value);
        assert_eq!(id.as_deref(), Some("42"));
        assert_eq!(extra, vec!["-R", "group/project"]);
    }

    #[test]
    fn test_split_identifier_with_repo_flag_after() {
        // glab mr view 42 -R group/project
        let args: Vec<String> = vec!["42".into(), "-R".into(), "group/project".into()];
        let (id, extra) = split_identifier(&args, &view_takes_value);
        assert_eq!(id.as_deref(), Some("42"));
        assert_eq!(extra, vec!["-R", "group/project"]);
    }

    #[test]
    fn test_split_identifier_with_group_flag() {
        let args: Vec<String> = vec!["-g".into(), "mygroup".into(), "7".into()];
        let (id, extra) = split_identifier(&args, &view_takes_value);
        assert_eq!(id.as_deref(), Some("7"));
        assert_eq!(extra, vec!["-g", "mygroup"]);
    }

    #[test]
    fn test_split_identifier_empty_yields_no_id() {
        // `glab mr view` (no args) must surface as (None, []) so the caller
        // hands the request to glab, which resolves the current branch's MR.
        let (id, extra) = split_identifier(&[], &view_takes_value);
        assert!(id.is_none());
        assert!(extra.is_empty());
    }

    #[test]
    fn test_split_identifier_only_flags_preserves_flags() {
        // Regression: `glab mr view -R group/project` previously triggered
        // "MR number required". Now flags must round-trip into `extra`.
        let args: Vec<String> = vec!["-R".into(), "group/project".into()];
        let (id, extra) = split_identifier(&args, &view_takes_value);
        assert!(id.is_none());
        assert_eq!(extra, vec!["-R", "group/project"]);
    }

    // ── regressions: a flag's value is never the identifier ─────────────

    #[test]
    fn test_view_pagination_values_are_not_the_identifier() {
        for flag in ["-p", "--page", "-P", "--per-page"] {
            let args: Vec<String> = vec![flag.into(), "2".into()];
            let (id, extra) = split_identifier(&args, &view_takes_value);
            assert!(id.is_none(), "{} swallowed its value as the id", flag);
            assert_eq!(extra, vec![flag, "2"]);
        }
    }

    #[test]
    fn test_view_jq_expression_is_not_the_identifier() {
        let args: Vec<String> = vec!["--jq".into(), ".iid".into()];
        let (id, _) = split_identifier(&args, &view_takes_value);
        assert!(id.is_none());
    }

    #[test]
    fn test_view_identifier_past_double_dash() {
        // `glab mr view -- 42` views MR 42: `--` ends flag parsing, not positional parsing.
        let args: Vec<String> = vec!["--".into(), "42".into()];
        let (id, extra) = split_identifier(&args, &view_takes_value);
        assert_eq!(id.as_deref(), Some("42"));
        assert_eq!(extra, vec!["--"]);
    }

    #[test]
    fn test_mr_merge_message_is_not_the_mr_number() {
        // glab mr merge -m "ship it" previously confirmed "ok merged !ship it".
        for flag in ["-m", "--message", "--squash-message"] {
            let args: Vec<String> = vec![flag.into(), "ship it".into()];
            let (id, _) = split_identifier(&args, &mr_merge_takes_value);
            assert!(id.is_none(), "{} swallowed its value as the MR number", flag);
        }
    }

    #[test]
    fn test_mr_merge_squash_is_boolean_but_approve_sha_is_not() {
        // -s is --squash on merge and --sha on approve: one shared table would break one of them.
        let args: Vec<String> = vec!["-s".into(), "42".into()];
        assert_eq!(
            split_identifier(&args, &mr_merge_takes_value).0.as_deref(),
            Some("42")
        );
        assert!(split_identifier(&args, &mr_approve_takes_value).0.is_none());
    }

    #[test]
    fn test_mr_update_label_is_not_the_mr_number() {
        // glab mr update --add a label: -l/-t/-a values are not identifiers.
        for flag in ["-l", "--label", "-t", "--title", "-a", "--assignee"] {
            let args: Vec<String> = vec![flag.into(), "bug".into()];
            let (id, _) = split_identifier(&args, &mr_update_takes_value);
            assert!(id.is_none(), "{} swallowed its value as the MR number", flag);
        }
    }

    #[test]
    fn test_mr_update_keeps_an_explicit_mr_number() {
        let args: Vec<String> = vec!["--label".into(), "bug".into(), "42".into()];
        let (id, _) = split_identifier(&args, &mr_update_takes_value);
        assert_eq!(id.as_deref(), Some("42"));
    }

    #[test]
    fn test_mr_note_message_is_not_the_mr_number() {
        let args: Vec<String> = vec!["-m".into(), "looks good".into()];
        let (id, _) = split_identifier(&args, &mr_note_takes_value);
        assert!(id.is_none());
    }

    #[test]
    fn test_split_glab_region_rejects_a_leading_boundary() {
        let region: Vec<String> = vec!["--".into(), "mr".into(), "view".into()];
        assert!(split_glab_region(&region).is_none());

        let region: Vec<String> = vec!["mr".into(), "view".into(), "42".into()];
        let (subcommand, args) = split_glab_region(&region).expect("leading positional");
        assert_eq!(subcommand, "mr");
        assert_eq!(args, ["view", "42"]);
    }

    // ── has_output_flag tests ───────────────────────────────────────────

    #[test]
    fn test_has_output_flag_json() {
        assert!(has_output_flag(&["--json".into()]));
    }

    #[test]
    fn test_has_output_flag_format() {
        assert!(has_output_flag(&["-F".into(), "json".into()]));
        assert!(has_output_flag(&["--output".into(), "text".into()]));
    }

    #[test]
    fn test_has_output_flag_none() {
        assert!(!has_output_flag(&["mr".into(), "list".into()]));
    }

    // ── should_passthrough_view tests ───────────────────────────────────

    #[test]
    fn test_should_passthrough_view_web() {
        assert!(should_passthrough_view(&["--web".into()]));
    }

    #[test]
    fn test_should_passthrough_view_comments() {
        assert!(should_passthrough_view(&["--comments".into()]));
    }

    #[test]
    fn test_should_passthrough_view_output() {
        assert!(should_passthrough_view(&["-F".into(), "json".into()]));
    }

    #[test]
    fn test_should_passthrough_view_default() {
        assert!(!should_passthrough_view(&[]));
    }

    // ── mr_action identifier extraction ─────────────────────────────────

    #[test]
    fn test_split_identifier_with_message_flag() {
        // glab mr note -m "comment" 42 — number should be 42, not "comment"
        let args: Vec<String> = vec!["-m".into(), "comment".into(), "42".into()];
        let (id, extra) = split_identifier(&args, &mr_note_takes_value);
        assert_eq!(id.as_deref(), Some("42"));
        assert_eq!(extra, vec!["-m", "comment"]);
    }

    // ── release list tests ──────────────────────────────────────────────

    #[test]
    fn test_format_release_list() {
        let input = include_str!("../../../tests/fixtures/glab_release_list_raw.txt");
        let output = format_release_list(input).expect("should parse release list");
        assert!(output.starts_with("Releases\n"));
        assert!(output.contains("v3.2.1"));
        assert!(output.contains("about 2 days ago"));
    }

    #[test]
    fn test_format_release_list_token_savings() {
        let input = include_str!("../../../tests/fixtures/glab_release_list_raw.txt");
        let output = format_release_list(input).expect("should parse release list");
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        // Release list text is already compact (tab-separated); savings are modest.
        assert!(
            savings >= 20.0,
            "Release list: expected >=20% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_format_release_list_empty() {
        let input = "No releases available on owner/repo.\nName\tTag\tCreated\n";
        assert!(format_release_list(input).is_none());
    }

    #[test]
    fn test_format_release_list_name_differs_from_tag() {
        let input = "Showing 1 releases\n\nName\tTag\tCreated\nMy Release\tv1.0.0\t2 days ago\n";
        let output = format_release_list(input).expect("should parse");
        assert!(output.contains("My Release [v1.0.0]"));
    }

    // ── ci trace tests ──────────────────────────────────────────────────

    #[test]
    fn test_filter_ci_trace_strips_boilerplate() {
        let input = include_str!("../../../tests/fixtures/glab_ci_trace_raw.txt");
        let output = filter_ci_trace(input);
        // Runner boilerplate stripped
        assert!(!output.contains("Running with gitlab-runner"));
        assert!(!output.contains("Using Docker executor"));
        assert!(!output.contains("Fetching changes with git"));
        assert!(!output.contains("Checking out"));
        assert!(!output.contains("Uploading artifacts"));
        // Build output preserved
        assert!(output.contains("npm ci"));
        assert!(output.contains("npm run build"));
        assert!(output.contains("npm test"));
        // Test results preserved
        assert!(output.contains("FAIL"));
        assert!(output.contains("AssertionError"));
        // Final error line preserved
        assert!(output.contains("Job failed"));
    }

    #[test]
    fn test_filter_ci_trace_token_savings() {
        let input = include_str!("../../../tests/fixtures/glab_ci_trace_raw.txt");
        let output = filter_ci_trace(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        // CI trace preserves build output; savings come from stripping boilerplate.
        assert!(
            savings >= 30.0,
            "CI trace: expected >=30% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── release view tests ──────────────────────────────────────────────

    #[test]
    fn test_filter_release_view_strips_sources() {
        let input = include_str!("../../../tests/fixtures/glab_release_view_raw.txt");
        let output = filter_release_view(input);
        // SOURCES section stripped
        assert!(!output.contains("SOURCES"));
        assert!(!output.contains("toolkit-v2.0.0.zip"));
        assert!(!output.contains("toolkit-v2.0.0.tar.gz"));
        // Content preserved
        assert!(output.contains("Test Release v2.0"));
        assert!(output.contains("Added widget support"));
        assert!(output.contains("@alice_dev @bob_dev"));
        // Noise stripped
        assert!(!output.contains("--------"));
        assert!(!output.contains("Image:"));
        assert!(!output.contains("<!-- internal"));
        // Footer preserved
        assert!(output.contains("View this release"));
    }

    #[test]
    fn test_filter_release_view_token_savings() {
        let input = include_str!("../../../tests/fixtures/glab_release_view_raw.txt");
        let output = filter_release_view(input);
        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        // Release view is already short; savings come from stripping SOURCES URLs and noise.
        assert!(
            savings >= 20.0,
            "Release view: expected >=20% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── Edge cases ────────────────────────────────────────────────────────

    #[test]
    fn test_format_mr_list_empty_array() {
        let output = format_mr_list(&parse_fixture("[]"), false);
        assert_eq!(output, "No Merge Requests\n");
    }

    #[test]
    fn test_format_mr_list_empty_array_ultra_compact() {
        let output = format_mr_list(&parse_fixture("[]"), true);
        assert_eq!(output, "No MRs\n");
    }

    #[test]
    fn test_format_issue_list_empty_array() {
        let output = format_issue_list(&parse_fixture("[]"), false);
        assert_eq!(output, "No Issues\n");
    }

    #[test]
    fn test_format_ci_list_empty_array() {
        let output = format_ci_list(&parse_fixture("[]"), false);
        assert_eq!(output, "No Pipelines\n");
    }

    #[test]
    fn test_format_mr_view_null_nested_fields() {
        // Defensive: if the GitLab API omits or nulls out nested fields,
        // formatters must render placeholders without panicking.
        let json = parse_fixture(
            r#"{"iid":42,"title":"Edge","state":"opened","author":null,"web_url":"","merge_status":"unknown","description":null}"#,
        );
        let output = format_mr_view(&json, false);
        assert!(output.contains("MR !42: Edge"));
        assert!(output.contains("???")); // author fallback
    }

    #[test]
    fn test_format_issue_view_missing_description() {
        let json = parse_fixture(
            r#"{"iid":10,"title":"X","state":"closed","author":{"username":"u"},"web_url":"http://e","description":null}"#,
        );
        let output = format_issue_view(&json);
        assert!(output.contains("[closed] Issue #10: X"));
        assert!(output.contains("Author: @u"));
        // No "Description:" section when null
        assert!(!output.contains("Description:"));
    }

    #[test]
    fn test_format_ci_status_non_english_fallback() {
        // Non-English locale output with no recognized keyword must fall back to raw.
        let raw = "Le pipeline est en cours d'exécution\n";
        let output = format_ci_status(raw, false);
        // format_ci_status returns raw when no keywords match
        assert_eq!(output, raw);
    }

    #[test]
    fn test_filter_release_view_no_sources_section() {
        let input = "# Release 1.0\n\nJust a simple changelog entry.\n";
        let output = filter_release_view(input);
        assert!(output.contains("Release 1.0"));
        assert!(output.contains("changelog entry"));
    }

    // ── mr_view enrichment (branches / labels / reviewers) ───────────────

    const MR_VIEW_FULL: &str = r#"{
        "iid": 42,
        "title": "feat: widget",
        "state": "opened",
        "author": {"username": "alice_dev"},
        "web_url": "https://gitlab.example.com/acme/toolkit/-/merge_requests/42",
        "merge_status": "can_be_merged",
        "source_branch": "feat/widget",
        "target_branch": "main",
        "labels": ["enhancement", "cli"],
        "reviewers": [{"username": "bob_review"}, {"username": "carol_review"}],
        "head_pipeline": {"status": "success"},
        "description": null
    }"#;

    #[test]
    fn test_format_mr_view_branches() {
        let output = format_mr_view(&parse_fixture(MR_VIEW_FULL), false);
        assert!(
            output.contains("feat/widget -> main"),
            "expected branches line, got:\n{}",
            output
        );
    }

    #[test]
    fn test_format_mr_view_labels() {
        let output = format_mr_view(&parse_fixture(MR_VIEW_FULL), false);
        assert!(
            output.contains("Labels: enhancement, cli"),
            "expected labels line, got:\n{}",
            output
        );
    }

    #[test]
    fn test_format_mr_view_reviewers() {
        let output = format_mr_view(&parse_fixture(MR_VIEW_FULL), false);
        assert!(
            output.contains("Reviewers: @bob_review, @carol_review"),
            "expected reviewers line, got:\n{}",
            output
        );
    }

    #[test]
    fn test_format_mr_view_no_labels_no_reviewers() {
        let json = parse_fixture(
            r#"{
                "iid":1, "title":"X", "state":"opened",
                "author":{"username":"u1"}, "web_url":"",
                "merge_status":"can_be_merged",
                "source_branch":"a", "target_branch":"b",
                "labels":[], "reviewers":[], "description":null
            }"#,
        );
        let output = format_mr_view(&json, false);
        assert!(!output.contains("Labels:"));
        assert!(!output.contains("Reviewers:"));
        // branches line still present
        assert!(output.contains("a -> b"));
    }

    #[test]
    fn test_format_mr_view_mergeable_text_tag() {
        let output = format_mr_view(&parse_fixture(MR_VIEW_FULL), false);
        // merge_status="can_be_merged" -> "[ok]" (text tag, no emoji)
        assert!(
            output.contains("opened | [ok]"),
            "expected text-tag mergeable indicator, got:\n{}",
            output
        );
        // And no emoji anywhere in the rendered output
        assert!(!output.contains('✅'));
        assert!(!output.contains('❌'));
        assert!(!output.contains('✓'));
        assert!(!output.contains('✗'));
    }
}
