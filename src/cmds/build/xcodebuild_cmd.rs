//! Filters xcodebuild output — diagnostics, test failures, and summaries.

use crate::core::runner::{self, RunOptions};
use crate::core::stream::{BlockHandler, BlockStreamFilter};
use crate::core::utils::resolved_command;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("xcodebuild");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: xcodebuild {}", args.join(" "));
    }

    runner::run_streamed(
        cmd,
        "xcodebuild",
        &args.join(" "),
        Box::new(BlockStreamFilter::new(XcodebuildHandler::new())),
        RunOptions::with_tee("xcodebuild"),
    )
}

struct XcodebuildHandler {
    errors: usize,
    warnings: usize,
    tests: usize,
    summary: Option<String>,
}

impl XcodebuildHandler {
    fn new() -> Self {
        Self {
            errors: 0,
            warnings: 0,
            tests: 0,
            summary: None,
        }
    }
}

impl BlockHandler for XcodebuildHandler {
    fn should_skip(&mut self, line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with("CompileC ")
            || trimmed.starts_with("CompileSwift ")
            || trimmed.starts_with("SwiftCompile ")
            || trimmed.starts_with("Ld ")
            || trimmed.starts_with("PhaseScriptExecution ")
            || trimmed.starts_with("CodeSign ")
            || trimmed.starts_with("ProcessInfoPlistFile ")
            || trimmed.starts_with("CopySwiftLibs ")
            || trimmed.starts_with("Touch ")
            || trimmed.starts_with("note: Building targets")
            || trimmed.starts_with("Prepare packages")
            || trimmed.starts_with("CreateBuildDirectory ")
            || trimmed.starts_with("WriteAuxiliaryFile ")
        {
            return true;
        }

        if trimmed.starts_with("** BUILD ") || trimmed.starts_with("** TEST ") {
            self.summary = Some(trimmed.to_string());
            return true;
        }

        false
    }

    fn is_block_start(&mut self, line: &str) -> bool {
        let trimmed = line.trim_start();
        if trimmed.contains(": error:")
            || trimmed.starts_with("error:")
            || trimmed.starts_with("fatal error:")
            || trimmed.contains(" failed:")
            || trimmed.contains("Command PhaseScriptExecution failed")
            || trimmed.contains("Testing failed:")
            || trimmed.contains("Test Case '") && trimmed.contains(" failed")
        {
            self.errors += 1;
            return true;
        }

        if trimmed.contains(": warning:") || trimmed.starts_with("warning:") {
            self.warnings += 1;
            return true;
        }

        if trimmed.contains("Test Suite '")
            || trimmed.contains("Test Case '") && trimmed.contains(" passed")
        {
            self.tests += 1;
            return true;
        }

        false
    }

    fn is_block_continuation(&mut self, line: &str, block: &[String]) -> bool {
        if block.len() >= 30 {
            return false;
        }

        let trimmed = line.trim_start();
        !(trimmed.starts_with("CompileC ")
            || trimmed.starts_with("SwiftCompile ")
            || trimmed.starts_with("** BUILD ")
            || trimmed.starts_with("** TEST "))
    }

    fn format_summary(&self, exit_code: i32, _raw: &str) -> Option<String> {
        let status = if exit_code == 0 { "ok" } else { "failed" };
        let mut parts = vec![format!("xcodebuild: {}", status)];
        if self.errors > 0 {
            parts.push(format!("{} error blocks", self.errors));
        }
        if self.warnings > 0 {
            parts.push(format!("{} warning blocks", self.warnings));
        }
        if self.tests > 0 {
            parts.push(format!("{} test lines", self.tests));
        }
        if let Some(summary) = &self.summary {
            parts.push(summary.clone());
        }
        Some(format!("{}\n", parts.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::StreamFilter;

    #[test]
    fn filters_xcodebuild_success_noise() {
        let mut filter = BlockStreamFilter::new(XcodebuildHandler::new());
        assert_eq!(filter.feed_line("CompileSwift normal arm64 Foo.swift"), None);
        assert_eq!(filter.feed_line("** BUILD SUCCEEDED **"), None);
        assert_eq!(filter.flush(), "");
        assert_eq!(
            filter.on_exit(0, "").unwrap(),
            "xcodebuild: ok; ** BUILD SUCCEEDED **\n"
        );
    }

    #[test]
    fn keeps_xcodebuild_error_block() {
        let mut filter = BlockStreamFilter::new(XcodebuildHandler::new());
        assert_eq!(filter.feed_line("Foo.swift:10:5: error: cannot find 'x' in scope"), None);
        assert_eq!(filter.feed_line("** BUILD FAILED **"), None);
        let out = filter.flush();
        assert!(out.contains("Foo.swift:10:5: error"));
        assert!(filter.on_exit(65, "").unwrap().contains("xcodebuild: failed"));
    }
}
