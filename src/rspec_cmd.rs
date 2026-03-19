//! RSpec test runner filter.
//!
//! Injects `--format json` to get structured output, parses it to show only
//! failures. Falls back to a state-machine text parser when JSON is unavailable
//! (e.g., user specified `--format documentation`) or when injected JSON output
//! fails to parse.

use crate::tracking;
use crate::utils::{exit_code_from_output, fallback_tail, ruby_exec, truncate};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde::Deserialize;

// ── Noise-stripping regex patterns ──────────────────────────────────────────

lazy_static! {
    static ref RE_SPRING: Regex = Regex::new(r"(?i)running via spring preloader").unwrap();
    static ref RE_SIMPLECOV: Regex =
        Regex::new(r"(?i)(coverage report|simplecov|coverage/|\.simplecov|All Files.*Lines)")
            .unwrap();
    static ref RE_DEPRECATION: Regex = Regex::new(r"^DEPRECATION WARNING:").unwrap();
    static ref RE_FINISHED_IN: Regex = Regex::new(r"^Finished in \d").unwrap();
    static ref RE_SCREENSHOT: Regex = Regex::new(r"saved screenshot to (.+)").unwrap();
    static ref RE_RSPEC_SUMMARY: Regex = Regex::new(r"(\d+) examples?, (\d+) failures?").unwrap();
}

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

    let mut cmd = ruby_exec("rspec");

    // Inject --format json unless the user already specified a format.
    // Handles: --format, -f, --format=..., -fj, -fjson, -fdocumentation (from PR #534)
    let has_format = args.iter().any(|a| {
        a == "--format"
            || a == "-f"
            || a.starts_with("--format=")
            || (a.starts_with("-f") && a.len() > 2 && !a.starts_with("--"))
    });

    if !has_format {
        cmd.arg("--format").arg("json");
    }

    cmd.args(args);

    if verbose > 0 {
        let injected = if has_format { "" } else { " --format json" };
        eprintln!("Running: rspec{} {}", injected, args.join(" "));
    }

    let output = cmd.output().context(
        "Failed to run rspec. Is it installed? Try: gem install rspec or add it to your Gemfile",
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, "rspec");

    let filtered = if stdout.trim().is_empty() && !output.status.success() {
        "RSpec: FAILED (no stdout, see stderr below)".to_string()
    } else if has_format {
        // User specified format — use text fallback on stripped output
        let stripped = strip_noise(&stdout);
        filter_rspec_text(&stripped)
    } else {
        filter_rspec_output(&stdout)
    };

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "rspec", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() && (!output.status.success() || verbose > 0) {
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

// ── Noise stripping ─────────────────────────────────────────────────────────

/// Remove noise lines: Spring preloader, SimpleCov, DEPRECATION warnings,
/// "Finished in" timing line, and Capybara screenshot details (keep path only).
fn strip_noise(output: &str) -> String {
    let mut result = Vec::new();
    let mut in_simplecov_block = false;

    for line in output.lines() {
        let trimmed = line.trim();

        // Skip Spring preloader messages
        if RE_SPRING.is_match(trimmed) {
            continue;
        }

        // Skip lines starting with "DEPRECATION WARNING:" (single-line only)
        if RE_DEPRECATION.is_match(trimmed) {
            continue;
        }

        // Skip "Finished in N seconds" line
        if RE_FINISHED_IN.is_match(trimmed) {
            continue;
        }

        // SimpleCov block detection: once we see it, skip until blank line
        if RE_SIMPLECOV.is_match(trimmed) {
            in_simplecov_block = true;
            continue;
        }
        if in_simplecov_block {
            if trimmed.is_empty() {
                in_simplecov_block = false;
            }
            continue;
        }

        // Capybara screenshots: keep only the path
        if let Some(caps) = RE_SCREENSHOT.captures(trimmed) {
            if let Some(path) = caps.get(1) {
                result.push(format!("[screenshot: {}]", path.as_str().trim()));
                continue;
            }
        }

        result.push(line.to_string());
    }

    result.join("\n")
}

// ── Output filtering ─────────────────────────────────────────────────────────

fn filter_rspec_output(output: &str) -> String {
    if output.trim().is_empty() {
        return "RSpec: No output".to_string();
    }

    // Try parsing as JSON first (happy path when --format json is injected)
    if let Ok(rspec) = serde_json::from_str::<RspecOutput>(output) {
        return build_rspec_summary(&rspec);
    }

    // Strip noise (Spring, SimpleCov, etc.) and retry JSON parse
    let stripped = strip_noise(output);
    match serde_json::from_str::<RspecOutput>(&stripped) {
        Ok(rspec) => return build_rspec_summary(&rspec),
        Err(e) => {
            eprintln!(
                "[rtk] rspec: JSON parse failed ({}), using text fallback",
                e
            );
        }
    }

    filter_rspec_text(&stripped)
}

fn build_rspec_summary(rspec: &RspecOutput) -> String {
    let s = &rspec.summary;

    if s.example_count == 0 && s.errors_outside_of_examples_count == 0 {
        return "RSpec: No examples found".to_string();
    }

    if s.example_count == 0 && s.errors_outside_of_examples_count > 0 {
        return format!(
            "RSpec: {} errors outside of examples ({:.2}s)",
            s.errors_outside_of_examples_count, s.duration
        );
    }

    if s.failure_count == 0 && s.errors_outside_of_examples_count == 0 {
        let passed = s.example_count.saturating_sub(s.pending_count);
        let mut result = format!("✓ RSpec: {} passed", passed);
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

        if i < failures.len().min(5) - 1 {
            result.push('\n');
        }
    }

    if failures.len() > 5 {
        result.push_str(&format!("\n... +{} more failures\n", failures.len() - 5));
    }

    result.trim().to_string()
}

/// State machine text fallback parser for when JSON is unavailable.
fn filter_rspec_text(output: &str) -> String {
    #[derive(PartialEq)]
    enum State {
        Header,
        Failures,
        FailedExamples,
        Summary,
    }

    let mut state = State::Header;
    let mut failures: Vec<String> = Vec::new();
    let mut current_failure = String::new();
    let mut summary_line = String::new();

    for line in output.lines() {
        let trimmed = line.trim();

        match state {
            State::Header => {
                if trimmed == "Failures:" {
                    state = State::Failures;
                } else if trimmed == "Failed examples:" {
                    state = State::FailedExamples;
                } else if RE_RSPEC_SUMMARY.is_match(trimmed) {
                    summary_line = trimmed.to_string();
                    state = State::Summary;
                }
            }
            State::Failures => {
                // New failure block starts with numbered pattern like "  1) ..."
                if is_numbered_failure(trimmed) {
                    if !current_failure.trim().is_empty() {
                        failures.push(compact_failure_block(&current_failure));
                    }
                    current_failure = trimmed.to_string();
                    current_failure.push('\n');
                } else if trimmed == "Failed examples:" {
                    if !current_failure.trim().is_empty() {
                        failures.push(compact_failure_block(&current_failure));
                    }
                    current_failure.clear();
                    state = State::FailedExamples;
                } else if RE_RSPEC_SUMMARY.is_match(trimmed) {
                    if !current_failure.trim().is_empty() {
                        failures.push(compact_failure_block(&current_failure));
                    }
                    current_failure.clear();
                    summary_line = trimmed.to_string();
                    state = State::Summary;
                } else if !trimmed.is_empty() {
                    // Skip gem-internal backtrace lines
                    if is_gem_backtrace(trimmed) {
                        continue;
                    }
                    current_failure.push_str(trimmed);
                    current_failure.push('\n');
                }
            }
            State::FailedExamples => {
                if RE_RSPEC_SUMMARY.is_match(trimmed) {
                    summary_line = trimmed.to_string();
                    state = State::Summary;
                }
                // Skip "Failed examples:" section (just rspec commands to re-run)
            }
            State::Summary => {
                break;
            }
        }
    }

    // Capture remaining failure
    if !current_failure.trim().is_empty() && state == State::Failures {
        failures.push(compact_failure_block(&current_failure));
    }

    // If we found a summary line, build result
    if !summary_line.is_empty() {
        if failures.is_empty() {
            return format!("RSpec: {}", summary_line);
        }
        let mut result = format!("RSpec: {}\n", summary_line);
        result.push_str("═══════════════════════════════════════\n\n");
        for (i, failure) in failures.iter().take(5).enumerate() {
            result.push_str(&format!("{}. ❌ {}\n", i + 1, failure));
            if i < failures.len().min(5) - 1 {
                result.push('\n');
            }
        }
        if failures.len() > 5 {
            result.push_str(&format!("\n... +{} more failures\n", failures.len() - 5));
        }
        return result.trim().to_string();
    }

    // Fallback: look for summary anywhere
    for line in output.lines().rev() {
        let t = line.trim();
        if t.contains("example") && (t.contains("failure") || t.contains("pending")) {
            return format!("RSpec: {}", t);
        }
    }

    // Last resort: last 5 lines
    fallback_tail(output, "rspec", 5)
}

/// Check if a line is a numbered failure like "1) User#full_name..."
fn is_numbered_failure(line: &str) -> bool {
    let trimmed = line.trim();
    if let Some(pos) = trimmed.find(')') {
        let prefix = &trimmed[..pos];
        prefix.chars().all(|c| c.is_ascii_digit()) && !prefix.is_empty()
    } else {
        false
    }
}

/// Check if a backtrace line is from gems/rspec internals.
fn is_gem_backtrace(line: &str) -> bool {
    line.contains("/gems/")
        || line.contains("lib/rspec")
        || line.contains("lib/ruby/")
        || line.contains("vendor/bundle")
}

/// Compact a failure block: extract key info, strip verbose backtrace.
fn compact_failure_block(block: &str) -> String {
    let mut lines: Vec<&str> = block.lines().collect();

    // Remove empty lines
    lines.retain(|l| !l.trim().is_empty());

    // Extract spec file:line (lines starting with # ./spec/ or # ./test/)
    let mut spec_file = String::new();
    let mut kept_lines: Vec<String> = Vec::new();

    for line in &lines {
        let t = line.trim();
        if t.starts_with("# ./spec/") || t.starts_with("# ./test/") {
            spec_file = t.trim_start_matches("# ").to_string();
        } else if t.starts_with('#') && (t.contains("/gems/") || t.contains("lib/rspec")) {
            // Skip gem backtrace
            continue;
        } else {
            kept_lines.push(t.to_string());
        }
    }

    let mut result = kept_lines.join("\n   ");
    if !spec_file.is_empty() {
        result.push_str(&format!("\n   {}", spec_file));
    }
    result
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::utils::count_tokens;

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
              "description": "validates title",
              "full_description": "Post validates title",
              "status": "pending",
              "file_path": "./spec/models/post_spec.rb",
              "line_number": 8,
              "run_time": 0.0,
              "pending_message": "Not yet implemented",
              "exception": null
            }
          ],
          "summary": {
            "duration": 0.05,
            "example_count": 2,
            "failure_count": 0,
            "pending_count": 1,
            "errors_outside_of_examples_count": 0
          },
          "summary_line": "2 examples, 0 failures, 1 pending"
        }"#
    }

    fn large_suite_json() -> &'static str {
        r#"{
          "version": "3.12.0",
          "examples": [
            {"id":"1","description":"test1","full_description":"Suite test1","status":"passed","file_path":"./spec/a_spec.rb","line_number":1,"run_time":0.01,"pending_message":null,"exception":null},
            {"id":"2","description":"test2","full_description":"Suite test2","status":"passed","file_path":"./spec/a_spec.rb","line_number":2,"run_time":0.01,"pending_message":null,"exception":null},
            {"id":"3","description":"test3","full_description":"Suite test3","status":"passed","file_path":"./spec/a_spec.rb","line_number":3,"run_time":0.01,"pending_message":null,"exception":null},
            {"id":"4","description":"test4","full_description":"Suite test4","status":"passed","file_path":"./spec/a_spec.rb","line_number":4,"run_time":0.01,"pending_message":null,"exception":null},
            {"id":"5","description":"test5","full_description":"Suite test5","status":"passed","file_path":"./spec/a_spec.rb","line_number":5,"run_time":0.01,"pending_message":null,"exception":null},
            {"id":"6","description":"test6","full_description":"Suite test6","status":"passed","file_path":"./spec/a_spec.rb","line_number":6,"run_time":0.01,"pending_message":null,"exception":null},
            {"id":"7","description":"test7","full_description":"Suite test7","status":"passed","file_path":"./spec/a_spec.rb","line_number":7,"run_time":0.01,"pending_message":null,"exception":null},
            {"id":"8","description":"test8","full_description":"Suite test8","status":"passed","file_path":"./spec/a_spec.rb","line_number":8,"run_time":0.01,"pending_message":null,"exception":null},
            {"id":"9","description":"test9","full_description":"Suite test9","status":"passed","file_path":"./spec/a_spec.rb","line_number":9,"run_time":0.01,"pending_message":null,"exception":null},
            {"id":"10","description":"test10","full_description":"Suite test10","status":"passed","file_path":"./spec/a_spec.rb","line_number":10,"run_time":0.01,"pending_message":null,"exception":null}
          ],
          "summary": {
            "duration": 1.234,
            "example_count": 10,
            "failure_count": 0,
            "pending_count": 0,
            "errors_outside_of_examples_count": 0
          },
          "summary_line": "10 examples, 0 failures"
        }"#
    }

    #[test]
    fn test_filter_rspec_all_pass() {
        let result = filter_rspec_output(all_pass_json());
        assert!(result.starts_with("✓ RSpec:"));
        assert!(result.contains("2 passed"));
        assert!(result.contains("0.01s") || result.contains("0.02s"));
    }

    #[test]
    fn test_filter_rspec_with_failures() {
        let result = filter_rspec_output(with_failures_json());
        assert!(result.contains("1 passed, 1 failed"));
        assert!(result.contains("❌ User saves to database"));
        assert!(result.contains("user_spec.rb:10"));
        assert!(result.contains("ExpectationNotMetError"));
        assert!(result.contains("expected true but got false"));
    }

    #[test]
    fn test_filter_rspec_with_pending() {
        let result = filter_rspec_output(with_pending_json());
        assert!(result.starts_with("✓ RSpec:"));
        assert!(result.contains("1 passed"));
        assert!(result.contains("1 pending"));
    }

    #[test]
    fn test_filter_rspec_empty_output() {
        let result = filter_rspec_output("");
        assert_eq!(result, "RSpec: No output");
    }

    #[test]
    fn test_filter_rspec_no_examples() {
        let json = r#"{
          "version": "3.12.0",
          "examples": [],
          "summary": {
            "duration": 0.001,
            "example_count": 0,
            "failure_count": 0,
            "pending_count": 0,
            "errors_outside_of_examples_count": 0
          }
        }"#;
        let result = filter_rspec_output(json);
        assert_eq!(result, "RSpec: No examples found");
    }

    #[test]
    fn test_filter_rspec_errors_outside_examples() {
        let json = r#"{
          "version": "3.12.0",
          "examples": [],
          "summary": {
            "duration": 0.01,
            "example_count": 0,
            "failure_count": 0,
            "pending_count": 0,
            "errors_outside_of_examples_count": 1
          }
        }"#;
        let result = filter_rspec_output(json);
        // Should NOT say "No examples found" — there was an error outside examples
        assert!(
            !result.contains("No examples found"),
            "errors outside examples should not be treated as 'no examples': {}",
            result
        );
    }

    #[test]
    fn test_filter_rspec_text_fallback() {
        let text = r#"
..F.

Failures:

  1) User is valid
     Failure/Error: expect(user).to be_valid
       expected true got false
     # ./spec/models/user_spec.rb:5

4 examples, 1 failure
"#;
        let result = filter_rspec_output(text);
        assert!(result.contains("RSpec:"));
        assert!(result.contains("4 examples, 1 failure"));
        assert!(result.contains("❌"), "should show failure marker");
    }

    #[test]
    fn test_filter_rspec_text_fallback_extracts_failures() {
        let text = r#"Randomized with seed 12345
..F...E..

Failures:

  1) User#full_name returns first and last name
     Failure/Error: expect(user.full_name).to eq("John Doe")
       expected: "John Doe"
            got: "John D."
     # /usr/local/lib/ruby/gems/3.2.0/gems/rspec-expectations-3.12.0/lib/rspec/expectations/fail_with.rb:37
     # ./spec/models/user_spec.rb:15

  2) Api::Controller#index fails
     Failure/Error: get :index
       expected 200 got 500
     # ./spec/controllers/api_spec.rb:42

9 examples, 2 failures
"#;
        let result = filter_rspec_text(text);
        assert!(result.contains("2 failures"));
        assert!(result.contains("❌"));
        // Should show spec file path, not gem backtrace
        assert!(result.contains("spec/models/user_spec.rb:15"));
    }

    #[test]
    fn test_filter_rspec_backtrace_filters_gems() {
        let result = filter_rspec_output(with_failures_json());
        // Should show the spec file backtrace, not the gem one
        assert!(result.contains("user_spec.rb:11"));
        assert!(!result.contains("gems/rspec-expectations"));
    }

    #[test]
    fn test_filter_rspec_exception_class_shortened() {
        let result = filter_rspec_output(with_failures_json());
        // Should show "ExpectationNotMetError" not "RSpec::Expectations::ExpectationNotMetError"
        assert!(result.contains("ExpectationNotMetError"));
        assert!(!result.contains("RSpec::Expectations::ExpectationNotMetError"));
    }

    #[test]
    fn test_filter_rspec_many_failures_caps_at_five() {
        let json = r#"{
          "version": "3.12.0",
          "examples": [
            {"id":"1","description":"test 1","full_description":"A test 1","status":"failed","file_path":"./spec/a_spec.rb","line_number":5,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 1","backtrace":["./spec/a_spec.rb:6:in `block'"]}},
            {"id":"2","description":"test 2","full_description":"A test 2","status":"failed","file_path":"./spec/a_spec.rb","line_number":10,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 2","backtrace":["./spec/a_spec.rb:11:in `block'"]}},
            {"id":"3","description":"test 3","full_description":"A test 3","status":"failed","file_path":"./spec/a_spec.rb","line_number":15,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 3","backtrace":["./spec/a_spec.rb:16:in `block'"]}},
            {"id":"4","description":"test 4","full_description":"A test 4","status":"failed","file_path":"./spec/a_spec.rb","line_number":20,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 4","backtrace":["./spec/a_spec.rb:21:in `block'"]}},
            {"id":"5","description":"test 5","full_description":"A test 5","status":"failed","file_path":"./spec/a_spec.rb","line_number":25,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 5","backtrace":["./spec/a_spec.rb:26:in `block'"]}},
            {"id":"6","description":"test 6","full_description":"A test 6","status":"failed","file_path":"./spec/a_spec.rb","line_number":30,"run_time":0.001,"pending_message":null,"exception":{"class":"RuntimeError","message":"boom 6","backtrace":["./spec/a_spec.rb:31:in `block'"]}}
          ],
          "summary": {
            "duration": 0.05,
            "example_count": 6,
            "failure_count": 6,
            "pending_count": 0,
            "errors_outside_of_examples_count": 0
          },
          "summary_line": "6 examples, 6 failures"
        }"#;
        let result = filter_rspec_output(json);
        assert!(result.contains("1. ❌"), "should show first failure");
        assert!(result.contains("5. ❌"), "should show fifth failure");
        assert!(!result.contains("6. ❌"), "should not show sixth inline");
        assert!(
            result.contains("+1 more"),
            "should show overflow count: {}",
            result
        );
    }

    #[test]
    fn test_filter_rspec_text_fallback_no_summary() {
        // If no summary line, returns last 5 lines (does not panic)
        let text = "some output\nwithout a summary line";
        let result = filter_rspec_output(text);
        assert!(!result.is_empty());
    }

    #[test]
    fn test_filter_rspec_invalid_json_falls_back() {
        let garbage = "not json at all { broken";
        let result = filter_rspec_output(garbage);
        assert!(!result.is_empty(), "should not panic on invalid JSON");
    }

    // ── Noise stripping tests ────────────────────────────────────────────────

    #[test]
    fn test_strip_noise_spring() {
        let input = "Running via Spring preloader in process 12345\n...\n3 examples, 0 failures";
        let result = strip_noise(input);
        assert!(!result.contains("Spring"));
        assert!(result.contains("3 examples"));
    }

    #[test]
    fn test_strip_noise_simplecov() {
        let input = "...\n\nCoverage report generated for RSpec to /app/coverage.\n142 / 200 LOC (71.0%) covered.\n\n3 examples, 0 failures";
        let result = strip_noise(input);
        assert!(!result.contains("Coverage report"));
        assert!(!result.contains("LOC"));
        assert!(result.contains("3 examples"));
    }

    #[test]
    fn test_strip_noise_deprecation() {
        let input = "DEPRECATION WARNING: Using `return` in before callbacks is deprecated.\n...\n3 examples, 0 failures";
        let result = strip_noise(input);
        assert!(!result.contains("DEPRECATION"));
        assert!(result.contains("3 examples"));
    }

    #[test]
    fn test_strip_noise_finished_in() {
        let input = "...\nFinished in 12.34 seconds (files took 3.21 seconds to load)\n3 examples, 0 failures";
        let result = strip_noise(input);
        assert!(!result.contains("Finished in 12.34"));
        assert!(result.contains("3 examples"));
    }

    #[test]
    fn test_strip_noise_capybara_screenshot() {
        let input = "...\n     saved screenshot to /tmp/capybara/screenshots/2026_failed.png\n3 examples, 1 failure";
        let result = strip_noise(input);
        assert!(result.contains("[screenshot:"));
        assert!(result.contains("failed.png"));
        assert!(!result.contains("saved screenshot to"));
    }

    // ── Token savings tests ──────────────────────────────────────────────────

    #[test]
    fn test_token_savings_all_pass() {
        let input = large_suite_json();
        let output = filter_rspec_output(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "RSpec all-pass: expected ≥60% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_token_savings_with_failures() {
        let input = with_failures_json();
        let output = filter_rspec_output(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "RSpec failures: expected ≥60% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_token_savings_text_fallback() {
        let input = r#"Running via Spring preloader in process 12345
Randomized with seed 54321
..F...E..F..

Failures:

  1) User#full_name returns first and last name
     Failure/Error: expect(user.full_name).to eq("John Doe")
       expected: "John Doe"
            got: "John D."
     # /usr/local/lib/ruby/gems/3.2.0/gems/rspec-expectations-3.12.0/lib/rspec/expectations/fail_with.rb:37
     # ./spec/models/user_spec.rb:15
     # /usr/local/lib/ruby/gems/3.2.0/gems/rspec-core-3.12.0/lib/rspec/core/example.rb:258

  2) Api::Controller#index returns success
     Failure/Error: get :index
       expected 200 got 500
     # /usr/local/lib/ruby/gems/3.2.0/gems/rspec-expectations-3.12.0/lib/rspec/expectations/fail_with.rb:37
     # ./spec/controllers/api_spec.rb:42
     # /usr/local/lib/ruby/gems/3.2.0/gems/rspec-core-3.12.0/lib/rspec/core/example.rb:258

Failed examples:

rspec ./spec/models/user_spec.rb:15 # User#full_name returns first and last name
rspec ./spec/controllers/api_spec.rb:42 # Api::Controller#index returns success

12 examples, 2 failures

Coverage report generated for RSpec to /app/coverage.
142 / 200 LOC (71.0%) covered.
"#;
        let output = filter_rspec_text(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 30.0,
            "RSpec text fallback: expected ≥30% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── ANSI handling tests ────────────────────────────────────────────────

    #[test]
    fn test_filter_rspec_ansi_wrapped_json() {
        // ANSI codes around JSON should fall back to text, not panic
        let input = "\x1b[32m{\"version\":\"3.12.0\"\x1b[0m broken json";
        let result = filter_rspec_output(input);
        assert!(!result.is_empty(), "should not panic on ANSI-wrapped JSON");
    }

    // ── Text fallback >5 failures truncation (Issue 9) ─────────────────────

    #[test]
    fn test_filter_rspec_text_many_failures_caps_at_five() {
        let text = r#"Randomized with seed 12345
.......FFFFFFF

Failures:

  1) User#full_name fails
     Failure/Error: expect(true).to eq(false)
     # ./spec/models/user_spec.rb:5

  2) Post#title fails
     Failure/Error: expect(true).to eq(false)
     # ./spec/models/post_spec.rb:10

  3) Comment#body fails
     Failure/Error: expect(true).to eq(false)
     # ./spec/models/comment_spec.rb:15

  4) Session#token fails
     Failure/Error: expect(true).to eq(false)
     # ./spec/models/session_spec.rb:20

  5) Profile#avatar fails
     Failure/Error: expect(true).to eq(false)
     # ./spec/models/profile_spec.rb:25

  6) Team#members fails
     Failure/Error: expect(true).to eq(false)
     # ./spec/models/team_spec.rb:30

  7) Role#permissions fails
     Failure/Error: expect(true).to eq(false)
     # ./spec/models/role_spec.rb:35

14 examples, 7 failures
"#;
        let result = filter_rspec_text(text);
        assert!(result.contains("1. ❌"), "should show first failure");
        assert!(result.contains("5. ❌"), "should show fifth failure");
        assert!(!result.contains("6. ❌"), "should not show sixth inline");
        assert!(
            result.contains("+2 more"),
            "should show overflow count: {}",
            result
        );
    }

    // ── Header -> FailedExamples transition (Issue 13) ──────────────────────

    #[test]
    fn test_filter_rspec_text_header_to_failed_examples() {
        // Input that has "Failed examples:" directly (no "Failures:" block),
        // followed by a summary line
        let text = r#"..F..

Failed examples:

rspec ./spec/models/user_spec.rb:5 # User is valid

5 examples, 1 failure
"#;
        let result = filter_rspec_text(text);
        assert!(
            result.contains("5 examples, 1 failure"),
            "should contain summary: {}",
            result
        );
        assert!(
            result.contains("RSpec:"),
            "should have RSpec prefix: {}",
            result
        );
    }

    // ── Format flag detection tests (from PR #534) ───────────────────────

    #[test]
    fn test_has_format_flag_none() {
        let args: &[String] = &[];
        assert!(!args.iter().any(|a| {
            a == "--format"
                || a == "-f"
                || a.starts_with("--format=")
                || (a.starts_with("-f") && a.len() > 2 && !a.starts_with("--"))
        }));
    }

    #[test]
    fn test_has_format_flag_long() {
        let args = ["--format".to_string(), "documentation".to_string()];
        assert!(args.iter().any(|a| a == "--format"));
    }

    #[test]
    fn test_has_format_flag_short_combined() {
        // -fjson, -fj, -fdocumentation
        for flag in &["-fjson", "-fj", "-fdocumentation"] {
            let args = [flag.to_string()];
            assert!(
                args.iter()
                    .any(|a| a.starts_with("-f") && a.len() > 2 && !a.starts_with("--")),
                "should detect {}",
                flag
            );
        }
    }

    #[test]
    fn test_has_format_flag_equals() {
        let args = ["--format=json".to_string()];
        assert!(args.iter().any(|a| a.starts_with("--format=")));
    }
}
