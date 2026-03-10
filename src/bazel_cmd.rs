use crate::tracking;
use crate::utils::resolved_command;
use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::ffi::OsString;
use std::fmt;
use std::str::FromStr;

/**********************************************************************/
/*                       Shared Bazel Utilities                       */
/**********************************************************************/

lazy_static! {
    /// Matches Bazel target lines
    ///
    /// e.g. "//package/path:target_name", "//:root_target", "@repo//pkg:target"
    static ref TARGET_LINE: Regex =
        Regex::new(r"^((?:@[^/\s:]+)?//[^:]*):(.+)$").unwrap();

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
    static ref RUN_SENTINEL: Regex =
        Regex::new(r"^INFO: Running command line:").unwrap();
}

/// Extra flags to pass to Bazel commands.
const BAZEL_EXTRA_FLAGS: [&str; 3] = [
    "--noshow_progress",
    "--noshow_timestamps",
    "--noshow_loading_progress",
];

/// Maximum number of build issues (errors, warnings) to show.
const MAX_BUILD_ISSUES: usize = 15;

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

/// Run a bazel subcommand and filtering the output.
///
/// # Arguments
///
/// * `subcommand` - Bazel subcommand to run
/// * `args` - Subcommand arguments
/// * `verbose` - Verbosity level
/// * `filter_fn` - Function to filter the output
///
/// # Returns
///
/// Result of the operation
///
fn run_bazel_filtered<F>(subcommand: &str, args: &[String], verbose: u8, filter_fn: F) -> Result<()>
where
    F: Fn(&str) -> Result<String>,
{
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command("bazel");
    cmd.arg(subcommand);
    cmd.args(BAZEL_EXTRA_FLAGS);
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

    match filter_fn(&raw) {
        Ok(filtered) => {
            // Print filtered output
            match crate::tee::tee_and_hint(&raw, "bazel_build", exit_code) {
                Some(hint) => println!("{}\n{}", filtered, hint),
                None => println!("{}", filtered),
            };

            // Track filtering
            timer.track(
                &format!("bazel {} {}", subcommand, args.join(" ")),
                &format!("rtk bazel {} {}", subcommand, args.join(" ")),
                &raw,
                &filtered,
            );
        }
        Err(e) => {
            // Print raw output
            #[cfg(debug_assertions)]
            eprintln!(
                "rtk: filtering bazel {} failed, showing raw output: {}",
                subcommand, e
            );

            println!("{}", raw);
        }
    }

    if !output.status.success() {
        std::process::exit(exit_code);
    }

    Ok(())
}

/**********************************************************************/
/*                            bazel build                             */
/**********************************************************************/

/// Tracks state when filtering compiler diagnostic blocks.
#[derive(Debug, Default)]
struct DiagnosticBlockState {
    /// Lines of the diagnostic block
    pub block: Vec<String>,

    /// Whether the diagnostic block is an error
    pub is_error: bool,
}

impl DiagnosticBlockState {
    fn message(&self) -> String {
        self.block.join("\n")
    }

    const fn is_error(&self) -> bool {
        self.is_error
    }

    fn consume(&mut self) -> String {
        let message = self.message();
        self.is_error = false;
        self.block.clear();
        message
    }
}

/// Tracks state when filtering `bazel build` output.
#[derive(Debug, Default)]
struct BazelBuildState {
    /// Errors
    errors: Vec<String>,

    /// Warnings
    warnings: Vec<String>,

    /// Current diagnostic block being processed
    diagnostic: DiagnosticBlockState,

    /// Number of actions in the build
    action_count: Option<usize>,
}

impl BazelBuildState {
    const fn num_errors(&self) -> usize {
        self.errors.len()
    }

    const fn num_warnings(&self) -> usize {
        self.warnings.len()
    }

    const fn success(&self) -> bool {
        self.num_errors() == 0 && self.num_warnings() == 0
    }

    const fn has_errors(&self) -> bool {
        self.num_errors() > 0
    }

    const fn has_warnings(&self) -> bool {
        self.num_warnings() > 0
    }

    const fn in_diagnostic_block(&self) -> bool {
        !self.diagnostic.block.is_empty()
    }

    const fn action_count(&self) -> Option<usize> {
        self.action_count
    }

    fn errors(&self) -> &[String] {
        &self.errors
    }

    fn warnings(&self) -> &[String] {
        &self.warnings
    }

    fn consume_diagnostic(&mut self) {
        let is_error = self.diagnostic.is_error();
        let msg = self.diagnostic.consume();
        if !msg.is_empty() {
            if is_error {
                self.errors.push(msg);
            } else {
                self.warnings.push(msg);
            }
        }
    }

    fn digest_line(&mut self, line: &str) {
        let trimmed = line.trim();

        // Bazel action count
        if let Some(captures) = ACTION_COUNT.captures(trimmed) {
            // Incremental builds may emit "total actions" multiple times.
            // For simplicity, only remember the first action count seen.
            if self.action_count.is_some() {
                #[cfg(debug_assertions)]
                eprintln!("rtk: Duplicate action count in line '{}'", trimmed);
                return;
            }

            match captures.get(1).map(|capture| capture.as_str().parse()) {
                // Successfully parsed action count
                // (`.parse()` returned `Ok`)
                Some(Ok(action_count)) => {
                    self.action_count = Some(action_count);
                }
                // Failed to parse action count
                // (`.parse()` returned `Err`)
                Some(Err(e)) => {
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "rtk: Failed to parse action count from capture '{}' in line '{}': {}",
                        &captures.get(1).unwrap().as_str(),
                        trimmed,
                        e
                    );
                }
                // Not enough capture groups
                // (`.get(1)` returned `None`)
                None => {
                    #[cfg(debug_assertions)]
                    eprintln!(
                        "rtk: Not enough captures for action count in line '{}'",
                        trimmed
                    );
                }
            }
        }
        // Bazel error
        else if trimmed.starts_with("ERROR:") {
            // Flush any in-progress compiler diagnostic block.
            if self.in_diagnostic_block() {
                self.consume_diagnostic();
            }

            // Skip the summary "Build did NOT complete successfully" — we show our own header
            if trimmed.contains("Build did NOT complete successfully") {
                return;
            }

            // Add the Bazel error
            self.errors.push(trimmed.to_string());
            return;
        }
        // Bazel warning
        else if trimmed.starts_with("WARNING:") {
            // Flush any in-progress compiler diagnostic block.
            if self.in_diagnostic_block() {
                self.consume_diagnostic();
            }

            // Add the Bazel warning
            self.warnings.push(trimmed.to_string());
            return;
        }
        // Start of diagnostic block
        else if trimmed.starts_with("warning:")
            || trimmed.starts_with("error:")
            || trimmed.contains(": warning:")
            || trimmed.contains(": error:")
        {
            // Flush previous diagnostic block, if any.
            if self.in_diagnostic_block() {
                self.consume_diagnostic();
            }

            // Add the diagnostic block
            self.diagnostic.block.push(trimmed.to_string());
            self.diagnostic.is_error = trimmed.contains(": error:");
            return;
        }
        // Currently inside diagnostic block
        else if self.in_diagnostic_block() {
            if trimmed.is_empty() {
                // End of diagnostic block
                self.consume_diagnostic();
            } else {
                // Inside diagnostic block
                self.diagnostic.block.push(trimmed.to_string());
            }
        }

        // Everything else is ignored.
    }

    fn finalize(&mut self) {
        // Flush any in-progress compiler diagnostic block.
        if self.in_diagnostic_block() {
            self.consume_diagnostic();
        }
    }
}

/// Filter `bazel build` output.
///
/// # Arguments
///
/// * `output` - Output from `bazel build`
///
/// # Returns
///
/// Filtered `bazel build` output
///
/// # Notes
///
/// This function detects errors, warnings, and build results and
/// condenses them.
/// * Error lines (e.g. `ERROR: ...`)
/// * Warning lines (e.g. `WARNING: ...`)
/// * Compiler diagnostics (e.g. from gcc/clang/rustc)
///
/// [`BAZEL_EXTRA_FLAGS`] already filters out a lot of build noise.
/// This function assumes these things have already been filtered out:
/// * Progress lines (e.g. `[100 / 200] 5 actions, 4 running`)
/// * Status lines (e.g. `Loading ...`)
/// * Timestamps
///
pub fn filter_bazel_build(output: &str) -> Result<String> {
    let mut state = BazelBuildState::default();
    for line in output.lines() {
        state.digest_line(line);
    }
    state.finalize();

    // Build the summary line.
    //
    // Examples:
    // "✓ bazel build (1337 actions)"
    // "bazel build: 1 warning (1337 actions)"
    // "bazel build: 2 errors (1337 actions)"
    // "bazel build: 1 error, 4 warnings (1337 actions)"
    let build_summary = {
        let mut build_summary: String = String::new();

        // Success checkmark
        if state.success() {
            build_summary.push_str("✓ ");
        }

        // Command name
        build_summary.push_str("bazel build");

        // Errors
        if state.has_errors() {
            let suffix = if state.num_errors() == 1 { "" } else { "s" };
            build_summary.push_str(&format!(": {} error{}", state.num_errors(), suffix));
        }

        // Warnings
        if state.has_warnings() {
            let prefix = if state.has_errors() { ", " } else { ": " };
            let suffix = if state.num_warnings() == 1 { "" } else { "s" };
            build_summary.push_str(&format!(
                "{}{} warning{}",
                prefix,
                state.num_warnings(),
                suffix
            ));
        }

        // Actions
        if let Some(action_count) = state.action_count {
            let suffix = if action_count == 1 { "" } else { "s" };
            build_summary.push_str(&format!(" ({} action{})", action_count, suffix));
        }

        build_summary
    };

    // If succesful, return only the summary line.
    if state.success() {
        return Ok(build_summary);
    }

    // Otherwise, include a summary of the warnings and errors.
    let mut result = build_summary;
    result.push_str("\n═══════════════════════════════════════\n");

    // Show errors first, then warnings
    let all_blocks: Vec<&String> = state
        .errors()
        .iter()
        .chain(state.warnings().iter())
        .collect();
    for (i, block) in all_blocks.iter().enumerate().take(MAX_BUILD_ISSUES) {
        result.push_str(block);
        result.push('\n');
        if i < all_blocks.len().min(MAX_BUILD_ISSUES) - 1 {
            result.push('\n');
        }
    }

    if all_blocks.len() > MAX_BUILD_ISSUES {
        result.push_str(&format!(
            "\n... +{} more issues\n",
            all_blocks.len() - MAX_BUILD_ISSUES
        ));
    }

    Ok(result.trim().to_string())
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
    run_bazel_filtered("build", args, verbose, filter_bazel_build)
}

/**********************************************************************/
/*                            bazel test                              */
/**********************************************************************/

#[derive(Debug, Default)]
struct BazelTestState {
    passed: usize,
    failed: usize,
    elapsed: Option<String>,

    error_lines: Vec<String>,
    fail_lines: Vec<String>,
    failed_result_lines: Vec<String>,
    inline_output_blocks: Vec<String>,

    in_test_output: bool,
    current_output_block: Vec<String>,
}

impl BazelTestState {
    fn digest_line(&mut self, raw_line: &str) {
        let line = raw_line.trim_end_matches('\r');
        let trimmed = line.trim();
        let stripped = trimmed;

        // Collecting inline test output between delimiter lines.
        if self.in_test_output {
            if TEST_OUTPUT_END.is_match(stripped) {
                self.current_output_block.push(stripped.to_string());
                self.inline_output_blocks
                    .push(self.current_output_block.join("\n"));
                self.current_output_block.clear();
                self.in_test_output = false;
            } else {
                self.current_output_block.push(line.to_string());
            }
            return;
        }

        if stripped.is_empty() {
            return;
        }

        // Extract elapsed time before skipping INFO/DEBUG lines.
        if stripped.starts_with("INFO:") || stripped.starts_with("DEBUG:") {
            if let Some(caps) = ELAPSED_TIME.captures(stripped) {
                self.elapsed = Some(caps[1].to_string());
            }
            return;
        }

        // Strip loading/analyzing status.
        if stripped.starts_with("Loading:")
            || stripped.starts_with("Analyzing:")
            || stripped.starts_with("Computing main repo mapping:")
        {
            return;
        }

        // Strip Java notes.
        if stripped.starts_with("Note: ") {
            return;
        }

        // Strip target output paths.
        if stripped.starts_with("Target //") || stripped.starts_with("bazel-bin/") {
            return;
        }

        // Strip DEBUG lines.
        if stripped.starts_with("DEBUG:") {
            return;
        }

        // Strip timeout size warnings.
        if stripped.starts_with("There were tests whose specified size") {
            return;
        }

        // Test result lines: //pkg:test PASSED in 0.3s.
        if let Some(caps) = TEST_RESULT_LINE.captures(stripped) {
            let status = &caps[2];
            match status {
                "PASSED" => self.passed += 1,
                "FAILED" | "TIMEOUT" | "NO STATUS" => {
                    self.failed += 1;
                    self.failed_result_lines.push(stripped.to_string());
                }
                "FLAKY" => self.passed += 1, // flaky but passed on retry
                _ => {}
            }
            return;
        }

        // Executed summary line (skip — we produce our own).
        if TEST_SUMMARY.is_match(stripped) {
            return;
        }

        // FAIL: lines.
        if FAIL_LINE.is_match(stripped) {
            self.fail_lines.push(stripped.to_string());
            return;
        }

        // Inline test output start.
        if TEST_OUTPUT_START.is_match(stripped) {
            self.in_test_output = true;
            self.current_output_block.push(stripped.to_string());
            return;
        }

        // ERROR lines.
        if stripped.starts_with("ERROR:") {
            if stripped.contains("Build did NOT complete successfully")
                || stripped.contains("not all tests passed")
            {
                return;
            }
            self.error_lines.push(stripped.to_string());
            return;
        }

        // WARNING lines (strip — build noise).
        if stripped.starts_with("WARNING:") {
            return;
        }

        // Indented log paths after FAILED lines (e.g. "  /path/to/test.log").
        // Keep only if we have failures.
        if stripped.starts_with('/') && stripped.ends_with(".log") && self.failed > 0 {
            return; // skip log paths — we show inline output instead
        }

        // Everything else is noise — skip.
    }

    fn finalize(&mut self) {
        if !self.current_output_block.is_empty() {
            self.inline_output_blocks
                .push(self.current_output_block.join("\n"));
            self.current_output_block.clear();
            self.in_test_output = false;
        }
    }
}

/// Filter `bazel test` output.
///
/// # Arguments
///
/// * `output` - Output from `bazel test`
///
/// # Returns
///
/// The filtered output
///
/// # Notes
///
/// Strips the same build noise as `filter_bazel_build`, plus parses test
/// result lines (PASSED/FAILED/TIMEOUT) and inline test output blocks.
/// On all-pass, returns a one-liner. On failure, shows FAIL blocks and
/// inline test output while stripping surrounding noise.
///
pub fn filter_bazel_test(output: &str) -> Result<String> {
    let mut state = BazelTestState::default();
    for line in output.lines() {
        state.digest_line(line);
    }
    state.finalize();

    let elapsed_str = state.elapsed.unwrap_or_else(|| "0".to_string());

    // Build error — no test results but ERROR lines present.
    if state.passed == 0 && state.failed == 0 && !state.error_lines.is_empty() {
        let mut result = String::from("bazel test: build failed\n");
        result.push_str("═══════════════════════════════════════\n");
        for err in state.error_lines.iter().take(MAX_BUILD_ISSUES) {
            result.push_str(err);
            result.push('\n');
        }
        if state.error_lines.len() > 15 {
            result.push_str(&format!(
                "\n... +{} more errors\n",
                state.error_lines.len() - 15
            ));
        }
        return Ok(result.trim().to_string());
    }

    // All pass: one-liner.
    if state.failed == 0 {
        return Ok(format!(
            "\u{2713} bazel test: {} passed, 0 failed ({}s)",
            state.passed, elapsed_str
        ));
    }

    // Has failures: show details.
    let mut result = String::new();
    result.push_str(&format!(
        "bazel test: {} failed, {} passed ({}s)\n",
        state.failed, state.passed, elapsed_str
    ));
    result.push_str("═══════════════════════════════════════\n");

    // FAIL: lines.
    let mut block_count = 0;
    for fail in &state.fail_lines {
        if block_count >= 15 {
            break;
        }
        result.push_str(fail);
        result.push('\n');
        block_count += 1;
    }

    // Inline test output blocks.
    for block in &state.inline_output_blocks {
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

    // FAILED result lines.
    for line in &state.failed_result_lines {
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

    // Error lines (if any).
    for err in &state.error_lines {
        if block_count >= 15 {
            break;
        }
        result.push_str(err);
        result.push('\n');
        block_count += 1;
    }

    let total_blocks = state.fail_lines.len()
        + state.inline_output_blocks.len()
        + state.failed_result_lines.len()
        + state.error_lines.len();
    if total_blocks > MAX_BUILD_ISSUES {
        result.push_str(&format!(
            "\n... +{} more blocks\n",
            total_blocks - MAX_BUILD_ISSUES
        ));
    }

    Ok(result.trim().to_string())
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
    run_bazel_filtered("test", args, verbose, filter_bazel_test)
}

/**********************************************************************/
/*                            bazel run                               */
/**********************************************************************/

/// Current stage of the `bazel run` process
#[derive(Debug, PartialEq)]
enum BazelRunStage {
    Build,
    Run,
}

/// Filter `bazel run` output.
///
/// # Arguments
///
/// * `output` - Output from `bazel run`
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
pub fn filter_bazel_run(output: &str) -> Result<String> {
    let mut current_stage = BazelRunStage::Build;
    let mut build_state = BazelBuildState::default();
    let mut run_lines: Vec<String> = Vec::new();

    for line in output.split_inclusive('\n') {
        let trimmed = line.trim();
        match current_stage {
            // Build stage
            BazelRunStage::Build => {
                if RUN_SENTINEL.is_match(trimmed) {
                    // Move to execution stage and output all stdout
                    // and stderr verbatim
                    build_state.finalize();
                    current_stage = BazelRunStage::Run;
                } else {
                    build_state.digest_line(trimmed);
                }
            }
            // Run stage
            BazelRunStage::Run => {
                // Collect run lines verbatim
                run_lines.push(line.to_string());
            }
        }
    }

    // If no errors, just return the run lines verbatim
    //
    // Warnings are ignored unless errors also occured
    if !build_state.has_errors() {
        // `split_inclusive('\n')` preserves newline delimiters in each segment,
        // so concatenate directly to avoid injecting extra blank lines.
        return Ok(run_lines.concat());
    }

    // Build the summary line.
    //
    // Examples:
    // "bazel run: 2 errors"
    // "bazel run: 1 error, 4 warnings "
    let build_summary = {
        let mut build_summary: String = String::new();

        // Command name
        build_summary.push_str("bazel run");

        // Errors
        let error_num = build_state.num_errors();
        let error_suffix = if error_num == 1 { "" } else { "s" };
        build_summary.push_str(&format!(": {} error{}", error_num, error_suffix));

        // Warnings
        if build_state.has_warnings() {
            let prefix = if error_num > 0 { ", " } else { ": " };
            let suffix = if build_state.num_warnings() == 1 {
                ""
            } else {
                "s"
            };
            build_summary.push_str(&format!(
                "{}{} warning{}",
                prefix,
                build_state.num_warnings(),
                suffix
            ));
        }

        // Actions
        if let Some(action_count) = build_state.action_count() {
            let suffix = if action_count == 1 { "" } else { "s" };
            build_summary.push_str(&format!(" ({} action{})", action_count, suffix));
        }

        build_summary
    };

    // Include a summary of the warnings and errors.
    let mut result = build_summary;
    result.push_str("\n═══════════════════════════════════════\n");

    // Show errors first, then warnings
    let all_blocks: Vec<&String> = build_state
        .errors()
        .iter()
        .chain(build_state.warnings().iter())
        .collect();
    for (i, block) in all_blocks.iter().enumerate().take(MAX_BUILD_ISSUES) {
        result.push_str(block);
        result.push('\n');
        if i < all_blocks.len().min(MAX_BUILD_ISSUES) - 1 {
            result.push('\n');
        }
    }

    if all_blocks.len() > MAX_BUILD_ISSUES {
        result.push_str(&format!(
            "\n... +{} more issues\n",
            all_blocks.len() - MAX_BUILD_ISSUES
        ));
    }

    Ok(result.trim().to_string())
}

/// Run `bazel run` while filtering the output.
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
    run_bazel_filtered("run", args, verbose, filter_bazel_run)
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

/// Filter `bazel query` output.
///
/// # Arguments
///
/// * `output` - Output from `bazel query`
/// * `depth` - Maximum depth of the package tree to show
/// * `width` - Maximum number of items to show per package
///
/// # Returns
///
/// The filtered output
///
pub fn filter_bazel_query(output: &str, depth: usize, width: usize) -> Result<String> {
    let mut result = String::new();
    let mut has_error_lines = false;

    // Group targets by output-derived roots:
    // - local roots: "//" and "//level0"
    // - external roots: "@repo"
    let mut local_sections: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut external_sections: BTreeMap<String, BTreeMap<String, Vec<String>>> = BTreeMap::new();
    let mut section_order: Vec<QuerySectionRoot> = Vec::new();
    let mut seen_sections: HashSet<QuerySectionRoot> = HashSet::new();

    for line in output.lines() {
        let trimmed = line.trim();

        if trimmed.is_empty() {
            continue;
        }
        // Bazel error
        else if trimmed.starts_with("ERROR:") {
            has_error_lines = true;
            result.push_str(trimmed);
            result.push('\n');
        }
        // Bazel target
        else if let Some(caps) = TARGET_LINE.captures(trimmed) {
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

    Ok(result.trim_end().to_string())
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
    run_bazel_filtered("query", args, verbose, |output| {
        filter_bazel_query(output, depth.value(), width.value())
    })
}

/**********************************************************************/
/*                      Other bazel subcommands                       */
/**********************************************************************/

/// Run a unsupported `bazel` subcommand by passing it through directly.
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
pub fn run_passthrough(args: &[OsString], verbose: u8) -> Result<()> {
    if args.is_empty() {
        anyhow::bail!("bazel: no subcommand specified");
    }

    if verbose > 0 {
        eprintln!("bazel passthrough: {:?}", args);
    }

    let timer = tracking::TimedExecution::start();

    let status = resolved_command("bazel")
        .args(args)
        .status()
        .context("Failed to run bazel")?;

    let args_str = tracking::args_display(args);
    timer.track_passthrough(
        &format!("bazel {}", args_str),
        &format!("rtk bazel {} (passthrough)", args_str),
    );

    if !status.success() {
        std::process::exit(status.code().unwrap_or(1));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /******************************************************************/
    /*                     Test Helper Functions                      */
    /******************************************************************/

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    /******************************************************************/
    /*                       bazel build tests                        */
    /******************************************************************/
    mod build {
        use super::*;

        #[test]
        /// Test `bazel build` filtering on a succesful build with one action.
        fn test_filter_success_one_action() {
            let output = "\
INFO: Analyzed target //src:bazel-dev (0 packages loaded, 0 targets configured).
INFO: Found 1 target...
Target //src:bazel-dev up-to-date:
bazel-bin/src/bazel-dev
INFO: Elapsed time: 0.453s, Critical Path: 0.00s
INFO: 1 process: 1 internal.
INFO: Build completed successfully, 1 total action
    ";
            let result = filter_bazel_build(output).unwrap();
            assert_eq!(result, "✓ bazel build (1 action)");
        }

        #[test]
        /// Test `bazel build` filtering on a succesful build with multiple actions.
        fn test_filter_success_multiple_actions() {
            let output = "\
INFO: Analyzed target //src:bazel-dev (563 packages loaded, 24852 targets configured, 175 aspect applications).
INFO: Found 1 target...
Target //src:bazel-dev up-to-date:
bazel-bin/src/bazel-dev
INFO: Elapsed time: 54.859s, Critical Path: 49.98s
INFO: 2391 processes: 3 internal, 1537 processwrapper-sandbox, 881 worker.
INFO: Build completed successfully, 2391 total actions";
            let result = filter_bazel_build(output).unwrap();
            assert_eq!(result, "✓ bazel build (2391 actions)");
        }

        #[test]
        /// Test `bazel build` filtering on a succesful build with no actions.
        fn test_filter_success_no_actions() {
            let output = "\
INFO: Analyzed target //src:bazel-dev (563 packages loaded, 24852 targets configured, 175 aspect applications).
INFO: Found 1 target...
Target //src:bazel-dev up-to-date:
bazel-bin/src/bazel-dev
INFO: Elapsed time: 54.859s, Critical Path: 49.98s
    ";
            let result = filter_bazel_build(output).unwrap();
            assert_eq!(result, "✓ bazel build");
        }

        #[test]
        /// Test `bazel build` filtering on a build with warnings.
        fn test_filter_with_warnings() {
            let output = "\
Computing main repo mapping:
Loading:
Loading: 1 packages loaded
Analyzing: target //src:bazel-dev (6 packages loaded, 6 targets configured)
WARNING: /home/user/bazel/src/conditions/BUILD:119:15: select() on cpu is deprecated.
WARNING: /home/user/bazel/src/conditions/BUILD:202:15: select() on cpu is deprecated.
WARNING: /home/user/bazel/src/conditions/BUILD:193:15: select() on cpu is deprecated.
INFO: Analyzed target //src:bazel-dev (563 packages loaded).
INFO: Found 1 target...
Target //src:bazel-dev up-to-date:
bazel-bin/src/bazel-dev
INFO: Elapsed time: 54.859s, Critical Path: 49.98s
INFO: Build completed successfully, 4978 total actions";
            let result = filter_bazel_build(output).unwrap();

            eprintln!("[DEBUG] Result is:\n{}", result);

            assert!(result.contains("bazel build: 3 warnings (4978 actions)"));
            assert!(result.contains("═══════════════════════════════════════"));
            assert!(result.contains("WARNING:"));
            assert!(result.contains("select() on cpu is deprecated"));
            // Noise should be stripped
            assert!(!result.contains("Loading:"));
            assert!(!result.contains("Analyzing:"));
            assert!(!result.contains("INFO:"));
        }

        #[test]
        /// Test `bazel build` filtering on a build with errors.
        fn test_filter_with_errors() {
            let output = "\
Computing main repo mapping:
Loading:
Loading: 0 packages loaded
ERROR: Skipping '//src:bazel-dev-NONEXISTENT': no such target '//src:bazel-dev-NONEXISTENT'
ERROR: no such target '//src:bazel-dev-NONEXISTENT': target 'bazel-dev-NONEXISTENT' not declared in package 'src'
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
            let result = filter_bazel_build(output).unwrap();

            // Build summary
            assert!(result.contains("bazel build: 2 errors"));

            // Errors are printed
            assert!(result.contains("ERROR: Skipping"));
            assert!(result.contains("ERROR: no such target"));

            // "Build did NOT complete successfully" is stripped (we have our own header)
            assert!(!result.contains("Build did NOT complete successfully"));

            // Noise stripped
            assert!(!result.contains("Loading:"));
            assert!(!result.contains("INFO:"));
        }

        #[test]
        /// Test `bazel build` filtering on a build with errors and warnings.
        fn test_filter_with_errors_and_warnings() {
            let output = "\
Computing main repo mapping:
Loading:
Loading: 0 packages loaded
WARNING: /home/user/bazel/src/conditions/BUILD:119:15: select() on cpu is deprecated.
ERROR: Skipping '//src:bazel-dev-NONEXISTENT': no such target '//src:bazel-dev-NONEXISTENT'
ERROR: no such target '//src:bazel-dev-NONEXISTENT': target 'bazel-dev-NONEXISTENT' not declared in package 'src'
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
            let result = filter_bazel_build(output).unwrap();

            // Build summary
            assert!(result.contains("bazel build: 2 errors, 1 warning"));

            // Errors are printed
            assert!(result.contains("ERROR: Skipping"));
            assert!(result.contains("ERROR: no such target"));

            // Warnings are printed
            assert!(result.contains("WARNING:"));
            assert!(result.contains("select() on cpu is deprecated"));

            // "Build did NOT complete successfully" is stripped (we have our own header)
            assert!(!result.contains("Build did NOT complete successfully"));

            // Noise stripped
            assert!(!result.contains("Loading:"));
            assert!(!result.contains("INFO:"));
        }

        #[test]
        fn test_filter_java_compiler_warnings() {
            let output = "\
INFO: Analyzed target //src:bazel-dev (563 packages loaded).
INFO: From Building external/protobuf+/java/core/liblite_runtime_only.jar (94 source files):
bazel-out/k8-fastbuild/bin/src/main/protobuf/failure_details.pb.h:9953:111: warning: 'some_field' is deprecated [-Wdeprecated-declarations]
9953 |   [[deprecated]] static constexpr Code FIELD = value;
     |                                                ^~~~~
bazel-out/k8-fastbuild/bin/src/main/protobuf/failure_details.pb.h:1690:3: note: declared here
1690 |   SomeField [[deprecated]] = 2,
     |   ^~~~~~~~~

INFO: Build completed successfully, 200 total actions";
            let result = filter_bazel_build(output).unwrap();

            // Should keep the compiler warning block
            assert!(result.contains("warning:"));
            assert!(result.contains("deprecated"));
            assert!(result.contains("note: declared here"));
            // Should show warning count
            assert!(result.contains("1 warning"));
            // Noise stripped
            assert!(!result.contains("INFO:"));
        }

        #[test]
        fn test_filter_rustc_compiler_warnings() {
            let output = "\
INFO: Analyzed target //src:rust_app (100 packages loaded).
warning: field `value` is never read
--> src/lib.rs:12:5
|
12 |     value: usize,
|     ^^^^^
|
= note: `#[warn(dead_code)]` on by default

INFO: Build completed successfully, 42 total actions";
            let result = filter_bazel_build(output).unwrap();

            assert!(result.contains("warning: field `value` is never read"));
            assert!(result.contains("src/lib.rs:12:5"));
            assert!(result.contains("1 warning"));
            assert!(!result.contains("INFO:"));
        }

        #[test]
        fn test_filter_gcc_compiler_warnings() {
            let output = "\
INFO: Analyzed target //src:gcc_app (32 packages loaded).
src/main.c:14:9: warning: unused variable 'tmp' [-Wunused-variable]
14 |     int tmp = 0;
   |         ^~~

INFO: Build completed successfully, 7 total actions";
            let result = filter_bazel_build(output).unwrap();

            assert!(result.contains("warning: unused variable 'tmp'"));
            assert!(result.contains("src/main.c:14:9"));
            assert!(result.contains("1 warning"));
            assert!(!result.contains("INFO:"));
        }

        #[test]
        fn test_filter_clang_compiler_warnings() {
            let output = "\
INFO: Analyzed target //src:clang_app (48 packages loaded).
src/main.cc:21:7: warning: unused variable 'counter' [-Wunused-variable]
21 |   int counter = 0;
   |       ^~~~~~~
1 warning generated.

INFO: Build completed successfully, 11 total actions";
            let result = filter_bazel_build(output).unwrap();

            assert!(result.contains("warning: unused variable 'counter'"));
            assert!(result.contains("src/main.cc:21:7"));
            assert!(result.contains("1 warning"));
            assert!(!result.contains("INFO:"));
        }

        #[test]
        fn test_filter_strips_info_noise() {
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
            let result = filter_bazel_build(stderr).unwrap();

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
        fn test_filter_empty() {
            let result = filter_bazel_build("").unwrap();
            assert_eq!(result, "✓ bazel build");
        }

        #[test]
        fn test_filter_token_savings() {
            // Real-ish bazel build output (~80 lines of noise)
            let output = "\
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
INFO: From Building external/zstd-jni+/libzstd-jni-class.jar (30 source files) [for tool]:
Note: Some input files use or override a deprecated API that is marked for removal.
Note: Recompile with -Xlint:removal for details.
INFO: From Building external/zstd-jni+/libzstd-jni-class.jar (30 source files):
Note: Some input files use or override a deprecated API that is marked for removal.
Note: Recompile with -Xlint:removal for details.
INFO: Found 1 target...
Target //src:bazel-dev up-to-date:
bazel-bin/src/bazel-dev
INFO: Elapsed time: 54.859s, Critical Path: 49.98s
INFO: 2391 processes: 3 internal, 1537 processwrapper-sandbox, 881 worker.
INFO: Build completed successfully, 2391 total actions";

            let input_tokens = count_tokens(output);
            let result = filter_bazel_build(output).unwrap();
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
        fn test_filter_truncates_blocks() {
            // More than 15 issues should truncate
            let mut stderr = String::new();
            for i in 0..20 {
                stderr.push_str(&format!("ERROR: //pkg:target_{}: build failed\n\n", i));
            }
            let result = filter_bazel_build(&stderr).unwrap();

            assert!(result.contains("... +5 more issues"));
        }

        #[test]
        fn test_filter_mixed_compiler_and_bazel_errors() {
            let output = "\
WARNING: /home/user/bazel/BUILD:10:5: select() on cpu is deprecated.
INFO: Analyzed target //src:app
src/app.cc:42:10: error: use of undeclared identifier 'foo'
42 |   foo();
    |   ^~~

ERROR: //src:app failed to build
INFO: Build completed, 0 total actions
ERROR: Build did NOT complete successfully";
            let result = filter_bazel_build(output).unwrap();

            // Should have 2 errors (compiler + bazel ERROR) and 1 warning
            assert!(result.contains("2 errors"));
            assert!(result.contains("1 warning"));
            assert!(result.contains("error: use of undeclared identifier"));
            assert!(result.contains("ERROR: //src:app failed to build"));
            assert!(result.contains("WARNING:"));
            assert!(result.contains("select() on cpu is deprecated"));
        }
    }

    /******************************************************************/
    /*                       bazel query tests                        */
    /******************************************************************/
    mod query {
        use super::*;

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

        #[test]
        fn test_strips_info_warning_noise() {
            let output = "\
INFO: Invocation ID: abc-123
INFO: Build options changed
WARNING: some warning
DEBUG: debug info
INFO: plain info line
WARNING: plain warning
DEBUG: plain debug
//pkg:target";
            let result = filter_bazel_query(output, usize::MAX, usize::MAX).unwrap();

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
            let output = "\
INFO: Build options changed
ERROR: something went wrong
ERROR: another error
//pkg:target";
            let result = filter_bazel_query(output, usize::MAX, usize::MAX).unwrap();

            assert!(result.contains("ERROR: something went wrong"));
            assert!(result.contains("ERROR: another error"));
            assert!(!result.contains("Build options changed"));
        }

        #[test]
        fn test_empty_output() {
            let result = filter_bazel_query("", usize::MAX, usize::MAX).unwrap();
            // With default root, header is still produced
            assert!(result.contains("// (0 targets)"));
        }

        #[test]
        fn test_single_target_uses_singular() {
            let stdout = "//my/package:only_target";
            let result = filter_bazel_query(stdout, usize::MAX, usize::MAX).unwrap();
            assert!(result.contains("(1 target)"));
        }

        #[test]
        fn test_header_line() {
            let stdout = "\
//src/lib:a
//src/lib:b
//tools:c";
            let result = filter_bazel_query(stdout, usize::MAX, usize::MAX).unwrap();

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
            let result = filter_bazel_query(stdout, 1, usize::MAX).unwrap();

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
            let result = filter_bazel_query(stdout, 2, usize::MAX).unwrap();

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
            let result = filter_bazel_query(stdout, usize::MAX, usize::MAX).unwrap();

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
            let result = filter_bazel_query(stdout, 2, usize::MAX).unwrap();

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
            let result = filter_bazel_query(stdout, 1, 5).unwrap();

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
            let result = filter_bazel_query(stdout, 1, 3).unwrap();

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
            let result = filter_bazel_query(stdout, 1, 3).unwrap();

            assert!(result.contains("(+1 more sub-package, 3 more targets)"));
        }

        #[test]
        fn test_condensed_truncation_omits_zero_parts() {
            let stdout = "\
//root/a:t
//root/b:t
//root/c:t
//root/d:t";
            let result = filter_bazel_query(stdout, 1, 3).unwrap();

            assert!(result.contains("(+1 more sub-package)"));
            assert!(!result.contains("more target"));
        }

        #[test]
        fn test_root_targets_inline() {
            let stdout = "\
//:bazel-distfile
//:bazel-srcs
//src:lib";
            let result = filter_bazel_query(stdout, 1, usize::MAX).unwrap();

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
            let result = filter_bazel_query(stdout, 2, usize::MAX).unwrap();

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
            let result = filter_bazel_query(stdout, usize::MAX, usize::MAX).unwrap();

            // With full depth, targets are at leaf nodes
            assert!(result.contains("🎯 :target_a"));
            assert!(result.contains("🎯 :target_b"));
            assert!(result.contains("🎯 :target_c"));
            assert!(result.contains("🎯 :foo"));
            assert!(result.contains("🎯 :bar"));
        }

        #[test]
        fn test_real_bazel_output() {
            let output = "\
INFO: Invocation ID: 8e2f4a91-abc1-4def-9012-345678abcdef
INFO: Current date is 2026-03-01
WARNING: Build option --config=remote has changed
INFO: Repository rule @bazel_tools//tools/jdk:jdk configured
INFO: Found 16 targets...
INFO: Elapsed time: 1.234s
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

            let result = filter_bazel_query(output, usize::MAX, usize::MAX).unwrap();

            // Should strip all INFO/WARNING noise
            assert!(!result.contains("Invocation ID"));
            assert!(!result.contains("Elapsed time"));

            assert!(result.contains("//src/app/foo/bar (16 targets)"));

            assert!(result.contains("🎯 :bar\n"));
            assert!(result.contains("🎯 :runner_test"));
        }

        #[test]
        fn test_filter_multi_root_no_target_loss() {
            let output = "\
//src/app:bin
//tools/gen:tool
//third_party/lib:pkg";
            let result = filter_bazel_query(output, usize::MAX, usize::MAX).unwrap();

            assert!(result.contains("//src/app (1 target)"));
            assert!(result.contains("//tools/gen (1 target)"));
            assert!(result.contains("//third_party/lib (1 target)"));
            assert!(result.contains("🎯 :bin"));
            assert!(result.contains("🎯 :tool"));
            assert!(result.contains("🎯 :pkg"));
        }

        #[test]
        fn test_filter_multi_root_respects_width() {
            let output = "\
//src/s1:a
//src/s2:b
//tools/t1:c
//tools/t2:d";
            let result = filter_bazel_query(output, 1, 1).unwrap();

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
        fn test_filter_groups_external_repos() {
            let output = "\
//src/app:bin
@abseil-cpp//absl/base:core_headers
@abseil-cpp//absl/strings:str_format
@zlib//:zlib";
            let result = filter_bazel_query(output, 1, 10).unwrap();

            assert!(result.contains("//src/app (1 target)"));
            assert!(result.contains("@abseil-cpp//absl (2 targets"));
            assert!(result.contains("@zlib// (1 target)"));
            assert!(result.contains("📦 base (1 target)"));
            assert!(result.contains("📦 strings (1 target)"));
            assert!(result.contains("🎯 :zlib"));
        }

        #[test]
        fn test_filter_consolidates_deep_common_prefix() {
            let output = "\
//src/java_tools/buildjar:a
//src/java_tools/import_deps_checker:b
//src/java_tools/junitrunner:c";
            let result = filter_bazel_query(output, 1, 10).unwrap();

            assert!(result.starts_with("//src/java_tools (3 targets"));
            assert!(result.contains("📦 buildjar (1 target)"));
            assert!(result.contains("📦 import_deps_checker (1 target)"));
            assert!(result.contains("📦 junitrunner (1 target)"));
        }

        #[test]
        fn test_filter_splits_external_repos_by_repo_root() {
            let output = "\
@abseil-cpp//absl/base:core
@abseil-cpp//absl/strings:format
@bazel_skylib//lib:paths
@bazel_skylib//rules:copy";
            let result = filter_bazel_query(output, 1, 10).unwrap();

            assert!(result.contains("@abseil-cpp//absl (2 targets"));
            assert!(result.contains("@bazel_skylib// (2 targets, 2 packages)"));
            assert!(result.contains("📦 base (1 target)"));
            assert!(result.contains("📦 strings (1 target)"));
            assert!(result.contains("📦 lib (1 target)"));
            assert!(result.contains("📦 rules (1 target)"));
        }

        #[test]
        fn test_filter_external_root_targets_keep_repo_root_header() {
            let output = "\
@abseil-cpp//:root_target
@abseil-cpp//absl/base:core";
            let result = filter_bazel_query(output, 1, 10).unwrap();

            assert!(result.starts_with("@abseil-cpp// (2 targets, 2 packages)"));
            assert!(result.contains("📦 absl (1 target, 1 package)"));
            assert!(result.contains("🎯 :root_target"));
        }

        #[test]
        fn test_filter_depth_1_runtime_mode_stays_single_section() {
            let output = "\
//src:root
//src/conditions:a
//src/java_tools:b";
            let result = filter_bazel_query(output, 1, 10).unwrap();

            assert!(result.starts_with("//src (3 targets, 2 packages)"));
            assert!(result.contains("📦 conditions (1 target)"));
            assert!(result.contains("📦 java_tools (1 target)"));
            assert!(result.contains("🎯 :root"));
            assert!(!result.contains("..."));
        }

        #[test]
        fn test_filter_depth_2_runtime_mode_expands_to_sections() {
            let output = "\
//src:root_a
//src:root_b
//src/conditions:c1
//src/java_tools:j1
//src/java_tools/sub:s1";
            let result = filter_bazel_query(output, 2, 10).unwrap();

            assert!(result.contains("//src (2 targets)"));
            assert!(result.contains("🎯 :root_a"));
            assert!(result.contains("🎯 :root_b"));
            assert!(result.contains("//src/conditions (1 target)"));
            assert!(result.contains("//src/java_tools (2 targets, 1 package)"));
            // Depth sections are flat; no tree indentation in this mode.
            assert!(!result.contains("  📦"));
        }

        #[test]
        fn test_filter_depth_2_skips_empty_intermediate_section() {
            let output = "\
@xds+//xds/data/orca:alpha
@xds+//xds/data/orca:beta
@xds+//xds/service/orca:gamma
@xds+//xds/service/orca:delta";
            let result = filter_bazel_query(output, 2, 10).unwrap();

            assert!(!result.contains("@xds+//xds (0 targets)"));
            assert!(result.contains("@xds+//xds/data"));
            assert!(result.contains("@xds+//xds/service"));
        }

        #[test]
        fn test_filter_error_only_no_empty_header() {
            let output =
                "ERROR: Evaluation of query \"deps(//...)\" failed: preloading transitive closure failed";
            let result = filter_bazel_query(output, usize::MAX, 10).unwrap();

            assert_eq!(
                result,
                "ERROR: Evaluation of query \"deps(//...)\" failed: preloading transitive closure failed"
            );
            assert!(!result.contains("//... (0 targets)"));
        }

        #[test]
        fn test_filter_single_root_uses_subsections() {
            let output = "\
//src/lib:a
//src/app:b";
            let result = filter_bazel_query(output, 2, usize::MAX).unwrap();

            assert!(result.contains("//src/app (1 target)"));
            assert!(result.contains("//src/lib (1 target)"));
            assert!(result.contains("🎯 :a"));
            assert!(result.contains("🎯 :b"));
        }
    }

    /******************************************************************/
    /*                       bazel test tests                         */
    /******************************************************************/
    mod test {
        use super::*;

        #[test]
        fn test_filter_all_pass() {
            let output = "\
INFO: Analyzed 3 targets (0 packages loaded, 0 targets configured).
INFO: Found 1 target and 2 test targets...
INFO: Elapsed time: 0.508s, Critical Path: 0.00s
INFO: 1 process: 2 action cache hit, 1 internal.
INFO: Build completed successfully, 1 total action
//src/test/java/com/google/devtools/build/lib/runtime/commands/info:RemoteRequestedInfoItemHandlerTest PASSED in 2.4s
//src/test/java/com/google/devtools/build/lib/runtime/commands/info:StdoutInfoItemHandlerTest PASSED in 1.5s.";
            let result = filter_bazel_test(output).unwrap();
            assert_eq!(result, "\u{2713} bazel test: 2 passed, 0 failed (0.508s)");
        }

        #[test]
        fn test_filter_with_cached() {
            let output = "\
Loading:
INFO: Analyzed 2 targets (0 packages loaded, 0 targets configured).
INFO: Found 2 test targets...
//src/test/java/com/google/devtools/build/lib/util:CommandUtilsTest PASSED in 0.3s
//src/test/java/com/google/devtools/build/lib/util:StringEncodingTest (cached) PASSED in 0.1s
INFO: Elapsed time: 0.412s, Critical Path: 0.10s
INFO: 2 processes: 1 internal, 1 worker.
INFO: Build completed successfully, 2 total actions
Executed 1 out of 2 tests: 2 tests pass.";
            let result = filter_bazel_test(output).unwrap();
            assert_eq!(result, "\u{2713} bazel test: 2 passed, 0 failed (0.412s)");
        }

        #[test]
        fn test_filter_failure() {
            let output = "\
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
            let result = filter_bazel_test(output).unwrap();

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
        fn test_filter_failure_with_test_output() {
            let output = "\
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
            let result = filter_bazel_test(output).unwrap();

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
        fn test_filter_strips_build_noise() {
            let output = "\
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
            let result = filter_bazel_test(output).unwrap();

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
        fn test_filter_build_error() {
            let output = "\
Loading:
WARNING: Target pattern parsing failed.
ERROR: Skipping '//src:nonexistent': no such target '//src:nonexistent'
ERROR: no such target '//src:nonexistent': target 'nonexistent' not declared
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
            let result = filter_bazel_test(output).unwrap();

            assert!(result.contains("bazel test: build failed"));
            assert!(result.contains("═══════════════════════════════════════"));
            assert!(result.contains("ERROR: Skipping"));
            assert!(result.contains("ERROR: no such target"));
            // "Build did NOT complete successfully" stripped
            assert!(!result.contains("Build did NOT complete successfully"));
        }

        #[test]
        fn test_filter_empty() {
            let result = filter_bazel_test("").unwrap();
            assert_eq!(result, "\u{2713} bazel test: 0 passed, 0 failed (0s)");
        }

        #[test]
        fn test_filter_token_savings() {
            let output = "\
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

            let input_tokens = count_tokens(output);
            let result = filter_bazel_test(output).unwrap();
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
        fn test_filter_strips_timeout_warnings() {
            let output = "\
INFO: Analyzed 1 target (0 packages loaded).
INFO: Found 1 test target...
//pkg:test    PASSED in 0.5s
There were tests whose specified size is too big. Use the --test_verbose_timeout_warnings command line option to see which ones these are.
INFO: Elapsed time: 1.00s, Critical Path: 0.50s
INFO: Build completed successfully, 2 total actions
Executed 1 out of 1 test: 1 tests pass.";
            let result = filter_bazel_test(output).unwrap();

            assert!(!result.contains("There were tests whose specified size"));
            assert!(!result.contains("--test_verbose_timeout_warnings"));
            assert!(result.contains("\u{2713} bazel test: 1 passed, 0 failed"));
        }
    }

    /******************************************************************/
    /*                       bazel run tests                          */
    /******************************************************************/
    mod run {
        use super::*;

        #[test]
        /// Test a successful `bazel run`.
        fn test_filter_success() {
            let output = "\
INFO: Analyzed target //src:my_binary (81 packages loaded, 684 targets configured).
INFO: Found 1 target...
Target //src:my_binary up-to-date:
bazel-bin/src/my_binary
INFO: Elapsed time: 3.50s, Critical Path: 2.10s
INFO: 123 processes: 3 internal, 120 processwrapper-sandbox.
INFO: Build completed successfully, 123 total actions
INFO: Running command line: bazel-bin/src/my_binary
Hello from binary!
Result: 42
";
            let result = filter_bazel_run(output).unwrap();

            // Clean output — no build summary, just binary output
            assert_eq!(result, "Hello from binary!\nResult: 42\n");
        }

        #[test]
        /// Test that a successful `bazel run` ignores build warnings.
        fn test_filter_success_ignores_warnings() {
            let output = "\
WARNING: /home/user/BUILD:10:5: select() on cpu is deprecated.
WARNING: /home/user/BUILD:20:5: another deprecation warning.
INFO: Analyzed target //src:app (10 packages loaded).
INFO: Found 1 target...
Target //src:app up-to-date:
bazel-bin/src/app
INFO: Build completed successfully, 100 total actions
INFO: Running command line: bazel-bin/src/app
app output here";
            let result = filter_bazel_run(output).unwrap();

            // Clean output - no warnings shown
            assert_eq!(result, "app output here");
        }

        #[test]
        /// Test a failed `bazel run` with one error.
        fn test_filter_with_one_error() {
            let output = "\
ERROR: Skipping '//src:nonexistent': no such target '//src:nonexistent'
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
            let result = filter_bazel_run(output).unwrap();

            assert!(result.contains("bazel run: 1 error"));
            assert!(result.contains("ERROR: Skipping"));
            assert!(!result.contains("WARNING:"));
            assert!(!result.contains("Build did NOT complete successfully"));
        }

        #[test]
        /// Test a failed `bazel run` with multiple errors.
        fn test_filter_with_multiple_errors() {
            let output = "\
ERROR: Skipping '//src:nonexistent': no such target '//src:nonexistent'
ERROR: no such target '//src:nonexistent': target 'nonexistent' not declared
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
            let result = filter_bazel_run(output).unwrap();

            assert!(result.contains("bazel run: 2 errors"));
        }

        #[test]
        /// Test a failed `bazel run` with multiple errors and warnings.
        fn test_filter_with_multiple_errors_and_warnings() {
            let output = "\
WARNING: Target pattern parsing failed.
ERROR: Skipping '//src:nonexistent': no such target '//src:nonexistent'
ERROR: no such target '//src:nonexistent': target 'nonexistent' not declared
WARNING: File not found: /home/user/foo/bar.txt
INFO: Elapsed time: 0.142s
INFO: 0 processes.
ERROR: Build did NOT complete successfully";
            let result = filter_bazel_run(output).unwrap();

            assert!(
                result.contains("bazel run:")
                    && result.contains("error")
                    && result.contains("warning"),
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
        /// Test a `bazel run` with no sentinel (i.e. nothing was run).
        fn test_filter_no_sentinel() {
            let output = "\
INFO: Analyzed target //src:app (10 packages loaded).
[5 / 10] Compiling something.cc
INFO: Found 1 target...
Target //src:app up-to-date:
bazel-bin/src/app
INFO: Build completed successfully, 100 total actions";
            let result = filter_bazel_run(output).unwrap();
            assert!(result.is_empty());
        }

        #[test]
        fn test_filter_empty() {
            let result = filter_bazel_run("").unwrap();
            assert!(result.is_empty());
        }

        #[test]
        fn test_filter_token_savings() {
            let original = "\
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

            // Filtered output when using `BAZEL_EXTRA_FLAGS`
            //
            // The token savings computation assumes the agent does *not*
            // pass the `BAZEL_EXTRA_FLAGS` when running `bazel run`
            // directly.
            let prefiltered = "\
INFO: Analyzed target //src:my_binary (563 packages loaded, 24852 targets configured).
INFO: Found 1 target...
Target //src:my_binary up-to-date:
bazel-bin/src/my_binary
INFO: Elapsed time: 54.859s, Critical Path: 49.98s
INFO: 2391 processes: 3 internal, 1537 processwrapper-sandbox, 881 worker.
INFO: Build completed successfully, 2391 total actions
INFO: Running command line: bazel-bin/src/my_binary
Hello World";

            let input_tokens = count_tokens(original);
            let result = filter_bazel_run(prefiltered).unwrap();
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
        // Test that a successful `bazel run` preserves `bazel build` like
        // messages (e..g start with INFO/WARNING/DEBUG) after the run
        // sentinel.
        fn test_filter_preserves_messages_after_sentinel() {
            let output = "\
INFO: Build completed successfully, 10 total actions
INFO: Running command line: bazel-bin/app
INFO: Some trailing info line
WARNING: App warning
DEBUG: App debug
ERROR: App error
";
            let result = filter_bazel_run(output).unwrap();

            assert!(result.contains("INFO: Some trailing info line"));
            assert!(result.contains("WARNING: App warning"));
            assert!(result.contains("DEBUG: App debug"));
            assert!(result.contains("ERROR: App error"));
        }

        #[test]
        /// Test that a successful `bazel run` preserves all newlines.
        fn test_filter_preserves_newlines() {
            let output = "\
INFO: Build completed successfully, 10 total actions
INFO: Running command line: bazel-bin/app
line1

line2

";
            let result = filter_bazel_run(output).unwrap();
            assert_eq!(result, "line1\n\nline2\n\n");
        }

        #[test]
        fn test_filter_preserves_leading_whitespace() {
            let output = concat!(
                "INFO: Build completed successfully, 10 total actions\n",
                "INFO: Running command line: bazel-bin/app\n",
                "  indented\n",
                "\tTabbed\n",
            );
            let expected = concat!("  indented\n", "\tTabbed\n",);
            let actual = filter_bazel_run(output).unwrap();

            assert_eq!(expected, actual);
        }

        #[test]
        fn test_filter_preserves_trailing_whitespace() {
            let output = concat!(
                "INFO: Build completed successfully, 10 total actions\n",
                "INFO: Running command line: bazel-bin/app\n",
                "  hello! oops I left some trailing whitespace   \n",
            );
            let expected = "  hello! oops I left some trailing whitespace   \n";
            let actual = filter_bazel_run(output).unwrap();

            assert_eq!(expected, actual);
        }
    }
}
