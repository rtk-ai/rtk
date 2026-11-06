//! Filters gotestsum output by asking it for `--format standard-json` and reusing the `go test` NDJSON filter.

use crate::core::arg_tokenizer::{self, Dialect, TokenKind, ValueSpec};
use crate::core::args_utils;
use crate::core::runner;
use crate::core::shell::display_args;
use crate::core::utils::resolved_command;
use crate::go_cmd;
use anyhow::Result;
use std::ffi::OsString;

/// gotestsum's own flags that take a value (from `gotestsum --help`, v1.13).
/// Everything after `--` belongs to `go test` and is not classified.
fn takes_value(kind: TokenKind, name: &str) -> Option<ValueSpec> {
    match kind {
        TokenKind::Long => match name {
            // `--rerun-fails` is `int[=2]`: the count can only be attached.
            "rerun-fails" => Some(ValueSpec::attached_only()),
            "format"
            | "format-icons"
            | "hide-summary"
            | "jsonfile"
            | "jsonfile-timing-events"
            | "junitfile"
            | "junitfile-project-name"
            | "junitfile-testcase-classname"
            | "junitfile-testsuite-name"
            | "max-fails"
            | "packages"
            | "post-run-command"
            | "rerun-fails-max-failures"
            | "rerun-fails-report" => Some(ValueSpec::value()),
            _ => None,
        },
        TokenKind::Short => (name == "f").then(ValueSpec::value),
        _ => None,
    }
}

/// Invocations whose output we must not touch: an explicit `--format` (the user wants that layout),
/// watch mode (never exits), subcommands such as `gotestsum tool slowest`, and help/version.
fn is_passthrough(args: &[String]) -> bool {
    if matches!(
        args.first().map(String::as_str),
        Some("tool" | "help" | "completion")
    ) {
        return true;
    }
    let tokens = arg_tokenizer::tokenize_grammar(args, &takes_value, Dialect::Posix);
    let own = arg_tokenizer::before_dashdash(&tokens);
    own.iter().any(|t| match t.kind {
        TokenKind::Long => matches!(t.text, "format" | "watch" | "help" | "version"),
        TokenKind::Short => matches!(t.text, "f" | "h"),
        _ => false,
    })
}

/// With `--rerun-fails` the stream holds every attempt, so a test that failed and then passed on
/// rerun still shows as failed while gotestsum exits 0.
const RERUN_NOTE: &str = "note: --rerun-fails is on; failures above include attempts that may have passed on rerun, \
the exit code is the final result";

fn reruns_failures(args: &[String]) -> bool {
    let tokens = arg_tokenizer::tokenize_grammar(args, &takes_value, Dialect::Posix);
    arg_tokenizer::before_dashdash(&tokens)
        .iter()
        .any(|t| t.kind == TokenKind::Long && t.text == "rerun-fails")
}

fn with_rerun_note(filtered: String) -> String {
    format!("{}\n{RERUN_NOTE}", filtered.trim_end())
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let args = &args_utils::restore_double_dash(args);

    if is_passthrough(args) {
        let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough("gotestsum", &os_args, verbose);
    }

    let rerun = reruns_failures(args);
    let mut cmd = resolved_command("gotestsum");
    // gotestsum's own flags go before `--`, so prepending is always safe (unlike `go test -C`).
    cmd.args(["--format", "standard-json"]);
    cmd.args(args);

    if verbose > 0 {
        eprintln!(
            "Running: gotestsum --format standard-json {}",
            args.join(" ")
        );
    }

    // standard-json prints the `go test -json` stream followed by gotestsum's text summary;
    // the go test filter skips non-JSON lines.
    runner::run_filtered(
        cmd,
        "gotestsum",
        &display_args(args),
        |stdout| {
            let filtered = go_cmd::filter_go_test_json(stdout);
            if rerun && filtered.contains("failed") {
                with_rerun_note(filtered)
            } else {
                filtered
            }
        },
        runner::RunOptions::stdout_only().tee("gotestsum"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn test_filtered_invocations() {
        for case in [
            &[][..],
            &["./..."],
            &["--", "-race", "./..."],
            &["--junitfile", "report.xml", "--", "./..."],
            &["--rerun-fails", "--packages", "./...", "--", "-count=1"],
            // `-f` after `--` is a go test flag, not gotestsum's format.
            &["--", "-f", "x"],
        ] {
            assert!(!is_passthrough(&args(case)), "{case:?} should be filtered");
        }
    }

    #[test]
    fn test_passthrough_invocations() {
        for case in [
            &["--format", "testname", "./..."][..],
            &["--format=dots"],
            &["-f", "pkgname"],
            &["-fdots"],
            &["--watch"],
            &["--help"],
            &["--version"],
            &["tool", "slowest", "--jsonfile", "x.json"],
        ] {
            assert!(is_passthrough(&args(case)), "{case:?} should pass through");
        }
    }

    #[test]
    fn test_rerun_fails_detection() {
        assert!(reruns_failures(&args(&[
            "--rerun-fails",
            "--packages",
            "./..."
        ])));
        assert!(reruns_failures(&args(&["--rerun-fails=3"])));
        assert!(!reruns_failures(&args(&["--rerun-fails-report", "r.txt"])));
        assert!(!reruns_failures(&args(&["--", "-run", "x"])));
        assert!(
            with_rerun_note("1 failed\n".into()).ends_with("the exit code is the final result")
        );
    }

    #[test]
    fn test_real_standard_json_output() {
        let raw = include_str!("../../../tests/fixtures/gotestsum_standard_json_raw.txt");
        let filtered = go_cmd::filter_go_test_json(raw);

        assert!(
            filtered.contains("9 passed, 1 failed, 1 skipped in 3 packages"),
            "{filtered}"
        );
        assert!(filtered.contains("TestFail"));
        assert!(filtered.contains("want 1, got 2"));
        // gotestsum's own text summary must not leak through.
        assert!(!filtered.contains("DONE 11 tests"));

        let savings =
            100.0 - estimate_tokens(&filtered) as f64 / estimate_tokens(raw) as f64 * 100.0;
        assert!(
            savings >= 60.0,
            "expected >= 60% savings, got {savings:.1}%"
        );
    }
}
