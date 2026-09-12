//! Gitea CLI (tea) command output compression.
//!
//! Provides token-optimized alternatives to verbose `tea` commands.
//! Mirrors gh_cmd.rs patterns, adapted for tea-specific differences:
//! - Entities have aliases: `pulls`/`pull`/`pr`, `issues`/`issue`/`i`,
//!   `releases`/`release`/`r`.
//! - JSON is requested via `-o json`, not a `--json <fields>` flag.
//! - List-mode JSON renders every field as a string (even `index`), while
//!   single-item view JSON renders `index` as a number — callers must accept
//!   both.
//! - `labels` is a comma-joined string in list mode but an array of label
//!   objects in view mode.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_LIST;
use crate::core::utils::{resolved_command, truncate};
use anyhow::Result;
use serde_json::Value;
use std::process::Command;

/// Collapse tea's entity aliases down to a canonical name.
fn normalize_entity(subcommand: &str) -> &str {
    match subcommand {
        "pulls" | "pull" | "pr" => "pr",
        "issues" | "issue" | "i" => "issue",
        "releases" | "release" | "r" => "release",
        other => other,
    }
}

pub fn run(subcommand: &str, args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
    match normalize_entity(subcommand) {
        "pr" => run_pr(args, verbose, ultra_compact),
        "issue" => run_issue(args, verbose, ultra_compact),
        "release" => run_release(args, verbose, ultra_compact),
        _ => run_passthrough("tea", subcommand, args),
    }
}

/// User already asked for a specific output format — respect it instead of
/// forcing `-o json` and reformatting.
fn has_output_flag(args: &[String]) -> bool {
    args.iter().any(|a| a == "-o" || a == "--output")
}

fn is_index(arg: &str) -> bool {
    !arg.is_empty() && arg.chars().all(|c| c.is_ascii_digit())
}

fn run_pr(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return list_prs(args, ultra_compact);
    }
    match args[0].as_str() {
        "list" | "ls" => list_prs(&args[1..], ultra_compact),
        idx if is_index(idx) => view_pr(args, ultra_compact),
        _ => run_passthrough("tea", "pr", args),
    }
}

fn list_prs(args: &[String], ultra_compact: bool) -> Result<i32> {
    if has_output_flag(args) {
        return run_passthrough_with_extra("tea", &["pr", "list"], args);
    }
    let mut cmd = resolved_command("tea");
    cmd.args(["pr", "list", "-f", "index,title,state,author,updated,labels"]);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.args(["-o", "json"]);
    run_tea_json(cmd, "pr list", |json| format_pr_list(json, ultra_compact))
}

fn view_pr(args: &[String], ultra_compact: bool) -> Result<i32> {
    let index = &args[0];
    let extra = &args[1..];
    if has_output_flag(extra) {
        return run_passthrough_with_extra("tea", &["pr", index], extra);
    }
    let mut cmd = resolved_command("tea");
    cmd.args(["pr", index]);
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.args(["-o", "json"]);
    run_tea_json(cmd, &format!("pr {}", index), |json| {
        format_pr_view(json, ultra_compact)
    })
}

fn run_issue(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return list_issues(args, ultra_compact);
    }
    match args[0].as_str() {
        "list" | "ls" => list_issues(&args[1..], ultra_compact),
        idx if is_index(idx) => view_issue(args, ultra_compact),
        _ => run_passthrough("tea", "issue", args),
    }
}

fn list_issues(args: &[String], ultra_compact: bool) -> Result<i32> {
    if has_output_flag(args) {
        return run_passthrough_with_extra("tea", &["issue", "list"], args);
    }
    let mut cmd = resolved_command("tea");
    cmd.args([
        "issue",
        "list",
        "-f",
        "index,title,state,author,updated,labels",
    ]);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.args(["-o", "json"]);
    run_tea_json(cmd, "issue list", |json| {
        format_issue_list(json, ultra_compact)
    })
}

fn view_issue(args: &[String], ultra_compact: bool) -> Result<i32> {
    let index = &args[0];
    let extra = &args[1..];
    if has_output_flag(extra) {
        return run_passthrough_with_extra("tea", &["issue", index], extra);
    }
    let mut cmd = resolved_command("tea");
    cmd.args(["issue", index]);
    for arg in extra {
        cmd.arg(arg);
    }
    cmd.args(["-o", "json"]);
    run_tea_json(cmd, &format!("issue {}", index), |json| {
        format_issue_view(json, ultra_compact)
    })
}

fn run_release(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
    if args.is_empty() {
        return list_releases(args, ultra_compact);
    }
    match args[0].as_str() {
        "list" | "ls" => list_releases(&args[1..], ultra_compact),
        _ => run_passthrough("tea", "release", args),
    }
}

fn list_releases(args: &[String], ultra_compact: bool) -> Result<i32> {
    if has_output_flag(args) {
        return run_passthrough_with_extra("tea", &["release", "list"], args);
    }
    let mut cmd = resolved_command("tea");
    cmd.args(["release", "list"]);
    for arg in args {
        cmd.arg(arg);
    }
    cmd.args(["-o", "json"]);
    run_tea_json(cmd, "release list", |json| {
        format_release_list(json, ultra_compact)
    })
}

fn run_tea_json<F>(cmd: Command, label: &str, filter_fn: F) -> Result<i32>
where
    F: Fn(&Value) -> String,
{
    runner::run_filtered(
        cmd,
        "tea",
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

fn run_passthrough(cmd: &str, subcommand: &str, args: &[String]) -> Result<i32> {
    let mut os_args: Vec<std::ffi::OsString> = vec![std::ffi::OsString::from(subcommand)];
    os_args.extend(args.iter().map(std::ffi::OsString::from));
    crate::core::runner::run_passthrough(cmd, &os_args, 0)
}

fn run_passthrough_with_extra(cmd: &str, base_args: &[&str], extra_args: &[String]) -> Result<i32> {
    let mut os_args: Vec<std::ffi::OsString> =
        base_args.iter().map(std::ffi::OsString::from).collect();
    os_args.extend(extra_args.iter().map(std::ffi::OsString::from));
    crate::core::runner::run_passthrough(cmd, &os_args, 0)
}

/// tea's list-mode JSON renders every field as a string, including numeric
/// indexes; single-item view JSON renders `index` as a number. Accept both.
fn get_index(json: &Value, key: &str) -> String {
    match &json[key] {
        Value::Number(n) => n.to_string(),
        Value::String(s) => s.clone(),
        _ => "?".to_string(),
    }
}

fn get_str<'a>(json: &'a Value, key: &str) -> &'a str {
    json[key].as_str().unwrap_or("")
}

fn get_bool(json: &Value, key: &str) -> bool {
    match &json[key] {
        Value::Bool(b) => *b,
        Value::String(s) => s == "true",
        _ => false,
    }
}

/// `labels` is a comma-joined string in list mode but an array of label
/// objects (each with a `name` field) in view mode.
fn labels_str(json: &Value) -> String {
    match &json["labels"] {
        Value::String(s) => s.clone(),
        Value::Array(items) => items
            .iter()
            .filter_map(|l| l.get("name").and_then(|n| n.as_str()))
            .collect::<Vec<_>>()
            .join(","),
        _ => String::new(),
    }
}

/// State icon for PR/issue states (tea uses lowercase, like glab).
fn state_icon(state: &str, ultra_compact: bool) -> &'static str {
    if ultra_compact {
        match state {
            "open" => "O",
            "merged" => "M",
            "closed" => "C",
            _ => "?",
        }
    } else {
        match state {
            "open" => "[open]",
            "merged" => "[merged]",
            "closed" => "[closed]",
            _ => "[unknown]",
        }
    }
}

fn format_pr_list(json: &Value, ultra_compact: bool) -> String {
    let prs = match json.as_array() {
        Some(prs) => prs,
        None => return String::new(),
    };
    if prs.is_empty() {
        return if ultra_compact {
            "No PRs\n".to_string()
        } else {
            "No Pull Requests\n".to_string()
        };
    }
    let mut out = String::new();
    out.push_str(if ultra_compact {
        "PRs\n"
    } else {
        "Pull Requests\n"
    });
    let all_lines: Vec<String> = prs
        .iter()
        .map(|pr| {
            let index = get_index(pr, "index");
            let title = get_str(pr, "title");
            let state = get_str(pr, "state");
            let author = get_str(pr, "author");
            let icon = state_icon(state, ultra_compact);
            format!("  {} #{} {} ({})", icon, index, truncate(title, 60), author)
        })
        .collect();
    const MAX_LIST: usize = CAP_LIST;
    for line in all_lines.iter().take(MAX_LIST) {
        out.push_str(&format!("{}\n", line));
    }
    if all_lines.len() > MAX_LIST {
        out.push_str(&format!("  … +{} more\n", all_lines.len() - MAX_LIST));
        let all_text = all_lines.join("\n");
        if let Some(hint) = crate::core::tee::force_tee_tail_hint(&all_text, "tea-prs", MAX_LIST + 1)
        {
            out.push_str(&format!("  {}\n", hint));
        }
    }
    out
}

fn format_pr_view(json: &Value, ultra_compact: bool) -> String {
    let mut out = String::new();
    let index = get_index(json, "index");
    let title = get_str(json, "title");
    let state = get_str(json, "state");
    let author = get_str(json, "user");
    let url = get_str(json, "url");
    let mergeable = get_bool(json, "mergeable");
    let base = get_str(json, "base");
    let head = get_str(json, "head");

    let icon = state_icon(state, ultra_compact);
    out.push_str(&format!("{} PR #{}: {}\n", icon, index, title));
    out.push_str(&format!("  {}\n", author));

    let mergeable_str = if mergeable { "[ok]" } else { "[x]" };
    out.push_str(&format!("  {} | {}\n", state, mergeable_str));
    out.push_str(&format!("  {} -> {}\n", head, base));

    let labels = labels_str(json);
    if !labels.is_empty() {
        out.push_str(&format!("  Labels: {}\n", labels));
    }

    if let Some(reviews) = json["reviews"].as_array() {
        let approved = reviews
            .iter()
            .filter(|r| r["state"].as_str() == Some("APPROVED"))
            .count();
        let changes = reviews
            .iter()
            .filter(|r| r["state"].as_str() == Some("REQUEST_CHANGES"))
            .count();
        if approved > 0 || changes > 0 {
            out.push_str(&format!(
                "  Reviews: {} approved, {} changes requested\n",
                approved, changes
            ));
        }
    }

    out.push_str(&format!("  {}\n", url));

    let body = get_str(json, "body");
    if !body.is_empty() {
        out.push('\n');
        for line in body.lines() {
            out.push_str(&format!("  {}\n", line));
        }
    }

    out
}

fn format_issue_list(json: &Value, ultra_compact: bool) -> String {
    let issues = match json.as_array() {
        Some(issues) => issues,
        None => return String::new(),
    };
    if issues.is_empty() {
        return "No Issues\n".to_string();
    }
    let mut out = String::new();
    out.push_str("Issues\n");
    let all_lines: Vec<String> = issues
        .iter()
        .map(|issue| {
            let index = get_index(issue, "index");
            let title = get_str(issue, "title");
            let state = get_str(issue, "state");
            let icon = if ultra_compact {
                if state == "open" { "O" } else { "C" }
            } else if state == "open" {
                "[open]"
            } else {
                "[closed]"
            };
            format!("  {} #{} {}", icon, index, truncate(title, 60))
        })
        .collect();
    const MAX_LIST: usize = CAP_LIST;
    for line in all_lines.iter().take(MAX_LIST) {
        out.push_str(&format!("{}\n", line));
    }
    if all_lines.len() > MAX_LIST {
        out.push_str(&format!("  … +{} more\n", all_lines.len() - MAX_LIST));
        let all_text = all_lines.join("\n");
        if let Some(hint) =
            crate::core::tee::force_tee_tail_hint(&all_text, "tea-issues", MAX_LIST + 1)
        {
            out.push_str(&format!("  {}\n", hint));
        }
    }
    out
}

fn format_issue_view(json: &Value, _ultra_compact: bool) -> String {
    let mut out = String::new();
    let index = get_index(json, "index");
    let title = get_str(json, "title");
    let state = get_str(json, "state");
    let author = get_str(json, "user");
    let url = get_str(json, "url");

    let icon = if state == "open" { "[open]" } else { "[closed]" };
    out.push_str(&format!("{} Issue #{}: {}\n", icon, index, title));
    out.push_str(&format!("  Author: @{}\n", author));
    out.push_str(&format!("  Status: {}\n", state));
    let labels = labels_str(json);
    if !labels.is_empty() {
        out.push_str(&format!("  Labels: {}\n", labels));
    }
    out.push_str(&format!("  URL: {}\n", url));

    let body = get_str(json, "body");
    if !body.is_empty() {
        out.push_str("\n  Description:\n");
        for line in body.lines() {
            out.push_str(&format!("    {}\n", line));
        }
    }
    out
}

/// tea's release-list JSON field names have observed inconsistencies across
/// versions (e.g. `tag-_name` instead of `tag_name`) — try known variants
/// defensively rather than tying this to one build's exact keys.
fn get_first<'a>(json: &'a Value, keys: &[&str]) -> &'a str {
    for key in keys {
        if let Some(s) = json[*key].as_str() {
            if !s.is_empty() {
                return s;
            }
        }
    }
    ""
}

fn format_release_list(json: &Value, ultra_compact: bool) -> String {
    let releases = match json.as_array() {
        Some(r) => r,
        None => return String::new(),
    };
    if releases.is_empty() {
        return if ultra_compact {
            "No releases\n".to_string()
        } else {
            "No Releases\n".to_string()
        };
    }
    let mut out = String::new();
    out.push_str("Releases\n");
    let all_lines: Vec<String> = releases
        .iter()
        .map(|release| {
            let tag = get_first(release, &["tag_name", "tagName", "tag-_name", "tag"]);
            let title = get_first(release, &["title", "name"]);
            let published = get_first(
                release,
                &["published_at", "publishedAt", "published _at", "created_at"],
            );
            let status = get_first(release, &["status"]);
            let label = if !title.is_empty() && title != tag {
                format!("{} ({})", tag, truncate(title, 40))
            } else {
                tag.to_string()
            };
            if status == "draft" || status == "prerelease" {
                format!("  [{}] {} — {}", status, label, published)
            } else {
                format!("  {} — {}", label, published)
            }
        })
        .collect();
    const MAX_LIST: usize = CAP_LIST;
    for line in all_lines.iter().take(MAX_LIST) {
        out.push_str(&format!("{}\n", line));
    }
    if all_lines.len() > MAX_LIST {
        out.push_str(&format!("  … +{} more\n", all_lines.len() - MAX_LIST));
        let all_text = all_lines.join("\n");
        if let Some(hint) =
            crate::core::tee::force_tee_tail_hint(&all_text, "tea-releases", MAX_LIST + 1)
        {
            out.push_str(&format!("  {}\n", hint));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn test_normalize_entity_aliases() {
        assert_eq!(normalize_entity("pulls"), "pr");
        assert_eq!(normalize_entity("pull"), "pr");
        assert_eq!(normalize_entity("pr"), "pr");
        assert_eq!(normalize_entity("issues"), "issue");
        assert_eq!(normalize_entity("i"), "issue");
        assert_eq!(normalize_entity("releases"), "release");
        assert_eq!(normalize_entity("r"), "release");
        assert_eq!(normalize_entity("api"), "api");
    }

    #[test]
    fn test_is_index() {
        assert!(is_index("2910"));
        assert!(is_index("0"));
        assert!(!is_index(""));
        assert!(!is_index("list"));
        assert!(!is_index("-5"));
    }

    #[test]
    fn test_has_output_flag() {
        assert!(has_output_flag(&["-o".into(), "json".into()]));
        assert!(has_output_flag(&["--output".into(), "yaml".into()]));
        assert!(!has_output_flag(&["--repo".into(), "eos/eos".into()]));
    }

    #[test]
    fn test_get_index_from_string_and_number() {
        let list_style = json!({"index": "2910"});
        let view_style = json!({"index": 2910});
        assert_eq!(get_index(&list_style, "index"), "2910");
        assert_eq!(get_index(&view_style, "index"), "2910");
        assert_eq!(get_index(&json!({}), "index"), "?");
    }

    #[test]
    fn test_get_bool_from_bool_and_string() {
        assert!(get_bool(&json!({"mergeable": true}), "mergeable"));
        assert!(get_bool(&json!({"mergeable": "true"}), "mergeable"));
        assert!(!get_bool(&json!({"mergeable": "false"}), "mergeable"));
        assert!(!get_bool(&json!({}), "mergeable"));
    }

    #[test]
    fn test_labels_str_from_csv_string() {
        let list_style = json!({"labels": "bug,priority:low"});
        assert_eq!(labels_str(&list_style), "bug,priority:low");
    }

    #[test]
    fn test_labels_str_from_object_array() {
        let view_style = json!({"labels": [{"name": "bug"}, {"name": "help wanted"}]});
        assert_eq!(labels_str(&view_style), "bug,help wanted");
    }

    #[test]
    fn test_labels_str_empty() {
        assert_eq!(labels_str(&json!({"labels": []})), "");
        assert_eq!(labels_str(&json!({})), "");
    }

    #[test]
    fn test_state_icon_pr_states() {
        assert_eq!(state_icon("open", false), "[open]");
        assert_eq!(state_icon("merged", false), "[merged]");
        assert_eq!(state_icon("closed", false), "[closed]");
        assert_eq!(state_icon("open", true), "O");
        assert_eq!(state_icon("weird", false), "[unknown]");
    }

    #[test]
    fn test_format_pr_list_empty() {
        assert_eq!(format_pr_list(&json!([]), false), "No Pull Requests\n");
        assert_eq!(format_pr_list(&json!([]), true), "No PRs\n");
    }

    #[test]
    fn test_format_pr_list_basic() {
        let data = json!([
            {"index": "42", "title": "Fix bug", "state": "open", "author": "alice"}
        ]);
        let out = format_pr_list(&data, false);
        assert!(out.contains("#42"));
        assert!(out.contains("Fix bug"));
        assert!(out.contains("alice"));
        assert!(out.contains("[open]"));
    }

    #[test]
    fn test_format_pr_list_truncates_at_cap() {
        let prs: Vec<Value> = (0..CAP_LIST + 5)
            .map(|i| json!({"index": i.to_string(), "title": "t", "state": "open", "author": "a"}))
            .collect();
        let out = format_pr_list(&json!(prs), false);
        assert!(out.contains("+5 more"));
    }

    #[test]
    fn test_format_pr_view_mergeable() {
        let data = json!({
            "index": 42,
            "title": "Fix bug",
            "state": "open",
            "user": "alice",
            "url": "https://example.test/pulls/42",
            "mergeable": true,
            "base": "main",
            "head": "fix-branch",
            "labels": [],
            "body": ""
        });
        let out = format_pr_view(&data, false);
        assert!(out.contains("PR #42"));
        assert!(out.contains("[ok]"));
        assert!(out.contains("fix-branch -> main"));
    }

    #[test]
    fn test_format_issue_list_empty() {
        assert_eq!(format_issue_list(&json!([]), false), "No Issues\n");
    }

    #[test]
    fn test_format_issue_view_includes_body() {
        let data = json!({
            "index": 179,
            "title": "Dependency Dashboard",
            "state": "open",
            "user": "renovate-bot",
            "url": "https://example.test/issues/179",
            "labels": "",
            "body": "hello"
        });
        let out = format_issue_view(&data, false);
        assert!(out.contains("Issue #179"));
        assert!(out.contains("@renovate-bot"));
        assert!(out.contains("hello"));
    }

    #[test]
    fn test_format_release_list_key_variants() {
        // Regression: some tea builds emit odd JSON keys like "tag-_name"
        // instead of "tag_name" — the formatter must tolerate both.
        let data = json!([
            {"tag_name": "v1.0.0", "title": "v1.0.0", "published_at": "2026-01-01", "status": "released"},
            {"tag-_name": "v0.9.0", "title": "v0.9.0", "published _at": "2025-12-01", "status": "released"}
        ]);
        let out = format_release_list(&data, false);
        assert!(out.contains("v1.0.0"));
        assert!(out.contains("v0.9.0"));
        assert!(out.contains("2025-12-01"));
    }

    #[test]
    fn test_format_release_list_empty() {
        assert_eq!(format_release_list(&json!([]), false), "No Releases\n");
    }
}
