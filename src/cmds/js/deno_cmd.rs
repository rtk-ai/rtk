//! Filters deno output — lint, check, and task command output.

use crate::core::utils::{join_or_ok, resolved_command, strip_ansi};
use anyhow::Result;
use std::ffi::OsString;

/// Filter deno output: strip ANSI codes, download lines, and empty lines.
pub fn filter_deno_output(output: &str) -> String {
    let cleaned = strip_ansi(output);
    let filtered: Vec<&str> = cleaned
        .lines()
        .filter(|line| {
            let trimmed = line.trim();
            !trimmed.is_empty() && !trimmed.starts_with("Download ")
        })
        .collect();

    join_or_ok(&filtered)
}

/// Run a deno subcommand through the shared core runner, which applies the
/// filter, tee recovery, tracking, and the never_worse output guard.
fn run_filtered_subcmd(subcmd: &str, args: &[String], verbose: u8) -> Result<i32> {
    if crate::core::runner::is_watch_mode(args) {
        return passthrough_subcmd(subcmd, args, verbose);
    }

    let mut cmd = resolved_command("deno");
    cmd.arg(subcmd);
    cmd.args(args);

    if verbose > 0 {
        eprintln!("Running: deno {} {}", subcmd, args.join(" "));
    }

    let display = format!("{} {}", subcmd, args.join(" "));
    let tee_label = format!("deno_{}", subcmd);
    crate::core::runner::run_filtered(
        cmd,
        "deno",
        display.trim_end(),
        filter_deno_output,
        crate::core::runner::RunOptions::with_tee(&tee_label),
    )
}

/// Whether the user asked deno for a specific output format. `--reporter=junit`
/// and `--junit-path -` write machine-readable output to stdout, so filtering it
/// would replace the report with a summary; a format the user named is the one
/// they want, so the run goes through unfiltered.
fn chose_output_format(args: &[String]) -> bool {
    args.iter()
        .any(|a| a == "--reporter" || a.starts_with("--reporter=") || a.starts_with("--junit-path"))
}

/// Run a subcommand unfiltered, keeping the argv rtk would have run.
fn passthrough_subcmd(subcmd: &str, args: &[String], verbose: u8) -> Result<i32> {
    let mut passthrough: Vec<OsString> = vec![OsString::from(subcmd)];
    passthrough.extend(args.iter().map(OsString::from));
    crate::core::runner::run_passthrough("deno", &passthrough, verbose)
}

pub fn run_lint(args: &[String], verbose: u8) -> Result<i32> {
    run_filtered_subcmd("lint", args, verbose)
}

pub fn run_check(args: &[String], verbose: u8) -> Result<i32> {
    run_filtered_subcmd("check", args, verbose)
}

/// Run `deno compile` with error-only filtering. Args are passed as a vector, never via a shell.
pub fn run_compile(args: &[String], verbose: u8) -> Result<i32> {
    // Unfiltered: the emitted binary's path and size are what the run is for,
    // and on failure the type-check diagnostics are the payload. The errors-only
    // filter dropped both, since deno colorizes even when piped.
    passthrough_subcmd("compile", args, verbose)
}

/// Run `deno test` showing only failures. Args are passed as a vector, never via a shell.
pub fn run_test(args: &[String], verbose: u8) -> Result<i32> {
    if crate::core::runner::is_watch_mode(args) || chose_output_format(args) {
        return passthrough_subcmd("test", args, verbose);
    }

    let mut cmd = resolved_command("deno");
    cmd.arg("test").args(args);
    let display = format!("test {}", args.join(" "));
    crate::core::runner::run_test_cmd(
        cmd,
        "deno",
        display.trim_end(),
        "deno_test",
        crate::core::runner::TestEcosystem::Deno,
        verbose,
    )
}

/// Passthrough for `deno run`, `deno task`, and other unfiltered subcommands.
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<i32> {
    crate::core::runner::run_passthrough("deno", args, verbose)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_deno_output_savings_on_real_output() {
        // deno colorizes even when piped, and the filter keeps every
        // diagnostic, so the reduction is in bytes rather than in content.
        for (name, raw, floor) in [
            (
                "lint",
                include_str!("../../../tests/fixtures/deno_lint_raw.txt"),
                40.0,
            ),
            (
                "check",
                include_str!("../../../tests/fixtures/deno_check_raw.txt"),
                50.0,
            ),
        ] {
            let out = filter_deno_output(raw);
            let savings = 100.0 - (out.len() as f64 / raw.len() as f64 * 100.0);
            assert!(
                savings >= floor,
                "deno {name}: expected >={floor}% byte savings, got {savings:.1}%"
            );
        }
    }

    #[test]
    fn test_filter_deno_lint_keeps_every_diagnostic() {
        let raw = include_str!("../../../tests/fixtures/deno_lint_raw.txt");
        let out = filter_deno_output(raw);
        for rule in [
            "no-var",
            "no-unused-vars",
            "prefer-const",
            "no-explicit-any",
        ] {
            assert!(out.contains(rule), "{rule} dropped from: {out}");
        }
    }
    #[test]
    fn test_filter_deno_output_strips_download() {
        let input = r#"Download https://deno.land/std@0.200.0/path/mod.ts
Download https://deno.land/x/oak@v12.6.1/mod.ts
error: Expected ';' at main.ts:5:10
some warning here"#;

        let result = filter_deno_output(input);
        assert!(!result.contains("Download "));
        assert!(result.contains("error: Expected ';' at main.ts:5:10"));
        assert!(result.contains("some warning here"));
    }

    #[test]
    fn test_filter_deno_output_strips_download_lines() {
        // Download lines appear only on a cold cache; the diagnostics that
        // follow them are the point of the run and must survive.
        let input = "Download https://deno.land/std@0.200.0/path/mod.ts\n\
Download https://deno.land/x/oak@v12.6.1/mod.ts\n\
Check file:///project/main.ts\n\
error: Expected ';' at main.ts:5:10\n";
        let output = filter_deno_output(input);
        assert!(!output.contains("Download "), "{output}");
        assert!(output.contains("error: Expected ';'"), "{output}");
        assert!(output.contains("Check file:///project/main.ts"), "{output}");
    }

    #[test]
    fn test_filter_deno_output_empty() {
        let input = r#"Download https://deno.land/std@0.200.0/path/mod.ts

Download https://deno.land/x/oak@v12.6.1/mod.ts

"#;

        let result = filter_deno_output(input);
        assert_eq!(result, "ok");
    }

    #[test]
    fn test_filter_deno_strips_ansi() {
        let input = "\x1b[33mDownload https://deno.land/std@0.200.0/path/mod.ts\x1b[0m\n\x1b[31merror: something\x1b[0m\n";
        let result = filter_deno_output(input);
        assert!(!result.contains("Download"));
        assert!(result.contains("error: something"));
    }

    #[test]
    fn test_filter_deno_preserves_check_lines() {
        let input = "Check file:///project/main.ts\n";
        let result = filter_deno_output(input);
        assert!(result.contains("Check"));
    }

    #[test]
    fn test_filter_deno_preserves_errors_strips_downloads() {
        let input = r#"Download https://deno.land/std@0.210.0/path/mod.ts
error: Module not found "https://deno.land/x/nonexistent/mod.ts"
"#;
        let result = filter_deno_output(input);
        assert!(result.contains("error:"));
        assert!(result.contains("Module not found"));
        assert!(!result.contains("Download"));
    }

    #[test]
    fn test_chose_output_format_detects_a_named_reporter() {
        for a in [
            "--reporter=junit",
            "--reporter",
            "--junit-path",
            "--junit-path=r.xml",
        ] {
            let args = vec![a.to_string()];
            assert!(chose_output_format(&args), "{a}");
        }
        for a in ["--allow-all", "--filter=x", "report"] {
            let args = vec![a.to_string()];
            assert!(!chose_output_format(&args), "{a}");
        }
    }
}
