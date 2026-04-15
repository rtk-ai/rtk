//! Jira filter module — `rtk acli jira <object> <verb> [args...]`
//!
//! Dispatch table maps (object, verb) to a filter function.
//! Unknown combinations fallthrough to unfiltered passthrough.

use anyhow::Result;
use serde_json::Value;

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, resolved_command, truncate};

const MAX_ITEMS: usize = 25;
const MAX_DESC_CHARS: usize = 600;
const MAX_AC_CHARS: usize = 600;
const MAX_COMMENT_CHARS: usize = 250;
const MAX_COMMENTS: usize = 3;

/// Atlassian customfield ID for acceptance criteria (Anywhere workspace).
const FIELD_ACCEPTANCE_CRITERIA: &str = "customfield_10047";

/// Entry point: `args` = ["workitem", "view", "AWM-123"] etc.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let object = args.first().map(|s| s.as_str()).unwrap_or("");
    let verb = args.get(1).map(|s| s.as_str()).unwrap_or("");
    let rest = if args.len() > 2 { &args[2..] } else { &[][..] };

    match (object, verb) {
        ("workitem", "view") => run_workitem_view(rest, verbose),
        ("workitem", "search") => run_filtered(
            &["jira", "workitem", "search"],
            rest,
            "acli_jira_workitem_search",
            verbose,
            filter_workitem_search,
        ),
        ("sprint", "list-workitems") => run_filtered(
            &["jira", "sprint", "list-workitems"],
            rest,
            "acli_jira_sprint_workitems",
            verbose,
            filter_workitem_search, // same JSON shape as workitem search
        ),
        _ => run_passthrough(args, verbose),
    }
}

/// Specialised runner for `workitem view` — injects `--fields *all` so
/// description, acceptance criteria, and comments are always fetched.
fn run_workitem_view(extra_args: &[String], verbose: u8) -> Result<i32> {
    let sub_args: &[&str] = &["jira", "workitem", "view"];
    let tee_slug = "acli_jira_workitem_view";
    let timer = tracking::TimedExecution::start();
    let cmd_label = "acli jira workitem view".to_string();
    let rtk_label = format!("rtk {}", cmd_label);

    if verbose > 0 {
        eprintln!(
            "rtk acli jira: running acli {} {}",
            sub_args.join(" "),
            extra_args.join(" ")
        );
    }

    let mut cmd = resolved_command("acli");
    cmd.args(sub_args);
    cmd.args(extra_args);

    // Ensure we always get JSON + all fields (description, AC, comments)
    let has_json = extra_args.iter().any(|a| a == "--json");
    let has_fields = extra_args.iter().any(|a| a == "--fields" || a == "-f");
    if !has_json {
        cmd.arg("--json");
    }
    if !has_fields {
        cmd.arg("--fields").arg("*all");
    }

    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run acli: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let raw = format!("{}\n{}", stdout, stderr);
    let exit_code = exit_code_from_output(&output, "acli");

    if exit_code != 0 {
        if let Some(hint) = crate::core::tee::tee_and_hint(&raw, tee_slug, exit_code) {
            eprintln!("{}\n{}", stderr.trim(), hint);
        } else {
            eprint!("{}", stderr);
        }
        timer.track(&cmd_label, &rtk_label, &raw, &stderr);
        return Ok(exit_code);
    }

    let filtered = filter_workitem_view(&stdout);
    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, tee_slug, 0) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(&cmd_label, &rtk_label, &raw, &filtered);
    Ok(0)
}

/// Execute acli jira <sub_args> <extra_args>, apply filter_fn to stdout, track savings.
fn run_filtered(
    sub_args: &[&str],
    extra_args: &[String],
    tee_slug: &str,
    verbose: u8,
    filter_fn: fn(&str) -> String,
) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    let cmd_label = format!("acli {}", sub_args.join(" "));
    let rtk_label = format!("rtk {}", cmd_label);

    if verbose > 0 {
        eprintln!(
            "rtk acli jira: running acli {} {}",
            sub_args.join(" "),
            extra_args.join(" ")
        );
    }

    let mut cmd = resolved_command("acli");
    cmd.args(sub_args);
    cmd.args(extra_args);
    let needs_json = !extra_args.iter().any(|a| a == "--json" || a == "--csv");
    if needs_json {
        cmd.arg("--json");
    }

    let output = cmd
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run acli: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    let raw = format!("{}\n{}", stdout, stderr);
    let exit_code = exit_code_from_output(&output, "acli");

    if exit_code != 0 {
        if let Some(hint) = crate::core::tee::tee_and_hint(&raw, tee_slug, exit_code) {
            eprintln!("{}\n{}", stderr.trim(), hint);
        } else {
            eprint!("{}", stderr);
        }
        timer.track(&cmd_label, &rtk_label, &raw, &stderr);
        return Ok(exit_code);
    }

    let filtered = filter_fn(&stdout);
    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, tee_slug, 0) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(&cmd_label, &rtk_label, &raw, &filtered);
    Ok(0)
}

/// Passthrough for unhandled jira subcommands (create, edit, transition, etc.)
fn run_passthrough(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();
    if verbose > 0 {
        eprintln!("rtk acli jira: passthrough for '{}'", args.join(" "));
    }
    let mut cmd_args: Vec<String> = vec!["jira".to_string()];
    cmd_args.extend_from_slice(args);

    let output = resolved_command("acli")
        .args(&cmd_args)
        .output()
        .map_err(|e| anyhow::anyhow!("Failed to run acli jira: {}", e))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let exit_code = exit_code_from_output(&output, "acli");

    if !stdout.is_empty() {
        print!("{}", stdout);
    }
    if !stderr.is_empty() {
        eprint!("{}", stderr);
    }

    let raw = format!("{}\n{}", stdout, stderr);
    timer.track(
        &format!("acli jira {}", args.join(" ")),
        &format!("rtk acli jira {} (passthrough)", args.join(" ")),
        &raw,
        &raw,
    );

    Ok(exit_code)
}

// ---------------------------------------------------------------------------
// Filter functions
// ---------------------------------------------------------------------------

/// Filter `acli jira workitem view --fields *all --json` output.
///
/// Extracts: key, type, summary, status, priority, assignee, reporter,
/// description, acceptance criteria (customfield_10047), and last N comments.
pub fn filter_workitem_view(raw: &str) -> String {
    let v: Value = match serde_json::from_str(raw) {
        Ok(v) => v,
        Err(_) => return raw.to_string(),
    };

    let key = v["key"].as_str().unwrap_or("").to_string();
    let fields = &v["fields"];

    let summary = fields["summary"].as_str().unwrap_or("").to_string();
    let issue_type = str_path(fields, &["issuetype", "name"]);
    let status = str_path(fields, &["status", "name"]);
    let status_cat = str_path(fields, &["status", "statusCategory", "name"]);
    let priority = str_path(fields, &["priority", "name"]);
    let assignee = str_path(fields, &["assignee", "displayName"]);
    let reporter = str_path(fields, &["reporter", "displayName"]);
    let parent_key = str_path(fields, &["parent", "key"]);
    let parent_summary = str_path(fields, &["parent", "fields", "summary"]);

    let mut lines: Vec<String> = Vec::new();

    // Header: KEY [Type] Summary
    let type_tag = if issue_type.is_empty() {
        String::new()
    } else {
        format!(" [{}]", issue_type)
    };
    lines.push(format!(
        "{}{}{}",
        key,
        type_tag,
        if summary.is_empty() {
            String::new()
        } else {
            format!(" {}", summary)
        }
    ));

    // Parent context (useful for sub-tasks)
    if !parent_key.is_empty() {
        let parent_info = if parent_summary.is_empty() {
            parent_key.clone()
        } else {
            format!("{} — {}", parent_key, truncate(&parent_summary, 60))
        };
        lines.push(format!("Parent: {}", parent_info));
    }

    // Metadata
    let cat_str = if status_cat.is_empty() || status_cat == status {
        String::new()
    } else {
        format!(" ({})", status_cat)
    };
    if !status.is_empty() {
        lines.push(format!("Status: {}{}", status, cat_str));
    }
    if !priority.is_empty() {
        lines.push(format!("Priority: {}", priority));
    }
    if assignee.is_empty() {
        lines.push("Assignee: unassigned".to_string());
    } else {
        lines.push(format!("Assignee: {}", assignee));
    }
    if !reporter.is_empty() && reporter != assignee {
        lines.push(format!("Reporter: {}", reporter));
    }

    // Description
    let description = adf_to_text(&fields["description"]);
    if !description.is_empty() {
        lines.push(String::new());
        lines.push("Description:".to_string());
        lines.push(truncate(&description, MAX_DESC_CHARS));
    }

    // Acceptance Criteria (customfield_10047)
    let ac = adf_to_text(&fields[FIELD_ACCEPTANCE_CRITERIA]);
    if !ac.is_empty() {
        lines.push(String::new());
        lines.push("Acceptance Criteria:".to_string());
        lines.push(truncate(&ac, MAX_AC_CHARS));
    }

    // Comments: last N, oldest-first within the window
    let comments = extract_comments(fields);
    if !comments.is_empty() {
        let total = comments.len();
        let start = total.saturating_sub(MAX_COMMENTS);
        let shown = &comments[start..];
        lines.push(String::new());
        lines.push(format!(
            "Comments ({total}){}:",
            if total > MAX_COMMENTS {
                format!(" — showing last {MAX_COMMENTS}")
            } else {
                String::new()
            }
        ));
        for (author, body) in shown {
            lines.push(format!("  {}: {}", author, truncate(body, MAX_COMMENT_CHARS)));
        }
    }

    lines.join("\n")
}

/// Filter `acli jira workitem search --json` output.
/// Compact table: key | type | status | priority | assignee | summary, capped at MAX_ITEMS.
pub fn filter_workitem_search(raw: &str) -> String {
    let items: Vec<Value> = match serde_json::from_str(raw) {
        Ok(Value::Array(arr)) => arr,
        _ => return raw.to_string(),
    };

    if items.is_empty() {
        return "No issues found.".to_string();
    }

    let total = items.len();

    let mut lines: Vec<String> = Vec::with_capacity(total.min(MAX_ITEMS) + 2);
    lines.push(format!(
        "{:<10} {:<11} {:<11} {:<4} {:<14} {}",
        "KEY", "TYPE", "STATUS", "PRI", "ASSIGNEE", "SUMMARY"
    ));
    lines.push("-".repeat(100));

    for item in items.iter().take(MAX_ITEMS) {
        let key = item["key"].as_str().unwrap_or("").to_string();
        let fields = &item["fields"];
        let issue_type = str_path(fields, &["issuetype", "name"]);
        let status = str_path(fields, &["status", "name"]);
        let priority = str_path(fields, &["priority", "name"]);
        let assignee = str_path(fields, &["assignee", "displayName"]);
        let summary = fields["summary"].as_str().unwrap_or("").to_string();

        let pri_char = priority.chars().next().map(|c| c.to_string()).unwrap_or_default();

        lines.push(format!(
            "{:<10} {:<11} {:<11} {:<4} {:<14} {}",
            truncate(&key, 10),
            truncate(&issue_type, 11),
            truncate(&status, 11),
            pri_char,
            truncate(&assignee, 14),
            truncate(&summary, 60),
        ));
    }

    if total > MAX_ITEMS {
        lines.push(format!("... +{} more issues", total - MAX_ITEMS));
    }

    lines.join("\n")
}

// ---------------------------------------------------------------------------
// ADF (Atlassian Document Format) text extractor
// ---------------------------------------------------------------------------

/// Recursively extract plain text from an ADF node (object or JSON string).
/// Formats headings with `#` markers and preserves list bullets.
pub fn adf_to_text(node: &Value) -> String {
    match node {
        Value::String(s) => {
            // Sometimes the ADF is a JSON string — try to parse it
            if let Ok(parsed) = serde_json::from_str::<Value>(s) {
                return adf_to_text(&parsed);
            }
            s.clone()
        }
        Value::Object(_) => {
            let mut buf = String::new();
            collect_adf(&mut buf, node, 0);
            buf.trim().to_string()
        }
        Value::Null => String::new(),
        _ => String::new(),
    }
}

fn collect_adf(buf: &mut String, node: &Value, depth: usize) {
    let node_type = node["type"].as_str().unwrap_or("");

    match node_type {
        "doc" => {
            for child in node["content"].as_array().unwrap_or(&vec![]) {
                collect_adf(buf, child, depth);
            }
        }
        "heading" => {
            let level = node["attrs"]["level"].as_u64().unwrap_or(1) as usize;
            let prefix = "#".repeat(level.min(3));
            let mut text = String::new();
            for child in node["content"].as_array().unwrap_or(&vec![]) {
                collect_inline(child, &mut text);
            }
            if !text.trim().is_empty() {
                buf.push_str(&format!("{} {}\n", prefix, text.trim()));
            }
        }
        "paragraph" => {
            let mut text = String::new();
            for child in node["content"].as_array().unwrap_or(&vec![]) {
                collect_inline(child, &mut text);
            }
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                buf.push_str(trimmed);
                buf.push('\n');
            }
        }
        "bulletList" | "orderedList" => {
            for child in node["content"].as_array().unwrap_or(&vec![]) {
                collect_adf(buf, child, depth + 1);
            }
        }
        "listItem" => {
            let indent = "  ".repeat(depth);
            let mut text = String::new();
            for child in node["content"].as_array().unwrap_or(&vec![]) {
                // flatten nested content for list items
                if child["type"].as_str() == Some("paragraph") {
                    for inline in child["content"].as_array().unwrap_or(&vec![]) {
                        collect_inline(inline, &mut text);
                    }
                } else {
                    collect_adf(&mut text, child, depth + 1);
                }
            }
            let trimmed = text.trim();
            if !trimmed.is_empty() {
                buf.push_str(&format!("{}• {}\n", indent, trimmed));
            }
        }
        "rule" => {
            buf.push_str("---\n");
        }
        "codeBlock" => {
            let mut text = String::new();
            for child in node["content"].as_array().unwrap_or(&vec![]) {
                if let Some(t) = child["text"].as_str() {
                    text.push_str(t);
                }
            }
            if !text.trim().is_empty() {
                buf.push_str(&format!("```\n{}\n```\n", text.trim()));
            }
        }
        _ => {
            // Recurse into unknown node types
            if let Some(children) = node["content"].as_array() {
                for child in children {
                    collect_adf(buf, child, depth);
                }
            }
        }
    }
}

fn collect_inline(node: &Value, buf: &mut String) {
    match node["type"].as_str() {
        Some("text") => {
            if let Some(text) = node["text"].as_str() {
                buf.push_str(text);
            }
        }
        Some("hardBreak") => buf.push('\n'),
        Some("mention") => {
            let name = node["attrs"]["text"]
                .as_str()
                .or_else(|| node["attrs"]["id"].as_str())
                .unwrap_or("@user");
            buf.push_str(name);
        }
        _ => {
            // Recurse for inline nodes with children
            if let Some(children) = node["content"].as_array() {
                for child in children {
                    collect_inline(child, buf);
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// JSON helpers
// ---------------------------------------------------------------------------

fn str_path(v: &Value, path: &[&str]) -> String {
    let mut cur = v;
    for &key in path {
        cur = &cur[key];
    }
    cur.as_str().unwrap_or("").to_string()
}

/// Extract comments as (author, body) pairs.
fn extract_comments(fields: &Value) -> Vec<(String, String)> {
    let Some(comments) = fields["comment"]["comments"].as_array() else {
        return Vec::new();
    };
    comments
        .iter()
        .map(|c| {
            let author = c["author"]["displayName"]
                .as_str()
                .unwrap_or("unknown")
                .to_string();
            let body = adf_to_text(&c["body"]);
            (author, body)
        })
        .collect()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    // ---- workitem view ----

    #[test]
    fn test_workitem_view_snapshot() {
        let input = include_str!("../../../tests/fixtures/acli_jira_workitem_view_raw.txt");
        let output = filter_workitem_view(input);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_workitem_view_savings() {
        let input = include_str!("../../../tests/fixtures/acli_jira_workitem_view_raw.txt");
        let output = filter_workitem_view(input);
        let pct =
            100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            pct >= 60.0,
            "workitem view: expected ≥60% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_workitem_view_empty() {
        let out = filter_workitem_view("");
        assert_eq!(out, "");
    }

    #[test]
    fn test_workitem_view_not_json() {
        let out = filter_workitem_view("✗ Error: unauthorized");
        assert_eq!(out, "✗ Error: unauthorized");
    }

    #[test]
    fn test_workitem_view_has_key_and_summary() {
        let input = include_str!("../../../tests/fixtures/acli_jira_workitem_view_raw.txt");
        let output = filter_workitem_view(input);
        assert!(output.contains("DEMO-1234"), "should contain ticket key");
        assert!(output.contains("JWT"), "should contain summary");
    }

    #[test]
    fn test_workitem_view_has_description() {
        let input = include_str!("../../../tests/fixtures/acli_jira_workitem_view_raw.txt");
        let output = filter_workitem_view(input);
        assert!(
            output.contains("Description:"),
            "should contain description section"
        );
    }

    #[test]
    fn test_workitem_view_has_acceptance_criteria() {
        let input = include_str!("../../../tests/fixtures/acli_jira_workitem_view_raw.txt");
        let output = filter_workitem_view(input);
        assert!(
            output.contains("Acceptance Criteria:"),
            "should contain acceptance criteria section"
        );
    }

    #[test]
    fn test_workitem_view_has_comments() {
        let input = include_str!("../../../tests/fixtures/acli_jira_workitem_view_raw.txt");
        let output = filter_workitem_view(input);
        assert!(
            output.contains("Comments ("),
            "should contain comments section"
        );
    }

    // ---- adf_to_text ----

    #[test]
    fn test_adf_paragraph() {
        let adf = serde_json::json!({
            "type": "doc",
            "content": [{
                "type": "paragraph",
                "content": [{"type": "text", "text": "Hello world"}]
            }]
        });
        assert_eq!(adf_to_text(&adf), "Hello world");
    }

    #[test]
    fn test_adf_heading() {
        let adf = serde_json::json!({
            "type": "doc",
            "content": [{
                "type": "heading",
                "attrs": {"level": 2},
                "content": [{"type": "text", "text": "Section"}]
            }]
        });
        assert!(adf_to_text(&adf).contains("## Section"));
    }

    #[test]
    fn test_adf_bullet_list() {
        let adf = serde_json::json!({
            "type": "doc",
            "content": [{
                "type": "bulletList",
                "content": [{
                    "type": "listItem",
                    "content": [{
                        "type": "paragraph",
                        "content": [{"type": "text", "text": "Item one"}]
                    }]
                }]
            }]
        });
        assert!(adf_to_text(&adf).contains("• Item one"));
    }

    #[test]
    fn test_adf_null() {
        assert_eq!(adf_to_text(&Value::Null), "");
    }

    // ---- workitem search ----

    #[test]
    fn test_workitem_search_snapshot() {
        let input =
            include_str!("../../../tests/fixtures/acli_jira_workitem_search_raw.txt");
        let output = filter_workitem_search(input);
        insta::assert_snapshot!(output);
    }

    #[test]
    fn test_workitem_search_savings() {
        let input =
            include_str!("../../../tests/fixtures/acli_jira_workitem_search_raw.txt");
        let output = filter_workitem_search(input);
        let pct =
            100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            pct >= 60.0,
            "workitem search: expected ≥60% savings, got {:.1}%",
            pct
        );
    }

    #[test]
    fn test_workitem_search_empty_array() {
        assert_eq!(filter_workitem_search("[]"), "No issues found.");
    }

    #[test]
    fn test_workitem_search_not_json() {
        let out = filter_workitem_search("✗ Error: JQL invalid");
        assert_eq!(out, "✗ Error: JQL invalid");
    }

    // ---- sprint list-workitems (same filter) ----

    #[test]
    fn test_sprint_workitems_snapshot() {
        let input =
            include_str!("../../../tests/fixtures/acli_jira_sprint_workitems_raw.txt");
        let output = filter_workitem_search(input);
        insta::assert_snapshot!(output);
    }
}
