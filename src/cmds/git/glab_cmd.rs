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
use crate::core::runner::{self, RunOptions};
use crate::core::truncate::{reduced, CAP_LIST, CAP_WARNINGS};
use crate::core::utils::{
    ok_confirmation, resolved_command, strip_ansi, truncate, truncate_iso_date,
};
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
/// Match a whole `<details>` block, capturing its `<summary>` text.
static DETAILS_BLOCK_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?is)<details[^>]*>\s*<summary[^>]*>(.*?)</summary>.*?</details>").unwrap()
});
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

/// Fold `<details>…</details>` back to its own `<summary>` label, returning the folded
/// line count. The author collapsed that content by default and GitLab honors it, so the
/// summary plus the tee hint lose nothing.
///
/// Runs before `filter_markdown_body`: folded blocks embed code fences, and its
/// fence-aware segmentation would split `<details>` from its `</details>`. Consequence:
/// a `<details>` inside a fenced example is folded too — cosmetic, accepted.
fn collapse_details_blocks(body: &str) -> (String, usize) {
    if !body.contains("<details") {
        return (body.to_string(), 0);
    }

    let mut folded_lines = 0;
    let collapsed = DETAILS_BLOCK_RE.replace_all(body, |caps: &regex::Captures| {
        let whole = caps.get(0).map(|m| m.as_str()).unwrap_or("");
        let summary = caps.get(1).map(|m| m.as_str().trim()).unwrap_or("");
        let replacement = if summary.is_empty() {
            String::new()
        } else {
            format!("{}\n", summary)
        };
        folded_lines += whole.lines().count().saturating_sub(replacement.lines().count());
        replacement
    });

    (collapsed.to_string(), folded_lines)
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

/// Known glab flags that take a value — skipped along with their value when
/// looking for positional arguments.
const FLAGS_WITH_VALUE: &[&str] = &[
    "-R",
    "--repo",
    "-g",
    "--group",
    "-F",
    "--output",
    "-m",
    "--message",
    // `mr note` group: without these, a value like `unresolved` in
    // `--state unresolved` is mistaken for the MR identifier.
    "--state",
    "--type",
    "--file",
    "--line",
    "--old-line",
    "--reply",
];

/// Extract the first positional identifier (MR/issue number or URL) from args,
/// skipping glab flags that take a value. Returns the identifier and remaining args.
fn extract_identifier_and_extra_args(args: &[String]) -> Option<(String, Vec<String>)> {
    if args.is_empty() {
        return None;
    }

    let mut identifier = None;
    let mut extra = Vec::new();
    let mut skip_next = false;

    for arg in args {
        if skip_next {
            extra.push(arg.clone());
            skip_next = false;
            continue;
        }
        if FLAGS_WITH_VALUE.contains(&arg.as_str()) {
            extra.push(arg.clone());
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') {
            extra.push(arg.clone());
            continue;
        }
        // First non-flag arg is the identifier (number/URL)
        if identifier.is_none() {
            identifier = Some(arg.clone());
        } else {
            extra.push(arg.clone());
        }
    }

    identifier.map(|id| (id, extra))
}

/// Like `extract_identifier_and_extra_args` but yields `(None, args.to_vec())` when no
/// positional identifier is present, so callers can defer the "id required" decision
/// to `glab` itself (e.g. `glab mr view` defaults to the current branch's MR).
fn parse_optional_identifier(args: &[String]) -> (Option<String>, Vec<String>) {
    match extract_identifier_and_extra_args(args) {
        Some((id, extra)) => (Some(id), extra),
        None => (None, args.to_vec()),
    }
}

/// Nth positional (non-flag) argument, skipping `FLAGS_WITH_VALUE` and their values.
/// Callers pick the index because some `mr note` sub-commands take a note or discussion
/// id after the merge request — see `note_mr_ref`.
fn nth_positional(args: &[String], n: usize) -> Option<String> {
    let mut seen = 0;
    let mut skip_next = false;

    for arg in args {
        if skip_next {
            skip_next = false;
            continue;
        }
        if FLAGS_WITH_VALUE.contains(&arg.as_str()) {
            skip_next = true;
            continue;
        }
        if arg.starts_with('-') {
            continue;
        }
        if seen == n {
            return Some(arg.clone());
        }
        seen += 1;
    }

    None
}

/// Whether a token is recognizable as a merge request reference (number or MR URL).
/// A bare word is deliberately NOT one — see `run_mr_note`.
fn looks_like_mr_ref(token: &str) -> bool {
    (!token.is_empty() && token.chars().all(|c| c.is_ascii_digit())) || MR_URL_RE.is_match(token)
}

/// Check if user explicitly requested JSON/custom output format.
/// When present, passthrough to avoid double JSON injection.
fn has_output_flag(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--output" || a == "-F" || a == "--json")
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

/// Run a glab command with token-optimized output.
pub fn run(subcommand: &str, args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
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

// ── MR subcommands ──────────────────────────────────────────────────────

fn run_mr(args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return run_passthrough("glab", "mr", args);
    }

    match args[0].as_str() {
        "list" => mr_list(&args[1..], verbose, ultra_compact),
        "view" => mr_view(&args[1..], verbose, ultra_compact),
        "create" => mr_create(&args[1..], verbose),
        "merge" => mr_action(&["mr", "merge"], "merged", &args[1..]),
        "approve" => mr_action(&["mr", "approve"], "approved", &args[1..]),
        "diff" => mr_diff(&args[1..], verbose),
        "note" => run_mr_note(&args[1..], ultra_compact),
        "update" => mr_action(&["mr", "update"], "updated", &args[1..]),
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
    let (mr_number_opt, extra_args) = parse_optional_identifier(args);

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

// ── MR note list ────────────────────────────────────────────────────────

/// Current glab's wording for a merge request with no discussion.
const NO_DISCUSSIONS: &str = "No discussions found.";

/// Flatten discussions into renderable notes, paired with "is a thread reply", and count
/// the `system: true` notes dropped along the way.
///
/// Those are GitLab activity events — "assigned to @user", "added 2 commits", "mentioned
/// in commit <sha>" — which carry nothing an agent reading review feedback can act on.
/// They are the bulk of a busy merge request, so they are filtered out and reported as a
/// count; `-F json` passes the full set through untouched.
fn visible_notes(discussions: &[Value]) -> (Vec<(&Value, bool)>, usize) {
    let mut visible = Vec::new();
    let mut activity_events = 0;

    for discussion in discussions {
        let Some(notes) = discussion["notes"].as_array() else {
            continue;
        };
        let mut is_reply = false;
        for note in notes {
            if note["system"].as_bool().unwrap_or(false) {
                activity_events += 1;
                continue;
            }
            visible.push((note, is_reply));
            is_reply = true;
        }
    }

    (visible, activity_events)
}

/// `path:line` for a diff note, so the agent knows what the comment points at.
fn note_location(note: &Value) -> Option<String> {
    let position = note.get("position")?;
    if position.is_null() {
        return None;
    }
    let path = position["new_path"]
        .as_str()
        .or_else(|| position["old_path"].as_str())?;

    match position["new_line"]
        .as_i64()
        .or_else(|| position["old_line"].as_i64())
    {
        Some(line) => Some(format!("{}:{}", path, line)),
        None => Some(path.to_string()),
    }
}

/// Render notes as a recognizable subset of glab's own output. Bodies are never
/// line-capped: a note is a reviewer's instruction, and acting on half of one is worse
/// than spending the tokens. `fold` collapses author-folded `<details>` blocks; passing
/// `false` produces the complete rendering that backs the tee hint.
fn render_notes(notes: &[(&Value, bool)], fold: bool) -> (String, usize) {
    let mut out = String::new();
    let mut folded_total = 0;

    for (note, is_reply) in notes {
        let indent = if *is_reply { "  " } else { "" };
        let author = note["author"]["username"].as_str().unwrap_or("???");
        let date = truncate_iso_date(note["created_at"].as_str().unwrap_or(""));

        let mut header = format!("{}@{} {}", indent, author, date);
        if note["resolvable"].as_bool().unwrap_or(false) {
            header.push_str(if note["resolved"].as_bool().unwrap_or(false) {
                " [resolved]"
            } else {
                " [unresolved]"
            });
        }
        if let Some(location) = note_location(note) {
            header.push_str(&format!(" {}", location));
        }
        // A malformed API response can leave the date empty; do not trail a space.
        out.push_str(&format!("\n{}\n", header.trim_end()));

        let raw_body = note["body"].as_str().unwrap_or("");
        let (body, folded) = if fold {
            collapse_details_blocks(raw_body)
        } else {
            (raw_body.to_string(), 0)
        };
        folded_total += folded;

        for line in filter_markdown_body(&body).lines() {
            out.push_str(&format!("{}{}\n", indent, line));
        }
        if folded > 0 {
            out.push_str(&format!("{}[+{} lines folded]\n", indent, folded));
        }
    }

    (out, folded_total)
}

/// Report the filtered-out activity events, so their absence is never silent.
fn push_activity_count(out: &mut String, activity_events: usize) {
    if activity_events > 0 {
        let plural = if activity_events == 1 { "" } else { "s" };
        out.push_str(&format!(
            "  [+{} activity event{}]\n",
            activity_events, plural
        ));
    }
}

/// Format `glab mr note list` JSON into compact output (pure function, testable).
fn format_mr_note_list(json: &Value, ultra_compact: bool) -> String {
    let discussions = match json.as_array() {
        Some(arr) => arr,
        None => return String::new(),
    };

    let (visible, activity_events) = visible_notes(discussions);

    let mut filtered = String::new();
    if visible.is_empty() {
        let empty = if ultra_compact {
            "No discussions"
        } else {
            NO_DISCUSSIONS
        };
        filtered.push_str(empty);
        filtered.push('\n');
        push_activity_count(&mut filtered, activity_events);
        return filtered;
    }

    // Notes are multi-line entries (header + full body), so show fewer than the flat-list cap.
    const MAX_NOTES: usize = reduced(CAP_LIST, 10);
    let shown = visible.len().min(MAX_NOTES);
    let (rendered, folded) = render_notes(&visible[..shown], true);

    filtered.push_str(rendered.trim_start_matches('\n'));

    if visible.len() > shown {
        filtered.push_str(&format!("  … +{} more\n", visible.len() - shown));
    }
    push_activity_count(&mut filtered, activity_events);

    if folded > 0 || visible.len() > shown {
        let (full, _) = render_notes(&visible, false);
        if let Some(hint) = crate::core::tee::force_tee_hint(&full, "glab-notes") {
            filtered.push_str(&format!("  {}\n", hint));
        }
    }

    filtered
}

/// Reads the discussions as JSON rather than filtering glab's text output. That output is
/// presentation and it churns — headers, note and discussion ids, absolute timestamps and
/// system-note visibility have all changed across recent glab releases — whereas these
/// fields come straight from the GitLab discussions API and are pinned by the server.
/// On a glab too old to know `note list`, glab's own error surfaces with its exit code.
fn mr_note_list(args: &[String], ultra_compact: bool) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(["mr", "note", "list", "-F", "json"]);
    for arg in args {
        cmd.arg(arg);
    }
    run_glab_json(cmd, "mr note list", |json| {
        format_mr_note_list(json, ultra_compact)
    })
}

/// Generic MR action handler for write sub-commands (merge/approve/note/update).
/// `base` is the glab command path (`["mr", "merge"]`, `["mr", "note", "create"]`) and
/// `mr_ref` the merge request the confirmation should name, or `None` when the arguments
/// do not carry one (glab then resolves it from the current branch).
fn mr_action_with_ref(
    base: &[&str],
    label: &str,
    args: &[String],
    mr_ref: Option<String>,
) -> Result<i32> {
    let mut cmd = resolved_command("glab");
    cmd.args(base);
    for arg in args {
        cmd.arg(arg);
    }

    let mr_num = mr_ref.map(|id| format!("!{}", id)).unwrap_or_default();
    let label = label.to_string();
    runner::run_filtered(
        cmd,
        "glab",
        &base.join(" "),
        move |_stdout| ok_confirmation(&label, &mr_num),
        RunOptions::stdout_only().early_exit_on_failure(),
    )
}

/// MR action whose first positional is the merge request. Resolved past flags, so
/// `glab mr note -m "msg" 42` still reports `!42`.
fn mr_action(base: &[&str], label: &str, args: &[String]) -> Result<i32> {
    let mr_ref = nth_positional(args, 0);
    mr_action_with_ref(base, label, args, mr_ref)
}

/// The merge request named by a `mr note` sub-command that also takes a note or
/// discussion id.
///
/// The merge request comes first, the note or discussion id second:
/// `glab mr note update 1 12345` updates note 12345 on merge request 1. Older glab builds
/// advertised the reverse order in their own `--help` synopsis while behaving this way, so
/// do not "fix" this back on the strength of a help string. With a single positional the
/// id is the note and glab resolves the merge request from the current branch, leaving
/// nothing to name.
fn note_mr_ref(args: &[String]) -> Option<String> {
    nth_positional(args, 1).and(nth_positional(args, 0))
}

fn mr_note_action(sub: &str, label: &str, args: &[String]) -> Result<i32> {
    let mr_ref = note_mr_ref(args);
    mr_action_with_ref(&["mr", "note", sub], label, args, mr_ref)
}

/// Route the `glab mr note` command group.
///
/// ISSUE #3531: rtk answered every `mr note` invocation with a write confirmation built
/// from the first positional, ignoring that glab exposes create/delete/list/reopen/
/// resolve/update under it. The read sub-command came out as `ok noted !list`, its output
/// destroyed. Dispatch the group instead.
///
/// Anything not recognized here goes to passthrough rather than to a confirmation:
/// a bare word is either a branch name or a sub-command glab added later, and
/// guessing wrong is exactly what caused #3531. Passthrough is lossless.
fn run_mr_note(args: &[String], ultra_compact: bool) -> Result<i32> {
    // Bare `glab mr note` opens $EDITOR; passthrough is the only path inheriting stdin.
    let Some(first) = args.first() else {
        return run_passthrough_with_extra("glab", &["mr", "note"], args);
    };

    match first.as_str() {
        "list" => mr_note_list(&args[1..], ultra_compact),
        "create" => mr_action(&["mr", "note", "create"], "noted", &args[1..]),
        "update" => mr_note_action("update", "updated", &args[1..]),
        "delete" => mr_note_action("delete", "deleted", &args[1..]),
        "resolve" => mr_note_action("resolve", "resolved", &args[1..]),
        "reopen" => mr_note_action("reopen", "reopened", &args[1..]),
        // Legacy write form kept working: `glab mr note [<id>] -m "msg"`.
        token if token.starts_with('-') || looks_like_mr_ref(token) => {
            mr_action(&["mr", "note"], "noted", args)
        }
        _ => run_passthrough_with_extra("glab", &["mr", "note"], args),
    }
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
    let (issue_number_opt, extra_args) = parse_optional_identifier(args);

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
    fn test_extract_identifier_simple() {
        let args: Vec<String> = vec!["42".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "42");
        assert!(extra.is_empty());
    }

    #[test]
    fn test_extract_identifier_with_repo_flag_before() {
        // glab mr view -R group/project 42
        let args: Vec<String> = vec!["-R".into(), "group/project".into(), "42".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "42");
        assert_eq!(extra, vec!["-R", "group/project"]);
    }

    #[test]
    fn test_extract_identifier_with_repo_flag_after() {
        // glab mr view 42 -R group/project
        let args: Vec<String> = vec!["42".into(), "-R".into(), "group/project".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "42");
        assert_eq!(extra, vec!["-R", "group/project"]);
    }

    #[test]
    fn test_extract_identifier_with_group_flag() {
        let args: Vec<String> = vec!["-g".into(), "mygroup".into(), "7".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "7");
        assert_eq!(extra, vec!["-g", "mygroup"]);
    }

    #[test]
    fn test_extract_identifier_empty() {
        let args: Vec<String> = vec![];
        assert!(extract_identifier_and_extra_args(&args).is_none());
    }

    #[test]
    fn test_extract_identifier_only_flags() {
        let args: Vec<String> = vec!["-R".into(), "group/project".into()];
        assert!(extract_identifier_and_extra_args(&args).is_none());
    }

    // ── parse_optional_identifier tests ─────────────────────────────────

    #[test]
    fn test_parse_optional_identifier_empty_yields_no_id() {
        // `glab mr view` (no args) must surface as (None, []) so the caller
        // hands the request to glab, which resolves the current branch's MR.
        let (id, extra) = parse_optional_identifier(&[]);
        assert!(id.is_none());
        assert!(extra.is_empty());
    }

    #[test]
    fn test_parse_optional_identifier_only_flags_preserves_flags() {
        // Regression: `glab mr view -R group/project` previously triggered
        // "MR number required". Now flags must round-trip into `extra`.
        let args: Vec<String> = vec!["-R".into(), "group/project".into()];
        let (id, extra) = parse_optional_identifier(&args);
        assert!(id.is_none());
        assert_eq!(extra, vec!["-R", "group/project"]);
    }

    #[test]
    fn test_parse_optional_identifier_with_id_matches_extract() {
        let args: Vec<String> = vec!["-R".into(), "group/project".into(), "42".into()];
        let (id, extra) = parse_optional_identifier(&args);
        assert_eq!(id.as_deref(), Some("42"));
        assert_eq!(extra, vec!["-R", "group/project"]);
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
    fn test_extract_identifier_with_message_flag() {
        // glab mr note -m "comment" 42 — number should be 42, not "comment"
        let args: Vec<String> = vec!["-m".into(), "comment".into(), "42".into()];
        let (id, extra) = extract_identifier_and_extra_args(&args).unwrap();
        assert_eq!(id, "42");
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

    // ── mr note routing (ISSUE #3531) ────────────────────────────────────

    #[test]
    fn test_nth_positional_picks_by_index() {
        let args: Vec<String> = vec!["42".into(), "abc123".into()];
        assert_eq!(nth_positional(&args, 0).as_deref(), Some("42"));
        assert_eq!(nth_positional(&args, 1).as_deref(), Some("abc123"));
        assert_eq!(nth_positional(&args, 2), None);
    }

    #[test]
    fn test_nth_positional_skips_flags_and_their_values() {
        // glab mr note resolve -R group/project 42 abc123
        let args: Vec<String> = vec![
            "-R".into(),
            "group/project".into(),
            "42".into(),
            "abc123".into(),
        ];
        assert_eq!(nth_positional(&args, 0).as_deref(), Some("42"));
        assert_eq!(nth_positional(&args, 1).as_deref(), Some("abc123"));
    }

    #[test]
    fn test_note_mr_ref_takes_first_positional_when_note_id_follows() {
        // `glab mr note update 42 12345` updates note 12345 on MR 42, whatever the
        // `--help` synopsis claims.
        let args: Vec<String> = vec!["42".into(), "12345".into()];
        assert_eq!(note_mr_ref(&args).as_deref(), Some("42"));
    }

    #[test]
    fn test_note_mr_ref_is_none_with_a_single_positional() {
        // `glab mr note update 12345` — the id is the note, the MR comes from the branch,
        // so there is no merge request to name in the confirmation.
        let args: Vec<String> = vec!["12345".into()];
        assert_eq!(note_mr_ref(&args), None);
    }

    #[test]
    fn test_note_mr_ref_ignores_flag_values() {
        let args: Vec<String> = vec![
            "-R".into(),
            "group/project".into(),
            "42".into(),
            "abc123".into(),
            "-m".into(),
            "msg".into(),
        ];
        assert_eq!(note_mr_ref(&args).as_deref(), Some("42"));
    }

    #[test]
    fn test_nth_positional_state_value_is_not_a_positional() {
        // Regression: `--state unresolved` must not offer `unresolved` as the MR id.
        let args: Vec<String> = vec!["--state".into(), "unresolved".into(), "42".into()];
        assert_eq!(nth_positional(&args, 0).as_deref(), Some("42"));
    }

    #[test]
    fn test_looks_like_mr_ref_accepts_number_and_url() {
        assert!(looks_like_mr_ref("42"));
        assert!(looks_like_mr_ref(
            "https://gitlab.example.com/acme/toolkit/-/merge_requests/42"
        ));
    }

    #[test]
    fn test_looks_like_mr_ref_rejects_bare_words() {
        // Treating a bare word as an MR id is what produced `ok noted !list` (ISSUE #3531).
        assert!(!looks_like_mr_ref("list"));
        assert!(!looks_like_mr_ref("resolve"));
        assert!(!looks_like_mr_ref("feat/my-branch"));
        assert!(!looks_like_mr_ref(""));
    }

    // ── mr note list formatting ──────────────────────────────────────────

    const NOTE_LIST_FIXTURE: &str = include_str!("../../../tests/fixtures/glab_mr_note_list_raw.json");

    #[test]
    fn test_format_mr_note_list_header_and_authors() {
        let output = format_mr_note_list(&parse_fixture(NOTE_LIST_FIXTURE), false);
        // No invented header: current glab opens straight on the first note.
        assert!(output.starts_with("@release-bot 2026-08-06"));
        assert!(output.contains("@bob_dev 2026-08-07"));
    }

    #[test]
    fn test_format_mr_note_list_skips_system_notes() {
        let output = format_mr_note_list(&parse_fixture(NOTE_LIST_FIXTURE), false);
        assert!(!output.contains("changed the description"));
        assert!(!output.contains("mentioned in commit"));
    }

    #[test]
    fn test_format_mr_note_list_resolution_markers() {
        let output = format_mr_note_list(&parse_fixture(NOTE_LIST_FIXTURE), false);
        assert!(output.contains("[unresolved]"));
        assert!(output.contains("[resolved]"));
    }

    #[test]
    fn test_format_mr_note_list_diff_note_location() {
        let output = format_mr_note_list(&parse_fixture(NOTE_LIST_FIXTURE), false);
        assert!(
            output.contains("src/parser/lexer.rs:128"),
            "expected diff-note location, got:\n{}",
            output
        );
    }

    #[test]
    fn test_format_mr_note_list_keeps_human_bodies_complete() {
        let output = format_mr_note_list(&parse_fixture(NOTE_LIST_FIXTURE), false);
        // A note is a reviewer's instruction: it is never line-capped.
        assert!(output.contains(
            "otherwise a malformed API response takes the whole command down."
        ));
        assert!(output.contains("Bound to `CAP_LIST` with the deviation commented."));
    }

    #[test]
    fn test_format_mr_note_list_folds_details_block() {
        let output = format_mr_note_list(&parse_fixture(NOTE_LIST_FIXTURE), false);
        assert!(output.contains("Release v2.1.0 — 14 commits, 3 fixes"));
        assert!(!output.contains("off-by-one on empty input"));
        assert!(output.contains("lines folded]"));
    }

    #[test]
    fn test_format_mr_note_list_indents_thread_replies() {
        let output = format_mr_note_list(&parse_fixture(NOTE_LIST_FIXTURE), false);
        assert!(
            output.contains("  @alice_dev 2026-08-07"),
            "expected reply indent, got:\n{}",
            output
        );
    }

    #[test]
    fn test_format_mr_note_list_empty_array_uses_glab_wording() {
        let output = format_mr_note_list(&parse_fixture("[]"), false);
        assert_eq!(output, format!("{}\n", NO_DISCUSSIONS));
    }

    #[test]
    fn test_format_mr_note_list_empty_array_ultra_compact() {
        let output = format_mr_note_list(&parse_fixture("[]"), true);
        assert_eq!(output, "No discussions\n");
    }

    #[test]
    fn test_format_mr_note_list_all_system_reads_as_no_discussions() {
        let json = parse_fixture(
            r#"[{"id":"d1","individual_note":true,"notes":[
                {"body":"changed the description","author":{"username":"alice_dev"},
                 "system":true,"created_at":"2026-08-06T16:03:11.342+02:00",
                 "resolvable":false,"resolved":false,"position":null}]}]"#,
        );
        let output = format_mr_note_list(&json, false);
        // Filtered out, but never silently: the count says something happened.
        assert_eq!(
            output,
            format!("{}\n  [+1 activity event]\n", NO_DISCUSSIONS)
        );
    }

    #[test]
    fn test_format_mr_note_list_needs_only_api_fields() {
        // Robustness across glab versions: the formatter reads GitLab discussion-API
        // fields, so a payload carrying nothing else must still render fully. Any glab
        // presentation change (headers, ids, timestamps) is irrelevant by construction.
        let json = parse_fixture(
            r#"[{"individual_note":false,"notes":[
                {"body":"first","author":{"username":"alice_dev"},"system":false,
                 "created_at":"2026-08-06T16:02:53.775+02:00",
                 "resolvable":true,"resolved":false,
                 "position":{"new_path":"a.rs","new_line":7}},
                {"body":"reply","author":{"username":"bob_dev"},"system":false,
                 "created_at":"2026-08-07T09:00:00.000+02:00",
                 "resolvable":true,"resolved":false,"position":null}]}]"#,
        );
        let output = format_mr_note_list(&json, false);
        assert!(output.contains("@alice_dev 2026-08-06 [unresolved] a.rs:7"));
        assert!(output.contains("  @bob_dev 2026-08-07 [unresolved]"));
        assert!(output.contains("first"));
        assert!(output.contains("reply"));
    }

    #[test]
    fn test_format_mr_note_list_non_array_returns_empty() {
        let output = format_mr_note_list(&Value::Object(Default::default()), false);
        assert!(output.is_empty());
    }

    #[test]
    fn test_format_mr_note_list_null_author_does_not_panic() {
        let json = parse_fixture(
            r#"[{"id":"d1","individual_note":true,"notes":[
                {"body":"hi","author":null,"system":false,
                 "created_at":null,"resolvable":false,"resolved":false,"position":null}]}]"#,
        );
        let output = format_mr_note_list(&json, false);
        assert!(output.contains("@???"));
        // An absent date must not leave a trailing space on the header line.
        assert!(
            !output.lines().any(|l| l.ends_with(' ')),
            "trailing whitespace in:\n{:?}",
            output
        );
    }

    #[test]
    fn test_format_mr_note_list_token_savings() {
        let output = format_mr_note_list(&parse_fixture(NOTE_LIST_FIXTURE), false);
        let input_tokens = count_tokens(NOTE_LIST_FIXTURE);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        // Savings scale with how much author-folded content the comments carry; human
        // bodies are reproduced in full, so this stays well clear of the raw JSON.
        assert!(
            savings >= 60.0,
            "MR note list: expected >=60% savings, got {:.1}% ({} -> {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── collapse_details_blocks ──────────────────────────────────────────

    #[test]
    fn test_collapse_details_blocks_keeps_summary_and_counts_lines() {
        let body = "before\n<details>\n<summary>Folded label</summary>\n<p>\none\ntwo\n</p>\n</details>\nafter";
        let (collapsed, folded) = collapse_details_blocks(body);
        assert!(collapsed.contains("before"));
        assert!(collapsed.contains("Folded label"));
        assert!(collapsed.contains("after"));
        assert!(!collapsed.contains("one"));
        assert!(folded > 0, "expected a folded line count, got {}", folded);
    }

    #[test]
    fn test_collapse_details_blocks_folds_across_code_fences() {
        // Real folded blocks embed fences; the fold must span them.
        let body = "<details>\n<summary>Bump</summary>\n<p>\n\n```diff\n-1\n+2\n```\n\n</p>\n</details>";
        let (collapsed, folded) = collapse_details_blocks(body);
        assert!(collapsed.contains("Bump"));
        assert!(!collapsed.contains("+2"));
        assert!(folded > 0);
    }

    #[test]
    fn test_collapse_details_blocks_leaves_plain_body_untouched() {
        let body = "A normal review comment.\n\nWith a second paragraph.";
        let (collapsed, folded) = collapse_details_blocks(body);
        assert_eq!(collapsed, body);
        assert_eq!(folded, 0);
    }

    #[test]
    fn test_collapse_details_blocks_leaves_unterminated_block_untouched() {
        // No closing tag — nothing to fold, and the content must survive.
        let body = "<details>\n<summary>Open</summary>\nstill here";
        let (collapsed, folded) = collapse_details_blocks(body);
        assert!(collapsed.contains("still here"));
        assert_eq!(folded, 0);
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
