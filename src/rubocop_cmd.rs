//! RuboCop linter filter.
//!
//! Injects `--format json` for structured output, parses offenses grouped by
//! file and sorted by severity. Falls back to text parsing for autocorrect mode,
//! when the user specifies a custom format, or when injected JSON output fails
//! to parse.

use crate::tracking;
use crate::utils::{exit_code_from_output, ruby_exec};
use anyhow::{Context, Result};
use serde::Deserialize;

// ── JSON structures matching RuboCop's --format json output ─────────────────

#[derive(Deserialize)]
struct RubocopOutput {
    files: Vec<RubocopFile>,
    summary: RubocopSummary,
}

#[derive(Deserialize)]
struct RubocopFile {
    path: String,
    offenses: Vec<RubocopOffense>,
}

#[derive(Deserialize)]
struct RubocopOffense {
    cop_name: String,
    severity: String,
    message: String,
    correctable: bool,
    location: RubocopLocation,
}

#[derive(Deserialize)]
struct RubocopLocation {
    start_line: usize,
}

#[derive(Deserialize)]
struct RubocopSummary {
    offense_count: usize,
    #[allow(dead_code)]
    target_file_count: usize,
    inspected_file_count: usize,
    #[serde(default)]
    correctable_offense_count: usize,
}

// ── Public entry point ───────────────────────────────────────────────────────

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = ruby_exec("rubocop");

    // Detect autocorrect mode
    let is_autocorrect = args
        .iter()
        .any(|a| a == "-a" || a == "-A" || a == "--auto-correct" || a == "--auto-correct-all");

    // Inject --format json unless the user already specified a format
    let has_format = args
        .iter()
        .any(|a| a.starts_with("--format") || a.starts_with("-f"));

    if !has_format && !is_autocorrect {
        cmd.arg("--format").arg("json");
    }

    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: rubocop {}", args.join(" "));
    }

    let output = cmd.output().context(
        "Failed to run rubocop. Is it installed? Try: gem install rubocop or add it to your Gemfile",
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, "rubocop");

    let filtered = if stdout.trim().is_empty() && !output.status.success() {
        "RuboCop: FAILED (no stdout, see stderr below)".to_string()
    } else if has_format || is_autocorrect {
        filter_rubocop_text(&stdout)
    } else {
        filter_rubocop_json(&stdout)
    };

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "rubocop", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() && (!output.status.success() || verbose > 0) {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("rubocop {}", args.join(" ")),
        &format!("rtk rubocop {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

// ── JSON filtering ───────────────────────────────────────────────────────────

/// Rank severity for ordering: lower = more severe.
fn severity_rank(severity: &str) -> u8 {
    match severity {
        "fatal" | "error" => 0,
        "warning" => 1,
        "convention" | "refactor" | "info" => 2,
        _ => 3,
    }
}

fn filter_rubocop_json(output: &str) -> String {
    if output.trim().is_empty() {
        return "RuboCop: No output".to_string();
    }

    let parsed: Result<RubocopOutput, _> = serde_json::from_str(output);
    let rubocop = match parsed {
        Ok(r) => r,
        Err(e) => {
            eprintln!("[rtk] rubocop: JSON parse failed ({})", e);
            return crate::utils::fallback_tail(output, "rubocop (JSON parse error)", 5);
        }
    };

    let s = &rubocop.summary;

    if s.offense_count == 0 {
        return format!("ok ✓ rubocop ({} files)", s.inspected_file_count);
    }

    // When correctable_offense_count is 0, it could mean the field was absent
    // (older RuboCop) or genuinely zero. Manual count as consistent fallback.
    let correctable_count = if s.correctable_offense_count > 0 {
        s.correctable_offense_count
    } else {
        rubocop
            .files
            .iter()
            .flat_map(|f| &f.offenses)
            .filter(|o| o.correctable)
            .count()
    };

    let mut result = format!(
        "rubocop: {} offenses ({} files)\n",
        s.offense_count, s.inspected_file_count
    );

    // Build list of files with offenses, sorted by worst severity then file path
    let mut files_with_offenses: Vec<&RubocopFile> = rubocop
        .files
        .iter()
        .filter(|f| !f.offenses.is_empty())
        .collect();

    // Sort files: worst severity first, then alphabetically
    files_with_offenses.sort_by(|a, b| {
        let a_worst = a
            .offenses
            .iter()
            .map(|o| severity_rank(&o.severity))
            .min()
            .unwrap_or(3);
        let b_worst = b
            .offenses
            .iter()
            .map(|o| severity_rank(&o.severity))
            .min()
            .unwrap_or(3);
        a_worst.cmp(&b_worst).then(a.path.cmp(&b.path))
    });

    let max_files = 10;
    let max_offenses_per_file = 5;

    for file in files_with_offenses.iter().take(max_files) {
        let short = compact_ruby_path(&file.path);
        result.push_str(&format!("\n{}\n", short));

        // Sort offenses within file: by severity rank, then by line number
        let mut sorted_offenses: Vec<&RubocopOffense> = file.offenses.iter().collect();
        sorted_offenses.sort_by(|a, b| {
            severity_rank(&a.severity)
                .cmp(&severity_rank(&b.severity))
                .then(a.location.start_line.cmp(&b.location.start_line))
        });

        for offense in sorted_offenses.iter().take(max_offenses_per_file) {
            let first_msg_line = offense.message.lines().next().unwrap_or("");
            result.push_str(&format!(
                "  :{} {} — {}\n",
                offense.location.start_line, offense.cop_name, first_msg_line
            ));
        }
        if sorted_offenses.len() > max_offenses_per_file {
            result.push_str(&format!(
                "  ... +{} more\n",
                sorted_offenses.len() - max_offenses_per_file
            ));
        }
    }

    if files_with_offenses.len() > max_files {
        result.push_str(&format!(
            "\n... +{} more files\n",
            files_with_offenses.len() - max_files
        ));
    }

    if correctable_count > 0 {
        result.push_str(&format!(
            "\n({} correctable, run `rubocop -A`)",
            correctable_count
        ));
    }

    result.trim().to_string()
}

// ── Text fallback ────────────────────────────────────────────────────────────

fn filter_rubocop_text(output: &str) -> String {
    // Check for Ruby/Bundler errors first -- show error, truncated to avoid excessive tokens
    for line in output.lines() {
        let t = line.trim();
        if t.contains("cannot load such file")
            || t.contains("Bundler::GemNotFound")
            || t.contains("Gem::MissingSpecError")
            || t.starts_with("rubocop: command not found")
            || t.starts_with("rubocop: No such file")
        {
            let error_lines: Vec<&str> = output.trim().lines().take(20).collect();
            let truncated = error_lines.join("\n");
            let total_lines = output.trim().lines().count();
            if total_lines > 20 {
                return format!(
                    "RuboCop error:\n{}\n... ({} more lines)",
                    truncated,
                    total_lines - 20
                );
            }
            return format!("RuboCop error:\n{}", truncated);
        }
    }

    // Detect autocorrect summary: "N files inspected, M offenses detected, K offenses autocorrected"
    for line in output.lines().rev() {
        let t = line.trim();
        if t.contains("inspected") && t.contains("autocorrected") {
            // Extract counts for compact autocorrect message
            let files = extract_leading_number(t);
            let corrected = extract_autocorrect_count(t);
            if files > 0 && corrected > 0 {
                return format!(
                    "ok ✓ rubocop -A ({} files, {} autocorrected)",
                    files, corrected
                );
            }
            return format!("RuboCop: {}", t);
        }
        if t.contains("inspected") && (t.contains("offense") || t.contains("no offenses")) {
            if t.contains("no offenses") {
                let files = extract_leading_number(t);
                if files > 0 {
                    return format!("ok ✓ rubocop ({} files)", files);
                }
                return "ok ✓ rubocop (no offenses)".to_string();
            }
            return format!("RuboCop: {}", t);
        }
    }
    // Last resort: last 5 lines
    crate::utils::fallback_tail(output, "rubocop", 5)
}

/// Extract leading number from a string like "15 files inspected".
fn extract_leading_number(s: &str) -> usize {
    s.split_whitespace()
        .next()
        .and_then(|w| w.parse().ok())
        .unwrap_or(0)
}

/// Extract autocorrect count from summary like "... 3 offenses autocorrected".
fn extract_autocorrect_count(s: &str) -> usize {
    // Look for "N offenses autocorrected" near end
    let parts: Vec<&str> = s.split(',').collect();
    for part in parts.iter().rev() {
        let t = part.trim();
        if t.contains("autocorrected") {
            return extract_leading_number(t);
        }
    }
    0
}

/// Compact Ruby file path by finding the nearest Rails convention directory
/// and stripping the absolute path prefix.
fn compact_ruby_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    for prefix in &[
        "app/models/",
        "app/controllers/",
        "app/views/",
        "app/helpers/",
        "app/services/",
        "app/jobs/",
        "app/mailers/",
        "lib/",
        "spec/",
        "test/",
        "config/",
    ] {
        if let Some(pos) = path.find(prefix) {
            return path[pos..].to_string();
        }
    }

    // Generic: strip up to last known directory marker
    if let Some(pos) = path.rfind("/app/") {
        return path[pos + 1..].to_string();
    }
    if let Some(pos) = path.rfind('/') {
        return path[pos + 1..].to_string();
    }
    path
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::count_tokens;

    fn no_offenses_json() -> &'static str {
        r#"{
          "metadata": {"rubocop_version": "1.60.0"},
          "files": [],
          "summary": {
            "offense_count": 0,
            "target_file_count": 0,
            "inspected_file_count": 15
          }
        }"#
    }

    fn with_offenses_json() -> &'static str {
        r#"{
          "metadata": {"rubocop_version": "1.60.0"},
          "files": [
            {
              "path": "app/models/user.rb",
              "offenses": [
                {
                  "severity": "convention",
                  "message": "Trailing whitespace detected.",
                  "cop_name": "Layout/TrailingWhitespace",
                  "correctable": true,
                  "location": {"start_line": 10, "start_column": 5, "last_line": 10, "last_column": 8, "length": 3, "line": 10, "column": 5}
                },
                {
                  "severity": "convention",
                  "message": "Missing frozen string literal comment.",
                  "cop_name": "Style/FrozenStringLiteralComment",
                  "correctable": true,
                  "location": {"start_line": 1, "start_column": 1, "last_line": 1, "last_column": 1, "length": 1, "line": 1, "column": 1}
                },
                {
                  "severity": "warning",
                  "message": "Useless assignment to variable - `x`.",
                  "cop_name": "Lint/UselessAssignment",
                  "correctable": false,
                  "location": {"start_line": 25, "start_column": 5, "last_line": 25, "last_column": 6, "length": 1, "line": 25, "column": 5}
                }
              ]
            },
            {
              "path": "app/controllers/users_controller.rb",
              "offenses": [
                {
                  "severity": "convention",
                  "message": "Trailing whitespace detected.",
                  "cop_name": "Layout/TrailingWhitespace",
                  "correctable": true,
                  "location": {"start_line": 5, "start_column": 20, "last_line": 5, "last_column": 22, "length": 2, "line": 5, "column": 20}
                },
                {
                  "severity": "error",
                  "message": "Syntax error, unexpected end-of-input.",
                  "cop_name": "Lint/Syntax",
                  "correctable": false,
                  "location": {"start_line": 30, "start_column": 1, "last_line": 30, "last_column": 1, "length": 1, "line": 30, "column": 1}
                }
              ]
            }
          ],
          "summary": {
            "offense_count": 5,
            "target_file_count": 2,
            "inspected_file_count": 20
          }
        }"#
    }

    #[test]
    fn test_filter_rubocop_no_offenses() {
        let result = filter_rubocop_json(no_offenses_json());
        assert_eq!(result, "ok ✓ rubocop (15 files)");
    }

    #[test]
    fn test_filter_rubocop_with_offenses_per_file() {
        let result = filter_rubocop_json(with_offenses_json());
        // Should show per-file offenses
        assert!(result.contains("5 offenses (20 files)"));
        // controllers file has error severity, should appear first
        assert!(result.contains("app/controllers/users_controller.rb"));
        assert!(result.contains("app/models/user.rb"));
        // Per-file offense format: :line CopName — message
        assert!(result.contains(":30 Lint/Syntax — Syntax error"));
        assert!(result.contains(":10 Layout/TrailingWhitespace — Trailing whitespace"));
        assert!(result.contains(":25 Lint/UselessAssignment — Useless assignment"));
    }

    #[test]
    fn test_filter_rubocop_severity_ordering() {
        let result = filter_rubocop_json(with_offenses_json());
        // File with error should come before file with only convention/warning
        let ctrl_pos = result.find("users_controller.rb").unwrap();
        let model_pos = result.find("app/models/user.rb").unwrap();
        assert!(
            ctrl_pos < model_pos,
            "Error-file should appear before convention-file"
        );

        // Within users_controller.rb, error should come before convention
        let error_pos = result.find(":30 Lint/Syntax").unwrap();
        let conv_pos = result.find(":5 Layout/TrailingWhitespace").unwrap();
        assert!(
            error_pos < conv_pos,
            "Error offense should appear before convention"
        );
    }

    #[test]
    fn test_filter_rubocop_within_file_line_ordering() {
        let result = filter_rubocop_json(with_offenses_json());
        // Within user.rb, warning (line 25) should come before conventions (line 1, 10)
        let warning_pos = result.find(":25 Lint/UselessAssignment").unwrap();
        let conv1_pos = result.find(":1 Style/FrozenStringLiteralComment").unwrap();
        assert!(
            warning_pos < conv1_pos,
            "Warning should come before convention within same file"
        );
    }

    #[test]
    fn test_filter_rubocop_correctable_hint() {
        let result = filter_rubocop_json(with_offenses_json());
        assert!(result.contains("3 correctable"));
        assert!(result.contains("rubocop -A"));
    }

    #[test]
    fn test_filter_rubocop_text_fallback() {
        let text = r#"Inspecting 10 files
..........

10 files inspected, no offenses detected"#;
        let result = filter_rubocop_text(text);
        assert_eq!(result, "ok ✓ rubocop (10 files)");
    }

    #[test]
    fn test_filter_rubocop_text_autocorrect() {
        let text = r#"Inspecting 15 files
...C..CC.......

15 files inspected, 3 offenses detected, 3 offenses autocorrected"#;
        let result = filter_rubocop_text(text);
        assert_eq!(result, "ok ✓ rubocop -A (15 files, 3 autocorrected)");
    }

    #[test]
    fn test_filter_rubocop_empty_output() {
        let result = filter_rubocop_json("");
        assert_eq!(result, "RuboCop: No output");
    }

    #[test]
    fn test_filter_rubocop_invalid_json_falls_back() {
        let garbage = "some ruby warning\n{broken json";
        let result = filter_rubocop_json(garbage);
        assert!(!result.is_empty(), "should not panic on invalid JSON");
    }

    #[test]
    fn test_compact_ruby_path() {
        assert_eq!(
            compact_ruby_path("/home/user/project/app/models/user.rb"),
            "app/models/user.rb"
        );
        assert_eq!(
            compact_ruby_path("app/controllers/users_controller.rb"),
            "app/controllers/users_controller.rb"
        );
        assert_eq!(
            compact_ruby_path("/project/spec/models/user_spec.rb"),
            "spec/models/user_spec.rb"
        );
        assert_eq!(
            compact_ruby_path("lib/tasks/deploy.rake"),
            "lib/tasks/deploy.rake"
        );
    }

    #[test]
    fn test_filter_rubocop_caps_offenses_per_file() {
        // File with 7 offenses should show 5 + overflow
        let json = r#"{
          "metadata": {"rubocop_version": "1.60.0"},
          "files": [
            {
              "path": "app/models/big.rb",
              "offenses": [
                {"severity": "convention", "message": "msg1", "cop_name": "Cop/A", "correctable": false, "location": {"start_line": 1, "start_column": 1}},
                {"severity": "convention", "message": "msg2", "cop_name": "Cop/B", "correctable": false, "location": {"start_line": 2, "start_column": 1}},
                {"severity": "convention", "message": "msg3", "cop_name": "Cop/C", "correctable": false, "location": {"start_line": 3, "start_column": 1}},
                {"severity": "convention", "message": "msg4", "cop_name": "Cop/D", "correctable": false, "location": {"start_line": 4, "start_column": 1}},
                {"severity": "convention", "message": "msg5", "cop_name": "Cop/E", "correctable": false, "location": {"start_line": 5, "start_column": 1}},
                {"severity": "convention", "message": "msg6", "cop_name": "Cop/F", "correctable": false, "location": {"start_line": 6, "start_column": 1}},
                {"severity": "convention", "message": "msg7", "cop_name": "Cop/G", "correctable": false, "location": {"start_line": 7, "start_column": 1}}
              ]
            }
          ],
          "summary": {"offense_count": 7, "target_file_count": 1, "inspected_file_count": 5}
        }"#;
        let result = filter_rubocop_json(json);
        assert!(result.contains(":5 Cop/E"), "should show 5th offense");
        assert!(!result.contains(":6 Cop/F"), "should not show 6th inline");
        assert!(result.contains("+2 more"), "should show overflow");
    }

    #[test]
    fn test_filter_rubocop_text_bundler_error() {
        let text = "Bundler::GemNotFound: Could not find gem 'rubocop' in any sources.";
        let result = filter_rubocop_text(text);
        assert!(
            result.starts_with("RuboCop error:"),
            "should detect Bundler error: {}",
            result
        );
        assert!(result.contains("GemNotFound"));
    }

    #[test]
    fn test_filter_rubocop_text_load_error() {
        let text =
            "/usr/lib/ruby/3.2.0/rubygems.rb:250: cannot load such file -- rubocop (LoadError)";
        let result = filter_rubocop_text(text);
        assert!(
            result.starts_with("RuboCop error:"),
            "should detect load error: {}",
            result
        );
    }

    #[test]
    fn test_filter_rubocop_text_with_offenses() {
        let text = r#"Inspecting 5 files
..C..

5 files inspected, 1 offense detected"#;
        let result = filter_rubocop_text(text);
        assert_eq!(result, "RuboCop: 5 files inspected, 1 offense detected");
    }

    #[test]
    fn test_severity_rank() {
        assert!(severity_rank("error") < severity_rank("warning"));
        assert!(severity_rank("warning") < severity_rank("convention"));
        assert!(severity_rank("fatal") < severity_rank("warning"));
    }

    #[test]
    fn test_token_savings() {
        let input = with_offenses_json();
        let output = filter_rubocop_json(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "RuboCop: expected ≥60% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── ANSI handling test ──────────────────────────────────────────────────

    #[test]
    fn test_filter_rubocop_json_with_ansi_prefix() {
        // ANSI codes before JSON should trigger fallback, not panic
        let input = "\x1b[33mWarning: something\x1b[0m\n{\"broken\": true}";
        let result = filter_rubocop_json(input);
        assert!(!result.is_empty(), "should not panic on ANSI-prefixed JSON");
    }

    // ── 10-file cap test (Issue 12) ─────────────────────────────────────────

    #[test]
    fn test_filter_rubocop_caps_at_ten_files() {
        // Build JSON with 12 files, each having 1 offense
        let mut files_json = Vec::new();
        for i in 1..=12 {
            files_json.push(format!(
                r#"{{"path": "app/models/model_{}.rb", "offenses": [{{"severity": "convention", "message": "msg{}", "cop_name": "Cop/X{}", "correctable": false, "location": {{"start_line": 1, "start_column": 1}}}}]}}"#,
                i, i, i
            ));
        }
        let json = format!(
            r#"{{"metadata": {{"rubocop_version": "1.60.0"}}, "files": [{}], "summary": {{"offense_count": 12, "target_file_count": 12, "inspected_file_count": 12}}}}"#,
            files_json.join(",")
        );
        let result = filter_rubocop_json(&json);
        assert!(
            result.contains("+2 more files"),
            "should show +2 more files overflow: {}",
            result
        );
    }
}
