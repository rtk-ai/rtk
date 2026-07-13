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

    render_value(&value, 0)
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
}
