use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let display = args.join(" ");

    if verbose > 0 {
        eprintln!("Running: prek {}", display);
    }

    if args.first().map(|a| a == "run").unwrap_or(false) {
        let mut cmd = resolved_command("prek");
        for arg in args {
            cmd.arg(arg);
        }
        runner::run_filtered(
            cmd,
            "prek",
            &display,
            super::pre_commit_cmd::filter_pre_commit_output,
            runner::RunOptions::stdout_only().tee("prek"),
        )
    } else {
        let os_args: Vec<std::ffi::OsString> =
            args.iter().map(std::ffi::OsString::from).collect();
        runner::run_passthrough("prek", &os_args, verbose)
    }
}

#[cfg(test)]
mod tests {
    fn filter(output: &str) -> String {
        super::super::pre_commit_cmd::filter_pre_commit_output(output)
    }

    #[test]
    fn test_prek_filter_all_pass() {
        let output = "\
Trim trailing whitespace.................................................Passed
fix end of files.........................................................Passed";
        assert_eq!(
            filter(output),
            "Trim trailing whitespace [Passed]\nfix end of files [Passed]"
        );
    }

    #[test]
    fn test_prek_filter_failed_with_hook_id() {
        let output = "\
Check Yaml...............................................................Failed
- hook id: check-yaml
- exit code: 1
.yaml-lint:13:1: expected a mapping";
        assert_eq!(
            filter(output),
            "check-yaml [Failed]\n.yaml-lint:13:1: expected a mapping"
        );
    }

    #[test]
    fn test_prek_filter_skipped_with_reason() {
        let output = "\
ruff (lint + fix)....................................(no files to check)Skipped
ruff (format)........................................(no files to check)Skipped";
        assert_eq!(
            filter(output),
            "ruff (lint + fix) [Skipped]\nruff (format) [Skipped]"
        );
    }
}
