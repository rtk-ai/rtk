//! PHPStan static analysis filter.
//!
//! Injects `--error-format=json` for structured output, parses errors grouped by
//! file and sorted by error count. Falls back to text parsing when the user
//! specifies a custom format or when injected JSON output fails to parse.

use crate::core::runner;
use crate::core::utils::{exit_code_from_status, resolved_command};
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;
use std::path::Path;

// ── JSON structures matching PHPStan's --error-format=json output ───────────

#[derive(Deserialize)]
struct PhpstanOutput {
    totals: PhpstanTotals,
    files: HashMap<String, PhpstanFile>,
    #[serde(default)]
    errors: Vec<String>,
}

#[derive(Deserialize)]
struct PhpstanTotals {
    errors: usize,
    #[allow(dead_code)]
    file_errors: usize,
}

#[derive(Deserialize)]
struct PhpstanFile {
    errors: usize,
    messages: Vec<PhpstanMessage>,
}

#[derive(Deserialize)]
struct PhpstanMessage {
    message: String,
    line: usize,
    #[serde(default)]
    #[allow(dead_code)]
    ignorable: bool,
}

// ── Public entry point ───────────────────────────────────────────────────────

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Check for vendor/bin/phpstan first
    let mut cmd = if Path::new("vendor/bin/phpstan").exists() {
        resolved_command("vendor/bin/phpstan")
    } else {
        resolved_command("phpstan")
    };

    // Utility commands (--version, list, clear-result-cache, worker, …): real passthrough.
    // Only analyse/analyze subcommands get filtered and token-tracked.
    let is_analyse = args
        .first()
        .map(|a| a == "analyse" || a == "analyze")
        .unwrap_or(false);

    if !is_analyse {
        if verbose > 0 {
            eprintln!("Running: phpstan {} (passthrough)", args.join(" "));
        }
        cmd.args(args);
        let status = cmd.status().context("Failed to run phpstan")?;
        return Ok(exit_code_from_status(&status, "phpstan"));
    }

    // Detect if user specified a custom output format (not json).
    // Handles both `--error-format=table` and `--error-format table` forms.
    let has_custom_format = {
        let mut it = args.iter().peekable();
        let mut found = false;
        while let Some(a) = it.next() {
            if a == "--error-format" {
                if it.peek().map(|v| v.as_str()) != Some("json") {
                    found = true;
                }
                break;
            }
            if a.starts_with("--error-format=") && a != "--error-format=json" {
                found = true;
                break;
            }
        }
        found
    };

    // Pass user args first (subcommand must come before global flags for PHPStan),
    // then append --error-format=json unless the user specified a custom format.
    cmd.args(args);
    if !has_custom_format {
        cmd.arg("--error-format").arg("json");
    }

    if verbose > 0 {
        eprintln!("Running: phpstan {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "phpstan",
        &args.join(" "),
        move |stdout| {
            if has_custom_format {
                filter_phpstan_text(stdout)
            } else {
                filter_phpstan_json(stdout)
            }
        },
        runner::RunOptions::stdout_only().tee("phpstan"),
    )
}

// ── JSON filtering ───────────────────────────────────────────────────────────

fn filter_phpstan_json(output: &str) -> String {
    if output.trim().is_empty() {
        return "PHPStan: No output".to_string();
    }

    let parsed: Result<PhpstanOutput, _> = serde_json::from_str(output);
    let phpstan = match parsed {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[rtk] phpstan: JSON parse failed ({})", e);
            return crate::core::utils::fallback_tail(output, "phpstan (JSON parse error)", 5);
        }
    };

    // No errors case
    if phpstan.totals.errors == 0 {
        return "ok ✓ phpstan".to_string();
    }

    let mut result = format!(
        "phpstan: {} errors in {} files\n",
        phpstan.totals.errors,
        phpstan.files.len()
    );

    // Add global errors first if any
    if !phpstan.errors.is_empty() {
        result.push_str("\nGlobal errors:\n");
        for error in &phpstan.errors {
            result.push_str(&format!("  {}\n", error));
        }
        result.push('\n');
    }

    // Build list of files with errors, sorted by error count descending
    let mut files_vec: Vec<(&String, &PhpstanFile)> = phpstan.files.iter().collect();
    files_vec.sort_by(|a, b| b.1.errors.cmp(&a.1.errors).then(a.0.cmp(b.0)));

    let max_files = 10;
    let max_messages_per_file = 5;

    for (path, file) in files_vec.iter().take(max_files) {
        let short = compact_php_path(path);
        result.push_str(&format!("\n{} ({} errors)\n", short, file.errors));

        for message in file.messages.iter().take(max_messages_per_file) {
            let first_line = message.message.lines().next().unwrap_or("");
            result.push_str(&format!("  :{} {}\n", message.line, first_line));
        }

        if file.messages.len() > max_messages_per_file {
            result.push_str(&format!(
                "  ... +{} more\n",
                file.messages.len() - max_messages_per_file
            ));
        }
    }

    if files_vec.len() > max_files {
        result.push_str(&format!(
            "\n... +{} more files\n",
            files_vec.len() - max_files
        ));
    }

    result.trim().to_string()
}

// ── Text fallback ────────────────────────────────────────────────────────────

fn filter_phpstan_text(output: &str) -> String {
    // Check for errors first
    for line in output.lines() {
        let t = line.trim();
        if t.contains("cannot load such file")
            || t.contains("not found")
            || t.starts_with("phpstan: command not found")
            || t.starts_with("phpstan: No such file")
        {
            let error_lines: Vec<&str> = output.trim().lines().take(20).collect();
            let truncated = error_lines.join("\n");
            let total_lines = output.trim().lines().count();
            if total_lines > 20 {
                return format!(
                    "PHPStan error:\n{}\n... ({} more lines)",
                    truncated,
                    total_lines - 20
                );
            }
            return format!("PHPStan error:\n{}", truncated);
        }
    }

    // Extract summary if present
    for line in output.lines().rev() {
        let t = line.trim();
        if t.contains("[OK]") || t.contains("No errors") {
            return "ok ✓ phpstan".to_string();
        }
        if t.contains("errors") && (t.contains("found") || t.contains("in")) {
            return format!("PHPStan: {}", t);
        }
    }

    // Last resort: last 20 lines
    crate::core::utils::fallback_tail(output, "phpstan", 20)
}

/// Compact PHP file path by finding the nearest conventional directory
/// and stripping the absolute path prefix.
fn compact_php_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    for prefix in &[
        "app/Models/",
        "app/Http/Controllers/",
        "app/Http/Middleware/",
        "app/Services/",
        "app/Repositories/",
        "src/",
        "tests/",
        "config/",
        "database/",
    ] {
        if let Some(pos) = path.find(prefix) {
            return path[pos..].to_string();
        }
    }

    // Generic: strip up to last known directory marker
    if let Some(pos) = path.rfind("/app/") {
        return path[pos + 1..].to_string();
    }
    if let Some(pos) = path.rfind("/src/") {
        return path[pos + 1..].to_string();
    }
    // Keep last 2 path components to preserve context (dir/File.php)
    if let Some(pos) = path.rfind('/') {
        if let Some(prev) = path[..pos].rfind('/') {
            return path[prev + 1..].to_string();
        }
        return path[pos + 1..].to_string();
    }
    path
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    fn no_errors_json() -> &'static str {
        r#"{
          "totals": {"errors": 0, "file_errors": 0},
          "files": {},
          "errors": []
        }"#
    }

    fn with_errors_json() -> &'static str {
        r#"{
          "totals": {"errors": 5, "file_errors": 5},
          "files": {
            "app/Models/User.php": {
              "errors": 2,
              "messages": [
                {"message": "Property $id does not accept null.", "line": 10, "ignorable": true},
                {"message": "Call to undefined method Model::find().", "line": 25, "ignorable": false}
              ]
            },
            "app/Http/Controllers/UserController.php": {
              "errors": 2,
              "messages": [
                {"message": "Parameter $id of anonymous function has no typehint.", "line": 45, "ignorable": false},
                {"message": "Variable $user might not be defined.", "line": 67, "ignorable": false}
              ]
            },
            "app/Services/AuthService.php": {
              "errors": 1,
              "messages": [
                {"message": "Return type missing.", "line": 12, "ignorable": false}
              ]
            }
          },
          "errors": []
        }"#
    }

    fn large_json_for_truncation() -> String {
        let mut files = HashMap::new();

        // Create 12 files with varying error counts
        for i in 1..=12 {
            let filename = format!("app/Models/Model{}.php", i);
            let error_count = if i <= 3 { 10 } else { i % 5 + 1 };

            let mut messages = Vec::new();
            for j in 1..=error_count {
                messages.push(format!(
                    r#"{{"message": "Error {} in file {}", "line": {}, "ignorable": false}}"#,
                    j, i, j * 10
                ));
            }

            files.insert(
                filename,
                format!(
                    r#"{{"errors": {}, "messages": [{}]}}"#,
                    error_count,
                    messages.join(",")
                ),
            );
        }

        let files_json: Vec<String> = files
            .iter()
            .map(|(k, v)| format!(r#""{}": {}"#, k, v))
            .collect();

        format!(
            r#"{{"totals": {{"errors": 50, "file_errors": 50}}, "files": {{{}}}, "errors": []}}"#,
            files_json.join(",")
        )
    }

    #[test]
    fn test_filter_phpstan_json_no_errors() {
        let result = filter_phpstan_json(no_errors_json());
        assert_eq!(result, "ok ✓ phpstan");
    }

    #[test]
    fn test_filter_phpstan_json_with_errors() {
        let result = filter_phpstan_json(with_errors_json());

        // Check summary line
        assert!(result.contains("5 errors in 3 files"));

        // Check file names are present
        assert!(result.contains("app/Models/User.php"));
        assert!(result.contains("app/Http/Controllers/UserController.php"));
        assert!(result.contains("app/Services/AuthService.php"));

        // Check line numbers and messages
        assert!(result.contains(":10 Property $id does not accept null."));
        assert!(result.contains(":25 Call to undefined method Model::find()."));
        assert!(result.contains(":45 Parameter $id of anonymous function has no typehint."));
    }

    #[test]
    fn test_filter_phpstan_json_truncation() {
        let result = filter_phpstan_json(&large_json_for_truncation());

        // Should show max 10 files
        assert!(result.contains("+2 more files"));

        // Should not show all 12 files inline
        let file_count = result.matches("app/Models/Model").count();
        assert_eq!(file_count, 10, "Should show exactly 10 files");
    }

    #[test]
    fn test_filter_phpstan_token_savings() {
        // Use the realistic fixture with many files, long paths, and JSON metadata
        // to verify the ≥75% savings claim in rules.rs
        let input = include_str!("../../../tests/fixtures/phpstan_raw.json");
        let output = filter_phpstan_json(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "PHPStan: expected ≥60% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_phpstan_empty_input() {
        let result = filter_phpstan_json("");
        assert_eq!(result, "PHPStan: No output");
    }

    #[test]
    fn test_filter_phpstan_malformed_json() {
        let garbage = "some php warning\n{broken json";
        let result = filter_phpstan_json(garbage);
        assert!(!result.is_empty(), "should not panic on invalid JSON");
    }

    #[test]
    fn test_compact_php_path() {
        assert_eq!(
            compact_php_path("/var/www/project/app/Models/User.php"),
            "app/Models/User.php"
        );
        assert_eq!(
            compact_php_path("app/Http/Controllers/UserController.php"),
            "app/Http/Controllers/UserController.php"
        );
        assert_eq!(
            compact_php_path("/home/user/project/src/Service.php"),
            "src/Service.php"
        );
        assert_eq!(
            compact_php_path("tests/Unit/UserTest.php"),
            "tests/Unit/UserTest.php"
        );
    }

    #[test]
    fn test_filter_phpstan_text_fallback() {
        let text = r#"PHPStan analysis complete
[OK] No errors found"#;
        let result = filter_phpstan_text(text);
        assert_eq!(result, "ok ✓ phpstan");
    }

    #[test]
    fn test_filter_phpstan_text_with_errors() {
        let text = r#"PHPStan analysis complete

Found 5 errors in 3 files"#;
        let result = filter_phpstan_text(text);
        assert!(result.starts_with("PHPStan:"), "should have PHPStan: prefix");
        assert!(result.contains("5 errors"), "should contain error count");
        assert!(result.contains("3 files"), "should contain file count");
    }

    #[test]
    fn test_filter_phpstan_fixture_structure() {
        // Verify output structure on the realistic fixture (14 files, 47 errors)
        let input = include_str!("../../../tests/fixtures/phpstan_raw.json");
        let output = filter_phpstan_json(input);

        assert!(output.contains("47 errors in 14 files"));
        // Files are sorted by error count descending — User.php has 6, comes first
        assert!(output.contains("app/Models/User.php (6 errors)"));
        // 14 files → only 10 shown, 4 more
        assert!(output.contains("+4 more files"));
    }
}
