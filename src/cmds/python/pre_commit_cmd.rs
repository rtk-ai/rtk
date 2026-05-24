//! Filters pre-commit output, removing dots between hook names and status.
//! Success/skipped hooks show one line; failures show hook-id + full error.

use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

const HOOK_STATUSES: [&str; 4] = ["Passed", "Failed", "Skipped", "Ignored"];

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    run_with_tool("pre-commit", args, verbose)
}

pub fn run_prek(args: &[String], verbose: u8) -> Result<i32> {
    run_with_tool("prek", args, verbose)
}

fn run_with_tool(tool: &str, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command(tool);

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {}", tool, args.join(" "));
    }

    runner::run_filtered(
        cmd,
        tool,
        &args.join(" "),
        filter_pre_commit_output,
        runner::RunOptions::stdout_only().tee(tool),
    )
}

pub(crate) fn filter_pre_commit_output(output: &str) -> String {
    let mut result: Vec<String> = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        if line.starts_with("[INFO]") || line.trim().is_empty() {
            i += 1;
            continue;
        }

        if let Some((name, status)) = parse_hook_line(line) {
            i += 1;
            match status {
                "Failed" => {
                    let mut hook_id = None;
                    while i < lines.len() {
                        let next = lines[i].trim();
                        if next.starts_with("- hook id:") {
                            hook_id = Some(next.trim_start_matches("- hook id:").trim());
                            i += 1;
                        } else if next.starts_with("- exit code:") || next.is_empty() {
                            i += 1;
                        } else {
                            break;
                        }
                    }

                    let mut linter_output: Vec<&str> = Vec::new();
                    while i < lines.len() {
                        if parse_hook_line(lines[i]).is_some() || lines[i].starts_with("[INFO]") {
                            break;
                        }
                        linter_output.push(lines[i]);
                        i += 1;
                    }

                    let id = hook_id.unwrap_or(name);
                    let mut failure = format!("{} Failed", id);
                    if !linter_output.is_empty() {
                        failure.push('\n');
                        failure.push_str(&linter_output.join("\n"));
                    }
                    result.push(failure);
                }
                _ => {
                    result.push(format!("{} {}", name, status));
                }
            }
        } else {
            result.push(line.to_string());
            i += 1;
        }
    }

    result.join("\n")
}

fn parse_hook_line(line: &str) -> Option<(&str, &str)> {
    for status in &HOOK_STATUSES {
        if let Some(rest) = line.strip_suffix(status) {
            let trimmed = rest.trim_end_matches('.');
            if trimmed.len() < rest.len() {
                return Some((trimmed.trim_end(), *status));
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_all_pass() {
        let output = "\
Trim Trailing Whitespace.................................................Passed
Fix End of Files.........................................................Passed";
        let result = filter_pre_commit_output(output);
        assert_eq!(
            result,
            "Trim Trailing Whitespace Passed\nFix End of Files Passed"
        );
    }

    #[test]
    fn test_mixed_passed_and_failed() {
        let output = "\
Trim Trailing Whitespace.................................................Passed
Check Yaml...............................................................Failed
- hook id: check-yaml
- exit code: 1
.yaml-lint:13:1: expected a mapping
Fix End of Files.........................................................Passed";
        let result = filter_pre_commit_output(output);
        assert_eq!(
            result,
            "Trim Trailing Whitespace Passed\ncheck-yaml Failed\n.yaml-lint:13:1: expected a mapping\nFix End of Files Passed"
        );
    }

    #[test]
    fn test_info_lines_stripped() {
        let output = "\
[INFO] Installing environment for https://github.com/psf/black.
[INFO] Once installed this environment will be reused.
isort....................................................................Passed
black....................................................................Passed";
        let result = filter_pre_commit_output(output);
        assert_eq!(result, "isort Passed\nblack Passed");
    }

    #[test]
    fn test_failed_no_linter_output() {
        let output = "\
Check Yaml...............................................................Failed
- hook id: check-yaml
- exit code: 1";
        let result = filter_pre_commit_output(output);
        assert_eq!(result, "check-yaml Failed");
    }

    #[test]
    fn test_skipped_hook() {
        let output =
            "check-ast................................................................Skipped";
        let result = filter_pre_commit_output(output);
        assert_eq!(result, "check-ast Skipped");
    }

    #[test]
    fn test_multiple_failures() {
        let output = "\
black....................................................................Failed
- hook id: black
- exit code: 1
file.py:1:1: black would reformat
isort....................................................................Failed
- hook id: isort
- exit code: 1
file.py:2:3: import not sorted";
        let result = filter_pre_commit_output(output);
        assert_eq!(
            result,
            "black Failed\nfile.py:1:1: black would reformat\nisort Failed\nfile.py:2:3: import not sorted"
        );
    }

    #[test]
    fn test_passthrough_non_hook_lines() {
        let output = "pre-commit checks completed";
        let result = filter_pre_commit_output(output);
        assert_eq!(result, "pre-commit checks completed");
    }
}
