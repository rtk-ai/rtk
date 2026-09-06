use crate::core::tracking;
use anyhow::{Context, Result};
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("mix");
    cmd.args(args);

    let output = cmd.output().context("Failed to execute mix")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);

    if !output.status.success() {
        // Many mix commands (credo, dialyzer, etc.) write their report to stdout
        // even on non-zero exit codes, so we must print both streams.
        let filtered = filter_mix_output(&stdout, args, verbose);
        if !filtered.trim().is_empty() {
            println!("{}", filtered);
        }
        if !stderr.trim().is_empty() {
            eprintln!("{}", stderr);
        }
        // Track before exiting so failed commands still appear in rtk gain
        timer.track(
            &format!("mix {}", args.join(" ")),
            &format!("rtk mix {}", args.join(" ")),
            &stdout,
            &filtered,
        );
        return Ok(output.status.code().unwrap_or(1));
    }

    let filtered = filter_mix_output(&stdout, args, verbose);
    println!("{}", filtered);

    timer.track(
        &format!("mix {}", args.join(" ")),
        &format!("rtk mix {}", args.join(" ")),
        &stdout,
        &filtered,
    );

    Ok(0)
}

fn filter_mix_output(stdout: &str, args: &[String], verbose: u8) -> String {
    if verbose >= 3 {
        return stdout.to_string();
    }

    let cmd = args.first().map(|s| s.as_str()).unwrap_or("");

    // Check for compound command names (e.g., "ash.codegen", "ash_postgres.generate_migrations")
    let is_codegen = cmd == "ash.codegen" || cmd == "ash_postgres.generate_migrations";
    let is_credo = cmd == "credo";
    let is_compile = cmd == "compile";
    let is_test = cmd == "test";

    match cmd {
        "phx.routes" => filter_routes(stdout),
        "help" => filter_help(stdout),
        _ if is_codegen => filter_codegen(stdout, args),
        _ if is_credo => filter_credo(stdout, verbose),
        _ if is_compile => filter_compile(stdout),
        _ if is_test => filter_test(stdout, verbose),
        _ => stdout.to_string(),
    }
}

/// Filter ash.codegen output — strip JSON snapshots, keep migration SQL and file paths.
fn filter_codegen(stdout: &str, args: &[String]) -> String {
    let is_check = args.iter().any(|a| a == "--check");

    // For --check, just show the summary
    if is_check {
        let mut result = Vec::new();
        for line in stdout.lines() {
            let trimmed = line.trim();
            // Keep "Running codegen for ..." lines (1 line each)
            // Keep "Pending Code Generation" or similar error lines
            if trimmed.starts_with("Running codegen for")
                || trimmed.starts_with("Getting extensions")
                || trimmed.contains("Pending Code Generation")
                || trimmed.contains("Code generation is up to date")
                || trimmed.starts_with("Compiling")
                || trimmed.starts_with("Generated")
            {
                result.push(trimmed.to_string());
            }
        }
        if result.is_empty() {
            return "ash.codegen --check: ok".to_string();
        }
        return result.join("\n");
    }

    // For --dry-run or regular codegen, keep file paths and migration SQL,
    // strip the JSON resource snapshots entirely.
    let mut result = Vec::new();
    let mut in_json = false;
    let mut json_file: Option<String> = None;
    for line in stdout.lines() {
        let trimmed = line.trim();

        // Detect start of a JSON snapshot block (path ending in .json followed by {)
        if trimmed.ends_with(".json") && !in_json {
            json_file = Some(trimmed.to_string());
            continue;
        }

        if json_file.is_some() && trimmed == "{" {
            in_json = true;
            continue;
        }

        if in_json {
            if trimmed == "}" {
                // End of JSON block — emit a summary line instead
                if let Some(ref path) = json_file {
                    result.push(format!("  snapshot: {} (contents stripped)", path));
                }
                in_json = false;
                json_file = None;
            }
            // Skip all JSON content
            continue;
        }

        // Track created files
        if trimmed.starts_with("* creating") {
            result.push(trimmed.to_string());
            continue;
        }

        // Keep status lines, migration content, and file paths
        if trimmed.starts_with("Running codegen for")
            || trimmed.starts_with("Getting extensions")
            || trimmed.starts_with("Compiling")
            || trimmed.starts_with("Generated")
            || trimmed.ends_with(".exs")
            || trimmed.starts_with("defmodule")
            || trimmed.starts_with("use ")
            || trimmed.starts_with("def up")
            || trimmed.starts_with("def down")
            || trimmed.starts_with("alter ")
            || trimmed.starts_with("create ")
            || trimmed.starts_with("add ")
            || trimmed.starts_with("remove ")
            || trimmed.starts_with("end")
            || trimmed.starts_with("modify ")
            || trimmed.starts_with("rename ")
            || trimmed.starts_with("drop ")
            || trimmed.is_empty()
        {
            result.push(line.to_string());
        }
    }

    // Trim consecutive blank lines
    let mut final_result = Vec::new();
    let mut prev_blank = false;
    for line in &result {
        if line.trim().is_empty() {
            if !prev_blank {
                final_result.push(line.clone());
            }
            prev_blank = true;
        } else {
            final_result.push(line.clone());
            prev_blank = false;
        }
    }

    final_result.join("\n")
}

/// Filter credo output — show summary + only warnings/errors, skip design/readability suggestions.
fn filter_credo(stdout: &str, verbose: u8) -> String {
    // At verbose >= 2, show everything
    if verbose >= 2 {
        return stdout.to_string();
    }

    let lines: Vec<&str> = stdout.lines().collect();
    let mut result = Vec::new();
    let mut skipped_count: usize = 0;
    let mut in_low_priority_block = false;

    // Credo uses priority arrows:
    //   ↑ = high, ↗ = medium-high, → = normal  (actionable — keep these)
    //   ↘ = low, ↓ = very low                   (suggestions — skip these)
    // We only keep lines with ↑ ↗ → arrows, plus non-issue lines (headers, footers).

    for line in &lines {
        let trimmed = line.trim();

        // Issue lines live inside ┃ blocks
        if trimmed.starts_with('┃') {
            // Check if this line starts a new issue (has a severity marker)
            let is_new_issue = trimmed.contains("[D]")
                || trimmed.contains("[R]")
                || trimmed.contains("[C]")
                || trimmed.contains("[W]")
                || trimmed.contains("[F]");

            if is_new_issue {
                // Keep issues with high-priority arrows, skip low-priority ones
                if trimmed.contains('↑') || trimmed.contains('↗') || trimmed.contains('→') {
                    in_low_priority_block = false;
                    result.push(line.to_string());
                } else {
                    // ↘ or ↓ — skip this issue and its continuation lines
                    in_low_priority_block = true;
                    skipped_count += 1;
                }
            } else {
                // Continuation line (file path, details) — follows the previous issue's fate
                if !in_low_priority_block {
                    result.push(line.to_string());
                }
            }
            continue;
        }

        // Non-┃ lines: headers, footers, blank lines, category names
        in_low_priority_block = false;
        result.push(line.to_string());
    }

    // Append a note about skipped low-priority issues
    if skipped_count > 0 {
        result.push(format!(
            "\n({} low-priority suggestions hidden, use -vv to see all)",
            skipped_count
        ));
    }

    result.join("\n")
}

/// Filter compile output — keep only errors, warnings, and summary.
fn filter_compile(stdout: &str) -> String {
    let lines: Vec<&str> = stdout.lines().collect();

    // If output is short (< 10 lines), pass through as-is
    if lines.len() < 10 {
        return stdout.to_string();
    }

    let mut result = Vec::new();
    let mut in_warning = false;
    let mut in_error = false;

    for line in &lines {
        let trimmed = line.trim();

        // Always keep these status lines
        if trimmed.starts_with("Compiling")
            || trimmed.starts_with("Generated")
            || trimmed.starts_with("==>")
        {
            result.push(line.to_string());
            in_warning = false;
            in_error = false;
            continue;
        }

        // Detect warning/error blocks
        if trimmed.starts_with("warning:") {
            in_warning = true;
            in_error = false;
            result.push(line.to_string());
            continue;
        }

        if trimmed.starts_with("error:") || trimmed.starts_with("** (") {
            in_error = true;
            in_warning = false;
            result.push(line.to_string());
            continue;
        }

        // Continuation of warning/error block (indented or file reference)
        if (in_warning || in_error)
            && (trimmed.starts_with('│')
                || trimmed.starts_with('└')
                || trimmed.starts_with("~")
                || trimmed.contains(".ex:")
                || trimmed.contains(".exs:")
                || trimmed.is_empty())
        {
            result.push(line.to_string());
            if trimmed.is_empty() {
                in_warning = false;
                in_error = false;
            }
            continue;
        }

        // Reset if we hit a non-continuation line
        if in_warning || in_error {
            in_warning = false;
            in_error = false;
        }

        // Skip "Compiling N files" noise lines for dependencies
        // and other informational output
    }

    if result.is_empty() {
        return "compile: ok".to_string();
    }

    result.join("\n")
}

fn filter_routes(stdout: &str) -> String {
    let lines: Vec<&str> = stdout.lines().collect();
    if lines.len() <= 10 {
        return stdout.to_string();
    }

    let mut result = Vec::new();
    result.push(lines[0].to_string()); // Header

    // Phoenix routes output usually looks like:
    //   page_path  GET  /  PageController :index

    for line in lines.iter().skip(1) {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 3 {
            // Keep it simple but compact
            result.push(line.trim().to_string());
        }
    }

    let count = result.len();
    if count > 100 {
        result.truncate(50);
        result.push("...".to_string());
        result.push(format!("({} total routes)", count));
    }

    result.join("\n")
}

fn filter_help(stdout: &str) -> String {
    // Only keep the first paragraph of mix help
    let mut result = Vec::new();
    for line in stdout.lines() {
        if line.trim().is_empty() && !result.is_empty() {
            break;
        }
        result.push(line.to_string());
    }
    result.join("\n")
}

/// Filter mix test output — show failures with full context, compact summary for passes.
fn filter_test(stdout: &str, verbose: u8) -> String {
    // At verbose >= 2, show everything
    if verbose >= 2 {
        return stdout.to_string();
    }

    let lines: Vec<&str> = stdout.lines().collect();
    let mut result = Vec::new();
    let mut in_failure = false;
    let mut has_failures = false;

    for line in &lines {
        let trimmed = line.trim();

        // Always keep the summary line (e.g., "5 tests, 1 failure")
        if trimmed.contains(" test,") || trimmed.contains(" tests,") {
            result.push(line.to_string());
            continue;
        }

        // Always keep "Finished in" timing line
        if trimmed.starts_with("Finished in") {
            result.push(line.to_string());
            continue;
        }

        // Keep seed for reproducibility
        if trimmed.starts_with("Randomized with seed") {
            result.push(line.to_string());
            continue;
        }

        // Detect failure block start
        if trimmed.starts_with("1)")
            || trimmed.starts_with("2)")
            || trimmed.starts_with("3)")
            || trimmed.starts_with("4)")
            || trimmed.starts_with("5)")
            || trimmed.starts_with("6)")
            || trimmed.starts_with("7)")
            || trimmed.starts_with("8)")
            || trimmed.starts_with("9)")
        {
            // Check if this is a failure header (e.g., "1) test something (MyApp.SomeTest)")
            if trimmed.contains("test ") {
                in_failure = true;
                has_failures = true;
                result.push(line.to_string());
                continue;
            }
        }

        // Keep all lines within a failure block
        if in_failure {
            result.push(line.to_string());
            // Blank line ends the failure block
            if trimmed.is_empty() {
                in_failure = false;
            }
            continue;
        }

        // Keep compilation warnings/errors
        if trimmed.starts_with("warning:")
            || trimmed.starts_with("error:")
            || trimmed.starts_with("** (")
        {
            result.push(line.to_string());
            continue;
        }

        // Keep "Compiling" status for context
        if trimmed.starts_with("Compiling") || trimmed.starts_with("Generated") {
            result.push(line.to_string());
            continue;
        }

        // Keep ExUnit failure markers
        if trimmed.starts_with("Failures:") || trimmed.starts_with("failures:") {
            result.push(line.to_string());
            continue;
        }

        // Skip dot progress lines (e.g., "....F.....")
        // Skip "Running ExUnit" noise
        // Skip individual test pass lines
    }

    if !has_failures && result.is_empty() {
        return stdout.to_string();
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── mix test: all passing ──────────────────────────────────────────

    #[test]
    fn test_filter_test_all_pass() {
        let output = r#"Compiling 3 files (.ex)
Generated my_app app
......................

Finished in 0.3 seconds (0.1s async, 0.2s sync)
22 tests, 0 failures

Randomized with seed 12345"#;

        let result = filter_test(output, 0);
        assert!(result.contains("22 tests, 0 failures"), "must keep summary");
        assert!(result.contains("Finished in"), "must keep timing");
        assert!(result.contains("Randomized with seed"), "must keep seed");
        assert!(!result.contains("...."), "must strip dot progress");
        // Token savings: stripped dots and noise
        assert!(
            result.len() < output.len(),
            "filtered output must be shorter"
        );
    }

    // ── mix test: with failures ────────────────────────────────────────

    #[test]
    fn test_filter_test_with_failures() {
        let output = r#"Compiling 1 file (.ex)
...F..

Failures:

  1) test greets the world (MyAppTest)
     test/my_app_test.exs:5
     Assertion with == failed
     code:  assert MyApp.hello() == :world
     left:  :ok
     right: :world
     stacktrace:
       test/my_app_test.exs:6: (test)

Finished in 0.1 seconds (0.05s async, 0.05s sync)
6 tests, 1 failure

Randomized with seed 54321"#;

        let result = filter_test(output, 0);
        // Must preserve failure context
        assert!(result.contains("Failures:"), "must keep Failures header");
        assert!(
            result.contains("test greets the world"),
            "must keep failing test name"
        );
        assert!(result.contains("my_app_test.exs:5"), "must keep file:line");
        assert!(
            result.contains("Assertion with == failed"),
            "must keep assertion message"
        );
        assert!(result.contains("left:  :ok"), "must keep left value");
        assert!(result.contains("right: :world"), "must keep right value");
        assert!(result.contains("6 tests, 1 failure"), "must keep summary");
        assert!(result.contains("Randomized with seed"), "must keep seed");
        // Must strip noise
        assert!(!result.contains("...F.."), "must strip dot progress");
    }

    #[test]
    fn test_filter_test_multiple_failures() {
        let output = r#"..F.F

Failures:

  1) test addition (MathTest)
     test/math_test.exs:10
     Assertion with == failed
     code:  assert Math.add(1, 1) == 3
     left:  2
     right: 3

  2) test subtraction (MathTest)
     test/math_test.exs:15
     Assertion with == failed
     code:  assert Math.sub(5, 3) == 1
     left:  2
     right: 1

Finished in 0.2 seconds (0.1s async, 0.1s sync)
5 tests, 2 failures

Randomized with seed 99999"#;

        let result = filter_test(output, 0);
        assert!(result.contains("test addition"), "must keep first failure");
        assert!(
            result.contains("test subtraction"),
            "must keep second failure"
        );
        assert!(
            result.contains("math_test.exs:10"),
            "must keep first file:line"
        );
        assert!(
            result.contains("math_test.exs:15"),
            "must keep second file:line"
        );
        assert!(result.contains("5 tests, 2 failures"), "must keep summary");
    }

    #[test]
    fn test_filter_test_preserves_compilation_errors() {
        let output = r#"Compiling 1 file (.ex)
warning: variable "x" is unused
  lib/my_app.ex:10

error: undefined function foo/0
  lib/my_app.ex:15

** (CompileError) lib/my_app.ex:15: undefined function foo/0"#;

        let result = filter_test(output, 0);
        assert!(result.contains("warning: variable"), "must keep warnings");
        assert!(
            result.contains("error: undefined function"),
            "must keep errors"
        );
        assert!(
            result.contains("** (CompileError)"),
            "must keep compile errors"
        );
        assert!(result.contains("Compiling"), "must keep compile status");
    }

    #[test]
    fn test_filter_test_verbose_passthrough() {
        let output = "all the raw output\n....\n22 tests, 0 failures";
        let result = filter_test(output, 2);
        assert_eq!(result, output, "verbose >= 2 must passthrough");
    }

    #[test]
    fn test_filter_test_routed_via_filter_mix_output() {
        let output = r#"....

Finished in 0.1 seconds (0.05s async, 0.05s sync)
4 tests, 0 failures

Randomized with seed 12345"#;

        let args = vec!["test".to_string()];
        let result = filter_mix_output(output, &args, 0);
        // Must NOT be raw passthrough — must be filtered
        assert!(
            !result.contains("...."),
            "mix test must be routed through filter_test"
        );
        assert!(result.contains("4 tests, 0 failures"), "must keep summary");
    }

    // ── mix compile ────────────────────────────────────────────────────

    #[test]
    fn test_filter_compile_clean() {
        // Short output (< 10 lines) passes through
        let output = "Compiling 3 files (.ex)\nGenerated my_app app";
        let result = filter_compile(output);
        assert_eq!(result, output, "short output passes through");
    }

    #[test]
    fn test_filter_compile_with_warnings() {
        let output = r#"Compiling 15 files (.ex)
some noise line 1
some noise line 2
some noise line 3
some noise line 4
some noise line 5
some noise line 6
some noise line 7
some noise line 8
warning: variable "x" is unused
  lib/my_app.ex:10

Generated my_app app"#;

        let result = filter_compile(output);
        assert!(
            result.contains("Compiling 15 files"),
            "must keep compile status"
        );
        assert!(result.contains("warning: variable"), "must keep warning");
        assert!(
            result.contains("Generated my_app app"),
            "must keep generated line"
        );
        assert!(!result.contains("some noise"), "must strip noise");
    }

    #[test]
    fn test_filter_compile_with_errors() {
        let output = r#"Compiling 15 files (.ex)
noise 1
noise 2
noise 3
noise 4
noise 5
noise 6
noise 7
noise 8
error: undefined function foo/0
  lib/my_app.ex:15
** (CompileError) lib/my_app.ex:15: undefined function foo/0"#;

        let result = filter_compile(output);
        assert!(
            result.contains("error: undefined function"),
            "must keep error"
        );
        assert!(
            result.contains("** (CompileError)"),
            "must keep compile error"
        );
        assert!(!result.contains("noise"), "must strip noise");
    }

    #[test]
    fn test_filter_compile_empty_output() {
        let output = r#"noise 1
noise 2
noise 3
noise 4
noise 5
noise 6
noise 7
noise 8
noise 9
noise 10"#;

        let result = filter_compile(output);
        assert_eq!(
            result, "compile: ok",
            "no errors/warnings produces compact summary"
        );
    }

    // ── mix credo ──────────────────────────────────────────────────────

    #[test]
    fn test_filter_credo_keeps_high_priority() {
        let output = r#"
Checking 42 source files...

Code Readability

┃ [R] ↑ lib/my_app.ex:10:11 Modules should have a @moduledoc tag.
┃     lib/my_app.ex:10
"#;

        let result = filter_credo(output, 0);
        assert!(result.contains("↑"), "must keep high-priority issues");
        assert!(result.contains("@moduledoc"), "must keep issue text");
        assert!(
            result.contains("lib/my_app.ex:10"),
            "must keep file location"
        );
    }

    #[test]
    fn test_filter_credo_hides_low_priority() {
        let output = r#"
Code Readability

┃ [R] ↓ lib/my_app.ex:10:11 Modules should have a @moduledoc tag.
┃     lib/my_app.ex:10
┃ [D] ↘ lib/my_app.ex:20:5 Use pipe operator for readability.
┃     lib/my_app.ex:20
"#;

        let result = filter_credo(output, 0);
        assert!(
            !result.contains("@moduledoc"),
            "must hide low-priority issue text"
        );
        assert!(
            !result.contains("pipe operator"),
            "must hide low-priority issue text"
        );
        assert!(
            result.contains("2 low-priority suggestions hidden"),
            "must show skip count"
        );
    }

    #[test]
    fn test_filter_credo_mixed_priorities() {
        let output = r#"
Code Readability

┃ [W] → lib/my_app.ex:5:1 Function body too long.
┃     lib/my_app.ex:5
┃ [D] ↘ lib/my_app.ex:20:5 Use pipe operator.
┃     lib/my_app.ex:20
"#;

        let result = filter_credo(output, 0);
        assert!(
            result.contains("Function body too long"),
            "must keep normal-priority"
        );
        assert!(!result.contains("pipe operator"), "must hide low-priority");
        assert!(
            result.contains("1 low-priority suggestions hidden"),
            "must count skipped"
        );
    }

    #[test]
    fn test_filter_credo_verbose_passthrough() {
        let output = "┃ [D] ↘ low priority stuff";
        let result = filter_credo(output, 2);
        assert_eq!(result, output, "verbose >= 2 must passthrough");
    }

    // ── ash.codegen ────────────────────────────────────────────────────

    #[test]
    fn test_filter_codegen_check_ok() {
        let output = "";
        let args = vec!["ash.codegen".to_string(), "--check".to_string()];
        let result = filter_codegen(output, &args);
        assert_eq!(result, "ash.codegen --check: ok");
    }

    #[test]
    fn test_filter_codegen_check_pending() {
        let output = r#"Running codegen for AshPostgres
Getting extensions for repo
Pending Code Generation detected"#;

        let args = vec!["ash.codegen".to_string(), "--check".to_string()];
        let result = filter_codegen(output, &args);
        assert!(result.contains("Running codegen"), "must keep status");
        assert!(
            result.contains("Pending Code Generation"),
            "must keep pending status"
        );
    }

    #[test]
    fn test_filter_codegen_strips_json_snapshots() {
        let output = r#"Running codegen for AshPostgres
priv/resource_snapshots/repo/users/20240101.json
{
  "attributes": [
    {"name": "id", "type": "uuid"},
    {"name": "email", "type": "string"}
  ]
}
* creating priv/repo/migrations/20240101_create_users.exs"#;

        let args = vec!["ash.codegen".to_string()];
        let result = filter_codegen(output, &args);
        assert!(result.contains("snapshot:"), "must show snapshot summary");
        assert!(
            result.contains("(contents stripped)"),
            "must indicate stripping"
        );
        assert!(
            !result.contains("\"attributes\""),
            "must strip JSON content"
        );
        assert!(
            result.contains("* creating"),
            "must keep file creation lines"
        );
    }

    #[test]
    fn test_filter_codegen_keeps_migration_sql() {
        let output = r#"Running codegen for AshPostgres
priv/repo/migrations/20240101_create_users.exs
defmodule MyApp.Repo.Migrations.CreateUsers do
  use Ecto.Migration

  def up do
    create table(:users) do
      add :email, :string
    end
  end

  def down do
    drop table(:users)
  end
end"#;

        let args = vec!["ash.codegen".to_string()];
        let result = filter_codegen(output, &args);
        assert!(result.contains("defmodule"), "must keep module definition");
        assert!(result.contains("create table(:users)"), "must keep SQL");
        assert!(result.contains("add :email"), "must keep column definition");
        assert!(result.contains("def up"), "must keep up migration");
        assert!(result.contains("def down"), "must keep down migration");
    }

    // ── phx.routes ─────────────────────────────────────────────────────

    #[test]
    fn test_filter_routes_short_passthrough() {
        let output =
            "page_path  GET  /  PageController :index\napi_path  GET  /api  ApiController :index";
        let result = filter_routes(output);
        assert_eq!(result, output, "short routes list passes through");
    }

    #[test]
    fn test_filter_routes_preserves_all_routes() {
        // 15 routes — above threshold but under 100, should all be kept
        let mut lines = vec!["  Method  Path  Controller  Action".to_string()];
        for i in 0..14 {
            lines.push(format!(
                "  route_{}_path  GET  /route_{}  Controller :index",
                i, i
            ));
        }
        let output = lines.join("\n");
        let result = filter_routes(&output);
        assert!(result.contains("route_0_path"), "must keep first route");
        assert!(result.contains("route_13_path"), "must keep last route");
    }

    // ── mix help ───────────────────────────────────────────────────────

    #[test]
    fn test_filter_help_first_paragraph() {
        let output = r#"Mix is a build tool for Elixir.
It provides tasks for creating, compiling, and testing.

## Available tasks

mix compile   # Compiles source files
mix test      # Runs tests
mix deps.get  # Gets all dependencies"#;

        let result = filter_help(output);
        assert!(
            result.contains("Mix is a build tool"),
            "must keep first paragraph"
        );
        assert!(
            !result.contains("Available tasks"),
            "must strip after blank line"
        );
        assert!(!result.contains("mix compile"), "must strip task list");
    }

    #[test]
    fn test_filter_help_single_paragraph() {
        let output = "Mix is a build tool for Elixir.";
        let result = filter_help(output);
        assert_eq!(result, output, "single paragraph passes through");
    }

    // ── filter_mix_output routing ──────────────────────────────────────

    #[test]
    fn test_filter_mix_output_verbose_passthrough() {
        let output = "raw output";
        let args = vec!["test".to_string()];
        let result = filter_mix_output(output, &args, 3);
        assert_eq!(result, output, "verbose >= 3 must passthrough all commands");
    }

    #[test]
    fn test_filter_mix_output_routes_compile_credo() {
        // Verify routing works for each command
        let args_compile = vec!["compile".to_string()];
        let args_credo = vec!["credo".to_string()];
        let args_help = vec!["help".to_string()];

        // These just need to not panic — actual filtering tested above
        let _ = filter_mix_output("short compile output", &args_compile, 0);
        let _ = filter_mix_output("credo output", &args_credo, 0);
        let _ = filter_mix_output("help output\n\nmore help", &args_help, 0);
    }
}
