//! Filters Prettier output to show only files that need formatting.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::package_manager_exec;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = package_manager_exec("prettier");

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: prettier {}", args.join(" "));
    }

    // Prettier >= 1.19 logs its `--check` results ("[warn] <file>") to stderr,
    // not stdout — the filter must see the combined output, and the exit code,
    // to avoid reporting a failing check as success.
    runner::run_filtered_with_exit(
        cmd,
        "prettier",
        &args.join(" "),
        filter_prettier_output_with_exit,
        RunOptions::default(),
    )
}

/// Filter Prettier output - show only files that need formatting
pub fn filter_prettier_output(output: &str) -> String {
    filter_prettier_output_with_exit(output, 0)
}

fn filter_prettier_output_with_exit(output: &str, exit_code: i32) -> String {
    // #221: empty or whitespace-only output means prettier didn't run
    if output.trim().is_empty() {
        return "Error: prettier produced no output".to_string();
    }

    let mut files_to_format: Vec<String> = Vec::new();
    let mut files_checked = 0;
    let mut is_check_mode = true;

    for line in output.lines() {
        let trimmed = line.trim();

        // Prettier >= 1.19 prefixes each check result with "[warn] " (on
        // stderr). Strip the prefix so those file lines are recognized.
        let trimmed = trimmed
            .strip_prefix("[warn]")
            .map(str::trim_start)
            .unwrap_or(trimmed);

        // Detect check mode vs write mode
        if trimmed.contains("Checking formatting") {
            is_check_mode = true;
        }

        // Count files that need formatting (check mode)
        if !trimmed.is_empty()
            && !trimmed.starts_with("Checking")
            && !trimmed.starts_with("All matched")
            && !trimmed.starts_with("Code style")
            && !trimmed.contains("[error]")
            && (trimmed.ends_with(".ts")
                || trimmed.ends_with(".tsx")
                || trimmed.ends_with(".js")
                || trimmed.ends_with(".jsx")
                || trimmed.ends_with(".json")
                || trimmed.ends_with(".md")
                || trimmed.ends_with(".css")
                || trimmed.ends_with(".scss"))
        {
            files_to_format.push(trimmed.to_string());
        }

        // Count total files checked
        if trimmed.contains("All matched files use Prettier") {
            if let Some(count_str) = trimmed.split_whitespace().next() {
                if let Ok(count) = count_str.parse::<usize>() {
                    files_checked = count;
                }
            }
        }
    }

    // Check if all files are formatted
    if files_to_format.is_empty() && output.contains("All matched files use Prettier") {
        return "Prettier: All files formatted correctly".to_string();
    }

    // Check if files were written (write mode)
    if output.contains("modified") || output.contains("formatted") {
        is_check_mode = false;
    }

    let mut result = String::new();

    if is_check_mode {
        // Check mode: show files that need formatting
        if files_to_format.is_empty() {
            if exit_code != 0 {
                // Prettier failed but no file list could be parsed (unknown
                // extension, unexpected output shape). Never claim success on
                // a failing check — pass the real output through.
                return output.trim().to_string();
            }
            result.push_str("Prettier: All files formatted correctly\n");
        } else {
            result.push_str(&format!(
                "Prettier: {} files need formatting\n",
                files_to_format.len()
            ));

            const MAX_PRETTIER_FILES: usize = CAP_WARNINGS;
            for (i, file) in files_to_format.iter().take(MAX_PRETTIER_FILES).enumerate() {
                result.push_str(&format!("{}. {}\n", i + 1, file));
            }

            if files_to_format.len() > MAX_PRETTIER_FILES {
                result.push_str(&format!(
                    "\n... +{} more files\n",
                    files_to_format.len() - MAX_PRETTIER_FILES
                ));
            }

            if files_checked > 0 {
                result.push_str(&format!(
                    "\n{} files already formatted\n",
                    files_checked - files_to_format.len()
                ));
            }
        }
    } else {
        // Write mode: show what was formatted
        result.push_str(&format!(
            "Prettier: {} files formatted\n",
            files_to_format.len()
        ));
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_all_formatted() {
        let output = r#"
Checking formatting...
All matched files use Prettier code style!
        "#;
        let result = filter_prettier_output(output);
        assert!(result.contains("Prettier"));
        assert!(result.contains("All files formatted correctly"));
    }

    #[test]
    fn test_filter_files_need_formatting() {
        let output = r#"
Checking formatting...
src/components/ui/button.tsx
src/lib/auth/session.ts
src/pages/dashboard.tsx
Code style issues found in the above file(s). Forgot to run Prettier?
        "#;
        let result = filter_prettier_output(output);
        assert!(result.contains("3 files need formatting"));
        assert!(result.contains("button.tsx"));
        assert!(result.contains("session.ts"));
    }

    #[test]
    fn test_filter_many_files() {
        let mut output = String::from("Checking formatting...\n");
        for i in 0..15 {
            output.push_str(&format!("src/file{}.ts\n", i));
        }
        let result = filter_prettier_output(&output);
        assert!(result.contains("15 files need formatting"));
        assert!(result.contains("... +5 more files"));
    }

    // --- #221: empty output should not say "All files formatted" ---

    #[test]
    fn test_filter_empty_output() {
        let result = filter_prettier_output("");
        assert!(result.contains("Error"));
        assert!(!result.contains("All files formatted"));
    }

    #[test]
    fn test_filter_whitespace_only_output() {
        let result = filter_prettier_output("   \n\n  ");
        assert!(result.contains("Error"));
        assert!(!result.contains("All files formatted"));
    }

    // --- failing `--check` writes "[warn] <file>" to stderr (prettier >= 1.19) ---

    #[test]
    fn test_filter_check_failure_warn_prefixed_files() {
        // Real prettier 3.x --check output (combined stdout + stderr)
        let output = "Checking formatting...\n\
                      [warn] src/app.ts\n\
                      [warn] src/util.js\n\
                      [warn] Code style issues found in the above files. Run Prettier with --write to fix.";
        let result = filter_prettier_output_with_exit(output, 1);
        assert!(
            result.contains("2 files need formatting"),
            "got: {}",
            result
        );
        assert!(result.contains("src/app.ts"));
        assert!(result.contains("src/util.js"));
        assert!(!result.contains("All files formatted correctly"));
    }

    #[test]
    fn test_filter_check_failure_never_reports_success() {
        // Unrecognized extension: no file list parsed, but prettier exited 1 —
        // must not claim success.
        let output = "Checking formatting...\n\
                      [warn] src/component.vue\n\
                      [warn] Code style issues found in the above file. Run Prettier with --write to fix.";
        let result = filter_prettier_output_with_exit(output, 1);
        assert!(
            !result.contains("All files formatted correctly"),
            "failing check reported as success: {}",
            result
        );
        assert!(result.contains("src/component.vue"));
    }

    #[test]
    fn test_filter_check_success_exit_zero_unchanged() {
        let output = "Checking formatting...\nAll matched files use Prettier code style!";
        let result = filter_prettier_output_with_exit(output, 0);
        assert!(result.contains("All files formatted correctly"));
    }
}
