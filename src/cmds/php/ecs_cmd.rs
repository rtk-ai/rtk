//! EasyCodingStandard output filter.

use super::utils::{php_tool_command, strip_ansi_and_controls};
use crate::core::runner;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = php_tool_command("ecs");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: ecs {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "ecs",
        &args.join(" "),
        filter_ecs_output,
        runner::RunOptions::default(),
    )
}

pub(crate) fn filter_ecs_output(output: &str) -> String {
    let cleaned = strip_ansi_and_controls(output);
    if cleaned.contains("No errors found") {
        return "✓ ecs: no issues".to_string();
    }

    let mut lines = Vec::new();
    for line in cleaned.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        if trimmed.contains(".php")
            || trimmed.contains("ERROR")
            || trimmed.contains("FAIL")
            || trimmed.contains("Fixed")
            || trimmed.contains("checked")
            || trimmed.contains("files")
        {
            lines.push(trimmed.to_string());
        }
    }

    if lines.is_empty() {
        let fallback = cleaned.trim();
        if fallback.is_empty() {
            "ok".to_string()
        } else {
            fallback.to_string()
        }
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_ecs_success_output() {
        assert_eq!(
            filter_ecs_output("[OK] No errors found. Great job!"),
            "✓ ecs: no issues"
        );
    }

    #[test]
    fn test_ecs_keeps_file_errors() {
        let output = "src/Foo.php\n 10 | ERROR | Something bad\n";
        let filtered = filter_ecs_output(output);
        assert!(filtered.contains("src/Foo.php"));
        assert!(filtered.contains("ERROR"));
    }
}
