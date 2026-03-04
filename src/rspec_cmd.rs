use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use serde::Deserialize;
use std::process::Command;

// ── JSON structures matching RSpec's --format json output ───────────────────

#[derive(Deserialize)]
struct RspecOutput {
    examples: Vec<RspecExample>,
    summary: RspecSummary,
}

#[derive(Deserialize)]
struct RspecExample {
    full_description: String,
    status: String,
    file_path: String,
    line_number: u32,
    exception: Option<RspecException>,
}

#[derive(Deserialize)]
struct RspecException {
    class: String,
    message: String,
    #[serde(default)]
    backtrace: Vec<String>,
}

#[derive(Deserialize)]
struct RspecSummary {
    duration: f64,
    example_count: usize,
    failure_count: usize,
    pending_count: usize,
    #[serde(default)]
    errors_outside_of_examples_count: usize,
}

// ── Public entry point ───────────────────────────────────────────────────────

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let (program, pre_args) = detect_rspec_command();
    let mut cmd = Command::new(&program);
    for arg in &pre_args {
        cmd.arg(arg);
    }

    // Inject --format json unless the user already specified a format
    let has_format = args.iter().any(|a| a.starts_with("--format") || a == "-f");

    if !has_format {
        cmd.arg("--format").arg("json");
    }

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        let parts: Vec<&str> = std::iter::once(program.as_str())
            .chain(pre_args.iter().map(String::as_str))
            .chain(["--format", "json"])
            .chain(args.iter().map(String::as_str))
            .collect();
        eprintln!("Running: {}", parts.join(" "));
    }

    let output = cmd.output().context(
        "Failed to run rspec. Is it installed? Try: gem install rspec or add it to your Gemfile",
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = if has_format {
        stdout.to_string()
    } else {
        filter_rspec_output(&stdout)
    };

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "rspec", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() && verbose > 0 {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("rspec {}", args.join(" ")),
        &format!("rtk rspec {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

// ── Command detection ────────────────────────────────────────────────────────

/// Detect whether to use `bundle exec rspec` or plain `rspec`.
///
/// Prefers bundler when a Gemfile mentioning rspec is present, which is
/// the standard Rails project layout.
fn detect_rspec_command() -> (String, Vec<String>) {
    if std::path::Path::new("Gemfile").exists() {
        if let Ok(gemfile) = std::fs::read_to_string("Gemfile") {
            if gemfile.contains("rspec") {
                return (
                    "bundle".to_string(),
                    vec!["exec".to_string(), "rspec".to_string()],
                );
            }
        }
    }
    ("rspec".to_string(), vec![])
}

// ── Output filtering ─────────────────────────────────────────────────────────

fn filter_rspec_output(output: &str) -> String {
    if output.trim().is_empty() {
        return "RSpec: No output".to_string();
    }
    match serde_json::from_str::<RspecOutput>(output) {
        Ok(rspec) => build_rspec_summary(&rspec),
        Err(_) => filter_rspec_text(output),
    }
}

fn build_rspec_summary(rspec: &RspecOutput) -> String {
    let s = &rspec.summary;

    if s.example_count == 0 {
        return "RSpec: No examples found".to_string();
    }

    if s.failure_count == 0 && s.errors_outside_of_examples_count == 0 {
        let mut result = format!("✓ RSpec: {} passed", s.example_count - s.pending_count);
        if s.pending_count > 0 {
            result.push_str(&format!(", {} pending", s.pending_count));
        }
        result.push_str(&format!(" ({:.2}s)", s.duration));
        return result;
    }

    let passed = s
        .example_count
        .saturating_sub(s.failure_count + s.pending_count);
    let mut result = format!("RSpec: {} passed, {} failed", passed, s.failure_count);
    if s.pending_count > 0 {
        result.push_str(&format!(", {} pending", s.pending_count));
    }
    result.push_str(&format!(" ({:.2}s)\n", s.duration));
    result.push_str("═══════════════════════════════════════\n");

    let failures: Vec<&RspecExample> = rspec
        .examples
        .iter()
        .filter(|e| e.status == "failed")
        .collect();

    if failures.is_empty() {
        return result.trim().to_string();
    }

    result.push_str("\nFailures:\n");

    for (i, example) in failures.iter().take(5).enumerate() {
        result.push_str(&format!(
            "{}. ❌ {}\n   {}:{}\n",
            i + 1,
            example.full_description,
            example.file_path,
            example.line_number
        ));

        if let Some(exc) = &example.exception {
            let short_class = exc.class.split("::").last().unwrap_or(&exc.class);
            let first_msg = exc.message.lines().next().unwrap_or("");
            result.push_str(&format!(
                "   {}: {}\n",
                short_class,
                truncate(first_msg, 120)
            ));

            // First backtrace line not from gems/rspec internals
            for bt in &exc.backtrace {
                if !bt.contains("/gems/") && !bt.contains("lib/rspec") {
                    result.push_str(&format!("   {}\n", truncate(bt, 120)));
                    break;
                }
            }
        }

        if i < failures.len() - 1 {
            result.push('\n');
        }
    }

    if failures.len() > 5 {
        result.push_str(&format!("\n... +{} more failures\n", failures.len() - 5));
    }

    result.trim().to_string()
}

/// Fallback plain-text parser when JSON is unavailable (e.g. user passed --format).
fn filter_rspec_text(output: &str) -> String {
    // RSpec always prints a summary line like "3 examples, 1 failure, 1 pending"
    for line in output.lines().rev() {
        let t = line.trim();
        if t.contains("example") && (t.contains("failure") || t.contains("pending")) {
            return format!("RSpec: {}", t);
        }
    }
    // Last resort: last 5 lines
    let lines: Vec<&str> = output.lines().collect();
    let start = lines.len().saturating_sub(5);
    lines[start..].join("\n")
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // ── Fixtures (realistic RSpec JSON output) ───────────────────────────────

    fn all_pass_json() -> &'static str {
        r#"{
          "version": "3.12.0",
          "examples": [
            {
              "id": "./spec/models/user_spec.rb[1:1]",
              "description": "is valid with valid attributes",
              "full_description": "User is valid with valid attributes",
              "status": "passed",
              "file_path": "./spec/models/user_spec.rb",
              "line_number": 5,
              "run_time": 0.001234,
              "pending_message": null,
              "exception": null
            },
            {
              "id": "./spec/models/user_spec.rb[1:2]",
              "description": "validates email format",
              "full_description": "User validates email format",
              "status": "passed",
              "file_path": "./spec/models/user_spec.rb",
              "line_number": 12,
              "run_time": 0.0008,
              "pending_message": null,
              "exception": null
            }
          ],
          "summary": {
            "duration": 0.015,
            "example_count": 2,
            "failure_count": 0,
            "pending_count": 0,
            "errors_outside_of_examples_count": 0
          },
          "summary_line": "2 examples, 0 failures"
        }"#
    }

    fn with_failures_json() -> &'static str {
        r#"{
          "version": "3.12.0",
          "examples": [
            {
              "id": "./spec/models/user_spec.rb[1:1]",
              "description": "is valid",
              "full_description": "User is valid",
              "status": "passed",
              "file_path": "./spec/models/user_spec.rb",
              "line_number": 5,
              "run_time": 0.001,
              "pending_message": null,
              "exception": null
            },
            {
              "id": "./spec/models/user_spec.rb[1:2]",
              "description": "saves to database",
              "full_description": "User saves to database",
              "status": "failed",
              "file_path": "./spec/models/user_spec.rb",
              "line_number": 10,
              "run_time": 0.002,
              "pending_message": null,
              "exception": {
                "class": "RSpec::Expectations::ExpectationNotMetError",
                "message": "expected true but got false",
                "backtrace": [
                  "/usr/local/lib/ruby/gems/3.2.0/gems/rspec-expectations-3.12.0/lib/rspec/expectations/fail_with.rb:37:in `fail_with'",
                  "./spec/models/user_spec.rb:11:in `block (2 levels) in <top (required)>'"
                ]
              }
            }
          ],
          "summary": {
            "duration": 0.123,
            "example_count": 2,
            "failure_count": 1,
            "pending_count": 0,
            "errors_outside_of_examples_count": 0
          },
          "summary_line": "2 examples, 1 failure"
        }"#
    }

    fn with_pending_json() -> &'static str {
        r#"{
          "version": "3.12.0",
          "examples": [
            {
              "id": "./spec/models/post_spec.rb[1:1]",
              "description": "creates a post",
              "full_description": "Post creates a post",
              "status": "passed",
              "file_path": "./spec/models/post_spec.rb",
              "line_number": 4,
              "run_time": 0.002,
              "pending_message": null,
              "exception": null
            },
            {
              "id": "./spec/models/post_spec.rb[1:2]",
              "description": "sends notification",
              "full_description": "Post sends notification",
              "status": "pending",
              "file_path": "./spec/models/post_spec.rb",
              "line_number": 10,
              "run_time": 0.0,
              "pending_message": "Not yet implemented",
              "exception": null
            }
          ],
          "summary": {
            "duration": 0.045,
            "example_count": 2,
            "failure_count": 0,
            "pending_count": 1,
            "errors_outside_of_examples_count": 0
          },
          "summary_line": "2 examples, 0 failures, 1 pending"
        }"#
    }

    fn no_examples_json() -> &'static str {
        r#"{
          "version": "3.12.0",
          "examples": [],
          "summary": {
            "duration": 0.001,
            "example_count": 0,
            "failure_count": 0,
            "pending_count": 0,
            "errors_outside_of_examples_count": 0
          },
          "summary_line": "0 examples, 0 failures"
        }"#
    }

    fn many_failures_json() -> &'static str {
        r#"{
          "version": "3.12.0",
          "examples": [
            {"id":"./spec/a_spec.rb[1:1]","description":"test 1","full_description":"A test 1","status":"failed","file_path":"./spec/a_spec.rb","line_number":5,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 1","backtrace":["./spec/a_spec.rb:6:in `block'"]}},
            {"id":"./spec/a_spec.rb[1:2]","description":"test 2","full_description":"A test 2","status":"failed","file_path":"./spec/a_spec.rb","line_number":10,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 2","backtrace":["./spec/a_spec.rb:11:in `block'"]}},
            {"id":"./spec/a_spec.rb[1:3]","description":"test 3","full_description":"A test 3","status":"failed","file_path":"./spec/a_spec.rb","line_number":15,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 3","backtrace":["./spec/a_spec.rb:16:in `block'"]}},
            {"id":"./spec/a_spec.rb[1:4]","description":"test 4","full_description":"A test 4","status":"failed","file_path":"./spec/a_spec.rb","line_number":20,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 4","backtrace":["./spec/a_spec.rb:21:in `block'"]}},
            {"id":"./spec/a_spec.rb[1:5]","description":"test 5","full_description":"A test 5","status":"failed","file_path":"./spec/a_spec.rb","line_number":25,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 5","backtrace":["./spec/a_spec.rb:26:in `block'"]}},
            {"id":"./spec/a_spec.rb[1:6]","description":"test 6","full_description":"A test 6","status":"failed","file_path":"./spec/a_spec.rb","line_number":30,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 6","backtrace":["./spec/a_spec.rb:31:in `block'"]}}
          ],
          "summary": {
            "duration": 0.05,
            "example_count": 6,
            "failure_count": 6,
            "pending_count": 0,
            "errors_outside_of_examples_count": 0
          },
          "summary_line": "6 examples, 6 failures"
        }"#
    }

    // ── filter_rspec_output tests ────────────────────────────────────────────

    #[test]
    fn test_filter_all_pass() {
        let result = filter_rspec_output(all_pass_json());
        assert!(
            result.contains("✓ RSpec"),
            "should show checkmark: {}",
            result
        );
        assert!(
            result.contains("2 passed"),
            "should show passed count: {}",
            result
        );
        assert!(
            !result.contains("failed"),
            "should not mention failures: {}",
            result
        );
    }

    #[test]
    fn test_filter_all_pass_includes_duration() {
        let result = filter_rspec_output(all_pass_json());
        assert!(
            result.contains("0.01s") || result.contains("0.02s") || result.contains("s)"),
            "should show duration: {}",
            result
        );
    }

    #[test]
    fn test_filter_with_failures_shows_counts() {
        let result = filter_rspec_output(with_failures_json());
        assert!(
            result.contains("1 passed"),
            "should show passed: {}",
            result
        );
        assert!(
            result.contains("1 failed"),
            "should show failed: {}",
            result
        );
    }

    #[test]
    fn test_filter_with_failures_shows_test_name() {
        let result = filter_rspec_output(with_failures_json());
        assert!(
            result.contains("User saves to database"),
            "should show failing test name: {}",
            result
        );
    }

    #[test]
    fn test_filter_with_failures_shows_file_location() {
        let result = filter_rspec_output(with_failures_json());
        assert!(
            result.contains("user_spec.rb"),
            "should show spec file: {}",
            result
        );
        assert!(result.contains("10"), "should show line number: {}", result);
    }

    #[test]
    fn test_filter_with_failures_shows_exception_message() {
        let result = filter_rspec_output(with_failures_json());
        assert!(
            result.contains("expected true but got false"),
            "should show exception message: {}",
            result
        );
    }

    #[test]
    fn test_filter_with_failures_strips_gem_backtrace() {
        let result = filter_rspec_output(with_failures_json());
        assert!(
            !result.contains("/gems/rspec-expectations"),
            "should strip gem backtrace: {}",
            result
        );
    }

    #[test]
    fn test_filter_with_pending_shows_pending_count() {
        let result = filter_rspec_output(with_pending_json());
        assert!(
            result.contains("1 pending"),
            "should show pending count: {}",
            result
        );
        assert!(
            result.contains("✓ RSpec") || result.contains("passed"),
            "should show passes: {}",
            result
        );
    }

    #[test]
    fn test_filter_no_examples() {
        let result = filter_rspec_output(no_examples_json());
        assert!(
            result.contains("No examples"),
            "should indicate no examples: {}",
            result
        );
    }

    #[test]
    fn test_filter_many_failures_caps_at_five() {
        let result = filter_rspec_output(many_failures_json());
        assert!(
            result.contains("more failures") || result.contains("+1"),
            "should truncate beyond 5: {}",
            result
        );
        // Should show exactly 5 failure entries (1. through 5.)
        assert!(result.contains("1. ❌"), "should show first failure");
        assert!(result.contains("5. ❌"), "should show fifth failure");
        assert!(!result.contains("6. ❌"), "should not show sixth inline");
    }

    // ── Token savings ────────────────────────────────────────────────────────

    #[test]
    fn test_token_savings_all_pass() {
        let input = all_pass_json();
        let output = filter_rspec_output(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Expected ≥60% savings on all-pass, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_token_savings_with_failures() {
        let input = with_failures_json();
        let output = filter_rspec_output(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 50.0,
            "Expected ≥50% savings with failures, got {:.1}%",
            savings
        );
    }

    // ── Fallback text parser ─────────────────────────────────────────────────

    #[test]
    fn test_filter_text_fallback_summary_line() {
        let text = "Randomized with seed 12345\n\
                    Finished in 0.05 seconds (files took 1.2 seconds to load)\n\
                    5 examples, 1 failure, 2 pending\n\
                    Failed examples:\n\
                    rspec ./spec/models/user_spec.rb:10 # User saves to database";
        let result = filter_rspec_text(text);
        assert!(
            result.contains("5 examples"),
            "should extract summary: {}",
            result
        );
        assert!(
            result.contains("1 failure"),
            "should include failure: {}",
            result
        );
    }

    #[test]
    fn test_filter_text_fallback_no_summary() {
        // If no summary line, returns last 5 lines (does not panic)
        let text = "some output\nwithout a summary line";
        let result = filter_rspec_text(text);
        assert!(!result.is_empty(), "should return something: {}", result);
    }

    // ── Edge cases ───────────────────────────────────────────────────────────

    #[test]
    fn test_filter_invalid_json_falls_back_gracefully() {
        let garbage = "not json at all { broken";
        let result = filter_rspec_output(garbage);
        // Should not panic, returns something
        assert!(!result.is_empty());
    }

    #[test]
    fn test_filter_empty_output() {
        let result = filter_rspec_output("");
        assert!(!result.is_empty(), "should handle empty output");
    }

    #[test]
    fn test_exception_class_shortened() {
        let result = filter_rspec_output(with_failures_json());
        // Should show short class name, not full namespace
        assert!(
            result.contains("ExpectationNotMetError"),
            "should show short class: {}",
            result
        );
        assert!(
            !result.contains("RSpec::Expectations::ExpectationNotMetError"),
            "should not show full namespace: {}",
            result
        );
    }
}
