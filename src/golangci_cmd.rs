use crate::tracking;
use crate::utils::{resolved_command, truncate};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

#[derive(Debug, Deserialize)]
struct Position {
    #[serde(rename = "Filename")]
    filename: String,
    #[serde(rename = "Line")]
    line: usize,
    #[serde(rename = "Column")]
    column: usize,
}

#[derive(Debug, Deserialize)]
struct Issue {
    #[serde(rename = "FromLinter")]
    from_linter: String,
    #[serde(rename = "Text")]
    text: String,
    #[serde(rename = "Pos")]
    pos: Position,
}

#[derive(Debug, Deserialize)]
struct GolangciOutput {
    #[serde(rename = "Issues")]
    issues: Vec<Issue>,
}

/// Parse the major version number from `golangci-lint version` output text.
/// Expected format: "golangci-lint has version 2.11.3 built with ..."
/// or: "golangci-lint has version v1.64.8 built with ..."
/// Returns 1 as fallback when parsing fails.
fn parse_major_version(text: &str) -> u8 {
    let words: Vec<&str> = text.split_whitespace().collect();

    words
        .windows(2)
        .find(|pair| pair[0] == "version")
        .and_then(|pair| pair[1].trim_start_matches('v').split('.').next())
        .and_then(|major| major.parse::<u8>().ok())
        .filter(|&v| v >= 1)
        .unwrap_or(1)
}

/// Detect the installed golangci-lint major version by running `golangci-lint version`.
/// Returns 2 for v2+, 1 for v1 or if detection fails.
fn detect_major_version() -> u8 {
    let output = resolved_command("golangci-lint").arg("version").output();

    match output {
        Ok(o) => {
            let text = String::from_utf8_lossy(&o.stdout);
            parse_major_version(&text)
        }
        Err(_) => 1,
    }
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("golangci-lint");

    let major = detect_major_version();

    // Check if the user already supplied an output format flag
    let has_format = args.iter().any(|a| {
        a == "--out-format"
            || a.starts_with("--out-format=")
            || a.starts_with("--output.json.")
            || a.starts_with("--output.text.")
            || a.starts_with("--output.tab.")
    });

    if !has_format {
        cmd.arg("run");
        if major >= 2 {
            cmd.arg("--output.json.path=stdout");
        } else {
            cmd.arg("--out-format=json");
        }
    } else {
        cmd.arg("run");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        let flag = if major >= 2 {
            "--output.json.path=stdout"
        } else {
            "--out-format=json"
        };
        eprintln!("Running: golangci-lint run {}", flag);
    }

    let output = cmd.output().context(
        "Failed to run golangci-lint. Is it installed? Try: go install github.com/golangci/golangci-lint/cmd/golangci-lint@latest",
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_golangci_json(&stdout);

    println!("{}", filtered);

    // Include stderr if present (config errors, etc.)
    if !stderr.trim().is_empty() && verbose > 0 {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("golangci-lint {}", args.join(" ")),
        &format!("rtk golangci-lint {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // golangci-lint returns exit code 1 when issues found (expected behavior)
    // Don't exit with error code in that case
    Ok(())
}

/// Filter golangci-lint JSON output - group by linter and file
fn filter_golangci_json(output: &str) -> String {
    let result: Result<GolangciOutput, _> = serde_json::from_str(output);

    let golangci_output = match result {
        Ok(o) => o,
        Err(e) => {
            // Fallback if JSON parsing fails
            return format!(
                "golangci-lint (JSON parse failed: {})\n{}",
                e,
                truncate(output, 500)
            );
        }
    };

    let issues = golangci_output.issues;

    if issues.is_empty() {
        return "✓ golangci-lint: No issues found".to_string();
    }

    let total_issues = issues.len();

    // Count unique files
    let unique_files: std::collections::HashSet<_> =
        issues.iter().map(|i| &i.pos.filename).collect();
    let total_files = unique_files.len();

    // Group by linter
    let mut by_linter: HashMap<String, usize> = HashMap::new();
    for issue in &issues {
        *by_linter.entry(issue.from_linter.clone()).or_insert(0) += 1;
    }

    // Group by file
    let mut by_file: HashMap<&str, usize> = HashMap::new();
    for issue in &issues {
        *by_file.entry(&issue.pos.filename).or_insert(0) += 1;
    }

    let mut file_counts: Vec<_> = by_file.iter().collect();
    file_counts.sort_by(|a, b| b.1.cmp(a.1));

    // Build output
    let mut result = String::new();
    result.push_str(&format!(
        "golangci-lint: {} issues in {} files\n",
        total_issues, total_files
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Show top linters
    let mut linter_counts: Vec<_> = by_linter.iter().collect();
    linter_counts.sort_by(|a, b| b.1.cmp(a.1));

    if !linter_counts.is_empty() {
        result.push_str("Top linters:\n");
        for (linter, count) in linter_counts.iter().take(10) {
            result.push_str(&format!("  {} ({}x)\n", linter, count));
        }
        result.push('\n');
    }

    // Show top files
    result.push_str("Top files:\n");
    for (file, count) in file_counts.iter().take(10) {
        let short_path = compact_path(file);
        result.push_str(&format!("  {} ({} issues)\n", short_path, count));

        // Show top 3 linters in this file
        let mut file_linters: HashMap<String, usize> = HashMap::new();
        for issue in issues.iter().filter(|i| &i.pos.filename == *file) {
            *file_linters.entry(issue.from_linter.clone()).or_insert(0) += 1;
        }

        let mut file_linter_counts: Vec<_> = file_linters.iter().collect();
        file_linter_counts.sort_by(|a, b| b.1.cmp(a.1));

        for (linter, count) in file_linter_counts.iter().take(3) {
            result.push_str(&format!("    {} ({})\n", linter, count));
        }
    }

    if file_counts.len() > 10 {
        result.push_str(&format!("\n... +{} more files\n", file_counts.len() - 10));
    }

    result.trim().to_string()
}

/// Compact file path (remove common prefixes)
fn compact_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    if let Some(pos) = path.rfind("/pkg/") {
        format!("pkg/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/cmd/") {
        format!("cmd/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/internal/") {
        format!("internal/{}", &path[pos + 10..])
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
    fn test_filter_golangci_no_issues() {
        let output = r#"{"Issues":[]}"#;
        let result = filter_golangci_json(output);
        assert!(result.contains("✓ golangci-lint"));
        assert!(result.contains("No issues found"));
    }

    #[test]
    fn test_filter_golangci_with_issues() {
        let output = r#"{
  "Issues": [
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 42, "Column": 5}
    },
    {
      "FromLinter": "errcheck",
      "Text": "Error return value not checked",
      "Pos": {"Filename": "main.go", "Line": 50, "Column": 10}
    },
    {
      "FromLinter": "gosimple",
      "Text": "Should use strings.Contains",
      "Pos": {"Filename": "utils.go", "Line": 15, "Column": 2}
    }
  ]
}"#;

        let result = filter_golangci_json(output);
        assert!(result.contains("3 issues"));
        assert!(result.contains("2 files"));
        assert!(result.contains("errcheck"));
        assert!(result.contains("gosimple"));
        assert!(result.contains("main.go"));
        assert!(result.contains("utils.go"));
    }

    #[test]
    fn test_compact_path() {
        assert_eq!(
            compact_path("/Users/foo/project/pkg/handler/server.go"),
            "pkg/handler/server.go"
        );
        assert_eq!(
            compact_path("/home/user/app/cmd/main/main.go"),
            "cmd/main/main.go"
        );
        assert_eq!(
            compact_path("/project/internal/config/loader.go"),
            "internal/config/loader.go"
        );
        assert_eq!(compact_path("relative/file.go"), "file.go");
    }

    #[test]
    fn test_parse_major_version_v2() {
        let text = "golangci-lint has version 2.11.3 built with go1.26.1 from abc123 on 2026-03-10T10:25:44Z";
        assert_eq!(parse_major_version(text), 2);
    }

    #[test]
    fn test_parse_major_version_v1() {
        let text = "golangci-lint has version v1.64.8 built with go1.23.0 from def456 on 2025-01-15T08:30:00Z";
        assert_eq!(parse_major_version(text), 1);
    }

    #[test]
    fn test_parse_major_version_empty() {
        assert_eq!(parse_major_version(""), 1);
    }

    #[test]
    fn test_parse_major_version_garbage() {
        assert_eq!(parse_major_version("not a version string at all"), 1);
    }

    #[test]
    fn test_has_format_detection_v1_flag() {
        let args: Vec<String> = vec!["--out-format=json".into(), "./...".into()];
        let has_format = args.iter().any(|a| {
            a == "--out-format"
                || a.starts_with("--out-format=")
                || a.starts_with("--output.json.")
                || a.starts_with("--output.text.")
                || a.starts_with("--output.tab.")
        });
        assert!(has_format);
    }

    #[test]
    fn test_has_format_detection_v2_flag() {
        let args: Vec<String> = vec!["--output.json.path=stdout".into(), "./...".into()];
        let has_format = args.iter().any(|a| {
            a == "--out-format"
                || a.starts_with("--out-format=")
                || a.starts_with("--output.json.")
                || a.starts_with("--output.text.")
                || a.starts_with("--output.tab.")
        });
        assert!(has_format);
    }

    #[test]
    fn test_has_format_detection_no_flag() {
        let args: Vec<String> = vec!["./...".into()];
        let has_format = args.iter().any(|a| {
            a == "--out-format"
                || a.starts_with("--out-format=")
                || a.starts_with("--output.json.")
                || a.starts_with("--output.text.")
                || a.starts_with("--output.tab.")
        });
        assert!(!has_format);
    }
}
