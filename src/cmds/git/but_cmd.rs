//! Filters GitButler (`but`) CLI output for stacked-workflow commands.

use crate::core::guard::never_worse;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::utils::resolved_command;
use anyhow::{Context, Result};
use serde_json::Value;
use std::ffi::OsString;

const MAX_JSON_ITEMS: usize = 8;

/// Runs supported GitButler commands in JSON mode and renders their result compactly.
///
/// GitButler's human output is deliberately TUI-like and changes frequently. Its JSON output is
/// the stable automation interface, so RTK asks for it internally and falls back to passthrough
/// whenever a caller explicitly selects an output format or uses an unsupported command.
pub fn run(subcommand: &str, args: &[String], verbose: u8) -> Result<i32> {
    if !supports_json_filter(subcommand, args) {
        return passthrough(subcommand, args, verbose);
    }

    let timer = tracking::TimedExecution::start();
    let mut cmd = resolved_command("but");
    cmd.arg(subcommand).args(args).arg("--format=json");

    let result = exec_capture(&mut cmd)
        .with_context(|| format!("Failed to run but {subcommand}. Is GitButler installed?"))?;
    let raw = format!("{}{}", result.stdout, result.stderr);

    if !result.success() {
        print!("{}", result.stdout);
        eprint!("{}", result.stderr);
        return Ok(result.exit_code);
    }

    let output = if verbose > 0 {
        result.stdout.trim().to_string()
    } else {
        render_json(&result.stdout)
    };
    let shown = never_worse(&result.stdout, &output);
    println!("{shown}");

    let label = format!("but {} {}", subcommand, args.join(" ")).trim().to_string();
    timer.track(&label, &format!("rtk {label}"), &raw, shown);
    Ok(0)
}

fn supports_json_filter(subcommand: &str, args: &[String]) -> bool {
    if args.iter().any(|arg| {
        arg == "-j"
            || arg == "--json"
            || arg == "--format"
            || arg.starts_with("--format=")
    }) {
        return false;
    }

    matches!(subcommand, "status" | "diff" | "push" | "pull" | "show")
        || (subcommand == "branch" && args.first().is_some_and(|arg| arg == "list"))
}

fn passthrough(subcommand: &str, args: &[String], verbose: u8) -> Result<i32> {
    let mut command = vec![OsString::from(subcommand)];
    command.extend(args.iter().map(OsString::from));
    crate::core::runner::run_passthrough("but", &command, verbose)
}

fn render_json(input: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(input) else {
        return input.trim().to_string();
    };

    if let Some(status) = render_status(&value) {
        return status;
    }

    render_value(&value, 0)
}

fn render_status(value: &Value) -> Option<String> {
    let fields = value.as_object()?;
    let changes = fields.get("uncommittedChanges")?.as_array()?;
    let stacks = fields.get("stacks")?.as_array()?;

    let mut lines = Vec::new();
    let changes = render_changes(changes)?;
    if changes.is_empty() {
        lines.push("zz [uncommitted] (no changes)".to_string());
    } else {
        lines.push(format!("zz [uncommitted] {}", changes.join(" | ")));
    }

    for stack in stacks {
        lines.extend(render_stack(stack)?);
    }

    let merge_base = fields.get("mergeBase")?.as_object()?;
    let commit = merge_base.get("commitId")?.as_str()?;
    let message = merge_base
        .get("message")?
        .as_str()?
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim();
    let short_commit = &commit[..commit.len().min(7)];
    lines.push(format!("base: {short_commit} {message}"));

    if let Some(behind) = fields
        .get("upstreamState")
        .and_then(Value::as_object)
        .and_then(|state| state.get("behind"))
        .and_then(Value::as_u64)
        .filter(|behind| *behind > 0)
    {
        lines.push(format!("behind: {behind}"));
    }

    Some(lines.join("\n"))
}

fn render_stack(stack: &Value) -> Option<Vec<String>> {
    let stack = stack.as_object()?;
    let id = stack.get("cliId")?.as_str()?;
    let assigned = render_changes(stack.get("assignedChanges")?.as_array()?)?;
    let branches = stack.get("branches")?.as_array()?;
    let mut lines = vec![format!("{id} [stack]")];

    if !assigned.is_empty() {
        lines.push(format!("  assigned: {}", assigned.join(" | ")));
    }

    for branch in branches {
        lines.extend(render_branch(branch)?);
    }

    Some(lines)
}

fn render_branch(branch: &Value) -> Option<Vec<String>> {
    let branch = branch.as_object()?;
    let id = branch.get("cliId")?.as_str()?;
    let name = branch.get("name")?.as_str()?;
    let commits = branch.get("commits")?.as_array()?;
    let upstream = branch.get("upstreamCommits")?.as_array()?;
    let status = branch.get("branchStatus")?.as_str()?;
    let review = branch
        .get("reviewId")
        .and_then(Value::as_str)
        .map(|review| review.trim_matches(|character| character == '(' || character == ')'));

    let mut summary = format!("  {id} [{name}]");
    if let Some(review) = review.filter(|review| !review.is_empty()) {
        summary.push(' ');
        summary.push_str(review);
    }
    if commits.is_empty() && upstream.is_empty() {
        summary.push_str(" (no commits)");
    } else {
        summary.push_str(&format!(" ({} local", commits.len()));
        if !upstream.is_empty() {
            summary.push_str(&format!(", {} upstream", upstream.len()));
        }
        summary.push(')');
    }
    summary.push_str(&format!(" status={status}"));

    let mut lines = vec![summary];
    for commit in commits {
        lines.push(format!("    {}", render_commit(commit)?));
    }
    for commit in upstream {
        lines.push(format!("    ↑ {}", render_commit(commit)?));
    }
    Some(lines)
}

fn render_changes(changes: &[Value]) -> Option<Vec<String>> {
    changes
        .iter()
        .map(|change| {
            let change = change.as_object()?;
            let id = change.get("cliId")?.as_str()?;
            let path = change.get("filePath")?.as_str()?;
            let status = match change.get("changeType")?.as_str()? {
                "added" => "A",
                "deleted" => "D",
                "modified" => "M",
                other => other,
            };
            Some(format!("{id} {status} {path}"))
        })
        .collect()
}

fn render_commit(commit: &Value) -> Option<String> {
    let commit = commit.as_object()?;
    let id = commit.get("cliId")?.as_str()?;
    let commit_id = commit.get("commitId")?.as_str()?;
    let message = commit
        .get("message")?
        .as_str()?
        .lines()
        .find(|line| !line.trim().is_empty())?
        .trim();
    let short_commit = &commit_id[..commit_id.len().min(7)];
    Some(format!("{id} {short_commit} {message}"))
}

fn render_value(value: &Value, depth: usize) -> String {
    match value {
        Value::Null => "null".to_string(),
        Value::Bool(value) => value.to_string(),
        Value::Number(value) => value.to_string(),
        Value::String(value) => value.clone(),
        Value::Array(items) => render_array(items, depth),
        Value::Object(fields) => render_object(fields, depth),
    }
}

fn render_object(fields: &serde_json::Map<String, Value>, depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let mut lines = Vec::new();

    for (index, (name, value)) in fields.iter().enumerate() {
        if index == MAX_JSON_ITEMS {
            lines.push(format!("{indent}... +{} more fields", fields.len() - index));
            break;
        }

        match value {
            Value::Array(_) | Value::Object(_) => {
                lines.push(format!("{indent}{name}:"));
                lines.push(render_value(value, depth + 1));
            }
            _ => lines.push(format!("{indent}{name}: {}", render_value(value, depth))),
        }
    }

    lines.join("\n")
}

fn render_array(items: &[Value], depth: usize) -> String {
    let indent = "  ".repeat(depth);
    let mut lines = Vec::new();

    for item in items.iter().take(MAX_JSON_ITEMS) {
        let item = render_value(item, depth + 1).replace('\n', &format!("\n{indent}  "));
        lines.push(format!("{indent}- {item}"));
    }
    if items.len() > MAX_JSON_ITEMS {
        lines.push(format!("{indent}... +{} more", items.len() - MAX_JSON_ITEMS));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_render_json_status_error() {
        let input = r#"{"error":"setup_required","message":"No GitButler project found at .","hint":"run `but setup` to configure the project"}"#;

        assert_eq!(
            render_json(input),
            "error: setup_required\nmessage: No GitButler project found at .\nhint: run `but setup` to configure the project"
        );
    }

    #[test]
    fn test_render_json_status_compacts_workspace_metadata() {
        let input = r#"{
            "uncommittedChanges": [{"cliId":"uq","filePath":"note.txt","changeType":"added"}],
            "stacks": [],
            "mergeBase": {"commitId":"929705016a551ca2a71bd1310bb02b6b93d2d4c3","message":"init\n"},
            "upstreamState": {"behind": 0}
        }"#;

        assert_eq!(
            render_json(input),
            "zz [uncommitted] uq A note.txt\nbase: 9297050 init"
        );
    }

    #[test]
    fn test_render_json_status_compacts_stacks_without_losing_cli_ids() {
        let input = r#"{
            "uncommittedChanges": [],
            "stacks": [
                {"cliId":"g0","assignedChanges":[],"branches":[
                    {"cliId":"he","name":"feat/health","commits":[],"upstreamCommits":[],"branchStatus":"completelyUnpushed","reviewId":null}
                ]},
                {"cliId":"h0","assignedChanges":[],"branches":[
                    {"cliId":"qu","name":"feat/queues","commits":[{"cliId":"6c","commitId":"6c02d24ba050eb85afb8b9fe111c9387fc216579","message":"feat(gateway): isolate provider queues\n"}],"upstreamCommits":[],"branchStatus":"unpushedCommitsRequiringForce","reviewId":"(#158)"}
                ]}
            ],
            "mergeBase": {"commitId":"e676e664a435704b880612616a3bdf6f32b5b678","message":"feat(proxy): normalize roles\n\nCo-Authored-By: Codex\n"},
            "upstreamState": {"behind": 0}
        }"#;

        assert_eq!(
            render_json(input),
            "zz [uncommitted] (no changes)\ng0 [stack]\n  he [feat/health] (no commits) status=completelyUnpushed\nh0 [stack]\n  qu [feat/queues] #158 (1 local) status=unpushedCommitsRequiringForce\n    6c 6c02d24 feat(gateway): isolate provider queues\nbase: e676e66 feat(proxy): normalize roles"
        );
    }
}
