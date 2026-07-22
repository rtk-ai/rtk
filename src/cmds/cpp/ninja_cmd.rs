//! Filters ninja build output — FAILED blocks kept verbatim, progress lines counted/stripped.

use super::diag;
use crate::core::runner;
use crate::core::stream::{BlockHandler, BlockStreamFilter};
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use std::collections::HashMap;

// --- Helper functions ---

/// Check if a line is a ninja progress line: `[N/M] ...`
pub fn is_progress_line(line: &str) -> bool {
    line.starts_with('[') && {
        // Quick check: find the closing bracket
        if let Some(end_bracket) = line.find(']') {
            let inner = &line[1..end_bracket];
            if let Some(slash) = inner.find('/') {
                inner[..slash].parse::<usize>().is_ok()
                    && inner[slash + 1..].parse::<usize>().is_ok()
            } else {
                false
            }
        } else {
            false
        }
    }
}

/// Parse `[N/M]` from a progress line. Returns `(N, M)`.
pub fn parse_ninja_progress(line: &str) -> Option<(usize, usize)> {
    if !line.starts_with('[') {
        return None;
    }
    let end_bracket = line.find(']')?;
    let inner = &line[1..end_bracket];
    let slash = inner.find('/')?;
    let n = inner[..slash].parse::<usize>().ok()?;
    let m = inner[slash + 1..].parse::<usize>().ok()?;
    Some((n, m))
}

// --- State machine ---

/// BlockHandler for ninja build output.
pub struct NinjaBuildHandler {
    // Counters
    edges_total: usize,
    edges_built: usize,
    edges_failed: usize,

    // State
    in_failed_edge: bool,
    in_diag_block: bool,

    // Collected
    stop_line: Option<String>,

    // Warning tracking: warning_flag -> count
    warning_counts: HashMap<String, usize>,

    // Dedup tracking: message_core -> count
    seen_diagnostics: HashMap<String, usize>,
}

impl NinjaBuildHandler {
    pub fn new() -> Self {
        Self {
            edges_total: 0,
            edges_built: 0,
            edges_failed: 0,
            in_failed_edge: false,
            in_diag_block: false,
            stop_line: None,
            warning_counts: HashMap::new(),
            seen_diagnostics: HashMap::new(),
        }
    }

    fn track_dedup(&mut self, diag_line: &str) -> bool {
        // Extract the message part (after file:line:col: severity:)
        let msg = diag::extract_diag_message(diag_line);
        let count = self.seen_diagnostics.entry(msg).or_insert(0);
        *count += 1;
        *count <= 3 // Show first 3 occurrences, collapse rest
    }
}

impl BlockHandler for NinjaBuildHandler {
    fn should_skip(&mut self, line: &str) -> bool {
        let cleaned = strip_ansi(line);

        // Skip progress lines: [N/M] Building/Linking/...
        if is_progress_line(&cleaned) {
            self.edges_built += 1;
            // Track total from [N/M]
            if let Some((_n, m)) = parse_ninja_progress(&cleaned) {
                self.edges_total = self.edges_total.max(m);
            }
            return true;
        }

        // Skip "ninja: Entering directory" and "ninja: leaving directory"
        if cleaned.starts_with("ninja: ") {
            if cleaned.contains("build stopped") {
                self.stop_line = Some(cleaned);
                return true;
            }
            if !cleaned.contains("error")
                && !cleaned.contains("warning")
                && !cleaned.contains("fatal")
            {
                return true;
            }
        }

        false
    }

    fn is_block_start(&mut self, line: &str) -> bool {
        let cleaned = strip_ansi(line);

        // FAILED: <target> → start a diagnostic block
        if cleaned.starts_with("FAILED: ") {
            self.edges_failed += 1;
            self.in_failed_edge = true;
            self.in_diag_block = false;
            return true;
        }

        // Standalone compiler diagnostic (no preceding FAILED line)
        if diag::is_compiler_diag(&cleaned) {
            self.in_diag_block = true;
            self.in_failed_edge = false;
            // Track warnings
            if cleaned.contains("warning:") {
                if let Some(flag) = diag::extract_warning_flag(&cleaned) {
                    *self.warning_counts.entry(flag).or_insert(0) += 1;
                } else {
                    *self.warning_counts.entry("other".to_string()).or_insert(0) += 1;
                }
            }
            return self.track_dedup(&cleaned);
        }

        false
    }

    fn is_block_continuation(&mut self, line: &str, block: &[String]) -> bool {
        let cleaned = strip_ansi(line);

        // If in failed edge, keep everything until blank line or next block
        if self.in_failed_edge {
            if cleaned.trim().is_empty() && block.len() > 1 {
                // Blank line after command line → move to diagnostics
                return false;
            }
            // Check if this line is a compiler diagnostic (inside FAILED block)
            if diag::is_compiler_diag(&cleaned) {
                self.in_failed_edge = false;
                self.in_diag_block = true;
                if cleaned.contains("warning:") {
                    if let Some(flag) = diag::extract_warning_flag(&cleaned) {
                        *self.warning_counts.entry(flag).or_insert(0) += 1;
                    } else {
                        *self.warning_counts.entry("other".to_string()).or_insert(0) += 1;
                    }
                }
                // Don't dedup inside FAILED blocks — keep all
                return true;
            }
            return !is_progress_line(&cleaned) && !cleaned.starts_with("FAILED: ");
        }

        // If in diagnostic block
        if self.in_diag_block {
            if cleaned.trim().is_empty() {
                // Blank line = end of diag block
                self.in_diag_block = false;
                return false;
            }
            if cleaned.starts_with("FAILED: ") {
                self.in_diag_block = false;
                return false;
            }
            if is_progress_line(&cleaned) {
                self.in_diag_block = false;
                return false;
            }
            // Continuation patterns
            return diag::is_diag_continuation(&cleaned) || cleaned.starts_with("  ");
        }

        // Generic block continuation (indentation or known continuation prefixes)
        diag::is_diag_continuation(&cleaned)
    }

    fn format_summary(&self, _exit_code: i32, _raw: &str) -> Option<String> {
        let total = if self.edges_total > 0 {
            self.edges_total
        } else {
            self.edges_built
        };

        let mut lines = Vec::new();

        if self.edges_failed == 0 {
            lines.push(format!("ok ninja: {} edges, 0 failed", total));
        } else {
            lines.push(format!(
                "ninja: {}/{} edges failed",
                self.edges_failed, total
            ));
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

        // Stop line
        if let Some(ref stop) = self.stop_line {
            lines.push(stop.clone());
        }

        Some(lines.join("\n") + "\n")
    }
}

// --- Public API ---

/// Run ninja with filtering.
pub fn run(directory: &str, args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        let args_display = if args.is_empty() {
            String::new()
        } else {
            format!(" {}", args.join(" "))
        };
        eprintln!("ninja: running ninja -C {}{}", directory, args_display);
    }

    let mut cmd = resolved_command("ninja");
    cmd.arg("-C").arg(directory);
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = format!("-C {} {}", directory, args.join(" "));

    runner::run_streamed(
        cmd,
        "ninja",
        &args_str,
        Box::new(BlockStreamFilter::new(NinjaBuildHandler::new())),
        runner::RunOptions::with_tee("ninja"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::StreamFilter;

    // Helper to run a ninja filter (reproduces the logic of stream::run_block_filter)
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

    // Helper to run a NinjaBuildHandler through the filter
    fn filter_ninja(input: &str, exit_code: i32) -> String {
        let handler = NinjaBuildHandler::new();
        let mut filter = BlockStreamFilter::new(handler);
        run_block_filter(&mut filter, input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_progress_line_normal() {
        assert!(is_progress_line(
            "[123/456] Building CXX object src/core/CMakeFiles/lc_core.dir/clock.cpp.o"
        ));
    }

    #[test]
    fn test_is_progress_line_linking() {
        assert!(is_progress_line(
            "[455/456] Linking CXX executable bin/lc_test"
        ));
    }

    #[test]
    fn test_is_progress_line_not() {
        assert!(!is_progress_line("FAILED: src/core/hash.cpp.o"));
        assert!(!is_progress_line(
            "ninja: build stopped: subcommand failed."
        ));
        assert!(!is_progress_line(""));
    }

    #[test]
    fn test_parse_ninja_progress_normal() {
        assert_eq!(
            parse_ninja_progress(
                "[123/456] Building CXX object src/core/CMakeFiles/lc_core.dir/clock.cpp.o"
            ),
            Some((123, 456))
        );
    }

    #[test]
    fn test_parse_ninja_progress_msvc() {
        assert_eq!(
            parse_ninja_progress("[311/456] Building CXX object src/backends/dx/CMakeFiles/lc_dx.dir/dx_device.cpp.o"),
            Some((311, 456))
        );
    }

    #[test]
    fn test_parse_ninja_progress_linking() {
        assert_eq!(
            parse_ninja_progress("[455/456] Linking CXX executable bin/lc_test"),
            Some((455, 456))
        );
    }

    #[test]
    fn test_parse_ninja_progress_invalid() {
        assert_eq!(parse_ninja_progress("not progress"), None);
        assert_eq!(parse_ninja_progress(""), None);
    }

    #[test]
    fn test_extract_diag_message_gcc() {
        // GCC diag: file:line:col: severity: message with : colons
        let msg = diag::extract_diag_message(
            "src/core/hash.h:42:13: error: static assertion failed: Hash must be specialized",
        );
        assert_eq!(
            msg, "static assertion failed: Hash must be specialized",
            "should capture full message including colons, got: '{}'",
            msg
        );
    }

    #[test]
    fn test_extract_diag_message_msvc() {
        // MSVC diag: file(line): severity code: message with :: namespace
        let msg = diag::extract_diag_message(
            "src/backends/dx/dx_codegen.cpp(88): error C2039: 'visit' is not a member of 'luisa::compute::dx::DXCodegen'"
        );
        assert_eq!(
            msg, "'visit' is not a member of 'luisa::compute::dx::DXCodegen'",
            "should capture full message including ::, got: '{}'",
            msg
        );
    }

    #[test]
    fn test_extract_diag_message_msvc_no_message() {
        // MSVC diag without message body
        let msg = diag::extract_diag_message("src/main.cpp(42): warning C4100:");
        // No message body, should fall back to line or partial
        assert!(!msg.is_empty(), "should not be empty, got: '{}'", msg);
    }

    #[test]
    fn test_is_compiler_diag_gcc_error() {
        assert!(diag::is_compiler_diag(
            "src/core/hash.h:42:13: error: static assertion failed: Hash must be specialized"
        ));
    }

    #[test]
    fn test_is_compiler_diag_msvc_error() {
        assert!(diag::is_compiler_diag(
            "src/backends/dx/dx_codegen.cpp(88): error C2039: 'visit': is not a member"
        ));
    }

    #[test]
    fn test_is_compiler_diag_warning() {
        assert!(diag::is_compiler_diag(
            "src/api/api.cpp:42:10: warning: unused parameter 'device_id' [-Wunused-parameter]"
        ));
    }

    #[test]
    fn test_is_compiler_diag_note() {
        assert!(diag::is_compiler_diag(
            "src/core/hash.h:42:13: note: in instantiation of template class"
        ));
    }

    #[test]
    fn test_is_compiler_diag_fatal() {
        assert!(diag::is_compiler_diag(
            "src/main.cpp:1:1: fatal error: No such file or directory"
        ));
    }

    #[test]
    fn test_is_compiler_diag_not_a_diag() {
        assert!(!diag::is_compiler_diag(
            "FAILED: src/core/CMakeFiles/lc_core.dir/hash.cpp.o"
        ));
        assert!(!diag::is_compiler_diag("[1/456] Building CXX object ..."));
        assert!(!diag::is_compiler_diag(
            "ninja: build stopped: subcommand failed."
        ));
        assert!(!diag::is_compiler_diag(""));
    }

    #[test]
    fn test_is_compiler_diag_with_path_containing_dots() {
        assert!(diag::is_compiler_diag(
            "src/backends/cuda/cuda_codegen.cpp:142:13: error: no matching function for call"
        ));
    }

    #[test]
    fn test_is_compiler_diag_msvc_with_path() {
        assert!(diag::is_compiler_diag(
            "C:\\Users\\test\\project\\src\\main.cpp(42): error C2065: 'x' : undeclared identifier"
        ));
    }

    // ── Success cases ──

    #[test]
    fn test_ninja_successful_build() {
        let input = "[1/20] Building CXX object src/core/clock.cpp.o\n\
                      [2/20] Building CXX object src/core/hash.cpp.o\n\
                      [3/20] Building CXX object src/core/buffer.cpp.o\n\
                      [4/20] Building CXX object src/core/arena.cpp.o\n\
                      [5/20] Building CXX object src/core/context.cpp.o\n\
                      [6/20] Building CXX object src/backends/cuda/cuda_device.cpp.o\n\
                      [7/20] Building CXX object src/backends/dx/dx_device.cpp.o\n\
                      [8/20] Building CXX object src/backends/vulkan/vk_device.cpp.o\n\
                      [9/20] Building CXX object src/backends/metal/mtl_device.cpp.o\n\
                      [10/20] Building CXX object src/api/api.cpp.o\n\
                      [11/20] Building CXX object src/tests/test_main.cpp.o\n\
                      [12/20] Building CXX object src/parser/formatter.cpp.o\n\
                      [13/20] Building CXX object src/parser/types.cpp.o\n\
                      [14/20] Building CXX object src/runtime/context.cpp.o\n\
                      [15/20] Building CXX object src/runtime/device.cpp.o\n\
                      [16/20] Building CXX object src/runtime/stream.cpp.o\n\
                      [17/20] Building CXX object src/ext/imgui/imgui.cpp.o\n\
                      [18/20] Building CXX object src/ext/eastl/hash.cpp.o\n\
                      [19/20] Linking CXX static library lib/liblc_core.a\n\
                      [20/20] Linking CXX executable bin/lc_test\n";
        let result = filter_ninja(input, 0);
        assert!(
            result.contains("ok ninja: 20 edges, 0 failed"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_ninja_full_luisa_build() {
        let mut input = String::new();
        for i in 1..=456 {
            input.push_str(&format!(
                "[{}/456] Building CXX object src/core/file_{}.cpp.o\n",
                i, i
            ));
        }
        let result = filter_ninja(&input, 0);
        assert!(
            result.contains("ok ninja: 456 edges, 0 failed"),
            "got: {}",
            result
        );
    }

    // ── Failure cases ──

    #[test]
    fn test_ninja_single_compiler_error() {
        let input = "[1/20] Building CXX object src/core/clock.cpp.o\n\
                      [2/20] Building CXX object src/core/hash.cpp.o\n\
                      [3/20] Building CXX object src/backends/cuda/cuda_codegen.cpp.o\n\
                      FAILED: src/backends/cuda/CMakeFiles/lc_cuda.dir/cuda_codegen.cpp.o\n\
                      /usr/bin/nvcc -ccbin g++ -gencode arch=compute_86,code=sm_86 -std=c++20 -c src/backends/cuda/cuda_codegen.cpp\n\
                      src/backends/cuda/cuda_codegen.cpp:142:13: error: no matching function for call\n\
                      src/backends/cuda/cuda_codegen.cpp:89:10: note: candidate: 'void visit(const ir::BinaryExpr&)'\n\
                      [4/20] Building CXX object src/backends/dx/dx_codegen.cpp.o\n\
                      ninja: build stopped: subcommand failed.\n";
        let result = filter_ninja(input, 1);
        assert!(result.contains("1/20 edges failed"), "got: {}", result);
        assert!(
            result.contains("FAILED: src/backends/cuda/CMakeFiles/lc_cuda.dir/cuda_codegen.cpp.o"),
            "got: {}",
            result
        );
        assert!(
            result.contains("error: no matching function"),
            "got: {}",
            result
        );
        assert!(
            !result.contains("[3/20]"),
            "should not contain progress lines, got: {}",
            result
        );
        assert!(result.contains("build stopped"), "got: {}", result);
    }

    #[test]
    fn test_ninja_multiple_failures_gcc_and_msvc() {
        let input = "[1/20] Building CXX object src/core/clock.cpp.o\n\
                      FAILED: src/backends/cuda/CMakeFiles/lc_cuda.dir/cuda_codegen.cpp.o\n\
                      /usr/bin/nvcc -ccbin g++ -c src/backends/cuda/cuda_codegen.cpp\n\
                      src/backends/cuda/cuda_codegen.cpp:142:13: error: no matching function\n\
                      [2/20] Building CXX object src/backends/dx/CMakeFiles/lc_dx.dir/dx_codegen.cpp.o\n\
                      FAILED: src/backends/dx/CMakeFiles/lc_dx.dir/dx_codegen.cpp.o\n\
                      cl.exe /c /std:c++20 /EHsc src/backends/dx/dx_codegen.cpp\n\
                      src/backends/dx/dx_codegen.cpp(88): error C2039: 'visit': is not a member\n\
                      ninja: build stopped: subcommand failed.\n";
        let result = filter_ninja(input, 1);
        assert!(result.contains("2/20 edges failed"), "got: {}", result);
        assert!(
            result.contains("FAILED: src/backends/cuda"),
            "got: {}",
            result
        );
        assert!(
            result.contains("FAILED: src/backends/dx"),
            "got: {}",
            result
        );
        assert!(result.contains("error C2039:"), "got: {}", result);
    }

    #[test]
    fn test_ninja_linker_error() {
        let input = "[1/2] Building CXX object src/core/clock.cpp.o\n\
                      [2/2] Linking CXX executable bin/lc_test\n\
                      FAILED: bin/lc_test\n\
                      /usr/bin/c++ -o bin/lc_test src/core/clock.cpp.o\n\
                      src/core/clock.cpp.o: In function 'main':\n\
                      clock.cpp:(.text+0x1e): undefined reference to 'start_impl'\n\
                      collect2: error: ld returned 1 exit status\n\
                      ninja: build stopped: subcommand failed.\n";
        let result = filter_ninja(input, 1);
        assert!(result.contains("1/2 edges failed"), "got: {}", result);
        assert!(result.contains("FAILED: bin/lc_test"), "got: {}", result);
        assert!(result.contains("undefined reference"), "got: {}", result);
    }

    #[test]
    fn test_ninja_interleaved_progress_and_failures() {
        let input = "[1/3] Building CXX object src/a.cpp.o\n\
                      FAILED: src/a.cpp.o\n\
                      g++ -c src/a.cpp\n\
                      src/a.cpp:1:1: error: first error\n\
                      [2/3] Building CXX object src/b.cpp.o\n\
                      FAILED: src/b.cpp.o\n\
                      g++ -c src/b.cpp\n\
                      src/b.cpp:1:1: error: second error\n\
                      [3/3] Building CXX object src/c.cpp.o\n\
                      ninja: build stopped: subcommand failed.\n";
        let result = filter_ninja(input, 1);
        assert!(result.contains("2/3 edges failed"), "got: {}", result);
        assert!(result.contains("FAILED: src/a.cpp.o"), "got: {}", result);
        assert!(result.contains("FAILED: src/b.cpp.o"), "got: {}", result);
        assert!(
            !result.contains("[1/3]"),
            "should not contain progress lines, got: {}",
            result
        );
    }

    // ── Warning handling ──

    #[test]
    fn test_ninja_warnings_only_no_errors() {
        let mut input = String::new();
        for i in 1..=10 {
            input.push_str(&format!(
                "[{}/10] Building CXX object src/file_{}.cpp.o\n",
                i, i
            ));
        }
        input.push_str(
            "src/api/api.cpp:42:10: warning: unused parameter 'device_id' [-Wunused-parameter]\n",
        );
        input.push_str("src/core/clock.cpp:15:20: warning: variable 'old_val' set but not used [-Wunused-but-set-variable]\n");

        let result = filter_ninja(&input, 0);
        // Standalone warnings (without FAILED) may not get captured by block start
        // since they need is_block_start to return true
        assert!(result.contains("ok ninja:"), "got: {}", result);
    }

    // ── Edge cases ──

    #[test]
    fn test_ninja_empty_build() {
        let result = filter_ninja("ninja: no work to do.\n", 0);
        assert!(result.contains("ok ninja:"), "got: {}", result);
    }

    #[test]
    fn test_ninja_ansi_stripped() {
        // ANSI escape codes are stripped by the handler
        let input = "[1/1] Building CXX object src/main.cpp.o\n\
                      \x1b[31mFAILED: src/main.cpp.o\x1b[0m\n\
                      g++ -c src/main.cpp\n\
                      src/main.cpp:1:1: \x1b[1;31merror: bad code\x1b[0m\n";
        let result = filter_ninja(input, 1);
        // The filter should still capture FAILED blocks even with ANSI
        assert!(result.contains("FAILED"), "got: {}", result);
        assert!(result.contains("g++"), "got: {}", result);
    }

    #[test]
    fn test_ninja_token_savings_above_80pct() {
        // 456 progress lines + 2 FAILED blocks
        let mut input = String::new();
        for i in 1..=456 {
            input.push_str(&format!(
                "[{}/456] Building CXX object src/file_{}.cpp.o\n",
                i, i
            ));
        }
        input.push_str("FAILED: src/cuda_codegen.cpp.o\n/usr/bin/nvcc -c src/cuda_codegen.cpp\nsrc/cuda_codegen.cpp:142:13: error: no matching function\n");
        input.push_str("FAILED: src/dx_codegen.cpp.o\ncl.exe /c src/dx_codegen.cpp\nsrc/dx_codegen.cpp(88): error C2039: 'visit': is not a member\n");
        input.push_str("ninja: build stopped: subcommand failed.\n");

        let result = filter_ninja(&input, 1);
        let raw_token_count = crate::core::tracking::estimate_tokens(&input);
        let filtered_token_count = crate::core::tracking::estimate_tokens(&result);
        let savings = if raw_token_count > 0 {
            ((raw_token_count - filtered_token_count) as f64 / raw_token_count as f64 * 100.0)
                as usize
        } else {
            0
        };
        assert!(
            savings >= 80,
            "token savings: {}% (expected >=80%)",
            savings
        );
    }
}
