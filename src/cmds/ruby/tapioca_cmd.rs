//! Tapioca RBI generator filter.
//!
//! Strips verbose compile/fetch progress lines and keeps only errors,
//! warnings, and the final Done summary line.

use crate::core::tracking;
use crate::core::utils::{exit_code_from_output, fallback_tail, ruby_exec, strip_ansi};
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref RE_SPRING: Regex = Regex::new(r"(?i)running via spring preloader").unwrap();
    // Word-boundary match: avoids false positives like `actionable_error.rbi`
    // `errors?` matches both "error" and "errors" (e.g. "No errors found")
    static ref RE_ERROR: Regex = Regex::new(r"(?i)\berrors?\b").unwrap();
    static ref RE_WARNING: Regex = Regex::new(r"(?i)\bwarnings?\b").unwrap();
    // `gems` summary is "Checking generated RBI files...  Done" — contains but doesn't start with "Done"
    static ref RE_DONE: Regex = Regex::new(r"\bDone\b").unwrap();
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = ruby_exec("tapioca");
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: tapioca {}", args.join(" "));
    }

    let output = cmd.output().context(
        "Failed to run tapioca. Is it installed? Try: gem install tapioca or add it to your Gemfile",
    )?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = exit_code_from_output(&output, "tapioca");

    let filtered = if stdout.trim().is_empty() && !output.status.success() {
        "Tapioca: FAILED (no stdout, see stderr below)".to_string()
    } else {
        filter_tapioca(&stdout)
    };

    if let Some(hint) = crate::core::tee::tee_and_hint(&raw, "tapioca", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    if !stderr.trim().is_empty() && (!output.status.success() || verbose > 0) {
        eprintln!("{}", stderr.trim());
    }

    timer.track(
        &format!("tapioca {}", args.join(" ")),
        &format!("rtk tapioca {}", args.join(" ")),
        &raw,
        &filtered,
    );

    Ok(exit_code)
}

fn filter_tapioca(output: &str) -> String {
    if output.trim().is_empty() {
        return "tapioca: no output".to_string();
    }

    let clean = strip_ansi(output);

    let kept: Vec<&str> = clean
        .lines()
        .filter(|line| !RE_SPRING.is_match(line))
        .filter(|line| {
            let t = line.trim();
            if t.is_empty() {
                return false;
            }
            RE_ERROR.is_match(t) || RE_WARNING.is_match(t) || RE_DONE.is_match(t)
        })
        .collect();

    if kept.is_empty() {
        return fallback_tail(&clean, "tapioca", 5);
    }

    kept.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::utils::count_tokens;

    fn gems_clean_output() -> &'static str {
        "Removing RBI files of gems that have been removed:\n\
           Nothing to do.\n\
         Generating RBI files of gems that are added or updated:\n\
           Nothing to do.\n\
         Checking generated RBI files...  Done\n\
           No errors found\n\
         All operations performed in working directory.\n\
         Please review changes and commit them."
    }

    fn dsl_clean_output() -> &'static str {
        "Loading DSL extension Tapioca::Dsl::Compilers::ActiveRecordAssociations...\n\
         Loading DSL extension Tapioca::Dsl::Compilers::ActiveRecordColumns...\n\
         Compiling User, this may take a few seconds...\n\
         Compiled User\n\
         Compiling Post, this may take a few seconds...\n\
         Compiled Post\n\
         Done compiling DSL RBI files (2 files in 2.4s)"
    }

    fn annotations_clean_output() -> &'static str {
        "Fetching gem RBI annotations from sorbet-typed...\n\
         Fetching annotations for activesupport (7.0.0)...\n\
         Fetching annotations for addressable (2.8.0)...\n\
         Done fetching RBI annotations (2 gems in 1.5s)"
    }

    fn large_gems_fixture() -> String {
        let gems = [
            "activesupport",
            "addressable",
            "ast",
            "aws-sdk-core",
            "base64",
            "bcrypt",
            "bigdecimal",
            "bootsnap",
            "bundler",
            "capybara",
            "childprocess",
            "concurrent-ruby",
            "connection_pool",
            "crack",
            "crass",
            "date",
            "devise",
            "digest",
            "dry-core",
            "erubi",
            "factory_bot",
            "factory_bot_rails",
            "faker",
            "ffi",
            "globalid",
            "i18n",
            "io-console",
            "irb",
            "json",
            "launchy",
            "loofah",
            "mail",
            "marcel",
            "method_source",
            "mime-types",
            "minitest",
            "msgpack",
            "net-http",
            "nio4r",
            "nokogiri",
            "orm_adapter",
            "pg",
            "propshaft",
            "psych",
            "public_suffix",
            "rack",
            "rack-session",
            "rack-test",
            "rails",
            "railties",
        ];
        let mut lines: Vec<String> = gems
            .iter()
            .flat_map(|g| {
                vec![
                    format!("Compiling {g}, this may take a few seconds..."),
                    format!("Compiled {g}"),
                ]
            })
            .collect();
        lines.push("Done compiling RBI files for gems (50 files in 18.3s)".to_string());
        lines.join("\n")
    }

    #[test]
    fn test_filter_clean_gems_run() {
        let result = filter_tapioca(gems_clean_output());
        assert!(
            result.contains("Done"),
            "Done line should be kept: {result}"
        );
        assert!(
            result.contains("No errors found"),
            "no-errors line should be kept: {result}"
        );
        assert!(
            !result.contains("Nothing to do"),
            "noise should be stripped: {result}"
        );
        assert!(
            !result.contains("Please review"),
            "noise should be stripped: {result}"
        );
    }

    #[test]
    fn test_filter_with_warning() {
        let input = "Compiling foo, this may take a few seconds...\n\
                     Compiled foo\n\
                     Warning: Unresolvable constant 'MyApp::Foo' in sorbet/rbi/gems/foo.rbi\n\
                     Done compiling RBI files for gems (1 file in 1.2s)";
        let result = filter_tapioca(input);
        assert!(
            result.contains("Warning: Unresolvable constant"),
            "warning should be kept: {result}"
        );
        assert!(
            result.contains("Done compiling"),
            "Done line should be kept: {result}"
        );
        assert!(
            !result.contains("Compiling foo"),
            "progress should be stripped: {result}"
        );
        assert!(
            !result.contains("Compiled foo"),
            "compiled line should be stripped: {result}"
        );
    }

    #[test]
    fn test_filter_with_error() {
        let input = "Compiling foo, this may take a few seconds...\n\
                     Error: Gem 'foo' is not installed\n\
                     Done compiling RBI files for gems (0 files in 0.1s)";
        let result = filter_tapioca(input);
        assert!(
            result.contains("Error: Gem 'foo'"),
            "error line should be kept: {result}"
        );
        assert!(
            result.contains("Done compiling"),
            "Done line should be kept: {result}"
        );
    }

    #[test]
    fn test_filter_dsl_run() {
        let result = filter_tapioca(dsl_clean_output());
        assert_eq!(result, "Done compiling DSL RBI files (2 files in 2.4s)");
        assert!(
            !result.contains("Loading DSL"),
            "DSL loading lines should be stripped"
        );
        assert!(
            !result.contains("Compiling User"),
            "Compiling lines should be stripped"
        );
    }

    #[test]
    fn test_filter_annotations_run() {
        let result = filter_tapioca(annotations_clean_output());
        assert_eq!(result, "Done fetching RBI annotations (2 gems in 1.5s)");
        assert!(
            !result.contains("Fetching"),
            "Fetching lines should be stripped"
        );
    }

    #[test]
    fn test_filter_empty_output() {
        let result = filter_tapioca("");
        assert_eq!(result, "tapioca: no output");
    }

    #[test]
    fn test_filter_no_done_line() {
        let input = "Compiling foo, this may take a few seconds...\nCompiled foo\n";
        let result = filter_tapioca(input);
        assert!(
            result.contains("Compiled foo"),
            "fallback should include tail lines: {result}"
        );
    }

    #[test]
    fn test_filter_ansi_done_line() {
        let input = "\x1b[32mDone compiling RBI files for gems (5 files in 2.1s)\x1b[0m";
        let result = filter_tapioca(input);
        assert!(
            result.contains("Done compiling"),
            "ANSI-wrapped Done line should be kept after stripping: {result}"
        );
    }

    #[test]
    fn test_filter_spring_preloader() {
        let input = "Running via Spring preloader in process 12345\n\
                     Compiling foo, this may take a few seconds...\n\
                     Compiled foo\n\
                     Done compiling RBI files for gems (1 file in 1.0s)";
        let result = filter_tapioca(input);
        assert!(
            !result.contains("Spring"),
            "Spring preloader line should be stripped: {result}"
        );
        assert!(
            result.contains("Done compiling"),
            "Done line should be kept: {result}"
        );
    }

    #[test]
    fn test_filter_no_false_positive_error_in_filename() {
        // `actionable_error.rbi` contains "error" as a substring but is not an error
        let input = "Compiling DSL RBI files...\n\
                     identical  sorbet/rbi/dsl/active_support/actionable_error.rbi\n\
                     identical  sorbet/rbi/dsl/active_support/error_reporter.rbi\n\
                     Done compiling DSL RBI files (2 files in 1.2s)";
        let result = filter_tapioca(input);
        assert_eq!(
            result, "Done compiling DSL RBI files (2 files in 1.2s)",
            "filenames containing 'error' should not be kept: {result}"
        );
    }

    #[test]
    fn test_token_savings_gems() {
        let input = large_gems_fixture();
        let output = filter_tapioca(&input);
        let input_tokens = count_tokens(&input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Tapioca gems: expected ≥60% savings, got {:.1}% (in={}, out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    // ── Real fixture tests ───────────────────────────────────────────────────

    #[test]
    fn test_fixture_gems() {
        let input = include_str!("../../../tests/fixtures/tapioca_gems_raw.txt");
        let output = filter_tapioca(input);
        assert!(
            output.contains("Done"),
            "Done line should be kept: {output}"
        );
        assert!(
            output.contains("No errors found"),
            "no-errors line should be kept: {output}"
        );
        assert!(
            !output.contains("Nothing to do"),
            "noise should be stripped: {output}"
        );
        assert!(
            !output.contains("Please review"),
            "noise should be stripped: {output}"
        );
    }

    #[test]
    fn test_fixture_dsl() {
        let input = include_str!("../../../tests/fixtures/tapioca_dsl_raw.txt");
        let output = filter_tapioca(input);
        assert!(
            output.contains("Done"),
            "Done line should be kept: {output}"
        );
        assert!(
            output.contains("WARNING:"),
            "warning should be kept: {output}"
        );
        assert!(
            output.contains("No errors found"),
            "no-errors line should be kept: {output}"
        );
        assert!(
            !output.contains("identical"),
            "progress lines should be stripped: {output}"
        );
    }

    #[test]
    fn test_fixture_token_savings_gems() {
        let input = include_str!("../../../tests/fixtures/tapioca_gems_raw.txt");
        let output = filter_tapioca(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "gems fixture: expected ≥60% savings, got {:.1}%",
            savings
        );
    }

    #[test]
    fn test_fixture_token_savings_dsl() {
        let input = include_str!("../../../tests/fixtures/tapioca_dsl_raw.txt");
        let output = filter_tapioca(input);
        let savings = 100.0 - (count_tokens(&output) as f64 / count_tokens(input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "dsl fixture: expected ≥60% savings, got {:.1}%",
            savings
        );
    }
}
