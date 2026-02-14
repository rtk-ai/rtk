use crate::tracking;
use crate::utils::package_manager_exec;
use anyhow::{Context, Result};

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = package_manager_exec("prettier");

    // Add user arguments
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: prettier {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run prettier (try: npm install -g prettier)")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_prettier_output(&raw);

    println!("{}", filtered);

    timer.track(
        &format!("prettier {}", args.join(" ")),
        &format!("rtk prettier {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Filter Prettier output - show only files that need formatting
pub fn filter_prettier_output(output: &str) -> String {
    let mut files_to_format: Vec<String> = Vec::new();
    let mut files_checked = 0;
    let mut is_check_mode = true;

    for line in output.lines() {
        let trimmed = line.trim();

        // Detect check mode vs write mode
        if trimmed.contains("Checking formatting") {
            is_check_mode = true;
        }

        // Count files that need formatting (check mode)
        if !trimmed.is_empty()
            && !trimmed.starts_with("Checking")
            && !trimmed.starts_with("All matched")
            && !trimmed.starts_with("Code style")
            && !trimmed.contains("[warn]")
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
        return "✓ Prettier: All files formatted correctly".to_string();
    }

    // Check if files were written (write mode)
    if output.contains("modified") || output.contains("formatted") {
        is_check_mode = false;
    }

    let mut result = String::new();

    if is_check_mode {
        // Check mode: show files that need formatting
        if files_to_format.is_empty() {
            result.push_str("✓ Prettier: All files formatted correctly\n");
        } else {
            result.push_str(&format!(
                "Prettier: {} files need formatting\n",
                files_to_format.len()
            ));
            result.push_str("═══════════════════════════════════════\n");

            for (i, file) in files_to_format.iter().take(10).enumerate() {
                result.push_str(&format!("{}. {}\n", i + 1, file));
            }

            if files_to_format.len() > 10 {
                result.push_str(&format!(
                    "\n... +{} more files\n",
                    files_to_format.len() - 10
                ));
            }

            if files_checked > 0 {
                result.push_str(&format!(
                    "\n✓ {} files already formatted\n",
                    files_checked - files_to_format.len()
                ));
            }
        }
    } else {
        // Write mode: show what was formatted
        result.push_str(&format!(
            "✓ Prettier: {} files formatted\n",
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
        assert!(result.contains("✓ Prettier"));
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
}
