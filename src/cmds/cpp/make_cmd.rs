//! Filters make/ninja output — strips per-file noise, surfaces compiler diagnostics.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    static ref GCC_DIAG_RE: Regex =
        Regex::new(r"^[^:\s].*:\d+:\d+:\s+(?:error|warning|note|fatal error):").unwrap();
    static ref MAKE_ERR_RE: Regex = Regex::new(r"^make(\[\d+\])?:\s+\*\*\*").unwrap();
    static ref NINJA_PROGRESS_RE: Regex =
        Regex::new(r"^\[\d+/\d+\]\s+(Building|Linking|Generating|Compiling)").unwrap();
    static ref MAKE_BUILD_LINE_RE: Regex =
        Regex::new(r"^(?:cc|gcc|g\+\+|clang|clang\+\+|c\+\+|ld|ar)\b").unwrap();
}

pub fn run_make(args: &[String], verbose: u8) -> Result<i32> {
    run_inner("make", args, verbose)
}

pub fn run_ninja(args: &[String], verbose: u8) -> Result<i32> {
    run_inner("ninja", args, verbose)
}

fn run_inner(tool: &'static str, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command(tool);
    for a in args {
        cmd.arg(a);
    }
    if verbose > 0 {
        eprintln!("Running: {} {}", tool, args.join(" "));
    }
    runner::run_filtered(
        cmd,
        tool,
        &args.join(" "),
        move |raw| filter_output(raw, tool),
        RunOptions::with_tee("make"),
    )
}

fn filter_output(raw: &str, tool: &str) -> String {
    let mut out = Vec::new();
    let mut diag_context = 0usize;
    let mut emitted_diag = false;

    for line in raw.lines() {
        if NINJA_PROGRESS_RE.is_match(line) {
            continue;
        }
        if line.contains("Entering directory") || line.contains("Leaving directory") {
            continue;
        }
        if MAKE_BUILD_LINE_RE.is_match(line) {
            continue;
        }

        if GCC_DIAG_RE.is_match(line) {
            out.push(line.to_string());
            diag_context = 3;
            emitted_diag = true;
            continue;
        }
        if MAKE_ERR_RE.is_match(line) {
            out.push(line.to_string());
            emitted_diag = true;
            diag_context = 0;
            continue;
        }
        if line.contains("undefined reference") {
            out.push(line.to_string());
            emitted_diag = true;
            continue;
        }
        if diag_context > 0 {
            let trimmed = line.trim_start();
            if trimmed.is_empty()
                || trimmed.starts_with('|')
                || line.starts_with(' ')
                || line.starts_with('\t')
            {
                out.push(line.to_string());
                diag_context -= 1;
                continue;
            }
            diag_context = 0;
        }
    }

    if !emitted_diag {
        return format!("{}: ok", tool);
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_make_success() {
        let raw = "make: Entering directory '/tmp/x'\n\
            cc -c main.c -o main.o\n\
            cc -o myapp main.o\n\
            make: Leaving directory '/tmp/x'\n";
        assert_eq!(filter_output(raw, "make"), "make: ok");
    }

    #[test]
    fn test_make_failure_keeps_diag() {
        let raw = "cc -c main.c -o main.o\n\
            main.c:5:1: error: expected ';' before 'return'\n\
                5 | return 0\n\
                  | ^\n\
            make[1]: *** [Makefile:10: main.o] Error 1\n\
            make: *** [all] Error 2\n";
        let out = filter_output(raw, "make");
        assert!(out.contains("error:"));
        assert!(out.contains("make[1]: ***"));
        assert!(!out.contains("cc -c main.c"));
    }

    #[test]
    fn test_fixture_make_failure() {
        let raw = include_str!("../../../tests/fixtures/cpp/make_failure.txt");
        let out = filter_output(raw, "make");
        assert!(out.contains("error:"));
        assert!(out.contains("make[1]: ***") || out.contains("make: ***"));
        assert!(!out.contains("cc -Wall"));
    }

    #[test]
    fn test_ninja_progress_stripped() {
        let raw = "[1/3] Building CXX object x.o\n\
            [2/3] Building CXX object y.o\n\
            [3/3] Linking myapp\n";
        assert_eq!(filter_output(raw, "ninja"), "ninja: ok");
    }
}
