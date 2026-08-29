//! Filters `llvm-lit` / `lit` test runner output.
//!
//! `lit` (LLVM Integrated Tester) runs test suites and prints one status line
//! per test (`PASS:`, `FAIL:`, `UNRESOLVED:`, `UNSUPPORTED:`, `XFAIL:`,
//! `XPASS:`, `TIMEOUT:`, `SKIPPED:`) followed by a `Testing Time:` summary
//! block. Per-test PASS/XFAIL/UNSUPPORTED lines are pure noise for an LLM once
//! the aggregate counts are known, so they are suppressed by default. Failure
//! detail blocks (indented lines after a FAIL/UNRESOLVED/TIMEOUT) are preserved
//! because the agent needs them to fix the breakage.
//!
//! Verbose flags (`-v`, `--verbose`, `--show-all`) are respected: the user asked
//! for the full run, so output passes through unchanged.

use crate::core::runner;
use anyhow::{Context, Result};
use std::process::Command;
use std::sync::LazyLock;

/// Flags that request full/unfiltered output. When any is present we pass the
/// raw output through untouched (Design Philosophy: Correctness over savings).
static VERBOSE_FLAGS: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec!["-v", "--verbose", "--show-all", "--show-output", "-a"]
});

/// Result-prefixes that are suppressed in default mode (noise once the summary
/// is shown).
static SUPPRESSED_PREFIXES: LazyLock<Vec<&'static str>> = LazyLock::new(|| {
    vec!["PASS:", "XFAIL:", "UNSUPPORTED:", "SKIPPED:", "Unsupported:", "Expectedly Failed:"]
});

fn has_verbose_flag(args: &[String]) -> bool {
    args.iter().any(|a| VERBOSE_FLAGS.iter().any(|v| a == *v))
}

/// Pure filter: reduce lit output to failures + summary.
pub fn filter_lit(raw: &str, verbose: bool) -> String {
    if verbose {
        return raw.to_string();
    }

    let mut out: Vec<String> = Vec::new();
    let mut in_summary = false;
    let mut saw_failure_detail = false;

    for line in raw.lines() {
        let trimmed = line.trim_start();

        // Always keep the Testing Time summary block.
        if trimmed.starts_with("Testing Time:") || in_summary {
            // Summary block: lines like "Passed           : 40" with leading spaces.
            in_summary = true;
            out.push(line.to_string());
            continue;
        }

        // Keep the status banner lines for non-passing outcomes.
        if trimmed.starts_with("FAIL:")
            || trimmed.starts_with("UNRESOLVED:")
            || trimmed.starts_with("TIMEOUT:")
            || trimmed.starts_with("XPASS:")
        {
            out.push(line.to_string());
            saw_failure_detail = true;
            continue;
        }

        // Indented continuation / detail lines (after a failure header) belong to
        // the failure block; keep them so the agent can act on the failure.
        if saw_failure_detail
            && (line.starts_with(' ') || line.starts_with('\t')) && !line.trim().is_empty()
        {
            out.push(line.to_string());
            continue;
        }

        // Suppress the noisy-but-ok status lines.
        if SUPPRESSED_PREFIXES.iter().any(|p| trimmed.starts_with(*p)) {
            continue;
        }

        // Keep anything else (warnings, errors, unexpected output) so we never
        // hide information the agent might need.
        out.push(line.to_string());
    }

    // If no failures and the summary is present, collapse to a one-line summary
    // so a clean run is a single token-cheap signal.
    let has_summary = raw.lines().any(|l| l.trim_start().starts_with("Testing Time:"));
    if !saw_failure_detail && has_summary {
        let mut counts = Vec::new();
        for l in raw.lines() {
            let t = l.trim_start();
            let (label, rest) = if let Some(r) = t.strip_prefix("Passed") {
                ("passed", r)
            } else if let Some(r) = t.strip_prefix("Failed") {
                ("failed", r)
            } else if let Some(r) = t.strip_prefix("Unsupported") {
                ("unsupported", r)
            } else if let Some(r) = t.strip_prefix("Expectedly Failed") {
                ("xfail", r)
            } else {
                continue;
            };
            // Lit prints "Passed: 4" or "Passed           : 4"; drop the colon
            // and any surrounding whitespace before recording the count.
            let val = rest.trim_start_matches(':').trim();
            counts.push(format!("{}={}", label, val));
        }
        if !counts.is_empty() {
            return format!("[ok] lit: {}", counts.join(" "));
        }
    }

    out.join("\n")
}

/// Run `lit` / `llvm-lit` and filter its output.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let verbose_flag = verbose > 0 || has_verbose_flag(args);
    if verbose > 0 {
        eprintln!("Running: lit {}", args.join(" "));
    }

    // Try `llvm-lit` first, fall back to `lit` (pip-installed `lit` package).
    let bin = if Command::new("llvm-lit").arg("--version").output().is_ok() {
        "llvm-lit"
    } else {
        "lit"
    };

    let mut cmd = Command::new(bin);
    for arg in args {
        cmd.arg(arg);
    }

    runner::run_filtered(
        cmd,
        "lit",
        &args.join(" "),
        |raw: &str| filter_lit(raw, verbose_flag),
        runner::RunOptions::stdout_only().tee("lit"),
    )
    .context("Failed to run lit")
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
PASS: test/a.cpp (1 of 50)
PASS: test/b.cpp (2 of 50)
FAIL: test/c.cpp (3 of 50)
  Command: llvm-lit test/c.cpp
  clang: error: undefined reference to 'foo'
UNRESOLVED: test/d.cpp (4 of 50)
  Test output is missing
PASS: test/e.cpp (5 of 50)
PASS: test/f.cpp (6 of 50)
PASS: test/g.cpp (7 of 50)
PASS: test/h.cpp (8 of 50)
PASS: test/i.cpp (9 of 50)
PASS: test/j.cpp (10 of 50)
PASS: test/k.cpp (11 of 50)
PASS: test/l.cpp (12 of 50)
PASS: test/m.cpp (13 of 50)
PASS: test/n.cpp (14 of 50)
PASS: test/o.cpp (15 of 50)
PASS: test/p.cpp (16 of 50)
PASS: test/q.cpp (17 of 50)
PASS: test/r.cpp (18 of 50)
PASS: test/s.cpp (19 of 50)
PASS: test/t.cpp (20 of 50)
PASS: test/u.cpp (21 of 50)
PASS: test/v.cpp (22 of 50)
PASS: test/w.cpp (23 of 50)
PASS: test/x.cpp (24 of 50)
PASS: test/y.cpp (25 of 50)
PASS: test/z.cpp (26 of 50)
PASS: test/aa.cpp (27 of 50)
PASS: test/ab.cpp (28 of 50)
PASS: test/ac.cpp (29 of 50)
PASS: test/ad.cpp (30 of 50)
PASS: test/ae.cpp (31 of 50)
PASS: test/af.cpp (32 of 50)
PASS: test/ag.cpp (33 of 50)
PASS: test/ah.cpp (34 of 50)
PASS: test/ai.cpp (35 of 50)
PASS: test/aj.cpp (36 of 50)
PASS: test/ak.cpp (37 of 50)
PASS: test/al.cpp (38 of 50)
PASS: test/am.cpp (39 of 50)
PASS: test/an.cpp (40 of 50)
PASS: test/ao.cpp (41 of 50)
PASS: test/ap.cpp (42 of 50)
PASS: test/aq.cpp (43 of 50)
PASS: test/ar.cpp (44 of 50)
PASS: test/as.cpp (45 of 50)
PASS: test/at.cpp (46 of 50)
PASS: test/au.cpp (47 of 50)
PASS: test/av.cpp (48 of 50)
PASS: test/aw.cpp (49 of 50)
PASS: test/ax.cpp (50 of 50)
XFAIL: test/fx.cpp (skipped)
UNSUPPORTED: test/gx.cpp (skipped)

Testing Time: 12.34s
  Unsupported      : 1
  Passed           : 46
  Failed           : 1
  Unresolved       : 1
  Expectedly Failed: 1
";

    #[test]
    fn suppresses_pass_and_keeps_fail_detail_and_summary() {
        let out = filter_lit(SAMPLE, false);
        // PASS/XFAIL/UNSUPPORTED lines must be gone.
        assert!(!out.contains("PASS:"), "PASS lines should be suppressed");
        assert!(!out.contains("XFAIL:"), "XFAIL lines should be suppressed");
        assert!(!out.contains("UNSUPPORTED:"), "UNSUPPORTED should be suppressed");
        // Failure header + its indented detail must survive.
        assert!(out.contains("FAIL: test/c.cpp"), "FAIL header must survive");
        assert!(out.contains("undefined reference to 'foo'"), "FAIL detail must survive");
        assert!(out.contains("UNRESOLVED: test/d.cpp"), "UNRESOLVED must survive");
        // Summary block must survive.
        assert!(out.contains("Testing Time: 12.34s"), "summary must survive");
        // Savings: 7 status lines + summary collapses; filtered much shorter than raw.
        assert!(out.len() < SAMPLE.len() / 2, "should cut tokens by >50%");
    }

    #[test]
    fn verbose_flag_passthrough() {
        let out = filter_lit(SAMPLE, true);
        assert_eq!(out, SAMPLE, "verbose must pass through unchanged");
    }

    #[test]
    fn clean_run_collapses_to_summary() {
        let clean = "\
PASS: test/a.cpp (1 of 3)
PASS: test/b.cpp (2 of 3)
PASS: test/c.cpp (3 of 3)

Testing Time: 1.00s
  Passed: 3
";
        let out = filter_lit(clean, false);
        assert!(out.starts_with("[ok] lit:"), "clean run collapses to [ok] line");
        assert!(out.contains("passed=3"), "summary counts preserved");
        assert!(!out.contains("PASS:"), "PASS lines suppressed in clean run");
    }

    #[test]
    fn has_verbose_flag_detects_common_flags() {
        assert!(has_verbose_flag(&["-v".to_string()]));
        assert!(has_verbose_flag(&["--verbose".to_string()]));
        assert!(has_verbose_flag(&["--show-all".to_string()]));
        assert!(!has_verbose_flag(&["test/".to_string()]));
    }
}

