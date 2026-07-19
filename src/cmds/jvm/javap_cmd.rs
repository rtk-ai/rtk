//! Bounded output for `javap` class inspection.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_INVENTORY;
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::ffi::OsString;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if requests_metadata_only(args) {
        let args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough("javap", &args, verbose);
    }

    if verbose > 0 {
        eprintln!("Running: javap {}", args.join(" "));
    }

    let mut cmd = resolved_command("javap");
    cmd.args(args);
    runner::run_filtered(
        cmd,
        "javap",
        &args.join(" "),
        filter_output,
        RunOptions::with_tee("javap").early_exit_on_failure(),
    )
}

fn requests_metadata_only(args: &[String]) -> bool {
    args.iter().any(|arg| {
        matches!(
            arg.as_str(),
            "-help" | "--help" | "-version" | "--version" | "-J-version"
        )
    })
}

fn filter_output(raw: &str) -> String {
    let total = raw.lines().filter(|line| !line.trim().is_empty()).count();
    if total <= CAP_INVENTORY {
        return raw.trim_end().to_string();
    }

    let mut output = Vec::with_capacity(CAP_INVENTORY + 1);
    let mut shown = 0;
    let mut previous_blank = false;

    for line in raw.lines() {
        let line = line.trim_end();
        if line.trim().is_empty() {
            if !previous_blank && !output.is_empty() {
                output.push(String::new());
            }
            previous_blank = true;
            continue;
        }
        if shown == CAP_INVENTORY {
            break;
        }
        output.push(line.to_string());
        shown += 1;
        previous_blank = false;
    }

    while output.last().is_some_and(String::is_empty) {
        output.pop();
    }
    output.push(format!(
        "... +{} more lines (showing {} of {})",
        total - shown,
        shown,
        total
    ));
    output.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn help_and_version_bypass_filtering() {
        assert!(requests_metadata_only(&["-help".into()]));
        assert!(requests_metadata_only(&["--version".into()]));
        assert!(!requests_metadata_only(&["-p".into(), "Example".into()]));
    }

    #[test]
    fn short_output_is_unchanged() {
        let raw = "Compiled from \"Example.java\"\npublic class Example {\n  public Example();\n}\n";
        assert_eq!(filter_output(raw), raw.trim_end());
    }

    #[test]
    fn large_output_is_capped_by_nonempty_lines() {
        let mut lines = vec!["public class Example {".to_string()];
        lines.extend((0..74).map(|index| format!("  public void method{index}();")));
        lines.push("}".to_string());
        let filtered = filter_output(&lines.join("\n"));

        assert!(filtered.contains("public void method48();"));
        assert!(!filtered.contains("public void method49();"));
        assert!(filtered.ends_with("... +26 more lines (showing 50 of 76)"));
    }

    #[test]
    fn blank_lines_do_not_consume_the_inventory_cap() {
        let raw = (0..60)
            .map(|index| format!("member{index}\n"))
            .collect::<Vec<_>>()
            .join("\n");
        let filtered = filter_output(&raw);

        assert!(filtered.contains("member49"));
        assert!(filtered.ends_with("... +10 more lines (showing 50 of 60)"));
    }
}
