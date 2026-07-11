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
        ecs_run_options(),
    )
}

fn ecs_run_options() -> runner::RunOptions<'static> {
    runner::RunOptions::stdout_only()
        .tee("ecs")
        .early_exit_on_failure()
}

pub(crate) fn filter_ecs_output(output: &str) -> String {
    let cleaned = strip_ansi_and_controls(output);
    if cleaned.contains("No errors found") {
        return "鉁?ecs: no issues".to_string();
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
    fn ecs_failure_options_preserve_raw_diagnostics() {
        let options = ecs_run_options();

        assert!(options.filter_stdout_only);
        assert!(options.skip_filter_on_failure);
        assert_eq!(options.tee_label, Some("ecs"));
    }

    #[test]
    fn test_ecs_success_output() {
        assert_eq!(
            filter_ecs_output("[OK] No errors found. Great job!"),
            "鉁?ecs: no issues"
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
