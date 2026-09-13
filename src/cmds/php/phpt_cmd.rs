//! Filters `php run-tests.php` output to a compact summary with failure diffs.
//!
//! php-src and PHP extensions use `run-tests.php` to drive their .phpt test suite.
//! A full run prints one `TEST N/M [path]STATUS description [path]` line per test
//! and, for each failure, a `========DIFF========` block showing expected vs actual
//! output. On a 5000-test run this is ~1.3 MB of output dominated by PASS chatter.
//!
//! This filter keeps the environment header, the aggregate counts, and a bounded
//! number of failure diffs; it drops the per-test PASS/SKIP lines entirely.

use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

const MAX_FAILURES_SHOWN: usize = 20;
const MAX_DIFF_LINES_PER_FAILURE: usize = 6;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Status {
    Pass,
    Fail,
    Skip,
    Bork,
    Warn,
    Leak,
    Xfail,
    Xleak,
}

#[derive(Default, Debug, Clone)]
struct Counts {
    pass: usize,
    fail: usize,
    skip: usize,
    bork: usize,
    warn: usize,
    leak: usize,
    xfail: usize,
    xleak: usize,
}

impl Counts {
    fn inc(&mut self, s: Status) {
        match s {
            Status::Pass => self.pass += 1,
            Status::Fail => self.fail += 1,
            Status::Skip => self.skip += 1,
            Status::Bork => self.bork += 1,
            Status::Warn => self.warn += 1,
            Status::Leak => self.leak += 1,
            Status::Xfail => self.xfail += 1,
            Status::Xleak => self.xleak += 1,
        }
    }

    fn total(&self) -> usize {
        self.pass
            + self.fail
            + self.skip
            + self.bork
            + self.warn
            + self.leak
            + self.xfail
            + self.xleak
    }

    // XFAIL/XLEAK are expected results, not failures — mirrors run-tests.php,
    // which reports them under "Expected fail"/"Expected leak", not "Tests failed".
    fn broken_total(&self) -> usize {
        self.fail + self.bork + self.leak
    }
}

#[derive(Default, Debug)]
struct EnvInfo {
    version: String,
    sapi: String,
    os: String,
}

#[derive(Debug)]
struct Failure {
    path: String,
    description: String,
    // Capped at MAX_DIFF_LINES_PER_FAILURE; `diff_total` keeps the full count
    // so the "+N more diff lines" overflow note stays accurate.
    diff: Vec<String>,
    diff_total: usize,
}

// run-tests.php prints the DIFF block only when the caller passes --show-diff or
// --show-all (or selects a single test). Every other --show-* switch (--show-slow,
// --show-mem, --show-out, …) leaves failures diff-less, so they must not suppress
// the --show-diff we inject.
fn args_already_show_diff(args: &[String]) -> bool {
    args.iter().any(|a| a == "--show-diff" || a == "--show-all")
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("php");
    cmd.arg("run-tests.php");
    // The diff is what makes a failure actionable, and the filter caps it at
    // MAX_DIFF_LINES_PER_FAILURE regardless, so request it unless the caller
    // already did.
    let shows_diff = args_already_show_diff(args);
    if !shows_diff {
        cmd.arg("--show-diff");
    }
    for a in args {
        cmd.arg(a);
    }

    if verbose > 0 {
        let injected = if shows_diff { "" } else { "--show-diff " };
        eprintln!("Running: php run-tests.php {injected}{}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "phpt",
        &args.join(" "),
        filter_phpt_output,
        runner::RunOptions::with_tee("phpt"),
    )
}

pub(crate) fn filter_phpt_output(raw: &str) -> String {
    // Serial runs (no -j) redraw the progress line, so a status arrives as
    // "TEST 1/2 [x.phpt]\rFAIL desc [x.phpt]". Each CR segment is its own
    // rendered line; splitting on it is what lets the status anchor match.
    let stripped = strip_ansi(raw).replace('\r', "\n");
    let parsed = parse(&stripped);
    // A startup failure (no run-tests.php in cwd, php missing) produces nothing
    // parseable, and its diagnostic is on stderr. Summarising that as "no tests
    // ran" would drop the only line explaining why.
    if !parsed.looks_like_run_tests() {
        return stripped.trim_end().to_string();
    }
    build_summary(&parsed)
}

#[derive(Default, Debug)]
struct Parsed {
    env: EnvInfo,
    scanned_counts: Counts,
    summary_counts: Option<Counts>,
    total_tests: Option<usize>,
    time_seconds: Option<f64>,
    failures: Vec<Failure>,
}

impl Parsed {
    fn looks_like_run_tests(&self) -> bool {
        self.summary_counts.is_some()
            || self.scanned_counts.total() > 0
            || !self.env.version.is_empty()
    }
}

fn parse(stripped: &str) -> Parsed {
    static STATUS_RE: LazyLock<Regex> = LazyLock::new(|| {
        // Matches a status line. Anchor: either start-of-line ("FAIL desc [x.phpt]")
        // or immediately after the test bracket ("TEST N/M [x.phpt]PASS desc [x.phpt]").
        // We only accept the token after `]` or at line start so that prose like
        // "FAILED TEST SUMMARY" does not match.
        Regex::new(
            r"(?:^|\])(PASS|FAIL|SKIP|BORK|WARN|XLEAK|LEAK|XFAIL)\s+(.*?)\s*\[([^\[\]]+\.phpt)\]",
        )
        .unwrap()
    });
    static STAT_RE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^\s*(Tests passed|Tests failed|Tests skipped|Tests warned|Expected fail|Expected leak|Tests leaked|Tests borked)\s*:\s*(\d+)",
        )
        .unwrap()
    });
    static NUMBER_OF_TESTS_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*Number of tests\s*:\s*(\d+)").unwrap());
    static TIME_RE: LazyLock<Regex> =
        LazyLock::new(|| Regex::new(r"^\s*Time taken\s*:\s*([0-9.]+)").unwrap());

    let mut p = Parsed::default();
    let mut diff_buf: Vec<String> = Vec::new();
    let mut diff_total: usize = 0;
    let mut pending_diff: Option<(Vec<String>, usize)> = None;
    let mut in_diff = false;

    for raw_line in stripped.lines() {
        let line = raw_line.trim_end();

        if line == "========DIFF========" {
            diff_buf.clear();
            diff_total = 0;
            in_diff = true;
            continue;
        }
        if line == "========DONE========" {
            in_diff = false;
            pending_diff = Some((std::mem::take(&mut diff_buf), diff_total));
            continue;
        }
        if in_diff {
            // Only the first MAX_DIFF_LINES_PER_FAILURE lines are ever shown, so
            // stop buffering past that — but keep counting for the overflow note.
            diff_total += 1;
            if diff_buf.len() < MAX_DIFF_LINES_PER_FAILURE {
                diff_buf.push(line.to_string());
            }
            continue;
        }

        if p.env.version.is_empty()
            && let Some(rest) = line.strip_prefix("PHP_VERSION")
        {
            p.env.version = after_colon(rest);
        }
        if p.env.sapi.is_empty()
            && let Some(rest) = line.strip_prefix("PHP_SAPI")
        {
            p.env.sapi = after_colon(rest);
        }
        if p.env.os.is_empty()
            && let Some(rest) = line.strip_prefix("PHP_OS")
        {
            let val = after_colon(rest);
            p.env.os = val.split(" - ").next().unwrap_or(&val).trim().to_string();
        }

        if let Some(cap) = STATUS_RE.captures(line) {
            let status = match &cap[1] {
                "PASS" => Status::Pass,
                "FAIL" => Status::Fail,
                "SKIP" => Status::Skip,
                "BORK" => Status::Bork,
                "WARN" => Status::Warn,
                "LEAK" => Status::Leak,
                "XFAIL" => Status::Xfail,
                "XLEAK" => Status::Xleak,
                _ => continue,
            };
            p.scanned_counts.inc(status);
            if matches!(status, Status::Fail | Status::Bork | Status::Leak) {
                let (diff, diff_total) = pending_diff.take().unwrap_or_default();
                p.failures.push(Failure {
                    path: cap[3].to_string(),
                    description: cap[2].trim().to_string(),
                    diff,
                    diff_total,
                });
            } else {
                pending_diff = None;
            }
            continue;
        }

        if let Some(cap) = NUMBER_OF_TESTS_RE.captures(line) {
            p.total_tests = cap[1].parse().ok();
            continue;
        }
        if let Some(cap) = TIME_RE.captures(line) {
            p.time_seconds = cap[1].parse().ok();
            continue;
        }
        if let Some(cap) = STAT_RE.captures(line) {
            let n: usize = cap[2].parse().unwrap_or(0);
            let sc = p.summary_counts.get_or_insert_with(Counts::default);
            match &cap[1] {
                "Tests passed" => sc.pass = n,
                "Tests failed" => sc.fail = n,
                "Tests skipped" => sc.skip = n,
                "Tests warned" => sc.warn = n,
                "Expected fail" => sc.xfail = n,
                "Expected leak" => sc.xleak = n,
                "Tests leaked" => sc.leak = n,
                "Tests borked" => sc.bork = n,
                _ => {}
            }
        }
    }

    p
}

fn after_colon(s: &str) -> String {
    s.find(':')
        .map(|i| s[i + 1..].trim().to_string())
        .unwrap_or_default()
}

fn build_summary(p: &Parsed) -> String {
    let counts = p
        .summary_counts
        .clone()
        .unwrap_or_else(|| p.scanned_counts.clone());

    if counts.total() == 0 {
        return "phpt: no tests ran".to_string();
    }

    let mut out = String::new();

    let total_n = p.total_tests.unwrap_or_else(|| counts.total());
    out.push_str(&format!("phpt: {} passed", counts.pass));
    if counts.fail > 0 {
        out.push_str(&format!(", {} failed", counts.fail));
    }
    if counts.skip > 0 {
        out.push_str(&format!(", {} skipped", counts.skip));
    }
    if counts.xfail > 0 {
        out.push_str(&format!(", {} xfailed", counts.xfail));
    }
    if counts.warn > 0 {
        out.push_str(&format!(", {} warned", counts.warn));
    }
    if counts.bork > 0 {
        out.push_str(&format!(", {} borked", counts.bork));
    }
    if counts.leak > 0 {
        out.push_str(&format!(", {} leaked", counts.leak));
    }
    if counts.xleak > 0 {
        out.push_str(&format!(", {} xleaked", counts.xleak));
    }
    out.push_str(&format!("  ({} total", total_n));
    if let Some(t) = p.time_seconds {
        out.push_str(&format!(", {:.1}s", t));
    }
    out.push(')');
    out.push('\n');

    let mut env_parts: Vec<String> = Vec::new();
    if !p.env.version.is_empty() {
        env_parts.push(format!("PHP {}", p.env.version));
    }
    if !p.env.sapi.is_empty() {
        env_parts.push(format!("SAPI {}", p.env.sapi));
    }
    if !p.env.os.is_empty() {
        env_parts.push(format!("OS {}", p.env.os));
    }
    if !env_parts.is_empty() {
        out.push_str(&env_parts.join("  "));
        out.push('\n');
    }

    if counts.broken_total() == 0 {
        return out.trim_end().to_string();
    }

    out.push('\n');
    out.push_str(&format!("FAILURES ({}):\n", counts.broken_total()));

    // Counts can come from the summary block while the per-test lines were
    // truncated out of the capture, so we know how many broke but have no diffs.
    // Print the header (above) and say so, rather than silently dropping it.
    if p.failures.is_empty() {
        out.push_str("  (per-test details unavailable — output truncated)\n");
        return out.trim_end().to_string();
    }

    for (i, f) in p.failures.iter().take(MAX_FAILURES_SHOWN).enumerate() {
        if i > 0 {
            out.push('\n');
        }
        out.push_str("  ");
        out.push_str(&f.path);
        if !f.description.is_empty() {
            out.push_str(" -- ");
            out.push_str(&f.description);
        }
        out.push('\n');
        for line in &f.diff {
            out.push_str("    ");
            out.push_str(line);
            out.push('\n');
        }
        if f.diff_total > f.diff.len() {
            out.push_str(&format!(
                "    ... +{} more diff lines\n",
                f.diff_total - f.diff.len()
            ));
        }
    }

    if p.failures.len() > MAX_FAILURES_SHOWN {
        out.push_str(&format!(
            "\n... +{} more failures\n",
            p.failures.len() - MAX_FAILURES_SHOWN
        ));
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_show_diff_injection_ignores_unrelated_show_flags() {
        let s = |v: &[&str]| v.iter().map(|a| a.to_string()).collect::<Vec<_>>();
        // Unrelated --show-* flags don't produce diffs, so they must not suppress
        // our injected --show-diff.
        assert!(!args_already_show_diff(&s(&[
            "--show-slow",
            "1000",
            "ext/standard/"
        ])));
        assert!(!args_already_show_diff(&s(&["ext/standard/"])));
        // The two flags that do produce diffs suppress the injection.
        assert!(args_already_show_diff(&s(&["--show-diff"])));
        assert!(args_already_show_diff(&s(&["--show-all", "-j8"])));
    }

    #[test]
    fn test_all_pass() {
        let input = "\
=====================================================================
PHP         : /usr/bin/php8.4
PHP_SAPI    : cli
PHP_VERSION : 8.4.20
ZEND_VERSION: 4.4.20
PHP_OS      : Linux - Linux host 6.6
=====================================================================
Running selected tests.
TEST 1/3 [Zend/tests/a.phpt]\x1b[1;32mPASS\x1b[0m Test A [Zend/tests/a.phpt]
TEST 2/3 [Zend/tests/b.phpt]\x1b[1;32mPASS\x1b[0m Test B [Zend/tests/b.phpt]
TEST 3/3 [Zend/tests/c.phpt]\x1b[1;32mPASS\x1b[0m Test C [Zend/tests/c.phpt]
=====================================================================
Number of tests :    3              3
Tests skipped   :    0 (  0.0%) --------
Tests warned    :    0 (  0.0%) (  0.0%)
Tests failed    :    0 (  0.0%) (  0.0%)
Expected fail   :    0 (  0.0%) (  0.0%)
Tests passed    :    3 (100.0%) (100.0%)
---------------------------------------------------------------------
Time taken      :   1.234 seconds
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(out.contains("phpt: 3 passed"), "got: {}", out);
        assert!(out.contains("PHP 8.4.20"), "got: {}", out);
        assert!(out.contains("SAPI cli"), "got: {}", out);
        assert!(out.contains("OS Linux"), "got: {}", out);
        assert!(out.contains("(3 total, 1.2s)"), "got: {}", out);
        assert!(!out.contains("FAILURES"), "got: {}", out);
    }

    #[test]
    fn test_single_failure_with_diff() {
        let input = "\
=====================================================================
PHP_SAPI    : cli
PHP_VERSION : 8.4.20
PHP_OS      : Linux
=====================================================================
Running selected tests.
TEST 1/2 [Zend/tests/a.phpt]\x1b[1;32mPASS\x1b[0m Test A [Zend/tests/a.phpt]
TEST 2/2 [Zend/tests/b.phpt]
========DIFF========
001- expected output
001+ actual output
========DONE========
\x1b[1;31mFAIL\x1b[0m Bug #123 reproduces [Zend/tests/b.phpt]
=====================================================================
Number of tests :    2              2
Tests skipped   :    0 (  0.0%) --------
Tests failed    :    1 ( 50.0%) ( 50.0%)
Tests passed    :    1 ( 50.0%) ( 50.0%)
---------------------------------------------------------------------
Time taken      :   0.500 seconds
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(out.contains("1 passed, 1 failed"), "got: {}", out);
        assert!(out.contains("FAILURES (1):"), "got: {}", out);
        assert!(out.contains("Zend/tests/b.phpt"), "got: {}", out);
        assert!(out.contains("Bug #123 reproduces"), "got: {}", out);
        assert!(out.contains("001- expected output"), "got: {}", out);
        assert!(out.contains("001+ actual output"), "got: {}", out);
    }

    #[test]
    fn test_diff_truncation() {
        let mut input = String::from(
            "=====================================================================\n\
             PHP_VERSION : 8.4.20\n\
             =====================================================================\n\
             TEST 1/1 [t.phpt]\n\
             ========DIFF========\n",
        );
        for i in 1..=20 {
            input.push_str(&format!("{:03}- line {}\n", i, i));
        }
        input.push_str("========DONE========\n");
        input.push_str("\x1b[1;31mFAIL\x1b[0m long [t.phpt]\n");
        input.push_str("=====================================================================\n");
        input.push_str("Number of tests :    1              1\n");
        input.push_str("Tests failed    :    1\n");
        input.push_str("Tests passed    :    0\n");

        let out = filter_phpt_output(&input);
        assert!(out.contains("001- line 1"), "got: {}", out);
        assert!(out.contains("006- line 6"), "got: {}", out);
        assert!(!out.contains("007- line 7"), "got: {}", out);
        assert!(out.contains("+14 more diff lines"), "got: {}", out);
    }

    #[test]
    fn test_multiple_failures_truncated() {
        let mut input = String::from(
            "=====================================================================\n\
             PHP_VERSION : 8.4.20\n\
             =====================================================================\n",
        );
        for i in 1..=25 {
            input.push_str(&format!("TEST {}/25 [t{}.phpt]\n", i, i));
            input.push_str("========DIFF========\n");
            input.push_str(&format!("001- want {}\n", i));
            input.push_str(&format!("001+ got {}\n", i));
            input.push_str("========DONE========\n");
            input.push_str(&format!("\x1b[1;31mFAIL\x1b[0m test {} [t{}.phpt]\n", i, i));
        }
        input.push_str("=====================================================================\n");
        input.push_str("Tests failed    :   25\n");
        input.push_str("Tests passed    :    0\n");

        let out = filter_phpt_output(&input);
        assert!(out.contains("FAILURES (25):"), "got: {}", out);
        assert!(out.contains("t1.phpt"), "got: {}", out);
        assert!(out.contains("t20.phpt"), "got: {}", out);
        assert!(
            !out.contains("t21.phpt"),
            "truncated failures leaked: {}",
            out
        );
        assert!(out.contains("+5 more failures"), "got: {}", out);
    }

    #[test]
    fn test_skip_and_xfail() {
        let input = "\
=====================================================================
PHP_VERSION : 8.4.20
=====================================================================
TEST 1/3 [a.phpt]\x1b[1;32mPASS\x1b[0m A [a.phpt]
TEST 2/3 [b.phpt]\x1b[1;33mSKIP\x1b[0m B [b.phpt] reason: Required extension missing: foo
TEST 3/3 [c.phpt]\x1b[1;33mXFAIL\x1b[0m C expected fail [c.phpt]
=====================================================================
Number of tests :    3              2
Tests skipped   :    1
Tests failed    :    0
Expected fail   :    1
Tests passed    :    1
---------------------------------------------------------------------
Time taken      :   0.100 seconds
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(out.contains("1 passed"), "got: {}", out);
        assert!(out.contains("1 skipped"), "got: {}", out);
        assert!(out.contains("1 xfail"), "got: {}", out);
        assert!(!out.contains("FAILURES"), "got: {}", out);
    }

    #[test]
    fn test_bork_counts_as_failure() {
        let input = "\
=====================================================================
PHP_VERSION : 8.4.20
=====================================================================
TEST 1/1 [broken.phpt]\x1b[1;31mBORK\x1b[0m Broken test setup [broken.phpt]
=====================================================================
Tests borked    :    1
Tests passed    :    0
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(out.contains("1 bork"), "got: {}", out);
        assert!(out.contains("FAILURES (1):"), "got: {}", out);
        assert!(out.contains("broken.phpt"), "got: {}", out);
    }

    #[test]
    fn test_no_tests_ran() {
        let input = "\
=====================================================================
PHP_VERSION : 8.4.20
=====================================================================
No tests matched selector.
";
        let out = filter_phpt_output(input);
        assert!(out.contains("no tests ran"), "got: {}", out);
    }

    #[test]
    fn test_serial_run_progress_carriage_returns() {
        // Without -j, run-tests.php redraws the progress line: the status token
        // lands after a CR, not after the test bracket.
        let input = "\
=====================================================================
PHP_VERSION : 8.6.0-dev
PHP_SAPI    : cli
PHP_OS      : Linux - Linux box 6.18.0 x86_64
=====================================================================
TEST 1/2 [bad.phpt]\rFAIL failing sample [bad.phpt] \nTEST 2/2 [good.phpt]\rPASS passing sample [good.phpt] \n\
=====================================================================
Number of tests :     2                 2
Tests failed    :     1 ( 50.0%) ( 50.0%)
Tests passed    :     1 ( 50.0%) ( 50.0%)
Time taken      : 0.025 seconds
";
        let out = filter_phpt_output(input);
        assert!(out.starts_with("phpt: 1 passed, 1 failed"), "got: {}", out);
        assert!(out.contains("FAILURES (1):"), "got: {}", out);
        assert!(out.contains("bad.phpt -- failing sample"), "got: {}", out);
        assert!(!out.contains("unavailable"), "got: {}", out);
    }

    #[test]
    fn test_unparseable_output_passes_through() {
        // php itself failed before run-tests.php produced anything (wrong cwd).
        let input = "Could not open input file: run-tests.php\n";
        assert_eq!(
            filter_phpt_output(input),
            "Could not open input file: run-tests.php"
        );
    }

    #[test]
    fn test_failed_test_summary_block_is_not_mistaken_for_status() {
        // run-tests.php emits a FAILED TEST SUMMARY block at the very end that
        // lists failing tests in `description [path]` form with no status tag.
        // Our regex must ignore those lines to avoid double-counting.
        let input = "\
=====================================================================
PHP_VERSION : 8.4.20
=====================================================================
TEST 1/2 [a.phpt]\x1b[1;32mPASS\x1b[0m A [a.phpt]
TEST 2/2 [b.phpt]
========DIFF========
001- a
001+ b
========DONE========
\x1b[1;31mFAIL\x1b[0m Real failure [b.phpt]
=====================================================================
Number of tests :    2              2
Tests failed    :    1
Tests passed    :    1
=====================================================================

=====================================================================
FAILED TEST SUMMARY
---------------------------------------------------------------------
Real failure [b.phpt]
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(out.contains("FAILURES (1):"), "got: {}", out);
        // Must not double-count the failure.
        assert!(!out.contains("FAILURES (2):"), "got: {}", out);
    }

    #[test]
    fn test_ansi_stripping() {
        let input = "\
=====================================================================
PHP_VERSION : 8.4.20
=====================================================================
TEST 1/1 [a.phpt]\x1b[1;32mPASS\x1b[0m description [a.phpt]
=====================================================================
Tests passed    :    1
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(!out.contains('\x1b'), "ansi leaked: {:?}", out);
    }

    #[test]
    fn test_compression_ratio_on_large_pass_run() {
        // Synthesize an all-pass run and confirm the filter collapses it heavily.
        let mut input = String::from(
            "=====================================================================\n\
             PHP_VERSION : 8.4.20\n\
             PHP_SAPI    : cli\n\
             PHP_OS      : Linux\n\
             =====================================================================\n",
        );
        for i in 1..=1000 {
            input.push_str(&format!(
                "TEST {}/1000 [tests/file_{}.phpt]\x1b[1;32mPASS\x1b[0m description line for test {} [tests/file_{}.phpt]\n",
                i, i, i, i
            ));
        }
        input.push_str("=====================================================================\n");
        input.push_str("Number of tests :  1000              1000\n");
        input.push_str("Tests passed    :  1000\n");
        input.push_str("Time taken      :   1.0 seconds\n");
        input.push_str("=====================================================================\n");

        let out = filter_phpt_output(&input);
        let ratio = out.len() as f64 / input.len() as f64;
        assert!(ratio < 0.01, "expected >99% reduction, ratio={:.4}", ratio);
    }

    #[test]
    fn test_phpt_token_savings() {
        use crate::core::utils::count_tokens;

        // A realistic run: mostly passing with a handful of failures + diffs,
        // which is the worst case for savings (diffs are the only kept payload).
        let mut input = String::from(
            "=====================================================================\n\
             PHP_SAPI    : cli\n\
             PHP_VERSION : 8.4.20\n\
             PHP_OS      : Linux\n\
             =====================================================================\n\
             Running selected tests.\n",
        );
        for i in 1..=100 {
            if i % 25 == 0 {
                input.push_str(&format!("TEST {}/100 [tests/f{}.phpt]\n", i, i));
                input.push_str("========DIFF========\n");
                input.push_str(&format!("001- expected value {}\n", i));
                input.push_str(&format!("001+ actual value {}\n", i));
                input.push_str("========DONE========\n");
                input.push_str(&format!(
                    "\x1b[1;31mFAIL\x1b[0m regression in feature {} [tests/f{}.phpt]\n",
                    i, i
                ));
            } else {
                input.push_str(&format!(
                    "TEST {}/100 [tests/f{}.phpt]\x1b[1;32mPASS\x1b[0m feature {} works [tests/f{}.phpt]\n",
                    i, i, i, i
                ));
            }
        }
        input.push_str("=====================================================================\n");
        input.push_str("Number of tests :  100              100\n");
        input.push_str("Tests failed    :    4\n");
        input.push_str("Tests passed    :   96\n");
        input.push_str("Time taken      :   3.0 seconds\n");
        input.push_str("=====================================================================\n");

        let out = filter_phpt_output(&input);
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(&input) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "expected ≥60% savings, got {:.1}%\n{}",
            savings,
            out
        );
    }

    #[test]
    fn test_env_with_os_kernel_trim() {
        let input = "\
=====================================================================
PHP_VERSION : 8.4.20
PHP_OS      : Linux - Linux LAPTOP 6.6.87-microsoft-standard-WSL2 #1 SMP PREEMPT_DYNAMIC x86_64
=====================================================================
TEST 1/1 [a.phpt]\x1b[1;32mPASS\x1b[0m A [a.phpt]
=====================================================================
Tests passed    :    1
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(
            out.contains("OS Linux\n") || out.ends_with("OS Linux"),
            "got: {}",
            out
        );
        assert!(!out.contains("PREEMPT_DYNAMIC"), "got: {}", out);
    }

    #[test]
    fn test_summary_counts_override_scanned() {
        // If summary and scanned disagree (truncated capture), we prefer summary.
        let input = "\
=====================================================================
PHP_VERSION : 8.4.20
=====================================================================
TEST 1/100 [a.phpt]\x1b[1;32mPASS\x1b[0m A [a.phpt]
=====================================================================
Number of tests :  100              100
Tests failed    :   10
Tests passed    :   90
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(out.contains("90 passed"), "got: {}", out);
        assert!(out.contains("10 failed"), "got: {}", out);
        // No diffs captured but count was overridden from summary.
        assert!(out.contains("(100 total"), "got: {}", out);
    }

    #[test]
    fn test_broken_without_captured_failures_still_reports() {
        // Summary says tests failed, but the per-test lines were truncated out
        // of the capture, so `failures` is empty. We must still emit a FAILURES
        // header (with a note) instead of silently dropping it.
        let input = "\
=====================================================================
PHP_VERSION : 8.4.20
=====================================================================
Number of tests :  100              100
Tests failed    :   10
Tests passed    :   90
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(out.contains("FAILURES (10):"), "got: {}", out);
        assert!(out.contains("per-test details unavailable"), "got: {}", out);
    }

    #[test]
    fn test_xleak_is_expected_not_failure() {
        // XLEAK (expected leak, valgrind runs) is reported under "Expected leak"
        // by run-tests.php and must not count toward failures.
        let input = "\
=====================================================================
PHP_VERSION : 8.4.20
=====================================================================
TEST 1/2 [a.phpt]\x1b[1;32mPASS\x1b[0m A [a.phpt]
TEST 2/2 [b.phpt]\x1b[1;33mXLEAK\x1b[0m Known leak [b.phpt]
=====================================================================
Number of tests :    2              2
Tests leaked    :    0
Expected leak   :    1
Tests passed    :    1
=====================================================================
";
        let out = filter_phpt_output(input);
        assert!(out.contains("1 xleaked"), "got: {}", out);
        assert!(!out.contains("FAILURES"), "got: {}", out);
    }

    #[test]
    fn test_mixed_run() {
        // Full end-to-end fixture of a representative run: pass, skip, xfail,
        // and a failure with a diff. Guards the exact output format.
        let input = "\
=====================================================================
PHP         : /usr/bin/php8.4
PHP_SAPI    : cli
PHP_VERSION : 8.4.20
ZEND_VERSION: 4.4.20
PHP_OS      : Linux - Linux host 6.6
=====================================================================
Running selected tests.
TEST 1/4 [Zend/tests/a.phpt]\x1b[1;32mPASS\x1b[0m Test A [Zend/tests/a.phpt]
TEST 2/4 [Zend/tests/b.phpt]\x1b[1;33mSKIP\x1b[0m Test B [Zend/tests/b.phpt] reason: missing ext
TEST 3/4 [Zend/tests/c.phpt]\x1b[1;33mXFAIL\x1b[0m Test C [Zend/tests/c.phpt]
TEST 4/4 [Zend/tests/d.phpt]
========DIFF========
001- int(1)
001+ int(2)
========DONE========
\x1b[1;31mFAIL\x1b[0m Bug #123 [Zend/tests/d.phpt]
=====================================================================
Number of tests :    4              4
Tests skipped   :    1
Tests warned    :    0
Tests failed    :    1
Expected fail   :    1
Tests passed    :    1
---------------------------------------------------------------------
Time taken      :   2.500 seconds
=====================================================================
";
        let expected = "\
phpt: 1 passed, 1 failed, 1 skipped, 1 xfailed  (4 total, 2.5s)
PHP 8.4.20  SAPI cli  OS Linux

FAILURES (1):
  Zend/tests/d.phpt -- Bug #123
    001- int(1)
    001+ int(2)";

        let out = filter_phpt_output(input);
        assert_eq!(out, expected, "got: {}", out);
    }
}
