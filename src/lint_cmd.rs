use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::process::Command;

#[derive(Debug, Deserialize, Serialize)]
struct EslintMessage {
    #[serde(rename = "ruleId")]
    rule_id: Option<String>,
    severity: u8,
    message: String,
    line: usize,
    column: usize,
}

#[derive(Debug, Deserialize, Serialize)]
struct EslintResult {
    #[serde(rename = "filePath")]
    file_path: String,
    messages: Vec<EslintMessage>,
    #[serde(rename = "errorCount")]
    error_count: usize,
    #[serde(rename = "warningCount")]
    warning_count: usize,
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    // Detect if eslint or other linter (ignore paths containing / or .)
    let is_path_or_flag = args.is_empty()
        || args[0].starts_with('-')
        || args[0].contains('/')
        || args[0].contains('.');

    let linter = if is_path_or_flag {
        "eslint"
    } else {
        &args[0]
    };

    // Try linter directly first, then use package manager exec
    let linter_exists = Command::new("which")
        .arg(linter)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    // Detect package manager (pnpm/yarn have better CWD handling than npx)
    let is_pnpm = std::path::Path::new("pnpm-lock.yaml").exists();
    let is_yarn = std::path::Path::new("yarn.lock").exists();
    let uses_package_manager_exec = !linter_exists && (is_pnpm || is_yarn);

    let mut cmd = if linter_exists {
        Command::new(linter)
    } else if is_pnpm {
        // Use pnpm exec - preserves CWD correctly
        let mut c = Command::new("pnpm");
        c.arg("exec");
        c.arg("--");  // Separator to prevent pnpm from interpreting tool args
        c.arg(linter);
        c
    } else if is_yarn {
        // Use yarn exec - preserves CWD correctly
        let mut c = Command::new("yarn");
        c.arg("exec");
        c.arg("--");  // Separator
        c.arg(linter);
        c
    } else {
        // Fallback to npx
        let mut c = Command::new("npx");
        c.arg("--no-install");
        c.arg("--");  // Separator
        c.arg(linter);
        c
    };

    // Force JSON output for ESLint
    if linter == "eslint" {
        cmd.arg("-f").arg("json");
    }

    // Add user arguments (skip first if it was the linter name)
    let start_idx = if is_path_or_flag {
        0
    } else {
        1
    };

    // For pnpm/yarn exec, use relative paths (they preserve CWD)
    // For others, convert to absolute paths to avoid CWD issues
    for arg in &args[start_idx..] {
        if !uses_package_manager_exec && !arg.starts_with('-') {
            // Convert to absolute path for npx/global commands
            let path = std::path::Path::new(arg);
            if path.is_relative() {
                if let Ok(cwd) = std::env::current_dir() {
                    cmd.arg(cwd.join(path));
                    continue;
                }
            }
        }
        // Use argument as-is (for options or when using pnpm/yarn exec)
        cmd.arg(arg);
    }

    // Default to current directory if no path specified
    if args.iter().all(|a| a.starts_with('-')) {
        cmd.arg(".");
    }

    if verbose > 0 {
        eprintln!("Running: {} with JSON output", linter);
    }

    let output = cmd.output().context("Failed to run linter")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    // ESLint returns exit code 1 when lint errors found (expected behavior)
    let filtered = if linter == "eslint" {
        filter_eslint_json(&stdout)
    } else {
        filter_generic_lint(&raw)
    };

    println!("{}", filtered);

    tracking::track(
        &format!("{} {}", linter, args.join(" ")),
        &format!("rtk {} {}", linter, args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(())
}

/// Filter ESLint JSON output - group by rule and file
fn filter_eslint_json(output: &str) -> String {
    let results: Result<Vec<EslintResult>, _> = serde_json::from_str(output);

    let results = match results {
        Ok(r) => r,
        Err(e) => {
            // Fallback if JSON parsing fails
            return format!(
                "ESLint output (JSON parse failed: {})\n{}",
                e,
                truncate(output, 500)
            );
        }
    };

    // Count total issues
    let total_errors: usize = results.iter().map(|r| r.error_count).sum();
    let total_warnings: usize = results.iter().map(|r| r.warning_count).sum();
    let total_files = results.iter().filter(|r| !r.messages.is_empty()).count();

    if total_errors == 0 && total_warnings == 0 {
        return "✓ ESLint: No issues found".to_string();
    }

    // Group messages by rule
    let mut by_rule: HashMap<String, usize> = HashMap::new();
    for result in &results {
        for msg in &result.messages {
            if let Some(rule) = &msg.rule_id {
                *by_rule.entry(rule.clone()).or_insert(0) += 1;
            }
        }
    }

    // Group by file
    let mut by_file: Vec<(&EslintResult, usize)> = results
        .iter()
        .filter(|r| !r.messages.is_empty())
        .map(|r| (r, r.messages.len()))
        .collect();
    by_file.sort_by(|a, b| b.1.cmp(&a.1));

    // Build output
    let mut result = String::new();
    result.push_str(&format!(
        "ESLint: {} errors, {} warnings in {} files\n",
        total_errors, total_warnings, total_files
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Show top rules
    let mut rule_counts: Vec<_> = by_rule.iter().collect();
    rule_counts.sort_by(|a, b| b.1.cmp(a.1));

    if !rule_counts.is_empty() {
        result.push_str("Top rules:\n");
        for (rule, count) in rule_counts.iter().take(10) {
            result.push_str(&format!("  {} ({}x)\n", rule, count));
        }
        result.push('\n');
    }

    // Show top files with most issues
    result.push_str("Top files:\n");
    for (file_result, count) in by_file.iter().take(10) {
        let short_path = compact_path(&file_result.file_path);
        result.push_str(&format!("  {} ({} issues)\n", short_path, count));

        // Show top 3 rules in this file
        let mut file_rules: HashMap<String, usize> = HashMap::new();
        for msg in &file_result.messages {
            if let Some(rule) = &msg.rule_id {
                *file_rules.entry(rule.clone()).or_insert(0) += 1;
            }
        }

        let mut file_rule_counts: Vec<_> = file_rules.iter().collect();
        file_rule_counts.sort_by(|a, b| b.1.cmp(a.1));

        for (rule, count) in file_rule_counts.iter().take(3) {
            result.push_str(&format!("    {} ({})\n", rule, count));
        }
    }

    if by_file.len() > 10 {
        result.push_str(&format!("\n... +{} more files\n", by_file.len() - 10));
    }

    result.trim().to_string()
}

/// Filter generic linter output (fallback for non-ESLint linters)
fn filter_generic_lint(output: &str) -> String {
    let mut warnings = 0;
    let mut errors = 0;
    let mut issues: Vec<String> = Vec::new();

    for line in output.lines() {
        let line_lower = line.to_lowercase();
        if line_lower.contains("warning") {
            warnings += 1;
            issues.push(line.to_string());
        }
        if line_lower.contains("error") && !line_lower.contains("0 error") {
            errors += 1;
            issues.push(line.to_string());
        }
    }

    if errors == 0 && warnings == 0 {
        return "✓ Lint: No issues found".to_string();
    }

    let mut result = String::new();
    result.push_str(&format!("Lint: {} errors, {} warnings\n", errors, warnings));
    result.push_str("═══════════════════════════════════════\n");

    for issue in issues.iter().take(20) {
        result.push_str(&format!("{}\n", truncate(issue, 100)));
    }

    if issues.len() > 20 {
        result.push_str(&format!("\n... +{} more issues\n", issues.len() - 20));
    }

    result.trim().to_string()
}

/// Compact file path (remove common prefixes)
fn compact_path(path: &str) -> String {
    // Remove common prefixes like /Users/..., /home/..., C:\
    let path = path.replace('\\', "/");

    if let Some(pos) = path.rfind("/src/") {
        format!("src/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/lib/") {
        format!("lib/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_eslint_json() {
        let json = r#"[
            {
                "filePath": "/Users/test/project/src/utils.ts",
                "messages": [
                    {
                        "ruleId": "prefer-const",
                        "severity": 1,
                        "message": "Use const instead of let",
                        "line": 10,
                        "column": 5
                    },
                    {
                        "ruleId": "prefer-const",
                        "severity": 1,
                        "message": "Use const instead of let",
                        "line": 15,
                        "column": 5
                    }
                ],
                "errorCount": 0,
                "warningCount": 2
            },
            {
                "filePath": "/Users/test/project/src/api.ts",
                "messages": [
                    {
                        "ruleId": "@typescript-eslint/no-unused-vars",
                        "severity": 2,
                        "message": "Variable x is unused",
                        "line": 20,
                        "column": 10
                    }
                ],
                "errorCount": 1,
                "warningCount": 0
            }
        ]"#;

        let result = filter_eslint_json(json);
        assert!(result.contains("ESLint:"));
        assert!(result.contains("prefer-const"));
        assert!(result.contains("no-unused-vars"));
        assert!(result.contains("src/utils.ts"));
    }

    #[test]
    fn test_compact_path() {
        assert_eq!(
            compact_path("/Users/foo/project/src/utils.ts"),
            "src/utils.ts"
        );
        assert_eq!(
            compact_path("C:\\Users\\project\\src\\api.ts"),
            "src/api.ts"
        );
        assert_eq!(compact_path("simple.ts"), "simple.ts");
    }
}
