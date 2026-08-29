//! Filters `make` build output.
//!
//! `make` (and `gmake`) prints two low-signal noise sources that dominate agent
//! context windows:
//!
//! 1. **Directory chatter** — `make[N]: Entering directory '...'` /
//!    `make[N]: Leaving directory '...'` on every recursive invocation.
//! 2. **Recipe echo** — every non-`@`-prefixed recipe line is echoed before
//!    execution.
//!
//! This filter takes the conservative route (per issue #3487): it suppresses
//! only the directory-chatter lines, leaving recipe echo and — crucially — all
//! sub-tool output (gcc/clang/pytest diagnostics) untouched so the agent can
//! still act on failures. On a non-zero exit we pass the raw output through
//! unchanged (Design Philosophy: Never Worse).
//!
//! Verbose flags (`-v`/`--verbose`/`--trace`/`--debug`/`-d`) bypass filtering
//! entirely (Correctness over savings).

use crate::core::runner;
use anyhow::{Context, Result};
use std::process::Command;
use std::sync::LazyLock;

/// Flags that request full/unfiltered output.
static VERBOSE_FLAGS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec!["-v", "--verbose", "--trace", "--debug", "-d"]
});

/// Prefixes that mark `make`'s own directory-chatter lines (suppressed in
/// default mode).
static DIR_CHATTER_PREFIXES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec![
        "make[", // "make[1]: Entering directory '...'" / "Leaving ..."
    ]
});

fn has_verbose_flag(args: &[String]) -> bool {
    args.iter().any(|a| VERBOSE_FLAGS.iter().any(|v| a == *v))
}

/// Pure filter: drop `make[N]:` directory-chatter lines, keep everything else.
pub fn filter_make(raw: &str, verbose: bool) -> String {
    if verbose {
        return raw.to_string();
    }

    let mut out: Vec<String> = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim_start();
        // Suppress make's own "make[N]: Entering/Leaving directory" chatter.
        if DIR_CHATTER_PREFIXES.iter().any(|p| trimmed.starts_with(*p))
            && (trimmed.contains("Entering directory")
                || trimmed.contains("Leaving directory"))
        {
            continue;
        }
        out.push(line.to_string());
    }

    // Mirror the TOML `on_empty = "make: ok"` behaviour for a fully-collapsed run.
    if out.is_empty() {
        return "make: ok".to_string();
    }
    out.join("\n")
}

/// Run `make` / `gmake` and filter its output.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let verbose_flag = verbose > 0 || has_verbose_flag(args);
    if verbose > 0 {
        eprintln!("Running: make {}", args.join(" "));
    }

    // Prefer `make`; fall back to `gmake` (common on *BSD / some CI images).
    let bin = if Command::new("make").arg("--version").output().is_ok() {
        "make"
    } else {
        "gmake"
    };

    let mut cmd = Command::new(bin);
    for arg in args {
        cmd.arg(arg);
    }

    runner::run_filtered(
        cmd,
        "make",
        &args.join(" "),
        |raw: &str| filter_make(raw, verbose_flag),
        runner::RunOptions::stdout_only().tee("make"),
    )
    .context("make filter execution failed")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn suppresses_entering_leaving_directory() {
        let raw = "\
make[1]: Entering directory '/home/user/proj'
gcc -O2 foo.c
make[1]: Leaving directory '/home/user/proj'
";
        let out = filter_make(raw, false);
        assert!(!out.contains("Entering directory"), "dir chatter must be stripped");
        assert!(!out.contains("Leaving directory"), "dir chatter must be stripped");
        assert!(out.contains("gcc -O2 foo.c"), "sub-tool output must survive");
    }

    #[test]
    fn keeps_subtool_diagnostics() {
        let raw = "\
make[1]: Entering directory '/proj'
gcc -O2 bad.c
bad.c:5:3: error: 'foo' undeclared (first use in this function)
make[1]: Leaving directory '/proj'
";
        let out = filter_make(raw, false);
        assert!(out.contains("error: 'foo' undeclared"), "compiler error must survive");
        assert!(out.contains("gcc -O2 bad.c"), "recipe line kept (conservative)");
    }

    #[test]
    fn verbose_flag_passthrough() {
        let raw = "make[1]: Entering directory '/proj'\ngcc -O2 foo.c\n";
        let out = filter_make(raw, true);
        assert_eq!(out, raw, "verbose must pass raw through");
    }

    #[test]
    fn empty_collapses_to_ok() {
        let raw = "make[1]: Entering directory '/proj'\nmake[1]: Leaving directory '/proj'\n";
        let out = filter_make(raw, false);
        assert_eq!(out, "make: ok", "fully stripped run collapses to ok");
    }

    #[test]
    fn has_verbose_flag_detects_common_flags() {
        assert!(has_verbose_flag(&["--debug".to_string()]));
        assert!(has_verbose_flag(&["-d".to_string()]));
        assert!(!has_verbose_flag(&["all".to_string()]));
    }
}
