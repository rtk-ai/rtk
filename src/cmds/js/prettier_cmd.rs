//! Filters Prettier output to show only files that need formatting.

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::package_manager_exec;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = package_manager_exec("prettier");
    let mode = PrettierMode::from_args(args);

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: prettier {}", args.join(" "));
    }

    let args_display = args.join(" ");
    if mode == PrettierMode::Passthrough {
        return runner::run(
            cmd,
            "prettier",
            &args_display,
            runner::RunMode::Passthrough,
            RunOptions::default(),
        );
    }

    runner::run_filtered_with_exit(
        cmd,
        "prettier",
        &args_display,
        move |output, exit_code| filter_prettier_invocation(output, exit_code, mode),
        RunOptions::default(),
    )
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum PrettierMode {
    Check,
    ListDifferent,
    Passthrough,
}

impl PrettierMode {
    fn from_args(args: &[String]) -> Self {
        if args.iter().any(|arg| arg == "--check" || arg == "-c") {
            Self::Check
        } else if args
            .iter()
            .any(|arg| arg == "--list-different" || arg == "-l")
        {
            Self::ListDifferent
        } else {
            Self::Passthrough
        }
    }
}

fn filter_prettier_invocation(output: &str, exit_code: i32, mode: PrettierMode) -> String {
    match mode {
        PrettierMode::Check => filter_prettier_check_output(output, exit_code),
        PrettierMode::ListDifferent => filter_prettier_list_different_output(output, exit_code),
        PrettierMode::Passthrough => output.trim().to_string(),
    }
}

/// Filter Prettier output - show only files that need formatting
pub fn filter_prettier_output(output: &str) -> String {
    filter_prettier_check_output(output, 0)
}

fn filter_prettier_check_output(output: &str, exit_code: i32) -> String {
    // #221: empty or whitespace-only output means prettier didn't run
    if output.trim().is_empty() {
        return "Error: prettier produced no output".to_string();
    }

    let mut files_to_format: Vec<String> = Vec::new();
    let mut files_checked = 0;
    let mut is_check_mode = true;

    for line in output.lines() {
        let trimmed = line.trim();
        let path = trimmed.strip_prefix("[warn] ").unwrap_or(trimmed);

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
            && has_prettier_extension(path)
        {
            files_to_format.push(path.to_string());
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
    if exit_code == 0 && files_to_format.is_empty() && output.contains("All matched files use Prettier")
    {
        return "Prettier: All files formatted correctly".to_string();
    }

    if exit_code != 0 && files_to_format.is_empty() {
        return output.trim().to_string();
    }

    // Check if files were written (write mode)
    if output.contains("modified") || output.contains("formatted") {
        is_check_mode = false;
    }

    let mut result = String::new();

    if is_check_mode {
        // Check mode: show files that need formatting
        if files_to_format.is_empty() {
            result.push_str(output.trim());
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

fn filter_prettier_list_different_output(output: &str, exit_code: i32) -> String {
    if output.trim().is_empty() {
        return if exit_code == 0 {
            "Prettier: All files formatted correctly".to_string()
        } else {
            "Error: prettier produced no output".to_string()
        };
    }

    filter_prettier_check_output(output, exit_code)
}

fn has_prettier_extension(path: &str) -> bool {
    [
        ".ts", ".tsx", ".js", ".jsx", ".json", ".md", ".css", ".scss",
    ]
    .iter()
    .any(|suffix| path.ends_with(suffix))
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
    fn test_filter_warn_prefixed_files_need_formatting() {
        let output = r#"
Checking formatting...
[warn] src/components/ui/button.tsx
[warn] src/lib/auth/session.ts
[warn] Code style issues found in 2 files. Forgot to run Prettier?
        "#;
        let result = filter_prettier_check_output(output, 1);
        assert!(result.contains("2 files need formatting"));
        assert!(result.contains("button.tsx"));
        assert!(result.contains("session.ts"));
        assert!(!result.contains("All files formatted"));
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

    #[test]
    fn test_non_check_invocation_passes_output_through() {
        let result = filter_prettier_invocation("3.8.4\n", 0, PrettierMode::Passthrough);
        assert_eq!(result, "3.8.4");
    }

    #[test]
    fn test_check_failure_without_files_passes_error_through() {
        let output = r#"[ERR_PNPM_NO_PKG_MANIFEST] No package.json found
pnpm: Command failed with exit code 1"#;
        let result = filter_prettier_invocation(output, 1, PrettierMode::Check);
        assert!(result.contains("ERR_PNPM_NO_PKG_MANIFEST"));
        assert!(!result.contains("All files formatted"));
    }

    #[test]
    fn test_list_different_clean_empty_output_is_success() {
        let result = filter_prettier_invocation("", 0, PrettierMode::ListDifferent);
        assert_eq!(result, "Prettier: All files formatted correctly");
    }

    #[test]
    fn test_mode_from_args() {
        let check_args = vec!["--check".to_string(), "src/**/*.tsx".to_string()];
        assert_eq!(PrettierMode::from_args(&check_args), PrettierMode::Check);

        let list_args = vec!["--list-different".to_string(), ".".to_string()];
        assert_eq!(
            PrettierMode::from_args(&list_args),
            PrettierMode::ListDifferent
        );

        let version_args = vec!["--version".to_string()];
        assert_eq!(
            PrettierMode::from_args(&version_args),
            PrettierMode::Passthrough
        );
    }
}
