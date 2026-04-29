//! Filters Gradle wrapper output — build errors, test failures, and summaries.

use crate::core::runner::{self, RunOptions};
use crate::core::stream::{BlockHandler, BlockStreamFilter};
use crate::core::utils::resolved_command;
use anyhow::Result;

pub fn run(binary: &str, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command(binary);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {}", binary, args.join(" "));
    }

    runner::run_streamed(
        cmd,
        binary,
        &args.join(" "),
        Box::new(BlockStreamFilter::new(GradleHandler::new(binary))),
        RunOptions::with_tee("gradle"),
    )
}

struct GradleHandler {
    binary: String,
    failures: usize,
    warnings: usize,
    tasks: Option<String>,
    build_line: Option<String>,
}

impl GradleHandler {
    fn new(binary: &str) -> Self {
        Self {
            binary: binary.to_string(),
            failures: 0,
            warnings: 0,
            tasks: None,
            build_line: None,
        }
    }
}

impl BlockHandler for GradleHandler {
    fn should_skip(&mut self, line: &str) -> bool {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || (trimmed.starts_with("> Task ") && !trimmed.contains("FAILED"))
            || trimmed.starts_with("> Configure project")
            || trimmed.starts_with("> IDLE")
            || trimmed.starts_with("> Loading")
            || trimmed.starts_with("Starting a Gradle Daemon")
            || trimmed.starts_with("Calculating task graph")
            || trimmed.starts_with("Configuration cache")
        {
            return true;
        }

        if trimmed.starts_with("BUILD SUCCESSFUL") || trimmed.starts_with("BUILD FAILED") {
            self.build_line = Some(trimmed.to_string());
            return true;
        }

        if trimmed.contains(" actionable task") || trimmed.contains(" actionable tasks") {
            self.tasks = Some(trimmed.to_string());
            return true;
        }

        false
    }

    fn is_block_start(&mut self, line: &str) -> bool {
        let trimmed = line.trim_start();
        if trimmed.starts_with("FAILURE:")
            || trimmed.starts_with("* What went wrong:")
            || trimmed.starts_with("* Exception is:")
            || trimmed.starts_with("Execution failed for task")
            || trimmed.starts_with("FAILED")
            || trimmed.contains(" FAILED")
            || trimmed.contains(" failed")
            || trimmed.contains("error:")
            || trimmed.contains("Compilation failed")
        {
            self.failures += 1;
            return true;
        }

        if trimmed.starts_with("warning:")
            || trimmed.contains(" warning:")
            || trimmed.contains("Deprecated Gradle features")
        {
            self.warnings += 1;
            return true;
        }

        false
    }

    fn is_block_continuation(&mut self, line: &str, block: &[String]) -> bool {
        if block.len() >= 40 {
            return false;
        }

        let trimmed = line.trim_start();
        !(trimmed.starts_with("> Task ")
            || trimmed.starts_with("BUILD SUCCESSFUL")
            || trimmed.starts_with("BUILD FAILED"))
    }

    fn format_summary(&self, exit_code: i32, _raw: &str) -> Option<String> {
        let status = if exit_code == 0 { "ok" } else { "failed" };
        let mut parts = vec![format!("{}: {}", self.binary, status)];
        if self.failures > 0 {
            parts.push(format!("{} failure blocks", self.failures));
        }
        if self.warnings > 0 {
            parts.push(format!("{} warning blocks", self.warnings));
        }
        if let Some(line) = &self.build_line {
            parts.push(line.clone());
        }
        if let Some(tasks) = &self.tasks {
            parts.push(tasks.clone());
        }
        Some(format!("{}\n", parts.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::StreamFilter;

    #[test]
    fn filters_gradle_success_noise() {
        let mut filter = BlockStreamFilter::new(GradleHandler::new("./gradlew"));
        assert_eq!(filter.feed_line("> Task :compileJava"), None);
        assert_eq!(filter.feed_line("BUILD SUCCESSFUL in 4s"), None);
        assert_eq!(filter.feed_line("3 actionable tasks: 3 executed"), None);
        assert_eq!(filter.flush(), "");
        assert_eq!(
            filter.on_exit(0, "").unwrap(),
            "./gradlew: ok; BUILD SUCCESSFUL in 4s; 3 actionable tasks: 3 executed\n"
        );
    }

    #[test]
    fn keeps_gradle_failure_block() {
        let mut filter = BlockStreamFilter::new(GradleHandler::new("gradle"));
        assert_eq!(filter.feed_line("> Task :test FAILED"), None);
        assert_eq!(filter.feed_line(""), None);
        assert_eq!(filter.feed_line("BUILD FAILED in 2s"), None);
        let out = filter.flush();
        assert!(out.contains("> Task :test FAILED"));
        assert!(filter.on_exit(1, "").unwrap().contains("gradle: failed"));
    }
}
