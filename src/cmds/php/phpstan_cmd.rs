//! PHPStan static analysis filter.
//!
//! Injects `--error-format=json` for structured output, parses errors grouped by
//! file and sorted by error count. Falls back to text parsing when the user
//! specifies a custom format or when injected JSON output fails to parse.

use super::utils::php_tool_command;
use crate::core::runner;
use crate::core::utils::exit_code_from_status;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::collections::HashMap;

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
    // `errors` counts only non-file-specific (global/config) errors; per-file
    // errors live in `file_errors`. Gating "ok" on `errors` alone hides real
    // failures, since a normal failing run reports errors=0, file_errors=N.
    errors: usize,
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
    // Composer-aware resolution (COMPOSER_BIN_DIR / config.bin-dir), matching
    // the other php tools, with PATH fallback for a global install.
    let mut cmd = php_tool_command("phpstan");

    // Utility commands (--version, list, clear-result-cache, worker, …): real passthrough.
    // Only analyse/analyze subcommands get filtered and token-tracked.
    let is_analyse = is_analyse_command(args);

    if !is_analyse {
        if verbose > 0 {
            eprintln!("Running: phpstan {} (passthrough)", args.join(" "));
        }
        cmd.args(args);
        let status = cmd.status().context("Failed to run phpstan")?;
        return Ok(exit_code_from_status(&status, "phpstan"));
    }

    // Detect whether the user already chose an output format. When they passed
    // `--error-format=json` we must NOT append a second one; only inject json
    // when no format is present.
    let fmt = detect_error_format(args);

    cmd.args(args);
    if matches!(fmt, ErrorFormat::Unspecified) {
        cmd.arg("--error-format").arg("json");
    }

    if verbose > 0 {
        eprintln!("Running: phpstan {}", args.join(" "));
    }

    let use_text = matches!(fmt, ErrorFormat::Custom);
    runner::run_filtered(
        cmd,
        "phpstan",
        &args.join(" "),
        move |stdout| {
            if use_text {
                filter_phpstan_text(stdout)
            } else {
                filter_phpstan_json(stdout)
            }
        },
        runner::RunOptions::stdout_only().tee("phpstan"),
    )
}

/// True when the invocation runs the `analyse`/`analyze` subcommand. The
/// subcommand may appear after global options (`phpstan -c x.neon analyse src/`),
/// so scan every arg rather than just the first.
fn is_analyse_command(args: &[String]) -> bool {
    args.iter().any(|a| a == "analyse" || a == "analyze")
}

/// Which `--error-format` the user requested, if any.
enum ErrorFormat {
    /// No format flag — rtk injects `--error-format=json`.
    Unspecified,
    /// User explicitly asked for json — keep it, don't duplicate the flag.
    Json,
    /// User asked for a non-json format — leave args alone, use the text filter.
    Custom,
}

/// Parse `--error-format=<v>` / `--error-format <v>` from the args.
fn detect_error_format(args: &[String]) -> ErrorFormat {
    let mut it = args.iter().peekable();
    while let Some(a) = it.next() {
        if a == "--error-format" {
            return match it.peek().map(|v| v.as_str()) {
                Some("json") => ErrorFormat::Json,
                _ => ErrorFormat::Custom,
            };
        }
        if let Some(val) = a.strip_prefix("--error-format=") {
            return if val == "json" {
                ErrorFormat::Json
            } else {
                ErrorFormat::Custom
            };
        }
    }
    ErrorFormat::Unspecified
}

// ── JSON filtering ───────────────────────────────────────────────────────────

pub(crate) fn filter_phpstan_json(output: &str) -> String {
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

    // No errors case: both file-specific and global error counts must be zero.
    if phpstan.totals.file_errors == 0 && phpstan.totals.errors == 0 {
        return "phpstan: ok".to_string();
    }

    let mut result = format!(
        "phpstan: {} errors in {} files\n",
        phpstan.totals.file_errors,
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

pub(crate) fn filter_phpstan_text(output: &str) -> String {
    // The text path matches on substrings ("[OK]", "Found N errors"); strip
    // ANSI first so `--ansi` colorized output still matches.
    let cleaned = super::utils::strip_ansi_and_controls(output);
    let output = cleaned.as_str();

    // Check for errors first
    for line in output.lines() {
        let t = line.trim();
        // Shell-level "binary missing" lines only. A substring match on
        // "not found" would swallow real analysis output ("Class X not found").
        if t.starts_with("phpstan: command not found")
            || t.starts_with("phpstan: No such file")
            || t.starts_with("sh: ")
            || t.starts_with("bash: ")
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

    // Extract summary if present. Match case-insensitively: phpstan prints
    // "[ERROR] Found N errors" / "[OK] No errors" with varying capitalization.
    for line in output.lines().rev() {
        let t = line.trim();
        let lower = t.to_lowercase();
        if lower.contains("[ok]") || lower.contains("no errors") {
            return "phpstan: ok".to_string();
        }
        if lower.contains("error") && (lower.contains("found") || lower.contains("in")) {
            return format!("PHPStan: {}", t);
        }
    }

    // Last resort: last 20 lines
    crate::core::utils::fallback_tail(output, "phpstan", 20)
}

/// Compact a PHP file path to its last two components (`dir/File.php`),
/// which is enough context across frameworks without a per-convention list.
fn compact_php_path(path: &str) -> String {
    let path = path.replace('\\', "/");

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
        assert_eq!(result, "phpstan: ok");
    }

    #[test]
    fn test_filter_phpstan_file_errors_not_hidden() {
        // Real failing runs report errors=0 (no global errors) with the count
        // in file_errors. Gating "ok" on `errors` alone silently hid failures.
        let json = r#"{
          "totals": {"errors": 0, "file_errors": 2},
          "files": {
            "app/Models/User.php": {
              "errors": 2,
              "messages": [
                {"message": "Property $id does not accept null.", "line": 10, "ignorable": true},
                {"message": "Call to undefined method.", "line": 25, "ignorable": false}
              ]
            }
          },
          "errors": []
        }"#;
        let result = filter_phpstan_json(json);
        assert!(result.starts_with("phpstan: 2 errors in 1 files"), "got: {}", result);
        assert!(result.contains("Models/User.php (2 errors)"), "got: {}", result);
    }

    #[test]
    fn test_filter_phpstan_json_with_errors() {
        let result = filter_phpstan_json(with_errors_json());

        // Check summary line
        assert!(result.contains("5 errors in 3 files"));

        // Check file names are present (compacted to last two components)
        assert!(result.contains("Models/User.php"));
        assert!(result.contains("Controllers/UserController.php"));
        assert!(result.contains("Services/AuthService.php"));

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

        // Should not show all 12 files inline (paths compacted to last 2 components)
        let file_count = result.matches("Models/Model").count();
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
            "Models/User.php"
        );
        assert_eq!(
            compact_php_path("app/Http/Controllers/UserController.php"),
            "Controllers/UserController.php"
        );
        assert_eq!(
            compact_php_path("/home/user/project/src/Service.php"),
            "src/Service.php"
        );
        assert_eq!(
            compact_php_path("tests/Unit/UserTest.php"),
            "Unit/UserTest.php"
        );
    }

    #[test]
    fn test_filter_phpstan_text_fallback() {
        let text = r#"PHPStan analysis complete
[OK] No errors found"#;
        let result = filter_phpstan_text(text);
        assert_eq!(result, "phpstan: ok");
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
    fn test_filter_phpstan_text_not_found_in_analysis_not_swallowed() {
        // "not found" appears in real analysis messages. The text fallback must
        // report the summary, not mistake it for a missing-binary shell error.
        let text = r#"Note: Using configuration file phpstan.neon.

 ------ -----------------------------------------------
  Line   app/Foo.php
 ------ -----------------------------------------------
  10     Class 'NotFoundHttpException' not found.
  25     Method 'findById' not found in class 'UserRepository'.
 ------ -----------------------------------------------

 [ERROR] Found 2 errors
"#;
        let result = filter_phpstan_text(text);
        assert_eq!(result, "PHPStan: [ERROR] Found 2 errors");
    }

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn test_is_analyse_detects_subcommand_after_global_flag() {
        // Regression: `phpstan -c phpstan.neon analyse src/` must be filtered.
        assert!(is_analyse_command(&args(&[
            "-c",
            "phpstan.neon",
            "analyse",
            "src/"
        ])));
        assert!(is_analyse_command(&args(&["analyze", "src/"])));
        assert!(!is_analyse_command(&args(&["--version"])));
        assert!(!is_analyse_command(&args(&["clear-result-cache"])));
    }

    #[test]
    fn test_detect_error_format() {
        assert!(matches!(
            detect_error_format(&args(&["analyse", "src/"])),
            ErrorFormat::Unspecified
        ));
        assert!(matches!(
            detect_error_format(&args(&["analyse", "--error-format", "json"])),
            ErrorFormat::Json
        ));
        assert!(matches!(
            detect_error_format(&args(&["analyse", "--error-format=json"])),
            ErrorFormat::Json
        ));
        assert!(matches!(
            detect_error_format(&args(&["analyse", "--error-format", "table"])),
            ErrorFormat::Custom
        ));
        assert!(matches!(
            detect_error_format(&args(&["analyse", "--error-format=table"])),
            ErrorFormat::Custom
        ));
    }

    #[test]
    fn test_filter_phpstan_text_strips_ansi() {
        let text = "\x1b[32m [OK] No errors\x1b[0m";
        assert_eq!(filter_phpstan_text(text), "phpstan: ok");
    }

    #[test]
    fn test_filter_phpstan_text_error_summary_case_insensitive() {
        // phpstan prints "[ERROR] Found N errors" — capital F and no " in ",
        // so the summary must be matched case-insensitively (regression guard).
        let text = " ------\n  42   problem\n ------\n\n [ERROR] Found 2 errors\n";
        let result = filter_phpstan_text(text);
        assert_eq!(result, "PHPStan: [ERROR] Found 2 errors");
    }

    #[test]
    fn test_filter_phpstan_fixture_structure() {
        // Verify output structure on the realistic fixture (14 files, 47 errors)
        let input = include_str!("../../../tests/fixtures/phpstan_raw.json");
        let output = filter_phpstan_json(input);

        assert!(output.contains("47 errors in 14 files"));
        // Files are sorted by error count descending — User.php has 6, comes first
        assert!(output.contains("Models/User.php (6 errors)"));
        // 14 files → only 10 shown, 4 more
        assert!(output.contains("+4 more files"));
    }
}
