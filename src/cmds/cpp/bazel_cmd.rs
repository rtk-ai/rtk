//! Filters Bazel build output — progress / loading / fetching noise stripped,
//! errors and warnings kept in diagnostic blocks.
#![allow(dead_code)]

use super::diag;
use crate::core::runner;
use crate::core::stream::{BlockHandler, BlockStreamFilter};
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::collections::HashMap;

// ── Line classification helpers ──

/// Check for Bazel progress snapshot lines: `[N / M] Compiling src/foo.cc; 12s ...`.
/// Commas in numbers are ignored during parse; this function just checks the bracket
/// shape so it can be called before the more expensive parse.
pub fn is_progress_snapshot(trimmed: &str) -> bool {
    if !trimmed.starts_with('[') {
        return false;
    }
    if let Some(end_bracket) = trimmed.find(']') {
        let inner = &trimmed[1..end_bracket];
        // Bazel uses a space-padded format: "2,720 / 3,125"
        if let Some(slash) = inner.find('/') {
            let left = inner[..slash].trim();
            let right = inner[slash + 1..].trim();
            // Accept digits and commas
            return left.chars().all(|c| c.is_ascii_digit() || c == ',')
                && !left.is_empty()
                && right.chars().all(|c| c.is_ascii_digit() || c == ',')
                && !right.is_empty();
        }
    }
    false
}

/// Parse `[N / M]` from a Bazel progress snapshot.  Commas are stripped.
/// Returns `(N, M)`.
pub fn parse_bazel_progress(trimmed: &str) -> Option<(usize, usize)> {
    if !trimmed.starts_with('[') {
        return None;
    }
    let end_bracket = trimmed.find(']')?;
    let inner = &trimmed[1..end_bracket];
    let slash = inner.find('/')?;
    let n_str: String = inner[..slash].chars().filter(|c| *c != ',').collect();
    let m_str: String = inner[slash + 1..].chars().filter(|c| *c != ',').collect();
    let n = n_str.trim().parse::<usize>().ok()?;
    let m = m_str.trim().parse::<usize>().ok()?;
    Some((n, m))
}

/// Check if a line reports loading / analysing: `Loading:` or `Analyzing:`.
pub fn is_loading_analyzing(trimmed: &str) -> bool {
    trimmed.starts_with("Loading:") || trimmed.starts_with("Analyzing:")
}

/// Check for repository fetch lines: ` Fetching repository @foo; starting`.
pub fn is_fetching(trimmed: &str) -> bool {
    trimmed.starts_with("Fetching repository ") || trimmed.starts_with(" Fetching repository ")
}

/// Check for Bazel ERROR lines: `ERROR: /path/BUILD:42:10: ...`.
pub fn is_bazel_error(trimmed: &str) -> bool {
    trimmed.starts_with("ERROR: ")
}

/// Check for Bazel WARNING lines: `WARNING: /path/BUILD:42:10: ...`.
pub fn is_bazel_warning(trimmed: &str) -> bool {
    trimmed.starts_with("WARNING: ")
}

/// Check for Bazel INFO summary lines: `INFO: Found`, `INFO: Elapsed`,
/// `INFO: Build completed successfully`, `INFO: Build did NOT complete successfully`.
pub fn is_bazel_info_summary(trimmed: &str) -> bool {
    trimmed.starts_with("INFO: Found")
        || trimmed.starts_with("INFO: Elapsed")
        || trimmed.starts_with("INFO: Build completed successfully")
        || trimmed.starts_with("INFO: Build did NOT complete successfully")
}

/// Check for Bazel verdict lines (success / failure banner).
pub fn is_bazel_verdict(trimmed: &str) -> bool {
    trimmed.starts_with("ERROR: Build did NOT complete successfully")
        || trimmed.starts_with("INFO: Build completed successfully")
}

/// Check for action-detail separator lines (`---`, `===`, `___`).
pub fn is_action_detail_separator(trimmed: &str) -> bool {
    let stripped = trimmed.trim();
    if stripped.len() < 3 {
        return false;
    }
    let first = stripped.chars().next().unwrap();
    (first == '-' || first == '=' || first == '_') && stripped.chars().all(|c| c == first)
}

/// Check for `failed: (Exit N)` — a failed action line in Bazel.
fn is_failed_exit_line(trimmed: &str) -> bool {
    trimmed.contains("failed: (Exit ")
}

// ── Handler ──

/// BlockHandler for Bazel build output.
pub struct BazelBuildHandler {
    /// Total actions from progress snapshots.
    actions_total: usize,
    /// Actions completed (from progress snapshots).
    actions_completed: usize,
    /// Packages loaded (parsed from loading/analyzing lines).
    packages_loaded: usize,
    /// Targets configured (parsed from analyzing lines).
    targets_configured: usize,
    /// Whether we are inside an error block.
    in_error_block: bool,
    /// Whether we are inside an action-detail block (separator-delimited).
    in_action_detail: bool,
    /// Collected error lines.
    errors: Vec<String>,
    /// Collected warning lines.
    warnings: Vec<String>,
    /// Warning flag → count.
    warning_counts: HashMap<String, usize>,
    /// Dedup: message body → count.
    seen_diagnostics: HashMap<String, usize>,
    /// Elapsed time from INFO: Elapsed line.
    elapsed_time: Option<String>,
    /// Whether the build verdict was failure.
    build_did_not_complete: bool,
}

impl BazelBuildHandler {
    pub fn new() -> Self {
        Self {
            actions_total: 0,
            actions_completed: 0,
            packages_loaded: 0,
            targets_configured: 0,
            in_error_block: false,
            in_action_detail: false,
            errors: Vec::new(),
            warnings: Vec::new(),
            warning_counts: HashMap::new(),
            seen_diagnostics: HashMap::new(),
            elapsed_time: None,
            build_did_not_complete: false,
        }
    }

    fn track_dedup(&mut self, diag_line: &str) -> bool {
        let msg = diag::extract_diag_message(diag_line);
        let count = self.seen_diagnostics.entry(msg).or_insert(0);
        *count += 1;
        *count <= 3
    }

    fn track_warning(&mut self, line: &str) {
        if let Some(flag) = diag::extract_warning_flag(line) {
            *self.warning_counts.entry(flag).or_insert(0) += 1;
        } else {
            *self.warning_counts.entry("other".to_string()).or_insert(0) += 1;
        }
    }

    /// Try to parse an integer value preceded by a keyword, e.g.
    /// `Loading: 1 packages loaded` → ("packages loaded", 1)
    fn parse_kv_count(trimmed: &str) -> Option<(&str, usize)> {
        if let Some(colon) = trimmed.find(':') {
            let after = trimmed[colon + 1..].trim();
            // Split into numeric part and label
            if let Some(space) = after.find(' ') {
                let num_part = &after[..space];
                let label = &after[space + 1..];
                if let Ok(n) = num_part.parse::<usize>() {
                    return Some((label.trim(), n));
                }
            }
        }
        None
    }
}

impl BlockHandler for BazelBuildHandler {
    fn should_skip(&mut self, line: &str) -> bool {
        let normalized = diag::normalize(line);
        let trimmed = normalized.trim();

        if trimmed.is_empty() {
            return true;
        }

        // ── Loading / Analyzing ──
        if is_loading_analyzing(trimmed) {
            // Extract counts: "Loading: 1 packages loaded"
            // "Analyzing: 320 targets (160 packages loaded, 0 targets configured)"
            if let Some((_label, count)) = Self::parse_kv_count(trimmed) {
                if trimmed.starts_with("Loading:") {
                    self.packages_loaded = self.packages_loaded.max(count);
                } else {
                    // Analyzing: try to extract packages and targets
                    self.packages_loaded = self.packages_loaded.max(count);
                }
            }
            // Also try to parse parenthesised sub-counts for analyzing
            if let Some(open) = trimmed.find('(') {
                let paren_content = &trimmed[open + 1..];
                if let Some(close) = paren_content.find(')') {
                    let inside = &paren_content[..close];
                    for part in inside.split(',') {
                        let part = part.trim();
                        if let Some(space) = part.find(' ') {
                            let num_part = &part[..space];
                            if let Ok(n) = num_part.parse::<usize>() {
                                let label = part[space + 1..].trim();
                                if label.contains("packages loaded") {
                                    self.packages_loaded = self.packages_loaded.max(n);
                                }
                                if label.contains("targets configured") {
                                    self.targets_configured = self.targets_configured.max(n);
                                }
                            }
                        }
                    }
                }
            }
            return true;
        }

        // ── Fetching repository ──
        if is_fetching(trimmed) {
            return true;
        }

        // ── Progress snapshot ──
        if is_progress_snapshot(trimmed) {
            if let Some((n, m)) = parse_bazel_progress(trimmed) {
                self.actions_completed = n;
                self.actions_total = self.actions_total.max(m);
            }
            return true;
        }

        // ── INFO summary lines ──
        if is_bazel_info_summary(trimmed) {
            if trimmed.starts_with("INFO: Elapsed") {
                // Extract time: "INFO: Elapsed time: 42.123s, Critical Path: 12.34s"
                if let Some(colon) = trimmed.find(':') {
                    let after = trimmed[colon + 1..].trim();
                    if let Some(first_comma) = after.find(',') {
                        self.elapsed_time = Some(after[..first_comma].trim().to_string());
                    } else {
                        self.elapsed_time = Some(after.to_string());
                    }
                }
            }
            return true;
        }

        // ── Action detail separators ──
        if is_action_detail_separator(trimmed) {
            self.in_action_detail = true;
            return true;
        }

        false
    }

    fn is_block_start(&mut self, line: &str) -> bool {
        let normalized = diag::normalize(line);
        let trimmed = normalized.trim();

        if trimmed.is_empty() {
            return false;
        }

        // ── Bazel verdict ──
        if is_bazel_verdict(trimmed) {
            if trimmed.starts_with("ERROR: Build did NOT") {
                self.build_did_not_complete = true;
            }
            return true;
        }

        // ── Bazel ERROR ──
        if is_bazel_error(trimmed) {
            self.in_error_block = true;
            self.errors.push(trimmed.to_string());
            return true;
        }

        // ── Bazel WARNING ──
        if is_bazel_warning(trimmed) {
            self.in_error_block = true;
            self.warnings.push(trimmed.to_string());
            self.track_warning(trimmed);
            return true;
        }

        // ── Compiler diagnostic ──
        if diag::is_compiler_diag(trimmed) {
            self.in_error_block = true;
            if trimmed.to_lowercase().contains("warning") {
                self.track_warning(trimmed);
            }
            return self.track_dedup(trimmed);
        }

        // ── Linker error ──
        if diag::is_linker_error(trimmed) {
            self.in_error_block = true;
            return true;
        }

        // ── Failed exit line ──
        if is_failed_exit_line(trimmed) {
            self.in_error_block = true;
            return true;
        }

        false
    }

    fn is_block_continuation(&mut self, line: &str, _block: &[String]) -> bool {
        let normalized = diag::normalize(line);
        let trimmed = normalized.trim();

        if self.in_error_block {
            // Blank line ends the error block
            if trimmed.is_empty() {
                self.in_error_block = false;
                return false;
            }
            // Progress line ends the error block
            if is_progress_snapshot(trimmed) {
                self.in_error_block = false;
                return false;
            }
            // A new ERROR/WARNING starts a new block (handled by is_block_start)
            if is_bazel_error(trimmed) || is_bazel_warning(trimmed) {
                self.in_error_block = false;
                return false;
            }
            // Continuation patterns
            if diag::is_diag_continuation(trimmed) {
                return true;
            }
            // Indented lines
            if line.starts_with(' ') || line.starts_with('\t') {
                return true;
            }
            // Action detail separators within a block
            if is_action_detail_separator(trimmed) {
                return true;
            }
            // Additional Bazel detail lines (stack traces, info within errors)
            if trimmed.starts_with("Target ") || trimmed.starts_with("Use --") {
                return true;
            }
            // "failed: (Exit N)" lines within error blocks
            if is_failed_exit_line(trimmed) {
                return true;
            }
            self.in_error_block = false;
            return false;
        }

        false
    }

    fn format_summary(&self, exit_code: i32, _raw: &str) -> Option<String> {
        let total = if self.actions_total > 0 {
            self.actions_total
        } else {
            self.actions_completed
        };

        let mut lines = Vec::new();

        if !self.build_did_not_complete && self.errors.is_empty() && exit_code == 0 {
            // Success
            let time_part = if let Some(ref t) = self.elapsed_time {
                format!(" ({})", t)
            } else {
                String::new()
            };
            lines.push(format!(
                "ok bazel: {} actions, 0 failed{}",
                total, time_part
            ));
        } else if exit_code != 0 && !self.build_did_not_complete && self.errors.is_empty() {
            lines.push(format!(
                "bazel: exited with code {} (no specific errors captured)",
                exit_code
            ));
        } else {
            // Failure
            lines.push("bazel: build failed".to_string());
            // Show the last few unique errors (up to 3)
            let mut shown: Vec<&String> = Vec::new();
            for err in &self.errors {
                if shown.len() >= 3 {
                    break;
                }
                if !shown.contains(&err) {
                    shown.push(err);
                }
            }
            for err in &shown {
                lines.push(format!("  {}", err));
            }
            // Show the verdict if present
            if self.build_did_not_complete {
                lines.push("ERROR: Build did NOT complete successfully".to_string());
            }
        }

        // Warning summary
        if !self.warning_counts.is_empty() {
            let mut warnings: Vec<_> = self.warning_counts.iter().collect();
            warnings.sort_by(|a, b| b.1.cmp(a.1));
            let warn_parts: Vec<String> = warnings
                .iter()
                .map(|(flag, count)| format!("{} ×{}", flag, count))
                .collect();
            lines.push(format!("  warnings: {}", warn_parts.join(", ")));
        }

        Some(lines.join("\n") + "\n")
    }
}

// ── Public API ──

/// Run `bazel` with streaming block filter.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("bazel: running bazel {}", args.join(" "));
    }

    let mut cmd = resolved_command("bazel");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_streamed(
        cmd,
        "bazel",
        &args_str,
        Box::new(BlockStreamFilter::new(BazelBuildHandler::new())),
        runner::RunOptions::with_tee("bazel"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::StreamFilter;
    use crate::core::tracking::estimate_tokens;

    // Helper to run a block filter
    fn run_block_filter(filter: &mut dyn StreamFilter, input: &str, exit_code: i32) -> String {
        let mut output = String::new();
        for line in input.lines() {
            if let Some(s) = filter.feed_line(line) {
                output.push_str(&s);
            }
        }
        output.push_str(&filter.flush());
        if let Some(post) = filter.on_exit(exit_code, input) {
            output.push_str(&post);
        }
        output
    }

    fn filter_bazel(input: &str, exit_code: i32) -> String {
        let handler = BazelBuildHandler::new();
        let mut filter = BlockStreamFilter::new(handler);
        run_block_filter(&mut filter, input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_progress_snapshot() {
        assert!(is_progress_snapshot(
            "[2,720 / 3,125] Compiling src/foo.cc; 12s processwrapper-sandbox"
        ));
        assert!(is_progress_snapshot("[1 / 42] Linking foo"));
        assert!(is_progress_snapshot(
            "[0 / 1] BazelWorkspaceStatusAction stable-status.txt"
        ));
    }

    #[test]
    fn test_is_progress_snapshot_not() {
        assert!(!is_progress_snapshot("ERROR: /path/BUILD:42:10: foo"));
        assert!(!is_progress_snapshot("INFO: Found 125 targets"));
        assert!(!is_progress_snapshot(""));
        assert!(!is_progress_snapshot("[not numbers] something"));
    }

    #[test]
    fn test_parse_bazel_progress() {
        assert_eq!(
            parse_bazel_progress("[2,720 / 3,125] Compiling src/foo.cc"),
            Some((2720, 3125))
        );
        assert_eq!(parse_bazel_progress("[1 / 42] Linking foo"), Some((1, 42)));
        assert_eq!(parse_bazel_progress("not progress"), None);
    }

    #[test]
    fn test_is_fetching() {
        assert!(is_fetching(" Fetching repository @bazel_tools; starting"));
        assert!(is_fetching("Fetching repository @rules_cc; starting"));
        assert!(!is_fetching("ERROR: fetching failed"));
    }

    #[test]
    fn test_is_bazel_error() {
        assert!(is_bazel_error(
            "ERROR: /home/user/project/BUILD:42:10: Compiling src/foo.cc failed"
        ));
        assert!(!is_bazel_error("WARNING: /path/BUILD:42: deprecated"));
    }

    #[test]
    fn test_is_bazel_warning() {
        assert!(is_bazel_warning(
            "WARNING: /path/BUILD:42:10: target 'foo' is deprecated"
        ));
        assert!(!is_bazel_warning("ERROR: /path/BUILD: bad"));
    }

    #[test]
    fn test_is_bazel_info_summary() {
        assert!(is_bazel_info_summary("INFO: Found 125 targets..."));
        assert!(is_bazel_info_summary(
            "INFO: Elapsed time: 42.123s, Critical Path: 12.34s"
        ));
        assert!(is_bazel_info_summary(
            "INFO: Build completed successfully, 123 total actions"
        ));
    }

    #[test]
    fn test_is_bazel_verdict() {
        assert!(is_bazel_verdict(
            "ERROR: Build did NOT complete successfully"
        ));
        assert!(is_bazel_verdict(
            "INFO: Build completed successfully, 123 total actions"
        ));
        assert!(!is_bazel_verdict("INFO: Found 125 targets"));
    }

    #[test]
    fn test_is_action_detail_separator() {
        assert!(is_action_detail_separator("---"));
        assert!(is_action_detail_separator("==="));
        assert!(is_action_detail_separator("___"));
        assert!(!is_action_detail_separator("--x"));
        assert!(!is_action_detail_separator("--"));
        assert!(!is_action_detail_separator(""));
    }

    // ── Success cases ──

    #[test]
    fn test_bazel_successful_build() {
        let input = "\
Loading: 1 packages loaded
Analyzing: 320 targets (160 packages loaded, 0 targets configured)
 Fetching repository @bazel_tools; starting
 Fetching repository @rules_cc; starting
[0 / 5] BazelWorkspaceStatusAction stable-status.txt
[1 / 5] Compiling src/lib.cc; 2s processwrapper-sandbox
[2 / 5] Compiling src/main.cc; 1s processwrapper-sandbox
[3 / 5] Compiling src/util.cc; 3s processwrapper-sandbox
[4 / 5] Linking libfoo.so; 1s processwrapper-sandbox
[5 / 5] Linking myapp; 1s processwrapper-sandbox
INFO: Found 5 targets...
INFO: Elapsed time: 12.345s, Critical Path: 5.67s
INFO: Build completed successfully, 5 total actions
";
        let result = filter_bazel(input, 0);
        assert!(
            result.contains("ok bazel: 5 actions, 0 failed"),
            "got: {}",
            result
        );
        assert!(
            result.contains("12.345s"),
            "should include elapsed time, got: {}",
            result
        );
    }

    #[test]
    fn test_bazel_large_build_with_commas() {
        let mut input = String::new();
        input.push_str("Loading: 5 packages loaded\n");
        input.push_str("Analyzing: 3125 targets (1562 packages loaded, 3125 targets configured)\n");
        for i in 1..=3125 {
            input.push_str(&format!(
                "[{}/3,125] Compiling src/file_{}.cc; 1s processwrapper-sandbox\n",
                i, i
            ));
        }
        input.push_str("INFO: Elapsed time: 120.5s, Critical Path: 45.2s\n");
        input.push_str("INFO: Build completed successfully, 3125 total actions\n");

        let result = filter_bazel(&input, 0);
        assert!(
            result.contains("ok bazel: 3125 actions, 0 failed"),
            "got: {}",
            result
        );
    }

    // ── Failure cases ──

    #[test]
    fn test_bazel_single_error() {
        let input = "\
Loading: 1 packages loaded
Analyzing: 42 targets (21 packages loaded, 0 targets configured)
[1 / 3] Compiling src/good.cc; 1s processwrapper-sandbox
[2 / 3] Compiling src/bad.cc; 2s processwrapper-sandbox
ERROR: /home/user/project/BUILD:42:10: Compiling src/bad.cc failed: (Exit 1): gcc failed: error executing command
  /usr/bin/gcc -c src/bad.cc
src/bad.cc:5:13: error: 'x' was not declared in this scope
src/bad.cc:5:13: note: suggested alternative: 'y'
[3 / 3] Linking myapp; 0s processwrapper-sandbox
ERROR: Build did NOT complete successfully
";
        let result = filter_bazel(input, 1);
        assert!(result.contains("bazel: build failed"), "got: {}", result);
        assert!(
            result.contains("error: 'x' was not declared"),
            "got: {}",
            result
        );
        assert!(
            !result.contains("[1 / 3]"),
            "progress should be stripped, got: {}",
            result
        );
        assert!(
            !result.contains("Loading:"),
            "loading should be stripped, got: {}",
            result
        );
    }

    #[test]
    fn test_bazel_multiple_errors() {
        let input = "\
Loading: 2 packages loaded
[1 / 4] Compiling src/a.cc; 1s processwrapper-sandbox
ERROR: /proj/BUILD:10:1: Compiling src/a.cc failed: (Exit 1): gcc failed
src/a.cc:1:1: error: first error
ERROR: /proj/BUILD:20:1: Compiling src/b.cc failed: (Exit 1): clang failed
src/b.cc:1:1: error: second error
ERROR: /proj/BUILD:30:1: Compiling src/c.cc failed: (Exit 1): gcc failed
src/c.cc:1:1: error: third error
ERROR: Build did NOT complete successfully
";
        let result = filter_bazel(input, 1);
        assert!(result.contains("bazel: build failed"), "got: {}", result);
        assert!(result.contains("first error"), "got: {}", result);
        assert!(result.contains("second error"), "got: {}", result);
        assert!(result.contains("third error"), "got: {}", result);
    }

    #[test]
    fn test_bazel_analysis_failure() {
        let input = "\
Loading: 0 packages loaded
ERROR: /home/user/project/BUILD:1:1: error loading package 'foo': cannot load 'bar.bzl': no such file
ERROR: Build did NOT complete successfully
";
        let result = filter_bazel(input, 1);
        assert!(result.contains("bazel: build failed"), "got: {}", result);
        assert!(result.contains("cannot load"), "got: {}", result);
    }

    #[test]
    fn test_bazel_fetch_chatter() {
        let input = "\
 Fetching repository @bazel_tools; starting
 Fetching repository @bazel_tools; fetching
 Fetching repository @rules_cc; starting
 Fetching repository @rules_python; starting
 Fetching repository @rules_java; starting
 Fetching repository @rules_java; fetching
Loading: 1 packages loaded
[1 / 1] Compiling src/main.cc; 1s
INFO: Build completed successfully, 1 total actions
";
        let result = filter_bazel(input, 0);
        assert!(
            result.contains("ok bazel: 1 actions, 0 failed"),
            "got: {}",
            result
        );
        assert!(
            !result.contains("Fetching repository"),
            "fetch chatter should be stripped, got: {}",
            result
        );
    }

    #[test]
    fn test_bazel_action_detail_separators() {
        let input = "\
Loading: 1 packages loaded
[1 / 1] Compiling src/main.cc; 1s
ERROR: /proj/BUILD:1:1: failed action
--- action detail ---
=== more detail ===
___ end detail ___
src/main.cc:1:1: error: bad
ERROR: Build did NOT complete successfully
";
        let result = filter_bazel(input, 1);
        assert!(result.contains("bazel: build failed"), "got: {}", result);
        assert!(result.contains("error: bad"), "got: {}", result);
        // Separators inside error blocks should be kept as continuations
    }

    #[test]
    fn test_bazel_tty_progress() {
        // Simulate ANSI-escaped progress lines (Bazel uses control chars for TTY)
        let input = "\
\x1b[2K[1 / 3] Compiling src/a.cc; 1s processwrapper-sandbox
\x1b[2K[2 / 3] Compiling src/b.cc; 2s processwrapper-sandbox
\x1b[2K[3 / 3] Linking myapp; 0s processwrapper-sandbox
INFO: Build completed successfully, 3 total actions
";
        let result = filter_bazel(input, 0);
        assert!(
            result.contains("ok bazel: 3 actions, 0 failed"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_bazel_ansi_stripped() {
        let input = "\x1b[32m[1/1] Compiling src/main.cc\x1b[0m\n\
                      \x1b[31mERROR: /proj/BUILD:1:1: bad\x1b[0m\n\
                      \x1b[31msrc/main.cc:1:1: error: bad code\x1b[0m\n\
                      ERROR: Build did NOT complete successfully\n";
        let result = filter_bazel(input, 1);
        assert!(result.contains("error: bad code"), "got: {}", result);
        // ANSI may pass through on block-start lines (matches ninja/make handler behaviour)
    }

    #[test]
    fn test_bazel_empty_input() {
        let result = filter_bazel("", 0);
        assert!(
            result.contains("ok bazel"),
            "should have a summary, got: '{}'",
            result
        );
    }

    #[test]
    fn test_bazel_token_savings_above_85pct() {
        let mut input = String::new();
        input.push_str("Loading: 50 packages loaded\n");
        input.push_str("Analyzing: 3125 targets (1562 packages loaded, 3125 targets configured)\n");
        // 3125 progress lines
        for i in 1..=2000 {
            input.push_str(&format!(
                "[{} / 3,125] Compiling src/file_{}.cc; 2s processwrapper-sandbox\n",
                i, i
            ));
        }
        // A few errors
        input.push_str("ERROR: /proj/BUILD:42:10: Compiling src/bad.cc failed: (Exit 1)\n");
        input.push_str("src/bad.cc:5:13: error: 'x' was not declared in this scope\n");
        input.push_str("src/bad.cc:5:13: note: suggested alternative: 'y'\n");
        input.push_str("ERROR: Build did NOT complete successfully\n");

        let result = filter_bazel(&input, 1);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 85,
            "token savings: {}% (expected >=85%)",
            savings
        );
    }
}
