//! PHPStan output filter.

use super::utils::{php_tool_command, strip_ansi_and_controls};
use crate::core::runner;
use crate::core::utils::fallback_tail;
use anyhow::Result;
use serde_json::Value;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = php_tool_command("phpstan");

    let has_error_format = args
        .iter()
        .any(|a| a == "--error-format" || a.starts_with("--error-format="));
    let has_no_progress = args.iter().any(|a| a == "--no-progress");

    if !has_error_format {
        cmd.arg("--error-format=json");
    }
    if !has_no_progress {
        cmd.arg("--no-progress");
    }
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: phpstan {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "phpstan",
        &args.join(" "),
        move |stdout| {
            if has_error_format {
                filter_phpstan_text(stdout)
            } else {
                parse_phpstan_json(stdout).unwrap_or_else(|| fallback_tail(stdout, "phpstan", 60))
            }
        },
        runner::RunOptions::stdout_only().tee("phpstan"),
    )
}

fn parse_phpstan_json(output: &str) -> Option<String> {
    let json: Value = serde_json::from_str(output).ok()?;

    let mut issues = Vec::new();
    let mut general_errors = Vec::new();

    if let Some(errors) = json.get("errors").and_then(Value::as_array) {
        for err in errors {
            if let Some(text) = err.as_str() {
                let t = text.trim();
                if !t.is_empty() {
                    general_errors.push(t.to_string());
                }
            }
        }
    }

    if let Some(files) = json.get("files").and_then(Value::as_object) {
        let mut file_names: Vec<&String> = files.keys().collect();
        file_names.sort_unstable();

        for file in file_names {
            let Some(file_data) = files.get(file) else {
                continue;
            };
            let Some(messages) = file_data.get("messages").and_then(Value::as_array) else {
                continue;
            };

            for msg in messages {
                let line = msg.get("line").and_then(Value::as_i64).unwrap_or(0);
                let message = msg
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .trim();
                let identifier = msg
                    .get("identifier")
                    .and_then(Value::as_str)
                    .map(str::trim)
                    .filter(|s| !s.is_empty());

                if message.is_empty() {
                    continue;
                }

                if let Some(id) = identifier {
                    issues.push(format!("{}:{} {} [{}]", file, line, message, id));
                } else {
                    issues.push(format!("{}:{} {}", file, line, message));
                }
            }
        }
    }

    let total_errors = issues.len() + general_errors.len();
    if total_errors == 0 {
        return Some("✓ phpstan: 0 errors".to_string());
    }

    let mut out = vec![format!("phpstan: {} errors", total_errors)];
    for err in general_errors.iter().take(20) {
        out.push(format!("- {}", err));
    }
    if general_errors.len() > 20 {
        out.push(format!(
            "... +{} more general errors",
            general_errors.len() - 20
        ));
    }

    for issue in issues.iter().take(80) {
        out.push(issue.clone());
    }
    if issues.len() > 80 {
        out.push(format!("... +{} more issues", issues.len() - 80));
    }

    Some(out.join("\n"))
}

fn filter_phpstan_text(output: &str) -> String {
    let cleaned = strip_ansi_and_controls(output);
    let mut lines = Vec::new();

    for line in cleaned.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.contains('%') && trimmed.contains('|') {
            continue;
        }

        lines.push(trimmed.to_string());
    }

    if lines.is_empty() {
        "ok".to_string()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_phpstan_json_passed() {
        let json = r#"{"totals":{"errors":0,"file_errors":0},"files":{},"errors":[]}"#;
        assert_eq!(
            parse_phpstan_json(json),
            Some("✓ phpstan: 0 errors".to_string())
        );
    }

    #[test]
    fn test_phpstan_json_failed() {
        let json = r#"{
            "totals": {"errors": 1, "file_errors": 1},
            "files": {
              "src/Foo.php": {
                "errors": 1,
                "messages": [
                  {"message":"Undefined variable: $foo","line":12,"identifier":"variable.undefined"}
                ]
              }
            },
            "errors": []
          }"#;

        let parsed = parse_phpstan_json(json).expect("should parse json");
        assert!(parsed.contains("phpstan: 1 errors"));
        assert!(parsed.contains("src/Foo.php:12 Undefined variable: $foo [variable.undefined]"));
    }
}
