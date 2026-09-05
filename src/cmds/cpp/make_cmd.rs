//! Filters make/ninja output — strips per-file noise, surfaces compiler diagnostics.

use super::failure_fallback;
use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

static GCC_DIAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^[^:\s].*:\d+:\d+:\s+(?:error|warning|note|fatal error):").unwrap()
});
static MAKE_ERR_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^make(\[\d+\])?:\s+\*\*\*").unwrap());
static NINJA_PROGRESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[\d+/\d+\]\s+(Building|Linking|Generating|Compiling)").unwrap()
});
static MAKE_BUILD_LINE_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:cc|gcc|g\+\+|clang|clang\+\+|c\+\+|ld|ar)\b").unwrap()
});
static DRIVER_DIAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:clang|clang\+\+|gcc|g\+\+|cc|c\+\+|ld):\s+(?:fatal )?error:").unwrap()
});

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
    runner::run_filtered_with_exit(
        cmd,
        tool,
        &args.join(" "),
        move |raw, exit_code| filter_output(raw, tool, exit_code),
        RunOptions::with_tee(tool).preserve_filtered_failure_output(),
    )
}

fn filter_output(raw: &str, tool: &str, exit_code: i32) -> String {
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
        if GCC_DIAG_RE.is_match(line)
            || DRIVER_DIAG_RE.is_match(line)
            || MAKE_ERR_RE.is_match(line)
            || line.contains("undefined reference")
            || (tool == "ninja"
                && (line.trim_start().starts_with("FAILED:")
                    || line.to_ascii_lowercase().contains("build stopped: subcommand failed")))
        {
            out.push(line.to_string());
            diag_context = 3;
            emitted_diag = true;
            continue;
        }

        if MAKE_BUILD_LINE_RE.is_match(line) {
            continue;
        }
        if diag_context > 0 {
            let trimmed = line.trim_start();
            if trimmed.is_empty()
                || trimmed.starts_with('|')
                || line.starts_with(' ')
                || line.starts_with('\t')
                || (tool == "ninja" && diag_context > 0)
            {
                out.push(line.to_string());
                diag_context -= 1;
                continue;
            }
            diag_context = 0;
        }
    }

    if !emitted_diag {
        if exit_code != 0 {
            return failure_fallback(tool, exit_code, raw);
        }
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
        assert_eq!(filter_output(raw, "make", 0), "make: ok");
    }

    #[test]
    fn test_make_failure_keeps_diag() {
        let raw = "cc -c main.c -o main.o\n\
            main.c:5:1: error: expected ';' before 'return'\n\
                5 | return 0\n\
                  | ^\n\
            make[1]: *** [Makefile:10: main.o] Error 1\n\
            make: *** [all] Error 2\n";
        let out = filter_output(raw, "make", 1);
        assert!(out.contains("error:"));
        assert!(out.contains("make[1]: ***"));
        assert!(!out.contains("cc -c main.c"));
    }

    #[test]
    fn test_fixture_make_failure() {
        let raw = include_str!("../../../tests/fixtures/cpp/make_failure.txt");
        let out = filter_output(raw, "make", 1);
        assert!(out.contains("error:"));
        assert!(out.contains("make[1]: ***") || out.contains("make: ***"));
        assert!(!out.contains("cc -Wall"));
    }

    #[test]
    fn test_ninja_progress_stripped() {
        let raw = "[1/3] Building CXX object x.o\n\
            [2/3] Building CXX object y.o\n\
            [3/3] Linking myapp\n";
        assert_eq!(filter_output(raw, "ninja", 0), "ninja: ok");
    }

    #[test]
    fn test_ninja_failure_keeps_diagnostic() {
        let raw = "[1/2] Building CXX object main.o\n\
            main.cpp:7:1: error: missing ';'\n\
            ninja: build stopped: subcommand failed.\n";
        let out = filter_output(raw, "ninja", 1);
        assert!(out.contains("main.cpp:7:1: error:"));
        assert!(out.contains("ninja: build stopped"));
        assert!(!out.contains("[1/2] Building"));
    }

    #[test]
    fn unknown_and_empty_nonzero_output_is_failure() {
        assert!(filter_output("unknown failure", "make", 2).contains("make: failed (exit 2)"));
        assert_eq!(filter_output("", "ninja", 1), "ninja: failed (exit 1)");
    }

    #[test]
    fn clang_driver_and_ninja_failed_block_survive() {
        let raw = "FAILED: app\nclang: error: invalid argument\nninja: build stopped: subcommand failed.\n";
        let out = filter_output(raw, "ninja", 1);
        assert!(out.contains("FAILED: app"));
        assert!(out.contains("clang: error"));
        assert!(out.contains("build stopped"));
        assert!(!out.contains("ninja: ok"));
    }

    #[test]
    fn tee_label_matches_tool() {
        for tool in ["make", "ninja"] {
            assert_eq!(RunOptions::with_tee(tool).tee_label, Some(tool));
        }
    }
}
