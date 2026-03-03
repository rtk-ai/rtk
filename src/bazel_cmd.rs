use crate::tracking;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::process::Command;
use std::str::FromStr;

/**********************************************************************/
/*                       Shared Bazel Utilities                       */
/**********************************************************************/

lazy_static! {
    /// Matches optional leading Bazel timestamp prefix: "(HH:MM:SS) "
    static ref TIMESTAMP_PREFIX: Regex =
        Regex::new(r"^\(\d+:\d+:\d+\)\s*").unwrap();

    /// Matches Bazel target lines
    ///
    /// e.g. "//package/path:target_name", "//:root_target", "@repo//pkg:target"
    static ref TARGET_LINE: Regex =
        Regex::new(r"^((?:@[^/\s:]+)?//[^:]*):(.+)$").unwrap();

    /// Matches Bazel progress lines
    ///
    /// e.g. "[123 / 4,567] Progress message..."
    static ref PROGRESS_LINE: Regex =
        Regex::new(r"^\[[\d,]+ / [\d,]+\]").unwrap();

    /// Matches INFO lines with action counts
    ///
    /// e.g. "123 total actions", "1 total action"
    static ref ACTION_COUNT: Regex =
        Regex::new(r"(\d[\d,]*)\s+total actions?").unwrap();

    /// Matches test result lines
    ///
    /// e.g. "//pkg:test PASSED in 0.3s", "//pkg:test (cached) PASSED in 0.3s"
    static ref TEST_RESULT_LINE: Regex =
        Regex::new(r"^(//\S+)\s+(?:\(cached\)\s+)?(PASSED|FAILED|TIMEOUT|FLAKY|NO STATUS)\s+in\s+([\d.]+)s").unwrap();

    /// Matches the Executed summary line
    ///
    /// e.g. "Executed 3 out of 3 tests: 3 tests pass."
    static ref TEST_SUMMARY: Regex =
        Regex::new(r"^Executed\s+(\d+)\s+out\s+of\s+(\d+)\s+tests?:").unwrap();

    /// Matches test output delimiters
    ///
    /// e.g. "==================== Test output for //pkg:test:"
    static ref TEST_OUTPUT_START: Regex =
        Regex::new(r"^={10,}\s+Test output for\s+").unwrap();

    /// Matches test output end delimiter
    ///
    /// e.g. "================================================================================"
    static ref TEST_OUTPUT_END: Regex =
        Regex::new(r"^={40,}$").unwrap();

    /// Matches FAIL: lines with target
    ///
    /// e.g. "FAIL: //pkg:test (Exit 1) (see /path/to/test.log)"
    static ref FAIL_LINE: Regex =
        Regex::new(r"^FAIL:\s+(//\S+)").unwrap();

    /// Matches elapsed time from INFO lines
    ///
    /// e.g. "INFO: Elapsed time: 3.89s, Critical Path: 1.23s"
    static ref ELAPSED_TIME: Regex =
        Regex::new(r"Elapsed time:\s*([\d.]+)s").unwrap();

    /// Matches the "Running command line:" sentinel that separates build from execution
    ///
    /// e.g. "INFO: Running command line: bazel-bin/path/to/binary"
    /// Note: timestamp prefix is already stripped by strip_timestamp() before matching
    static ref RUN_SENTINEL: Regex =
        Regex::new(r"^INFO: Running command line:").unwrap();
}

/// Strip optional leading Bazel timestamp prefix "(HH:MM:SS) " from a line.
///
/// Bazel may prepend timestamps to all output lines (e.g. `(17:17:06) Loading:`).
/// This normalizes them so `starts_with` checks work regardless of timestamp presence.
fn strip_timestamp(line: &str) -> &str {
    TIMESTAMP_PREFIX
        .find(line)
        .map(|m| &line[m.end()..])
        .unwrap_or(line)
}

/// A limit value that can be a specific number or unlimited ("all").
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Limit {
    /// Fixed size
    N(usize),

    /// Unlimited
    All,
}

impl Limit {
    pub fn value(&self) -> usize {
        match self {
            Limit::N(n) => *n,
            Limit::All => usize::MAX,
        }
    }
}

impl FromStr for Limit {
    type Err = String;

    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        if s.eq_ignore_ascii_case("all") {
            Ok(Limit::All)
        } else {
            s.parse::<usize>()
                .map(Limit::N)
                .map_err(|_| format!("expected a number or 'all', got '{}'", s))
        }
    }
}

impl fmt::Display for Limit {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Limit::N(n) => write!(f, "{}", n),
            Limit::All => write!(f, "all"),
        }
    }
}

/**********************************************************************/
/*                            bazel build                             */
/**********************************************************************/

/// Filter `bazel build` output.
///
/// # Arguments
///
/// * `stdout` - stdout output from `bazel build`
/// * `stderr` - stderr output from `bazel build`
///
/// # Returns
///
/// The filtered `bazel build` output
///
/// # Notes
///
/// Strips progress and info noise, while keeping errors and warnings.
/// Bazel sends most output to stderr. This function reads the combined
/// stdout and stderr stream and filters the following:
/// * Progress lines `[N / M]`
/// * Loading/Analyzing
/// * INFO
/// * Note
/// * Target/bazel-bin output paths
///
/// Meanwhile, the following lines are kept:
/// * ERROR lines
/// * WARNING lines
/// * Build diagnostics (e.g. warnings/errors from gcc/clang)
///
pub fn filter_bazel_build(stdout: &str, stderr: &str) -> String {
    let mut errors: Vec<String> = Vec::new();
    let mut warnings: Vec<String> = Vec::new();
    let mut error_count: usize = 0;
    let mut warning_count: usize = 0;
    let mut action_count: Option<String> = None;

    // State for collecting multi-line compiler diagnostic blocks
    let mut in_diagnostic = false;
    let mut current_block: Vec<String> = Vec::new();
    let mut current_is_error = false;

    // Combine stdout and stderr (bazel sends most to stderr)
    let combined = format!("{}\n{}", stderr, stdout);

    for line in combined.lines() {
        let trimmed = line.trim();
        // Strip optional "(HH:MM:SS) " timestamp prefix so starts_with checks work
        let stripped = strip_timestamp(trimmed);
        if stripped.is_empty() {
            // Blank line ends a diagnostic block
            if in_diagnostic && !current_block.is_empty() {
                if current_is_error {
                    errors.push(current_block.join("\n"));
                } else {
                    warnings.push(current_block.join("\n"));
                }
                current_block.clear();
                in_diagnostic = false;
            }
            continue;
        }

        // Extract action count from INFO lines before skipping them
        if stripped.starts_with("INFO:") || stripped.starts_with("DEBUG:") {
            if let Some(caps) = ACTION_COUNT.captures(stripped) {
                action_count = Some(caps[1].to_string());
            }
            // "INFO: From ..." lines precede compiler output — skip the INFO line itself
            // but don't skip the following compiler diagnostic lines
            continue;
        }

        // Strip progress lines: [N / M] ...
        if PROGRESS_LINE.is_match(stripped) {
            continue;
        }

        // Strip loading/analyzing status
        if stripped.starts_with("Loading:")
            || stripped.starts_with("Analyzing:")
            || stripped.starts_with("Computing main repo mapping:")
        {
            continue;
        }

        // Strip Java notes
        if stripped.starts_with("Note: ") {
            continue;
        }

        // Strip target output paths
        if stripped.starts_with("Target //") || stripped.starts_with("bazel-bin/") {
            continue;
        }

        // Strip DEBUG lines
        if stripped.starts_with("DEBUG:") {
            continue;
        }

        // Bazel-level ERROR lines
        if stripped.starts_with("ERROR:") {
            // Flush any in-progress diagnostic block
            if in_diagnostic && !current_block.is_empty() {
                if current_is_error {
                    errors.push(current_block.join("\n"));
                } else {
                    warnings.push(current_block.join("\n"));
                }
                current_block.clear();
                in_diagnostic = false;
            }
            // Skip the summary "Build did NOT complete successfully" — we show our own header
            if stripped.contains("Build did NOT complete successfully") {
                error_count = error_count.max(1); // ensure we show error header
                continue;
            }
            error_count += 1;
            errors.push(stripped.to_string());
            continue;
        }

        // Bazel-level WARNING lines (already caught above in INFO/WARNING/DEBUG gate,
        // but standalone WARNING lines without prior INFO context reach here)
        if stripped.starts_with("WARNING:") {
            // Flush any in-progress diagnostic block
            if in_diagnostic && !current_block.is_empty() {
                if current_is_error {
                    errors.push(current_block.join("\n"));
                } else {
                    warnings.push(current_block.join("\n"));
                }
                current_block.clear();
                in_diagnostic = false;
            }
            warning_count += 1;
            warnings.push(stripped.to_string());
            continue;
        }

        // Compiler diagnostic: "file:line:col: warning: ..." or "file:line:col: error: ..."
        // These come from gcc/clang output embedded in bazel stderr
        if trimmed.contains(": warning:") || trimmed.contains(": error:") {
            // Flush previous block if any
            if in_diagnostic && !current_block.is_empty() {
                if current_is_error {
                    errors.push(current_block.join("\n"));
                } else {
                    warnings.push(current_block.join("\n"));
                }
                current_block.clear();
            }
            current_is_error = trimmed.contains(": error:");
            if current_is_error {
                error_count += 1;
            } else {
                warning_count += 1;
            }
            in_diagnostic = true;
            current_block.push(trimmed.to_string());
            continue;
        }

        // Continuation of a compiler diagnostic block (source context, notes, etc.)
        if in_diagnostic {
            // Lines with ` | `, `note:`, source locations, or caret lines are context
            current_block.push(trimmed.to_string());
            continue;
        }

        // Anything else that doesn't match known noise — skip
        // (indented bazel-bin paths, etc.)
    }

    // Flush final block
    if in_diagnostic && !current_block.is_empty() {
        if current_is_error {
            errors.push(current_block.join("\n"));
        } else {
            warnings.push(current_block.join("\n"));
        }
    }

    let actions_str = action_count.unwrap_or_else(|| "0".to_string());

    // No errors or warnings: one-liner success
    if error_count == 0 && warning_count == 0 {
        return format!("✓ bazel build ({} actions)", actions_str);
    }

    // Format with header + blocks
    let mut result = String::new();
    result.push_str(&format!(
        "bazel build: {} error{}, {} warning{} ({} actions)\n",
        error_count,
        if error_count == 1 { "" } else { "s" },
        warning_count,
        if warning_count == 1 { "" } else { "s" },
        actions_str,
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Show errors first, then warnings
    let all_blocks: Vec<&String> = errors.iter().chain(warnings.iter()).collect();
    for (i, block) in all_blocks.iter().enumerate().take(15) {
        result.push_str(block);
        result.push('\n');
        if i < all_blocks.len().min(15) - 1 {
            result.push('\n');
        }
    }

    if all_blocks.len() > 15 {
        result.push_str(&format!("\n... +{} more issues\n", all_blocks.len() - 15));
    }

    result.trim().to_string()
}

/// Run `bazel build` while filtering the output.
///
/// # Arguments
///
/// * `args` - Arguments to pass to `bazel build`
/// * `verbose` - Verbosity level
///
/// # Returns
///
/// Result of the operation
///
pub fn run_build(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("bazel");
    cmd.arg("build");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: bazel build {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run bazel build. Is Bazel installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = filter_bazel_build(&stdout, &stderr);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "bazel_build", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("bazel build {}", args.join(" ")),
        &format!("rtk bazel build {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/**********************************************************************/
/*                            bazel test                              */
/**********************************************************************/

/// Filter `bazel test` output.
///
/// # Arguments
///
/// * `stdout` - stdout output from `bazel test`
/// * `stderr` - stderr output from `bazel test`
///
/// # Returns
///
/// The filtered `bazel test` output
///
/// # Notes
///
/// Strips the same build noise as `filter_bazel_build`, plus parses test
/// result lines (PASSED/FAILED/TIMEOUT) and inline test output blocks.
/// On all-pass, returns a one-liner. On failure, shows FAIL blocks and
/// inline test output while stripping surrounding noise.
///
pub fn filter_bazel_test(stdout: &str, stderr: &str) -> String {
    let mut passed: usize = 0;
    let mut failed: usize = 0;
    let mut elapsed: Option<String> = None;
    let mut error_lines: Vec<String> = Vec::new();
    let mut fail_blocks: Vec<String> = Vec::new();
    let mut failed_result_lines: Vec<String> = Vec::new();
    let mut inline_output_blocks: Vec<String> = Vec::new();

    // State for collecting inline test output
    let mut in_test_output = false;
    let mut current_output_block: Vec<String> = Vec::new();

    // Combine stderr + stdout (bazel sends most to stderr)
    let combined = format!("{}\n{}", stderr, stdout);

    for line in combined.lines() {
        let trimmed = line.trim();
        let stripped = strip_timestamp(trimmed);

        // Collecting inline test output between delimiter lines
        if in_test_output {
            if TEST_OUTPUT_END.is_match(stripped) {
                current_output_block.push(stripped.to_string());
                inline_output_blocks.push(current_output_block.join("\n"));
                current_output_block.clear();
                in_test_output = false;
            } else {
                current_output_block.push(line.to_string());
            }
            continue;
        }

        if stripped.is_empty() {
            continue;
        }

        // Extract elapsed time before skipping INFO/DEBUG lines
        if stripped.starts_with("INFO:") || stripped.starts_with("DEBUG:") {
            if let Some(caps) = ELAPSED_TIME.captures(stripped) {
                elapsed = Some(caps[1].to_string());
            }
            continue;
        }

        // Strip progress lines: [N / M] ...
        if PROGRESS_LINE.is_match(stripped) {
            continue;
        }

        // Strip loading/analyzing status
        if stripped.starts_with("Loading:")
            || stripped.starts_with("Analyzing:")
            || stripped.starts_with("Computing main repo mapping:")
        {
            continue;
        }

        // Strip Java notes
        if stripped.starts_with("Note: ") {
            continue;
        }

        // Strip target output paths
        if stripped.starts_with("Target //") || stripped.starts_with("bazel-bin/") {
            continue;
        }

        // Strip DEBUG lines
        if stripped.starts_with("DEBUG:") {
            continue;
        }

        // Strip timeout size warnings
        if stripped.starts_with("There were tests whose specified size") {
            continue;
        }

        // Test result lines: //pkg:test PASSED in 0.3s
        if let Some(caps) = TEST_RESULT_LINE.captures(stripped) {
            let status = &caps[2];
            match status {
                "PASSED" => passed += 1,
                "FAILED" | "TIMEOUT" | "NO STATUS" => {
                    failed += 1;
                    failed_result_lines.push(stripped.to_string());
                }
                "FLAKY" => passed += 1, // flaky but passed on retry
                _ => {}
            }
            continue;
        }

        // Executed summary line (skip — we produce our own)
        if TEST_SUMMARY.is_match(stripped) {
            continue;
        }

        // FAIL: lines
        if FAIL_LINE.is_match(stripped) {
            fail_blocks.push(stripped.to_string());
            continue;
        }

        // Inline test output start
        if TEST_OUTPUT_START.is_match(stripped) {
            in_test_output = true;
            current_output_block.push(stripped.to_string());
            continue;
        }

        // ERROR lines
        if stripped.starts_with("ERROR:") {
            if stripped.contains("Build did NOT complete successfully")
                || stripped.contains("not all tests passed")
            {
                continue;
            }
            error_lines.push(stripped.to_string());
            continue;
        }

        // WARNING lines (strip — build noise)
        if stripped.starts_with("WARNING:") {
            continue;
        }

        // Indented log paths after FAILED lines (e.g. "  /path/to/test.log")
        // Keep only if we have failures
        if stripped.starts_with('/') && stripped.ends_with(".log") && failed > 0 {
            continue; // skip log paths — we show inline output instead
        }

        // Everything else is noise — skip
    }

    // Flush any unclosed test output block
    if !current_output_block.is_empty() {
        inline_output_blocks.push(current_output_block.join("\n"));
    }

    let elapsed_str = elapsed.unwrap_or_else(|| "0".to_string());

    // Build error — no test results but ERROR lines present
    if passed == 0 && failed == 0 && !error_lines.is_empty() {
        let mut result = String::from("bazel test: build failed\n");
        result.push_str("═══════════════════════════════════════\n");
        for err in error_lines.iter().take(15) {
            result.push_str(err);
            result.push('\n');
        }
        if error_lines.len() > 15 {
            result.push_str(&format!("\n... +{} more errors\n", error_lines.len() - 15));
        }
        return result.trim().to_string();
    }

    // All pass: one-liner
    if failed == 0 {
        return format!(
            "\u{2713} bazel test: {} passed, 0 failed ({}s)",
            passed, elapsed_str
        );
    }

    // Has failures: show details
    let mut result = String::new();
    result.push_str(&format!(
        "bazel test: {} failed, {} passed ({}s)\n",
        failed, passed, elapsed_str
    ));
    result.push_str("═══════════════════════════════════════\n");

    // FAIL: lines
    let mut block_count = 0;
    for fail in &fail_blocks {
        if block_count >= 15 {
            break;
        }
        result.push_str(fail);
        result.push('\n');
        block_count += 1;
    }

    // Inline test output blocks
    for block in &inline_output_blocks {
        if block_count >= 15 {
            break;
        }
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(block);
        result.push('\n');
        block_count += 1;
    }

    // FAILED result lines
    for line in &failed_result_lines {
        if block_count >= 15 {
            break;
        }
        if !result.ends_with('\n') {
            result.push('\n');
        }
        result.push_str(line);
        result.push('\n');
        block_count += 1;
    }

    // Error lines (if any)
    for err in &error_lines {
        if block_count >= 15 {
            break;
        }
        result.push_str(err);
        result.push('\n');
        block_count += 1;
    }

    let total_blocks = fail_blocks.len()
        + inline_output_blocks.len()
        + failed_result_lines.len()
        + error_lines.len();
    if total_blocks > 15 {
        result.push_str(&format!("\n... +{} more blocks\n", total_blocks - 15));
    }

    result.trim().to_string()
}

/// Run `bazel test` while filtering the output.
///
/// # Arguments
///
/// * `args` - Arguments to pass to `bazel test`
/// * `verbose` - Verbosity level
///
/// # Returns
///
/// Result of the operation
///
pub fn run_test(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("bazel");
    cmd.arg("test");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: bazel test {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run bazel test. Is Bazel installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = filter_bazel_test(&stdout, &stderr);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "bazel_test", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("bazel test {}", args.join(" ")),
        &format!("rtk bazel test {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/**********************************************************************/
/*                            bazel run                               */
/**********************************************************************/

/// Filter `bazel run` output.
///
/// # Arguments
///
/// * `stdout` - stdout output from `bazel run` (binary's stdout)
/// * `stderr` - stderr output from `bazel run` (build noise + binary's stderr)
///
/// # Returns
///
/// The filtered output: build summary + binary output (forwarded verbatim)
///
/// # Notes
///
/// `bazel run` builds a target then executes it. The build phase produces
/// noise on stderr identical to `bazel build`. After building, bazel prints
/// a sentinel line `INFO: Running command line: ...` then exec's the binary.
/// Everything after the sentinel in stderr is the binary's stderr output.
/// All of stdout is the binary's stdout (bazel writes nothing to stdout).
///
/// This filter splits stderr at the sentinel, applies `filter_bazel_build`
/// to the build phase, then appends the binary's output verbatim.
///
pub fn filter_bazel_run(stdout: &str, stderr: &str, args: &[String]) -> String {
    // Split stderr at the sentinel line, collecting warnings separately
    let mut build_stderr = String::new();
    let mut build_warnings: Vec<String> = Vec::new();
    let mut binary_stderr = String::new();
    let mut found_sentinel = false;
    let mut has_errors = false;

    for segment in stderr.split_inclusive('\n') {
        let line = segment.strip_suffix('\n').unwrap_or(segment);
        let stripped = strip_timestamp(line.trim());
        if !found_sentinel {
            if RUN_SENTINEL.is_match(stripped) {
                found_sentinel = true;
                continue;
            }
            // Collect warnings separately — only include if build has errors
            if stripped.starts_with("WARNING:") {
                build_warnings.push(line.to_string());
                continue;
            }
            if stripped.starts_with("ERROR:") {
                has_errors = true;
            }
            build_stderr.push_str(segment);
        } else {
            // Post-sentinel stderr belongs to the executed binary; preserve it verbatim.
            binary_stderr.push_str(segment);
        }
    }

    // Re-inject warnings if build had errors (they provide context)
    if has_errors {
        for w in &build_warnings {
            build_stderr.push_str(w);
            build_stderr.push('\n');
        }
    }

    // Filter the build phase using existing filter_bazel_build
    let build_summary = filter_bazel_build("", &build_stderr);

    // Combine binary output exactly as captured from stdout + post-sentinel stderr.
    let mut binary_output = String::new();
    binary_output.push_str(stdout);
    binary_output.push_str(&binary_stderr);

    // Format output based on build result
    let build_clean = build_summary.starts_with('\u{2713}');

    if binary_output.is_empty() {
        // No binary output — show build summary only
        build_summary
    } else if build_clean {
        // Clean build — skip build summary, just show binary output
        binary_output
    } else {
        // Build had warnings/errors — show both sections
        let run_header = format!(
            "\n\nbazel run {}\n═══════════════════════════════════════",
            args.join(" ")
        );
        format!("{}{}\n{}", build_summary, run_header, binary_output)
    }
}

/// Run `bazel run` while filtering the build output.
///
/// # Arguments
///
/// * `args` - Arguments to pass to `bazel run`
/// * `verbose` - Verbosity level
///
/// # Returns
///
/// Result of the operation
///
pub fn run_run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("bazel");
    cmd.arg("run");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: bazel run {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run bazel run. Is Bazel installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = filter_bazel_run(&stdout, &stderr, args);

    if let Some(hint) = crate::tee::tee_and_hint(&raw, "bazel_run", exit_code) {
        println!("{}\n{}", filtered, hint);
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("bazel run {}", args.join(" ")),
        &format!("rtk bazel run {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/**********************************************************************/
/*                            bazel query                             */
/**********************************************************************/

#[derive(Debug, Default)]
struct PackageStats {
    // Targets defined directly in this package.
    direct_targets: Vec<String>,
    // Immediate child package names, sorted.
    child_names: std::collections::BTreeSet<String>,
    // Cumulative targets in this package subtree (including this package).
    cumulative_targets: usize,
    // Number of descendant packages (excluding this package).
    cumulative_packages: usize,
    // Maximum descendant depth from this package (0 means no child packages).
    max_depth: usize,
}

#[derive(Debug, Default)]
struct PackageIndex {
    // Keyed by canonical package path like "//", "//src", "//src/main".
    nodes: BTreeMap<String, PackageStats>,
}

/// Build a compact package index from flat package->targets data.
///
/// This is a single aggregation pass: each package contributes to itself and
/// all ancestor prefixes, so cumulative counts are precomputed for rendering.
fn build_package_index(packages: &BTreeMap<String, Vec<String>>) -> PackageIndex {
    let mut index = PackageIndex::default();

    for (package, targets) in packages {
        let stripped = package.strip_prefix("//").unwrap_or(package);
        let parts: Vec<&str> = if stripped.is_empty() {
            Vec::new()
        } else {
            stripped.split('/').collect()
        };

        let full_path = if parts.is_empty() {
            "//".to_string()
        } else {
            format!("//{}", parts.join("/"))
        };
        index
            .nodes
            .entry(full_path)
            .or_default()
            .direct_targets
            .extend(targets.iter().cloned());

        let target_count = targets.len();
        for depth in 0..=parts.len() {
            let ancestor = if depth == 0 {
                "//".to_string()
            } else {
                format!("//{}", parts[..depth].join("/"))
            };

            let stats = index.nodes.entry(ancestor).or_default();
            stats.cumulative_targets += target_count;

            let relative_depth = parts.len().saturating_sub(depth);
            stats.max_depth = stats.max_depth.max(relative_depth);

            if depth < parts.len() {
                stats.child_names.insert(parts[depth].to_string());
            }
        }
    }

    // Compute descendant package counts from the child graph so intermediate
    // prefixes are counted uniformly (matching previous tree semantics).
    let mut memo: HashMap<String, usize> = HashMap::new();
    let keys: Vec<String> = index.nodes.keys().cloned().collect();
    for key in keys {
        let count = cumulative_packages_for(&index, &key, &mut memo);
        if let Some(stats) = index.nodes.get_mut(&key) {
            stats.cumulative_packages = count;
        }
    }

    index
}

fn cumulative_packages_for(
    index: &PackageIndex,
    path: &str,
    memo: &mut HashMap<String, usize>,
) -> usize {
    if let Some(&cached) = memo.get(path) {
        return cached;
    }

    let Some(node) = index.nodes.get(path) else {
        memo.insert(path.to_string(), 0);
        return 0;
    };

    let child_names: Vec<String> = node.child_names.iter().cloned().collect();
    let mut total = child_names.len();
    for child in child_names {
        total += cumulative_packages_for(index, &child_path(path, &child), memo);
    }

    memo.insert(path.to_string(), total);
    total
}

/// Format a count label like "5 targets" or "1 target", with optional package count.
fn format_counts(target_count: usize, package_count: usize) -> String {
    let mut parts = Vec::new();

    if target_count > 0 {
        let label = if target_count == 1 {
            "target"
        } else {
            "targets"
        };
        parts.push(format!("{} {}", target_count, label));
    }

    if package_count > 0 {
        let label = if package_count == 1 {
            "package"
        } else {
            "packages"
        };
        parts.push(format!("{} {}", package_count, label));
    }

    if parts.is_empty() {
        "0 targets".to_string()
    } else {
        parts.join(", ")
    }
}

fn child_path(parent: &str, child: &str) -> String {
    if parent == "//" {
        format!("//{}", child)
    } else {
        format!("{}/{}", parent, child)
    }
}

/// Render one section body: immediate child packages, then immediate targets.
fn render_section_body(
    index: &PackageIndex,
    section_path: &str,
    width: usize,
    result: &mut String,
) {
    let empty = PackageStats::default();
    let node = index.nodes.get(section_path).unwrap_or(&empty);
    let child_count = node.child_names.len();
    let target_count = node.direct_targets.len();

    // Width budget: sub-packages first, then targets
    let pkg_slots = width.min(child_count);
    let remaining_slots = width.saturating_sub(pkg_slots);
    let target_slots = remaining_slots.min(target_count);

    let hidden_packages = child_count.saturating_sub(pkg_slots);
    let hidden_targets = target_count.saturating_sub(target_slots);

    // Render sub-packages
    for (i, name) in node.child_names.iter().enumerate() {
        if i >= pkg_slots {
            break;
        }
        let child_key = child_path(section_path, name);
        let child_stats = index.nodes.get(&child_key).unwrap_or(&empty);
        let cum_targets = child_stats.cumulative_targets;
        let cum_packages = child_stats.cumulative_packages;
        let counts = format_counts(cum_targets, cum_packages);
        result.push_str(&format!("📦 {} ({})\n", name, counts));
    }

    // Render targets
    for (i, target) in node.direct_targets.iter().enumerate() {
        if i >= target_slots {
            break;
        }
        result.push_str(&format!("🎯 :{}\n", target));
    }

    // Truncation line
    if hidden_packages > 0 || hidden_targets > 0 {
        let mut parts = Vec::new();
        if hidden_packages > 0 {
            parts.push(format!(
                "{} more sub-package{}",
                hidden_packages,
                if hidden_packages == 1 { "" } else { "s" }
            ));
        }
        if hidden_targets > 0 {
            parts.push(format!(
                "{} more target{}",
                hidden_targets,
                if hidden_targets == 1 { "" } else { "s" }
            ));
        }
        result.push_str(&format!("(+{})\n", parts.join(", ")));
    }
}

fn render_query_section(
    result: &mut String,
    packages: &BTreeMap<String, Vec<String>>,
    depth: usize,
    width: usize,
    header_label: &str,
    root_path: &str,
    external_repo: Option<&str>,
) {
    let index = build_package_index(packages);
    let empty = PackageStats::default();
    let root_node = index.nodes.get(root_path).unwrap_or(&empty);

    let effective_depth = depth.min(root_node.max_depth.saturating_add(1));
    if effective_depth <= 1 {
        let total_targets = root_node.cumulative_targets;
        let total_packages = root_node.cumulative_packages;
        let counts = format_counts(total_targets, total_packages);
        result.push_str(&format!("{} ({})\n", header_label, counts));
        render_section_body(&index, root_path, width, result);
        return;
    }

    let mut sections: Vec<SectionNode> = Vec::new();
    collect_section_nodes(&index, root_path, 0, effective_depth, &mut sections);

    let mut rendered_sections = 0usize;
    for section in &sections {
        let stats = index.nodes.get(&section.path).unwrap_or(&empty);
        let is_leaf_section = section.level + 1 == effective_depth;
        let target_count = if is_leaf_section {
            stats.cumulative_targets
        } else {
            stats.direct_targets.len()
        };
        let package_count = if is_leaf_section {
            stats.cumulative_packages
        } else {
            0
        };

        // Skip empty intermediate headers (or any section with no visible content).
        if target_count == 0 && package_count == 0 {
            continue;
        }

        if rendered_sections > 0 {
            result.push('\n');
        }
        let counts = format_counts(target_count, package_count);
        let label = format_query_section_label(&section.path, external_repo);
        result.push_str(&format!("{} ({})\n", label, counts));

        if is_leaf_section {
            // At the final expanded depth, show one level of package/target items.
            render_section_body(&index, &section.path, width, result);
        } else {
            render_targets_only(&index, &section.path, width, result);
        }
        rendered_sections += 1;
    }
}

/// Find the deepest shared package prefix across all package keys.
///
/// Returns a `//`-prefixed path. If there is no shared non-root prefix,
/// returns `"//"`.
fn common_package_prefix(packages: &BTreeMap<String, Vec<String>>) -> String {
    let mut shared_parts: Option<Vec<String>> = None;

    for package in packages.keys() {
        let stripped = package.strip_prefix("//").unwrap_or(package);
        let parts: Vec<String> = if stripped.is_empty() {
            Vec::new()
        } else {
            stripped.split('/').map(ToString::to_string).collect()
        };

        match &mut shared_parts {
            None => shared_parts = Some(parts),
            Some(shared) => {
                let common_len = shared
                    .iter()
                    .zip(parts.iter())
                    .take_while(|(a, b)| a == b)
                    .count();
                shared.truncate(common_len);
            }
        }
    }

    let shared_parts = shared_parts.unwrap_or_default();
    if shared_parts.is_empty() {
        "//".to_string()
    } else {
        format!("//{}", shared_parts.join("/"))
    }
}

/// Base root for a local package:
/// * `//:x` -> `//`
/// * `//src/foo:bar` -> `//src`
fn local_base_root(package: &str) -> String {
    let stripped = package.strip_prefix("//").unwrap_or(package);
    if stripped.is_empty() {
        "//".to_string()
    } else {
        let top = stripped.split('/').next().unwrap_or("");
        if top.is_empty() {
            "//".to_string()
        } else {
            format!("//{}", top)
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
enum QuerySectionRoot {
    Local(String),    // "//", "//src", "//tools", ...
    External(String), // repo name without leading '@'
}

#[derive(Debug)]
struct SectionNode {
    path: String,
    level: usize,
}

fn format_query_section_label(path: &str, external_repo: Option<&str>) -> String {
    if let Some(repo) = external_repo {
        format!("@{}{}", repo, path)
    } else {
        path.to_string()
    }
}

fn render_targets_only(
    index: &PackageIndex,
    section_path: &str,
    width: usize,
    result: &mut String,
) {
    let empty = PackageStats::default();
    let node = index.nodes.get(section_path).unwrap_or(&empty);
    let target_slots = width.min(node.direct_targets.len());
    let hidden_targets = node.direct_targets.len().saturating_sub(target_slots);

    for target in node.direct_targets.iter().take(target_slots) {
        result.push_str(&format!("🎯 :{}\n", target));
    }

    if hidden_targets > 0 {
        result.push_str(&format!(
            "(+{} more target{})\n",
            hidden_targets,
            if hidden_targets == 1 { "" } else { "s" }
        ));
    }
}

fn collect_section_nodes(
    index: &PackageIndex,
    path: &str,
    level: usize,
    max_levels: usize,
    out: &mut Vec<SectionNode>,
) {
    if level >= max_levels {
        return;
    }
    out.push(SectionNode {
        path: path.to_string(),
        level,
    });
    if level + 1 >= max_levels {
        return;
    }

    let Some(node) = index.nodes.get(path) else {
        return;
    };

    for name in &node.child_names {
        let next = child_path(path, name);
        collect_section_nodes(index, &next, level + 1, max_levels, out);
    }
}

pub fn filter_bazel_query(stdout: &str, stderr: &str, depth: usize, width: usize) -> String {
    let mut result = String::new();
    let mut has_error_lines = false;

    // Collect ERROR lines from stderr
    for line in stderr.lines() {
        let stripped = strip_timestamp(line.trim());
        if stripped.is_empty() {
            continue;
        }
        if stripped.starts_with("ERROR:") {
            has_error_lines = true;
            result.push_str(stripped);
            result.push('\n');
        }
    }

    // Group targets by output-derived roots:
    // - local roots: "//" and "//level0"
    // - external roots: "@repo"
    let mut local_sections: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut external_sections: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut section_order: Vec<QuerySectionRoot> = Vec::new();
    let mut seen_sections: HashSet<QuerySectionRoot> = HashSet::new();
    let mut non_target_lines: Vec<String> = Vec::new();

    for line in stdout.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if let Some(caps) = TARGET_LINE.captures(trimmed) {
            let package = caps[1].to_string();
            let target = caps[2].to_string();

            if package.starts_with('@') {
                if let Some((repo, rest)) = package.split_once("//") {
                    let repo = repo.trim_start_matches('@').to_string();
                    let relative_package = if rest.is_empty() {
                        "//".to_string()
                    } else {
                        format!("//{}", rest)
                    };
                    external_sections
                        .entry(repo.clone())
                        .or_default()
                        .entry(relative_package)
                        .or_default()
                        .push(target);

                    let section = QuerySectionRoot::External(repo);
                    if seen_sections.insert(section.clone()) {
                        section_order.push(section);
                    }
                } else {
                    external_sections
                        .entry("external".to_string())
                        .or_default()
                        .entry("//".to_string())
                        .or_default()
                        .push(target);

                    let section = QuerySectionRoot::External("external".to_string());
                    if seen_sections.insert(section.clone()) {
                        section_order.push(section);
                    }
                }
            } else {
                let base_root = local_base_root(&package);
                local_sections
                    .entry(base_root.clone())
                    .or_default()
                    .entry(package)
                    .or_default()
                    .push(target);

                let section = QuerySectionRoot::Local(base_root);
                if seen_sections.insert(section.clone()) {
                    section_order.push(section);
                }
            }
        } else {
            non_target_lines.push(trimmed.to_string());
        }
    }

    if local_sections.is_empty() && external_sections.is_empty() {
        // If bazel query failed and only error lines are present, do not add
        // a synthetic empty target header.
        if !has_error_lines {
            render_query_section(
                &mut result,
                &BTreeMap::new(),
                depth,
                width,
                "//",
                "//",
                None,
            );
        }
    } else {
        let mut rendered_sections = 0usize;

        for section in section_order {
            let rendered = match section {
                QuerySectionRoot::Local(base_root) => {
                    if let Some(packages) = local_sections.get(&base_root) {
                        let shared_root = common_package_prefix(packages);
                        let section_root = if shared_root == "//" {
                            base_root
                        } else {
                            shared_root
                        };
                        let section_display = section_root.clone();
                        if rendered_sections > 0 {
                            result.push('\n');
                        }
                        render_query_section(
                            &mut result,
                            packages,
                            depth,
                            width,
                            &section_display,
                            &section_root,
                            None,
                        );
                        true
                    } else {
                        false
                    }
                }
                QuerySectionRoot::External(repo) => {
                    if let Some(packages) = external_sections.get(&repo) {
                        let shared_root = common_package_prefix(packages);
                        let (section_display, section_root) = if shared_root == "//" {
                            (format!("@{}//", repo), "//".to_string())
                        } else {
                            let suffix = shared_root.strip_prefix("//").unwrap_or(&shared_root);
                            (format!("@{}//{}", repo, suffix), shared_root)
                        };
                        if rendered_sections > 0 {
                            result.push('\n');
                        }
                        render_query_section(
                            &mut result,
                            packages,
                            depth,
                            width,
                            &section_display,
                            &section_root,
                            Some(&repo),
                        );
                        true
                    } else {
                        false
                    }
                }
            };

            if rendered {
                rendered_sections += 1;
            }
        }
    }

    // Output non-target lines
    for line in &non_target_lines {
        result.push_str(line);
        result.push('\n');
    }

    result.trim_end().to_string()
}

/// Run `bazel query` while filtering the output.
///
/// # Arguments
///
/// * `args` - Arguments to pass to `bazel query`
/// * `depth` - Maximum depth of the package tree to show
/// * `width` - Maximum number of items to show for each package
/// * `verbose` - Verbosity level
///
/// # Returns
///
/// Result of the operation
///
pub fn run_query(args: &[String], depth: Limit, width: Limit, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = Command::new("bazel");
    cmd.arg("query");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: bazel query {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run bazel query. Is Bazel installed?")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let exit_code = output
        .status
        .code()
        .unwrap_or(if output.status.success() { 0 } else { 1 });
    let filtered = filter_bazel_query(&stdout, &stderr, depth.value(), width.value());

    if output.status.success() {
        if let Some(hint) = crate::tee::tee_and_hint(&raw, "bazel_query", exit_code) {
            println!("{}\n{}", filtered, hint);
        } else {
            println!("{}", filtered);
        }
    } else {
        println!("{}", filtered);
    }

    timer.track(
        &format!("bazel query {}", args.join(" ")),
        &format!("rtk bazel query {}", args.join(" ")),
        &raw,
        &filtered,
    );

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/**********************************************************************/
/*                      Other bazel subcommands                       */
/**********************************************************************/

/// Run other `bazel` subcommands not handled by rtk.
///
/// # Arguments
///
/// * `args` - Arguments to pass to the `bazel` subcommand
/// * `verbose` - Verbosity level
///
/// # Returns
///
/// Result of the operation
///
pub fn run_other(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("bazel: no subcommand specified");
    }

    let timer = tracking::TimedExecution::start();

    let subcommand = args[0].to_string_lossy();
    let mut cmd = Command::new("bazel");
    cmd.arg(&*subcommand);

    for arg in &args[1..] {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: bazel {} ...", subcommand);
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run bazel {}", subcommand))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    print!("{}", stdout);
    eprint!("{}", stderr);

    timer.track(
        &format!("bazel {}", subcommand),
        &format!("rtk bazel {}", subcommand),
        &raw,
        &raw, // No filtering for unsupported commands
    );

    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /******************************************************************/
    /*                  Shared Bazel utilities tests                  */
    /******************************************************************/
    #[test]
    fn test_limit_from_str() {
        assert_eq!("1".parse::<Limit>().unwrap(), Limit::N(1));
        assert_eq!("10".parse::<Limit>().unwrap(), Limit::N(10));
        assert_eq!("0".parse::<Limit>().unwrap(), Limit::N(0));
        assert_eq!("all".parse::<Limit>().unwrap(), Limit::All);
        assert_eq!("ALL".parse::<Limit>().unwrap(), Limit::All);
        assert_eq!("All".parse::<Limit>().unwrap(), Limit::All);
        assert!("invalid".parse::<Limit>().is_err());
        assert!("".parse::<Limit>().is_err());
    }

    /******************************************************************/
    /*                       bazel build tests                        */
    /******************************************************************/
    fn build(stdout: &str, stderr: &str) -> String {
        filter_bazel_build(stdout, stderr)
    }

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    #[test]
    fn test_filter_bazel_build_success() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 1 packages loaded
Analyzing: target //src:bazel-dev (6 packages loaded, 6 targets configured)
INFO: Analyzed target //src:bazel-dev (563 packages loaded, 24852 targets configured, 175 aspect applications).
[1 / 1] no actions running
[889 / 4,978] Compiling absl/numeric/int128.cc; 0s processwrapper-sandbox ... (256 actions, 255 running)
[1,084 / 4,978] Compiling absl/time/internal/cctz/src/time_zone_info.cc; 1s processwrapper-sandbox ... (256 actions, 255 running)
[4,976 / 4,978] Executing genrule //src:package-zip_jdk_allmodules; 1s processwrapper-sandbox
INFO: Found 1 target...
Target //src:bazel-dev up-to-date:
  bazel-bin/src/bazel-dev
INFO: Elapsed time: 54.859s, Critical Path: 49.98s
INFO: 2391 processes: 3 internal, 1537 processwrapper-sandbox, 881 worker.
INFO: Build completed successfully, 2391 total actions";
        let result = build("", stderr);
        assert_eq!(result, "✓ bazel build (2391 actions)");
    }

    #[test]
    fn test_filter_bazel_build_with_warnings() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 1 packages loaded
Analyzing: target //src:bazel-dev (6 packages loaded, 6 targets configured)
WARNING: /home/user/bazel/src/conditions/BUILD:119:15: select() on cpu is deprecated.
WARNING: /home/user/bazel/src/conditions/BUILD:202:15: select() on cpu is deprecated.
WARNING: /home/user/bazel/src/conditions/BUILD:193:15: select() on cpu is deprecated.
INFO: Analyzed target //src:bazel-dev (563 packages loaded).
[889 / 4,978] Compiling absl/numeric/int128.cc; 0s processwrapper-sandbox
[4,976 / 4,978] Executing genrule //src:package-zip_jdk_allmodules; 1s
INFO: Found 1 target...
Target //src:bazel-dev up-to-date:
  bazel-bin/src/bazel-dev
INFO: Elapsed time: 54.859s, Critical Path: 49.98s
INFO: Build completed successfully, 4978 total actions";
        let result = build("", stderr);

        assert!(result.contains("bazel build: 0 errors, 3 warnings (4978 actions)"));
        assert!(result.contains("═══════════════════════════════════════"));
        assert!(result.contains("WARNING:"));
        assert!(result.contains("select() on cpu is deprecated"));
        // Noise should be stripped
        assert!(!result.contains("Loading:"));
        assert!(!result.contains("Analyzing:"));
        assert!(!result.contains("[889 / 4,978]"));
        assert!(!result.contains("INFO:"));
    }

    #[test]
    fn test_filter_bazel_build_errors() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 0 packages loaded
WARNING: Target pattern parsing failed.
ERROR: Skipping '//src:bazel-dev-NONEXISTENT': no such target '//src:bazel-dev-NONEXISTENT'
ERROR: no such target '//src:bazel-dev-NONEXISTENT': target 'bazel-dev-NONEXISTENT' not declared in package 'src'
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
        let result = build("", stderr);

        assert!(result.contains("bazel build: 2 errors, 1 warning"));
        assert!(result.contains("(0 actions)"));
        assert!(result.contains("ERROR: Skipping"));
        assert!(result.contains("ERROR: no such target"));
        assert!(result.contains("WARNING: Target pattern parsing failed"));
        // "Build did NOT complete successfully" is stripped (we have our own header)
        assert!(!result.contains("Build did NOT complete successfully"));
        // Noise stripped
        assert!(!result.contains("Loading:"));
        assert!(!result.contains("INFO:"));
    }

    #[test]
    fn test_filter_bazel_build_compiler_warnings() {
        let stderr = "\
INFO: Analyzed target //src:bazel-dev (563 packages loaded).
[100 / 200] Compiling something.cc
INFO: From Building external/protobuf+/java/core/liblite_runtime_only.jar (94 source files):
bazel-out/k8-fastbuild/bin/src/main/protobuf/failure_details.pb.h:9953:111: warning: 'some_field' is deprecated [-Wdeprecated-declarations]
 9953 |   [[deprecated]] static constexpr Code FIELD = value;
      |                                                ^~~~~
bazel-out/k8-fastbuild/bin/src/main/protobuf/failure_details.pb.h:1690:3: note: declared here
 1690 |   SomeField [[deprecated]] = 2,
      |   ^~~~~~~~~

[200 / 200] Linking src/main/cpp/client
INFO: Build completed successfully, 200 total actions";
        let result = build("", stderr);

        // Should keep the compiler warning block
        assert!(result.contains("warning:"));
        assert!(result.contains("deprecated"));
        assert!(result.contains("note: declared here"));
        // Should show warning count
        assert!(result.contains("1 warning"));
        // Noise stripped
        assert!(!result.contains("[100 / 200]"));
        assert!(!result.contains("[200 / 200]"));
        assert!(!result.contains("INFO:"));
    }

    #[test]
    fn test_filter_bazel_build_strips_progress() {
        let stderr = "\
[1 / 1] no actions running
[889 / 4,978] Compiling absl/numeric/int128.cc; 0s processwrapper-sandbox
[1,084 / 4,978] Compiling absl/time/internal/cctz/src/time_zone_info.cc; 1s
[4,976 / 4,978] Executing genrule //src:package-zip; 1s
INFO: Build completed successfully, 4978 total actions";
        let result = build("", stderr);

        assert!(!result.contains("[889"));
        assert!(!result.contains("[1,084"));
        assert!(!result.contains("[4,976"));
        assert!(!result.contains("[1 / 1]"));
        assert!(result.contains("✓ bazel build (4978 actions)"));
    }

    #[test]
    fn test_filter_bazel_build_strips_info_noise() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 1 packages loaded
Analyzing: target //src:bazel-dev (6 packages loaded)
INFO: Analyzed target //src:bazel-dev
INFO: Found 1 target...
INFO: Elapsed time: 54.859s
INFO: 2391 processes: 3 internal, 1537 processwrapper-sandbox
INFO: Build completed successfully, 100 total actions
Note: Some input files use or override a deprecated API.
Note: Recompile with -Xlint:removal for details.
Target //src:bazel-dev up-to-date:
  bazel-bin/src/bazel-dev
DEBUG: some debug info";
        let result = build("", stderr);

        assert!(!result.contains("Computing main repo"));
        assert!(!result.contains("Loading:"));
        assert!(!result.contains("Analyzing:"));
        assert!(!result.contains("INFO:"));
        assert!(!result.contains("Note:"));
        assert!(!result.contains("Target //src:bazel-dev up-to-date"));
        assert!(!result.contains("bazel-bin/"));
        assert!(!result.contains("DEBUG:"));
        assert!(result.contains("✓ bazel build (100 actions)"));
    }

    #[test]
    fn test_filter_bazel_build_empty() {
        let result = build("", "");
        assert_eq!(result, "✓ bazel build (0 actions)");
    }

    #[test]
    fn test_filter_bazel_build_token_savings() {
        // Real-ish bazel build output (~80 lines of noise)
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 1 packages loaded
Analyzing: target //src:bazel-dev (6 packages loaded, 6 targets configured)
Analyzing: target //src:bazel-dev (6 packages loaded, 6 targets configured)
WARNING: /home/user/bazel/src/conditions/BUILD:119:15: select() on cpu is deprecated.
WARNING: /home/user/bazel/src/conditions/BUILD:202:15: select() on cpu is deprecated.
WARNING: /home/user/bazel/src/conditions/BUILD:193:15: select() on cpu is deprecated.
DEBUG: /home/user/.cache/bazel/external/grpc-java/java_grpc_library.bzl:202:14: Multiple values deprecated
INFO: Analyzed target //src:bazel-dev (563 packages loaded, 24852 targets configured).
[1 / 1] no actions running
[889 / 4,978] Compiling absl/numeric/int128.cc; 0s processwrapper-sandbox ... (256 actions, 255 running)
[1,084 / 4,978] Compiling absl/time/internal/cctz/src/time_zone_info.cc; 1s processwrapper-sandbox ... (256 actions, 255 running)
[1,191 / 4,978] Compiling tools/cpp/modules_tools/common/common.cc; 2s processwrapper-sandbox ... (256 actions, 255 running)
[1,348 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 3s processwrapper-sandbox ... (256 actions, 255 running)
[1,469 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 4s processwrapper-sandbox ... (256 actions, 255 running)
[1,540 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 6s processwrapper-sandbox ... (256 actions, 255 running)
[1,605 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 7s processwrapper-sandbox ... (255 actions, 254 running)
[1,642 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 8s processwrapper-sandbox ... (240 actions running)
[1,681 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 9s processwrapper-sandbox ... (201 actions running)
[1,751 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 10s processwrapper-sandbox ... (256 actions, 202 running)
[1,810 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 12s processwrapper-sandbox ... (224 actions, 155 running)
[1,846 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 13s processwrapper-sandbox ... (188 actions, 128 running)
[1,904 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 14s processwrapper-sandbox ... (130 actions, 92 running)
[1,970 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 15s processwrapper-sandbox ... (179 actions, 151 running)
INFO: From Building external/zstd-jni+/libzstd-jni-class.jar (30 source files) [for tool]:
Note: Some input files use or override a deprecated API that is marked for removal.
Note: Recompile with -Xlint:removal for details.
[2,149 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 16s processwrapper-sandbox ... (85 actions, 54 running)
INFO: From Building external/zstd-jni+/libzstd-jni-class.jar (30 source files):
Note: Some input files use or override a deprecated API that is marked for removal.
Note: Recompile with -Xlint:removal for details.
[2,318 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 17s processwrapper-sandbox
[2,346 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 18s processwrapper-sandbox
[2,368 / 4,978] Executing genrule //src:embedded_jdk_allmodules; 19s processwrapper-sandbox
[4,974 / 4,978] Linking src/main/cpp/client; 1s processwrapper-sandbox
[4,976 / 4,978] Executing genrule //src:package-zip_jdk_allmodules; 1s processwrapper-sandbox
INFO: Found 1 target...
Target //src:bazel-dev up-to-date:
  bazel-bin/src/bazel-dev
INFO: Elapsed time: 54.859s, Critical Path: 49.98s
INFO: 2391 processes: 3 internal, 1537 processwrapper-sandbox, 881 worker.
INFO: Build completed successfully, 2391 total actions";

        let input_tokens = count_tokens(stderr);
        let result = build("", stderr);
        let output_tokens = count_tokens(&result);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Bazel build filter: expected ≥60% savings, got {:.1}% ({} → {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_bazel_build_truncates_blocks() {
        // More than 15 issues should truncate
        let mut stderr = String::new();
        for i in 0..20 {
            stderr.push_str(&format!("ERROR: //pkg:target_{}: build failed\n\n", i));
        }
        let result = build("", &stderr);

        assert!(result.contains("... +5 more issues"));
    }

    #[test]
    fn test_filter_bazel_build_mixed_compiler_and_bazel_errors() {
        let stderr = "\
WARNING: /home/user/bazel/BUILD:10:5: select() on cpu is deprecated.
INFO: Analyzed target //src:app
[10 / 100] Compiling src/app.cc
src/app.cc:42:10: error: use of undeclared identifier 'foo'
   42 |   foo();
      |   ^~~

ERROR: //src:app failed to build
INFO: Build completed, 0 total actions
ERROR: Build did NOT complete successfully";
        let result = build("", stderr);

        // Should have 2 errors (compiler + bazel ERROR) and 1 warning
        assert!(result.contains("2 errors"));
        assert!(result.contains("1 warning"));
        assert!(result.contains("error: use of undeclared identifier"));
        assert!(result.contains("ERROR: //src:app failed to build"));
        assert!(result.contains("WARNING:"));
        assert!(result.contains("select() on cpu is deprecated"));
    }

    /******************************************************************/
    /*                       bazel query tests                        */
    /******************************************************************/
    fn query(stdout: &str, stderr: &str, depth: usize, width: usize) -> String {
        filter_bazel_query(stdout, stderr, depth, width)
    }

    #[test]
    fn test_limit_value() {
        assert_eq!(Limit::N(5).value(), 5);
        assert_eq!(Limit::All.value(), usize::MAX);
    }

    #[test]
    fn test_limit_display() {
        assert_eq!(Limit::N(5).to_string(), "5");
        assert_eq!(Limit::All.to_string(), "all");
    }

    #[test]
    fn test_strips_info_warning_noise() {
        let stderr = "\
(10:23:45) INFO: Invocation ID: abc-123
(10:23:45) INFO: Build options changed
(10:23:46) WARNING: some warning
(10:23:47) DEBUG: debug info
INFO: plain info line
WARNING: plain warning
DEBUG: plain debug";
        let stdout = "//pkg:target";
        let result = query(stdout, stderr, usize::MAX, usize::MAX);

        assert!(!result.contains("Invocation ID"));
        assert!(!result.contains("Build options changed"));
        assert!(!result.contains("some warning"));
        assert!(!result.contains("debug info"));
        assert!(!result.contains("plain info line"));
        assert!(!result.contains("plain warning"));
        assert!(!result.contains("plain debug"));
        assert!(result.contains("🎯 :target"));
    }

    #[test]
    fn test_keeps_error_lines() {
        let stderr = "\
(10:23:45) INFO: Build options changed
(10:23:46) ERROR: something went wrong
ERROR: another error";
        let stdout = "//pkg:target";
        let result = query(stdout, stderr, usize::MAX, usize::MAX);

        assert!(result.contains("ERROR: something went wrong"));
        assert!(result.contains("ERROR: another error"));
        assert!(!result.contains("Build options changed"));
    }

    #[test]
    fn test_empty_output() {
        let result = query("", "", usize::MAX, usize::MAX);
        // With default root, header is still produced
        assert!(result.contains("// (0 targets)"));
    }

    #[test]
    fn test_non_target_lines_pass_through() {
        let stdout = "\
//pkg:target_a
some non-target output line
//:root_target";
        let result = query(stdout, "", usize::MAX, usize::MAX);

        assert!(result.contains("some non-target output line"));
        assert!(result.contains("🎯 :target_a"));
        assert!(result.contains("🎯 :root_target"));
    }

    #[test]
    fn test_single_target_uses_singular() {
        let stdout = "//my/package:only_target";
        let result = query(stdout, "", usize::MAX, usize::MAX);
        assert!(result.contains("(1 target)"));
    }

    #[test]
    fn test_header_line() {
        let stdout = "\
//src/lib:a
//src/lib:b
//tools:c";
        let result = query(stdout, "", usize::MAX, usize::MAX);

        assert!(result.contains("//src/lib (2 targets)"));
        assert!(result.contains("//tools (1 target)"));
    }

    #[test]
    fn test_depth_1_collapses_to_summary() {
        let stdout = "\
//src/lib:a
//src/lib:b
//src/app:c
//tools/gen:d
//tools/gen:e
//tools/gen:f
//:root_target";
        let result = query(stdout, "", 1, usize::MAX);

        assert!(result.contains("//src (3 targets, 2 packages)"));
        assert!(result.contains("//tools/gen (3 targets)"));
        assert!(result.contains("// (1 target)"));
        assert!(result.contains("🎯 :root_target"));
    }

    #[test]
    fn test_depth_2_shows_two_levels() {
        let stdout = "\
//src/lib/math:a
//src/lib/math:b
//src/lib/io:c
//src/app:d
//tools:e";
        let result = query(stdout, "", 2, usize::MAX);

        assert!(result.contains("//src/app (1 target)"));
        assert!(result.contains("//src/lib (3 targets, 2 packages)"));
        assert!(result.contains("//tools (1 target)"));
    }

    #[test]
    fn test_depth_all_shows_everything() {
        let stdout = "\
//src/lib/math:a
//src/lib/io:b
//src/app:c";
        let result = query(stdout, "", usize::MAX, usize::MAX);

        assert!(result.contains("//src/app (1 target)"));
        assert!(result.contains("//src/lib/io (1 target)"));
        assert!(result.contains("//src/lib/math (1 target)"));
        assert!(result.contains("🎯 :a"));
        assert!(result.contains("🎯 :b"));
        assert!(result.contains("🎯 :c"));
    }

    #[test]
    fn test_always_cumulative_counts() {
        // Even when expanded, parent shows full subtree count
        let stdout = "\
//examples/cpp:a
//examples/cpp:b
//examples/go:c
//examples/java/sub:d";
        let result = query(stdout, "", 2, usize::MAX);

        assert!(result.contains("//examples/cpp (2 targets)"));
        assert!(result.contains("//examples/go (1 target)"));
        assert!(result.contains("//examples/java (1 target, 1 package)"));
    }

    #[test]
    fn test_width_budget_packages_then_targets() {
        let stdout = "\
//root/a:t
//root/b:t
//root/c:t
//root/d:t
//root:root_a
//root:root_b
//root:root_c";
        let result = query(stdout, "", 1, 5);

        assert!(result.contains("//root (7 targets, 4 packages)"));
        assert!(result.contains("📦 a (1 target)"));
        assert!(result.contains("📦 b (1 target)"));
        assert!(result.contains("📦 c (1 target)"));
        assert!(result.contains("📦 d (1 target)"));
        assert!(result.contains("🎯 :root_a"));
        assert!(!result.contains("🎯 :root_b"));
        assert!(result.contains("(+2 more targets)"));
    }

    #[test]
    fn test_width_limits_packages() {
        let stdout = "\
//root/a:t1
//root/b:t2
//root/c:t3
//root/d:t4
//root/e:t5";
        let result = query(stdout, "", 1, 3);

        assert!(result.contains("//root (5 targets, 5 packages)"));
        assert!(result.contains("📦 a"));
        assert!(result.contains("📦 b"));
        assert!(result.contains("📦 c"));
        assert!(!result.contains("📦 d"));
        assert!(!result.contains("📦 e"));
        assert!(result.contains("(+2 more sub-packages)"));
    }

    #[test]
    fn test_condensed_truncation_line() {
        let stdout = "\
//root/a:t
//root/b:t
//root/c:t
//root/d:t
//root:x
//root:y
//root:z";
        let result = query(stdout, "", 1, 3);

        assert!(result.contains("(+1 more sub-package, 3 more targets)"));
    }

    #[test]
    fn test_condensed_truncation_omits_zero_parts() {
        let stdout = "\
//root/a:t
//root/b:t
//root/c:t
//root/d:t";
        let result = query(stdout, "", 1, 3);

        assert!(result.contains("(+1 more sub-package)"));
        assert!(!result.contains("more target"));
    }

    #[test]
    fn test_root_targets_inline() {
        let stdout = "\
//:bazel-distfile
//:bazel-srcs
//src:lib";
        let result = query(stdout, "", 1, usize::MAX);

        assert!(result.contains("//src (1 target)"));
        assert!(result.contains("// (2 targets)"));
        assert!(result.contains("🎯 :bazel-distfile"));
        assert!(result.contains("🎯 :bazel-srcs"));
    }

    #[test]
    fn test_relative_names() {
        let stdout = "\
//examples/cpp:a
//examples/go:b";
        let result = query(stdout, "", 2, usize::MAX);

        assert!(result.contains("//examples/cpp (1 target)"));
        assert!(result.contains("//examples/go (1 target)"));
    }

    #[test]
    fn test_groups_targets_by_package() {
        let stdout = "\
//src/lib/math/compute:target_a
//src/lib/math/compute:target_b
//src/lib/math/compute:target_c
//tools/codegen:foo
//tools/codegen:bar";
        let result = query(stdout, "", usize::MAX, usize::MAX);

        // With full depth, targets are at leaf nodes
        assert!(result.contains("🎯 :target_a"));
        assert!(result.contains("🎯 :target_b"));
        assert!(result.contains("🎯 :target_c"));
        assert!(result.contains("🎯 :foo"));
        assert!(result.contains("🎯 :bar"));
    }

    #[test]
    fn test_real_bazel_output() {
        let stderr = "\
(10:23:45) INFO: Invocation ID: 8e2f4a91-abc1-4def-9012-345678abcdef
(10:23:45) INFO: Current date is 2026-03-01
(10:23:46) WARNING: Build option --config=remote has changed
(10:23:46) INFO: Repository rule @bazel_tools//tools/jdk:jdk configured
(10:23:47) INFO: Found 16 targets...
(10:23:47) INFO: Elapsed time: 1.234s";
        let stdout = "\
//src/app/foo/bar:bar
//src/app/foo/bar:bar_test
//src/app/foo/bar:bar_lib
//src/app/foo/bar:config
//src/app/foo/bar:config_test
//src/app/foo/bar:utils
//src/app/foo/bar:utils_test
//src/app/foo/bar:integration_test
//src/app/foo/bar:benchmark
//src/app/foo/bar:benchmark_lib
//src/app/foo/bar:data
//src/app/foo/bar:test_data
//src/app/foo/bar:model
//src/app/foo/bar:model_test
//src/app/foo/bar:runner
//src/app/foo/bar:runner_test";

        let result = query(stdout, stderr, usize::MAX, usize::MAX);

        // Should strip all INFO/WARNING noise
        assert!(!result.contains("Invocation ID"));
        assert!(!result.contains("Elapsed time"));

        assert!(result.contains("//src/app/foo/bar (16 targets)"));

        assert!(result.contains("🎯 :bar\n"));
        assert!(result.contains("🎯 :runner_test"));
    }

    #[test]
    fn test_filter_bazel_query_multi_root_no_target_loss() {
        let stdout = "\
//src/app:bin
//tools/gen:tool
//third_party/lib:pkg";
        let result = filter_bazel_query(stdout, "", usize::MAX, usize::MAX);

        assert!(result.contains("//src/app (1 target)"));
        assert!(result.contains("//tools/gen (1 target)"));
        assert!(result.contains("//third_party/lib (1 target)"));
        assert!(result.contains("🎯 :bin"));
        assert!(result.contains("🎯 :tool"));
        assert!(result.contains("🎯 :pkg"));
    }

    #[test]
    fn test_filter_bazel_query_multi_root_respects_width() {
        let stdout = "\
//src/s1:a
//src/s2:b
//tools/t1:c
//tools/t2:d";
        let result = filter_bazel_query(stdout, "", 1, 1);

        assert!(
            result.contains("//src (2 targets"),
            "unexpected output:\n{}",
            result
        );
        assert!(
            result.contains("//tools (2 targets"),
            "unexpected output:\n{}",
            result
        );
        // Width 1 at each section root: one child package shown, one hidden.
        assert_eq!(result.matches("(+1 more sub-package)").count(), 2);
        assert!(result.contains("📦 s1 (1 target)"));
        assert!(result.contains("📦 t1 (1 target)"));
    }

    #[test]
    fn test_filter_bazel_query_groups_external_repos() {
        let stdout = "\
//src/app:bin
@abseil-cpp//absl/base:core_headers
@abseil-cpp//absl/strings:str_format
@zlib//:zlib";
        let result = filter_bazel_query(stdout, "", 1, 10);

        assert!(result.contains("//src/app (1 target)"));
        assert!(result.contains("@abseil-cpp//absl (2 targets"));
        assert!(result.contains("@zlib// (1 target)"));
        assert!(result.contains("📦 base (1 target)"));
        assert!(result.contains("📦 strings (1 target)"));
        assert!(result.contains("🎯 :zlib"));
    }

    #[test]
    fn test_filter_bazel_query_consolidates_deep_common_prefix() {
        let stdout = "\
//src/java_tools/buildjar:a
//src/java_tools/import_deps_checker:b
//src/java_tools/junitrunner:c";
        let result = filter_bazel_query(stdout, "", 1, 10);

        assert!(result.starts_with("//src/java_tools (3 targets"));
        assert!(result.contains("📦 buildjar (1 target)"));
        assert!(result.contains("📦 import_deps_checker (1 target)"));
        assert!(result.contains("📦 junitrunner (1 target)"));
    }

    #[test]
    fn test_filter_bazel_query_splits_external_repos_by_repo_root() {
        let stdout = "\
@abseil-cpp//absl/base:core
@abseil-cpp//absl/strings:format
@bazel_skylib//lib:paths
@bazel_skylib//rules:copy";
        let result = filter_bazel_query(stdout, "", 1, 10);

        assert!(result.contains("@abseil-cpp//absl (2 targets"));
        assert!(result.contains("@bazel_skylib// (2 targets, 2 packages)"));
        assert!(result.contains("📦 base (1 target)"));
        assert!(result.contains("📦 strings (1 target)"));
        assert!(result.contains("📦 lib (1 target)"));
        assert!(result.contains("📦 rules (1 target)"));
    }

    #[test]
    fn test_filter_bazel_query_external_root_targets_keep_repo_root_header() {
        let stdout = "\
@abseil-cpp//:root_target
@abseil-cpp//absl/base:core";
        let result = filter_bazel_query(stdout, "", 1, 10);

        assert!(result.starts_with("@abseil-cpp// (2 targets, 2 packages)"));
        assert!(result.contains("📦 absl (1 target, 1 package)"));
        assert!(result.contains("🎯 :root_target"));
    }

    #[test]
    fn test_filter_bazel_query_depth_1_runtime_mode_stays_single_section() {
        let stdout = "\
//src:root
//src/conditions:a
//src/java_tools:b";
        let result = filter_bazel_query(stdout, "", 1, 10);

        assert!(result.starts_with("//src (3 targets, 2 packages)"));
        assert!(result.contains("📦 conditions (1 target)"));
        assert!(result.contains("📦 java_tools (1 target)"));
        assert!(result.contains("🎯 :root"));
        assert!(!result.contains("..."));
    }

    #[test]
    fn test_filter_bazel_query_depth_2_runtime_mode_expands_to_sections() {
        let stdout = "\
//src:root_a
//src:root_b
//src/conditions:c1
//src/java_tools:j1
//src/java_tools/sub:s1";
        let result = filter_bazel_query(stdout, "", 2, 10);

        assert!(result.contains("//src (2 targets)"));
        assert!(result.contains("🎯 :root_a"));
        assert!(result.contains("🎯 :root_b"));
        assert!(result.contains("//src/conditions (1 target)"));
        assert!(result.contains("//src/java_tools (2 targets, 1 package)"));
        // Depth sections are flat; no tree indentation in this mode.
        assert!(!result.contains("  📦"));
    }

    #[test]
    fn test_filter_bazel_query_depth_2_skips_empty_intermediate_section() {
        let stdout = "\
@xds+//xds/data/orca:alpha
@xds+//xds/data/orca:beta
@xds+//xds/service/orca:gamma
@xds+//xds/service/orca:delta";
        let result = filter_bazel_query(stdout, "", 2, 10);

        assert!(!result.contains("@xds+//xds (0 targets)"));
        assert!(result.contains("@xds+//xds/data"));
        assert!(result.contains("@xds+//xds/service"));
    }

    #[test]
    fn test_filter_bazel_query_error_only_no_empty_header() {
        let stderr =
            "ERROR: Evaluation of query \"deps(//...)\" failed: preloading transitive closure failed";
        let result = filter_bazel_query("", stderr, usize::MAX, 10);

        assert_eq!(
            result,
            "ERROR: Evaluation of query \"deps(//...)\" failed: preloading transitive closure failed"
        );
        assert!(!result.contains("//... (0 targets)"));
    }

    #[test]
    fn test_filter_bazel_query_single_root_uses_subsections() {
        let stdout = "\
//src/lib:a
//src/app:b";
        let result = filter_bazel_query(stdout, "", 2, usize::MAX);

        assert!(result.contains("//src/app (1 target)"));
        assert!(result.contains("//src/lib (1 target)"));
        assert!(result.contains("🎯 :a"));
        assert!(result.contains("🎯 :b"));
    }

    /******************************************************************/
    /*                       bazel test tests                         */
    /******************************************************************/
    fn btest(stdout: &str, stderr: &str) -> String {
        filter_bazel_test(stdout, stderr)
    }

    #[test]
    fn test_filter_bazel_test_all_pass() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 0 packages loaded
Analyzing: 3 targets (81 packages loaded, 684 targets configured)
INFO: Analyzed 3 targets (81 packages loaded, 684 targets configured).
INFO: Found 3 test targets...
[0 / 4] [Prepa] BazelWorkspaceStatusAction stable-status.txt
[5 / 14] Compiling src/test/java/com/google/devtools/build/lib/util/CommandUtilsTest.java; 0s worker
[14 / 14] 3 tests, 1 action running
//src/test/java/com/google/devtools/build/lib/util:CommandUtilsTest    PASSED in 0.3s
//src/test/java/com/google/devtools/build/lib/util:DecimalBucketerTest    PASSED in 0.3s
//src/test/java/com/google/devtools/build/lib/util:StringEncodingTest    PASSED in 0.3s
INFO: Elapsed time: 5.164s, Critical Path: 3.89s
INFO: 6 processes: 3 internal, 3 worker.
INFO: Build completed successfully, 6 total actions
Executed 3 out of 3 tests: 3 tests pass.";
        let result = btest("", stderr);
        assert_eq!(result, "\u{2713} bazel test: 3 passed, 0 failed (5.164s)");
    }

    #[test]
    fn test_filter_bazel_test_with_cached() {
        let stderr = "\
Loading:
INFO: Analyzed 2 targets (0 packages loaded, 0 targets configured).
INFO: Found 2 test targets...
//src/test/java/com/google/devtools/build/lib/util:CommandUtilsTest (cached) PASSED in 0.3s
//src/test/java/com/google/devtools/build/lib/util:StringEncodingTest    PASSED in 0.1s
INFO: Elapsed time: 0.412s, Critical Path: 0.10s
INFO: 2 processes: 1 internal, 1 worker.
INFO: Build completed successfully, 2 total actions
Executed 1 out of 2 tests: 2 tests pass.";
        let result = btest("", stderr);
        assert_eq!(result, "\u{2713} bazel test: 2 passed, 0 failed (0.412s)");
    }

    #[test]
    fn test_filter_bazel_test_failure() {
        let stderr = "\
Loading:
INFO: Analyzed 1 target (0 packages loaded, 0 targets configured).
INFO: Found 1 test target...
FAIL: //src/test/java/com/google/devtools/build/lib/util:StringEncodingTest (Exit 1) (see /home/user/.cache/bazel/_bazel_user/abc/execroot/io_bazel/bazel-out/k8-fastbuild/testlogs/src/test/java/com/google/devtools/build/lib/util/StringEncodingTest/test.log)
//src/test/java/com/google/devtools/build/lib/util:StringEncodingTest    FAILED in 0.3s
  /home/user/.cache/bazel/testlogs/src/test/java/com/google/devtools/build/lib/util/StringEncodingTest/test.log
INFO: Elapsed time: 0.340s, Critical Path: 0.30s
INFO: 2 processes: 1 internal, 1 worker.
INFO: Build completed, 1 test FAILED, 2 total actions
Executed 1 out of 1 test: 1 fails locally.";
        let result = btest("", stderr);

        assert!(result.contains("bazel test: 1 failed, 0 passed (0.340s)"));
        assert!(result.contains("═══════════════════════════════════════"));
        assert!(result.contains("FAIL: //src/test/java"));
        assert!(result.contains("FAILED in 0.3s"));
        // Noise stripped
        assert!(!result.contains("Loading:"));
        assert!(!result.contains("INFO:"));
        assert!(!result.contains("Executed 1 out of"));
    }

    #[test]
    fn test_filter_bazel_test_failure_with_test_output() {
        let stderr = "\
Loading:
INFO: Analyzed 1 target (0 packages loaded, 0 targets configured).
INFO: Found 1 test target...
FAIL: //src/test/java/com/google/devtools/build/lib/util:StringEncodingTest (Exit 1)
==================== Test output for //src/test/java/com/google/devtools/build/lib/util:StringEncodingTest:
JUnit4 Test Runner
.EE
Time: 0.002
There were 2 failures:
1) initializationError(org.junit.runner.manipulation.Filter)
java.lang.Exception: No tests found matching RegEx[NONEXISTENT_TEST]
\tat org.junit.internal.requests.FilterRequest.getRunner(FilterRequest.java:40)
\tat com.google.testing.junit.runner.internal.junit4.JUnit4Runner.createErrorReportingRequestForFilterError(JUnit4Runner.java:233)
================================================================================
//src/test/java/com/google/devtools/build/lib/util:StringEncodingTest    FAILED in 0.3s
INFO: Elapsed time: 0.340s, Critical Path: 0.30s
INFO: Build completed, 1 test FAILED, 2 total actions
Executed 1 out of 1 test: 1 fails locally.";
        let result = btest("", stderr);

        assert!(result.contains("bazel test: 1 failed, 0 passed"));
        // Inline test output preserved
        assert!(result.contains("==================== Test output for"));
        assert!(result.contains("JUnit4 Test Runner"));
        assert!(result.contains("No tests found matching"));
        assert!(result.contains(
            "================================================================================"
        ));
        // FAIL line preserved
        assert!(result.contains("FAIL: //src/test/java"));
        // Result line preserved
        assert!(result.contains("FAILED in 0.3s"));
    }

    #[test]
    fn test_filter_bazel_test_strips_build_noise() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 0 packages loaded
Analyzing: 1 target (81 packages loaded, 684 targets configured)
INFO: Analyzed 1 target (81 packages loaded).
[0 / 4] [Prepa] BazelWorkspaceStatusAction stable-status.txt
[5 / 14] Compiling something.java; 0s worker
[14 / 14] 1 test running
INFO: Found 1 test target...
DEBUG: /some/debug/info
Note: Some input files use deprecated API.
Target //src:target up-to-date:
  bazel-bin/src/target
//pkg:test    PASSED in 0.5s
INFO: Elapsed time: 1.00s, Critical Path: 0.50s
INFO: Build completed successfully, 4 total actions
Executed 1 out of 1 test: 1 tests pass.";
        let result = btest("", stderr);

        assert!(!result.contains("Computing main repo"));
        assert!(!result.contains("Loading:"));
        assert!(!result.contains("Analyzing:"));
        assert!(!result.contains("[0 / 4]"));
        assert!(!result.contains("[5 / 14]"));
        assert!(!result.contains("[14 / 14]"));
        assert!(!result.contains("INFO:"));
        assert!(!result.contains("DEBUG:"));
        assert!(!result.contains("Note:"));
        assert!(!result.contains("Target //src:target"));
        assert!(!result.contains("bazel-bin/"));
        assert!(!result.contains("Executed 1 out of"));
        assert!(result.contains("\u{2713} bazel test: 1 passed, 0 failed"));
    }

    #[test]
    fn test_filter_bazel_test_build_error() {
        let stderr = "\
Loading:
WARNING: Target pattern parsing failed.
ERROR: Skipping '//src:nonexistent': no such target '//src:nonexistent'
ERROR: no such target '//src:nonexistent': target 'nonexistent' not declared
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
        let result = btest("", stderr);

        assert!(result.contains("bazel test: build failed"));
        assert!(result.contains("═══════════════════════════════════════"));
        assert!(result.contains("ERROR: Skipping"));
        assert!(result.contains("ERROR: no such target"));
        // "Build did NOT complete successfully" stripped
        assert!(!result.contains("Build did NOT complete successfully"));
    }

    #[test]
    fn test_filter_bazel_test_empty() {
        let result = btest("", "");
        assert_eq!(result, "\u{2713} bazel test: 0 passed, 0 failed (0s)");
    }

    #[test]
    fn test_filter_bazel_test_token_savings() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 0 packages loaded
Analyzing: 3 targets (81 packages loaded, 684 targets configured)
Analyzing: 3 targets (81 packages loaded, 684 targets configured)
INFO: Analyzed 3 targets (81 packages loaded, 684 targets configured).
INFO: Found 3 test targets...
[0 / 4] [Prepa] BazelWorkspaceStatusAction stable-status.txt
[1 / 14] Compiling src/test/java/com/google/devtools/build/lib/util/CommandUtilsTest.java; 0s worker
[2 / 14] Compiling src/test/java/com/google/devtools/build/lib/util/DecimalBucketerTest.java; 0s worker
[5 / 14] Compiling src/test/java/com/google/devtools/build/lib/util/StringEncodingTest.java; 0s worker
[10 / 14] Building test deploy jar
[14 / 14] 3 tests, 1 action running
//src/test/java/com/google/devtools/build/lib/util:CommandUtilsTest    PASSED in 0.3s
//src/test/java/com/google/devtools/build/lib/util:DecimalBucketerTest    PASSED in 0.3s
//src/test/java/com/google/devtools/build/lib/util:StringEncodingTest    PASSED in 0.3s
There were tests whose specified size is too big. Use the --test_verbose_timeout_warnings command line option to see which ones these are.
INFO: Elapsed time: 5.164s, Critical Path: 3.89s
INFO: 6 processes: 3 internal, 3 worker.
INFO: Build completed successfully, 6 total actions
Executed 3 out of 3 tests: 3 tests pass.";

        let input_tokens = count_tokens(stderr);
        let result = btest("", stderr);
        let output_tokens = count_tokens(&result);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Bazel test filter: expected ≥60% savings, got {:.1}% ({} → {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_bazel_test_strips_timeout_warnings() {
        let stderr = "\
INFO: Analyzed 1 target (0 packages loaded).
INFO: Found 1 test target...
//pkg:test    PASSED in 0.5s
There were tests whose specified size is too big. Use the --test_verbose_timeout_warnings command line option to see which ones these are.
INFO: Elapsed time: 1.00s, Critical Path: 0.50s
INFO: Build completed successfully, 2 total actions
Executed 1 out of 1 test: 1 tests pass.";
        let result = btest("", stderr);

        assert!(!result.contains("There were tests whose specified size"));
        assert!(!result.contains("--test_verbose_timeout_warnings"));
        assert!(result.contains("\u{2713} bazel test: 1 passed, 0 failed"));
    }

    /******************************************************************/
    /*                       bazel run tests                          */
    /******************************************************************/
    fn brun(stdout: &str, stderr: &str) -> String {
        brun_with_args(stdout, stderr, &[])
    }

    fn brun_with_args(stdout: &str, stderr: &str, args: &[String]) -> String {
        filter_bazel_run(stdout, stderr, args)
    }

    #[test]
    fn test_filter_bazel_run_success() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 0 packages loaded
Analyzing: target //src:my_binary (6 packages loaded)
INFO: Analyzed target //src:my_binary (81 packages loaded, 684 targets configured).
[0 / 4] [Prepa] BazelWorkspaceStatusAction stable-status.txt
[10 / 14] Compiling src/main.cc
INFO: Found 1 target...
Target //src:my_binary up-to-date:
  bazel-bin/src/my_binary
INFO: Elapsed time: 3.50s, Critical Path: 2.10s
INFO: 123 processes: 3 internal, 120 processwrapper-sandbox.
INFO: Build completed successfully, 123 total actions
INFO: Running command line: bazel-bin/src/my_binary
binary stderr line";
        let stdout = "Hello from binary!\nResult: 42";
        let args: Vec<String> = vec!["//src:my_binary".into()];
        let result = brun_with_args(stdout, stderr, &args);

        // Clean build — no build summary, just binary output
        assert!(!result.contains("bazel build"));
        assert!(!result.contains("═══════════════════════════════════════"));
        assert!(result.contains("Hello from binary!"));
        assert!(result.contains("Result: 42"));
        assert!(result.contains("binary stderr line"));
        // Noise stripped
        assert!(!result.contains("Loading:"));
        assert!(!result.contains("[10 / 14]"));
        assert!(!result.contains("INFO:"));
        assert!(!result.contains("Computing main repo"));
    }

    #[test]
    fn test_filter_bazel_run_warnings_stripped() {
        let stderr = "\
WARNING: /home/user/BUILD:10:5: select() on cpu is deprecated.
WARNING: /home/user/BUILD:20:5: another deprecation warning.
INFO: Analyzed target //src:app (10 packages loaded).
[5 / 10] Compiling something.cc
INFO: Found 1 target...
Target //src:app up-to-date:
  bazel-bin/src/app
INFO: Build completed successfully, 100 total actions
INFO: Running command line: bazel-bin/src/app
app output here";
        let stdout = "app stdout";
        let result = brun(stdout, stderr);

        // Warnings stripped — clean build, no build section
        assert!(!result.contains("WARNING:"));
        assert!(!result.contains("select() on cpu"));
        assert!(!result.contains("bazel build"));
        // Binary output only
        assert!(result.contains("app stdout"));
        assert!(result.contains("app output here"));
    }

    #[test]
    fn test_filter_bazel_run_build_error() {
        let stderr = "\
Loading:
WARNING: Target pattern parsing failed.
ERROR: Skipping '//src:nonexistent': no such target '//src:nonexistent'
ERROR: no such target '//src:nonexistent': target 'nonexistent' not declared
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
        let result = brun("", stderr);

        assert!(
            result.contains("bazel build:") && result.contains("warning"),
            "unexpected output:\n{}",
            result
        );
        assert!(result.contains("ERROR: Skipping"));
        assert!(result.contains("ERROR: no such target"));
        assert!(!result.contains("Build did NOT complete successfully"));
        // No binary output
        assert!(!result.contains("Running command line"));
    }

    #[test]
    fn test_filter_bazel_run_build_error_no_warnings() {
        let stderr = "\
Loading:
ERROR: Skipping '//src:nonexistent': no such target '//src:nonexistent'
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
        let result = brun("", stderr);

        assert!(result.contains("bazel build: 1 error, 0 warnings"));
        assert!(result.contains("ERROR: Skipping"));
        assert!(!result.contains("WARNING:"));
        assert!(!result.contains("Build did NOT complete successfully"));
    }

    #[test]
    fn test_filter_bazel_run_binary_stderr() {
        let stderr = "\
INFO: Analyzed target //src:app (0 packages loaded).
INFO: Found 1 target...
INFO: Build completed successfully, 50 total actions
INFO: Running command line: bazel-bin/src/app
Error: could not connect to database
Stack trace:
  at main.cc:42
  at db.cc:100";
        let result = brun("", stderr);

        // Clean build — no build summary
        assert!(!result.contains("bazel build"));
        assert!(result.contains("Error: could not connect to database"));
        assert!(result.contains("Stack trace:"));
        assert!(result.contains("at main.cc:42"));
    }

    #[test]
    fn test_filter_bazel_run_no_sentinel() {
        // No sentinel = build-only, no binary ran (e.g. build phase completed but no run)
        let stderr = "\
INFO: Analyzed target //src:app (10 packages loaded).
[5 / 10] Compiling something.cc
INFO: Found 1 target...
Target //src:app up-to-date:
  bazel-bin/src/app
INFO: Build completed successfully, 100 total actions";
        let result = brun("", stderr);

        // Falls back to filter_bazel_build behavior
        assert!(result.contains("\u{2713} bazel build (100 actions)"));
    }

    #[test]
    fn test_filter_bazel_run_strips_build_noise() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 1 packages loaded
Analyzing: target //src:app (6 packages loaded)
DEBUG: /some/debug/info
Note: Some input files use deprecated API.
[0 / 4] [Prepa] BazelWorkspaceStatusAction
[100 / 200] Compiling something.cc
Target //src:app up-to-date:
  bazel-bin/src/app
INFO: Elapsed time: 5.00s
INFO: 200 processes: 3 internal, 197 processwrapper-sandbox.
INFO: Build completed successfully, 200 total actions
INFO: Running command line: bazel-bin/src/app";
        let stdout = "output";
        let result = brun(stdout, stderr);

        assert!(!result.contains("Computing main repo"));
        assert!(!result.contains("Loading:"));
        assert!(!result.contains("Analyzing:"));
        assert!(!result.contains("DEBUG:"));
        assert!(!result.contains("Note:"));
        assert!(!result.contains("[0 / 4]"));
        assert!(!result.contains("[100 / 200]"));
        assert!(!result.contains("Target //src:app"));
        assert!(!result.contains("bazel-bin/src/app"));
        assert!(!result.contains("INFO:"));
        assert!(result.contains("output"));
    }

    #[test]
    fn test_filter_bazel_run_empty() {
        let result = brun("", "");
        assert_eq!(result, "\u{2713} bazel build (0 actions)");
    }

    #[test]
    fn test_filter_bazel_run_token_savings() {
        let stderr = "\
Computing main repo mapping:
Loading:
Loading: 0 packages loaded
Analyzing: target //src:my_binary (6 packages loaded, 6 targets configured)
Analyzing: target //src:my_binary (6 packages loaded, 6 targets configured)
INFO: Analyzed target //src:my_binary (563 packages loaded, 24852 targets configured).
[1 / 1] no actions running
[889 / 4,978] Compiling absl/numeric/int128.cc; 0s processwrapper-sandbox ... (256 actions, 255 running)
[1,084 / 4,978] Compiling absl/time/internal/cctz/src/time_zone_info.cc; 1s processwrapper-sandbox ... (256 actions, 255 running)
[1,191 / 4,978] Compiling tools/cpp/modules_tools/common/common.cc; 2s processwrapper-sandbox ... (256 actions, 255 running)
[1,348 / 4,978] Executing genrule //src:embedded_jdk; 3s processwrapper-sandbox
[1,469 / 4,978] Executing genrule //src:embedded_jdk; 4s processwrapper-sandbox
[1,540 / 4,978] Executing genrule //src:embedded_jdk; 6s processwrapper-sandbox
[4,976 / 4,978] Executing genrule //src:package-zip; 1s processwrapper-sandbox
INFO: Found 1 target...
Target //src:my_binary up-to-date:
  bazel-bin/src/my_binary
INFO: Elapsed time: 54.859s, Critical Path: 49.98s
INFO: 2391 processes: 3 internal, 1537 processwrapper-sandbox, 881 worker.
INFO: Build completed successfully, 2391 total actions
INFO: Running command line: bazel-bin/src/my_binary
Hello World";

        let input_tokens = count_tokens(stderr);
        let result = brun("", stderr);
        let output_tokens = count_tokens(&result);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "Bazel run filter: expected ≥60% savings, got {:.1}% ({} → {} tokens)",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_filter_bazel_run_timestamp_sentinel() {
        let stderr = "\
(10:23:45) INFO: Analyzed target //src:app (10 packages loaded).
(10:23:46) INFO: Found 1 target...
(10:23:47) INFO: Build completed successfully, 50 total actions
(10:23:48) INFO: Running command line: bazel-bin/src/app
binary output on stderr";
        let stdout = "binary output on stdout";
        let result = brun(stdout, stderr);

        // Clean build — no build summary
        assert!(!result.contains("bazel build"));
        assert!(result.contains("binary output on stdout"));
        assert!(result.contains("binary output on stderr"));
        // Sentinel itself should not appear
        assert!(!result.contains("Running command line"));
    }

    #[test]
    fn test_filter_bazel_run_real_world_output() {
        // Realistic output from `bazel run` with timestamped lines, env-prefixed
        // sentinel, and trailing INFO after the sentinel
        let stderr = "\
(17:17:06) WARNING: some build config deprecation warning
(17:17:06) INFO: Current date is 2026-03-02
(17:17:06) Computing main repo mapping:
(17:17:06) Loading:
(17:17:06) Loading: 0 packages loaded
(17:17:06) Analyzing: target //src/tools/my_tool:my_tool (0 packages loaded, 0 targets configured)
[0 / 1] checking cached actions
(17:17:06) INFO: Analyzed target //src/tools/my_tool:my_tool (0 packages loaded, 0 targets configured).
(17:17:06) INFO: Found 1 target...
Target //src/tools/my_tool:my_tool up-to-date:
  bazel-bin/src/tools/my_tool/my_tool
(17:17:06) INFO: Elapsed time: 0.518s, Critical Path: 0.09s
(17:17:06) INFO: 1 process: 3 action cache hit, 1 internal.
(17:17:06) INFO: Build completed successfully, 1 total action
(17:17:06) INFO:
(17:17:06) INFO: Running command line: env FOO=1 BAR=/tmp/cache bazel-bin/src/tools/my_tool/my_tool
(17:17:06) INFO: Some trailing info line";
        let stdout = "Processing input...\nDone.";
        let args: Vec<String> = vec![
            "//src/tools/my_tool".into(),
            "--".into(),
            "\"some-arg\"".into(),
        ];
        let result = brun_with_args(stdout, stderr, &args);

        // WARNING stripped — clean build, no build section
        assert!(!result.contains("WARNING:"));
        assert!(!result.contains("bazel build"));
        // Binary output only
        assert!(result.contains("Processing input..."));
        assert!(result.contains("Done."));
        // Pre-sentinel build noise stripped
        assert!(!result.contains("Computing main repo"));
        assert!(!result.contains("Loading:"));
        assert!(!result.contains("Analyzing:"));
        assert!(!result.contains("[0 / 1]"));
        // Post-sentinel output is preserved verbatim
        assert!(result.contains("INFO: Some trailing info line"));
        assert!(!result.contains("Running command line"));
        assert!(!result.contains("FOO=1"));
    }

    #[test]
    fn test_filter_bazel_run_post_sentinel_prefixed_lines_preserved() {
        // INFO/WARNING/DEBUG after sentinel are binary stderr and must be preserved.
        let stderr = "\
INFO: Build completed successfully, 10 total actions
INFO: Running command line: bazel-bin/app
INFO: Some trailing info line
WARNING: App warning
DEBUG: App debug
INFO: Another trailing info line
actual binary error output";
        let result = brun("binary stdout", stderr);

        assert!(result.contains("binary stdout"));
        assert!(result.contains("actual binary error output"));
        assert!(result.contains("INFO: Some trailing info line"));
        assert!(result.contains("WARNING: App warning"));
        assert!(result.contains("DEBUG: App debug"));
        assert!(result.contains("INFO: Another trailing info line"));
    }

    #[test]
    fn test_filter_bazel_run_preserves_trailing_newline() {
        let stderr = "\
INFO: Build completed successfully, 10 total actions
INFO: Running command line: bazel-bin/app";
        let result = brun("line1\nline2\n", stderr);

        assert!(result.ends_with('\n'));
        assert!(result.contains("line1\nline2\n"));
    }

    #[test]
    fn test_filter_bazel_run_preserves_leading_whitespace() {
        let stderr = "\
INFO: Build completed successfully, 10 total actions
INFO: Running command line: bazel-bin/app";
        let result = brun("  indented\n\tTabbed\n", stderr);

        assert!(result.contains("  indented"));
        assert!(result.contains("\tTabbed"));
    }
}
