use crate::prettier_cmd;
use crate::ruff_cmd;
use crate::tracking;
use crate::utils::package_manager_exec;
use anyhow::{Context, Result};
use std::path::Path;
use std::process::Command;

/// Detect formatter from project files or explicit argument
fn detect_formatter(args: &[String]) -> String {
    detect_formatter_in_dir(args, Path::new("."))
}

/// Detect formatter with explicit directory (for testing)
fn detect_formatter_in_dir(args: &[String], dir: &Path) -> String {
    // Check if first arg is a known formatter
    if !args.is_empty() {
        let first_arg = &args[0];
        if matches!(first_arg.as_str(), "prettier" | "black" | "ruff" | "biome") {
            return first_arg.clone();
        }
    }

    // Auto-detect from project files
    // Priority: pyproject.toml > package.json > fallback
    let pyproject_path = dir.join("pyproject.toml");
    if pyproject_path.exists() {
        // Read pyproject.toml to detect formatter
        if let Ok(content) = std::fs::read_to_string(&pyproject_path) {
            // Check for [tool.black] section
            if content.contains("[tool.black]") {
                return "black".to_string();
            }
            // Check for [tool.ruff.format] section
            if content.contains("[tool.ruff.format]") || content.contains("[tool.ruff]") {
                return "ruff".to_string();
            }
        }
    }

    // Check for package.json or prettier config
    if dir.join("package.json").exists()
        || dir.join(".prettierrc").exists()
        || dir.join(".prettierrc.json").exists()
        || dir.join(".prettierrc.js").exists()
    {
        return "prettier".to_string();
    }

    // Fallback: try ruff -> black -> prettier in order
    "ruff".to_string()
}

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    // Detect formatter
    let formatter = detect_formatter(args);

    // Determine start index for actual arguments
    let start_idx = if !args.is_empty() && args[0] == formatter {
        1 // Skip formatter name if it was explicitly provided
    } else {
        0 // Use all args if formatter was auto-detected
    };

    if verbose > 0 {
        eprintln!("Detected formatter: {}", formatter);
        eprintln!("Arguments: {}", args[start_idx..].join(" "));
    }

    // Build command based on formatter
    let mut cmd = match formatter.as_str() {
        "prettier" => package_manager_exec("prettier"),
        "black" | "ruff" => Command::new(formatter.as_str()),
        "biome" => package_manager_exec("biome"),
        _ => Command::new(formatter.as_str()),
    };

    // Add formatter-specific flags
    let user_args = args[start_idx..].to_vec();

    match formatter.as_str() {
        "black" => {
            // Inject --check if not present for check mode
            if !user_args.iter().any(|a| a == "--check" || a == "--diff") {
                cmd.arg("--check");
            }
        }
        "ruff" => {
            // Add "format" subcommand if not present
            if user_args.is_empty() || !user_args[0].starts_with("format") {
                cmd.arg("format");
            }
        }
        _ => {}
    }

    // Add user arguments
    for arg in &user_args {
        cmd.arg(arg);
    }

    // Default to current directory if no path specified
    if user_args.iter().all(|a| a.starts_with('-')) {
        cmd.arg(".");
    }

    if verbose > 0 {
        eprintln!("Running: {} {}", formatter, user_args.join(" "));
    }

    let output = cmd.output().context(format!(
        "Failed to run {}. Is it installed? Try: pip install {} (or npm/pnpm for JS formatters)",
        formatter, formatter
    ))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    // Dispatch to appropriate filter based on formatter
    let filtered = match formatter.as_str() {
        "prettier" => prettier_cmd::filter_prettier_output(&raw),
        "ruff" => ruff_cmd::filter_ruff_format(&raw),
        "black" => filter_black_output(&raw),
        _ => raw.trim().to_string(),
    };

    println!("{}", filtered);

    timer.track(
        &format!("{} {}", formatter, user_args.join(" ")),
        &format!("rtk format {} {}", formatter, user_args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve exit code for CI/CD
    if !output.status.success() {
        std::process::exit(output.status.code().unwrap_or(1));
    }

    Ok(())
}

/// Filter black output - show files that need formatting
fn filter_black_output(output: &str) -> String {
    let mut files_to_format: Vec<String> = Vec::new();
    let mut files_unchanged = 0;
    let mut files_would_reformat = 0;
    let mut all_done = false;
    let mut oh_no = false;

    for line in output.lines() {
        let trimmed = line.trim();
        let lower = trimmed.to_lowercase();

        // Check for "would reformat" lines
        if lower.starts_with("would reformat:") {
            // Extract filename from "would reformat: path/to/file.py"
            if let Some(filename) = trimmed.split(':').nth(1) {
                files_to_format.push(filename.trim().to_string());
            }
        }

        // Parse summary line like "2 files would be reformatted, 3 files would be left unchanged."
        if lower.contains("would be reformatted") || lower.contains("would be left unchanged") {
            // Split by comma to handle both parts
            for part in trimmed.split(',') {
                let part_lower = part.to_lowercase();
                let words: Vec<&str> = part.trim().split_whitespace().collect();

                if part_lower.contains("would be reformatted") {
                    // Parse "X file(s) would be reformatted"
                    for (i, word) in words.iter().enumerate() {
                        if (word == &"file" || word == &"files") && i > 0 {
                            if let Ok(count) = words[i - 1].parse::<usize>() {
                                files_would_reformat = count;
                                break;
                            }
                        }
                    }
                }

                if part_lower.contains("would be left unchanged") {
                    // Parse "X file(s) would be left unchanged"
                    for (i, word) in words.iter().enumerate() {
                        if (word == &"file" || word == &"files") && i > 0 {
                            if let Ok(count) = words[i - 1].parse::<usize>() {
                                files_unchanged = count;
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Check for "left unchanged" (standalone)
        if lower.contains("left unchanged") && !lower.contains("would be") {
            let words: Vec<&str> = trimmed.split_whitespace().collect();
            for (i, word) in words.iter().enumerate() {
                if (word == &"file" || word == &"files") && i > 0 {
                    if let Ok(count) = words[i - 1].parse::<usize>() {
                        files_unchanged = count;
                        break;
                    }
                }
            }
        }

        // Check for success/failure indicators
        if lower.contains("all done!") || lower.contains("all done ✨") {
            all_done = true;
        }
        if lower.contains("oh no!") {
            oh_no = true;
        }
    }

    // Build output
    let mut result = String::new();

    // Determine if all files are formatted
    let needs_formatting = !files_to_format.is_empty() || files_would_reformat > 0 || oh_no;

    if !needs_formatting && (all_done || files_unchanged > 0) {
        // All files formatted correctly
        result.push_str("✓ Format (black): All files formatted");
        if files_unchanged > 0 {
            result.push_str(&format!(" ({} files checked)", files_unchanged));
        }
    } else if needs_formatting {
        // Files need formatting
        let count = if !files_to_format.is_empty() {
            files_to_format.len()
        } else {
            files_would_reformat
        };

        result.push_str(&format!(
            "Format (black): {} files need formatting\n",
            count
        ));
        result.push_str("═══════════════════════════════════════\n");

        if !files_to_format.is_empty() {
            for (i, file) in files_to_format.iter().take(10).enumerate() {
                result.push_str(&format!("{}. {}\n", i + 1, compact_path(file)));
            }

            if files_to_format.len() > 10 {
                result.push_str(&format!(
                    "\n... +{} more files\n",
                    files_to_format.len() - 10
                ));
            }
        }

        if files_unchanged > 0 {
            result.push_str(&format!(
                "\n✓ {} files already formatted\n",
                files_unchanged
            ));
        }

        result.push_str("\n💡 Run `black .` to format these files\n");
    } else {
        // Fallback: show raw output
        result.push_str(output.trim());
    }

    result.trim().to_string()
}

/// Compact file path (remove common prefixes)
fn compact_path(path: &str) -> String {
    let path = path.replace('\\', "/");

    if let Some(pos) = path.rfind("/src/") {
        format!("src/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/lib/") {
        format!("lib/{}", &path[pos + 5..])
    } else if let Some(pos) = path.rfind("/tests/") {
        format!("tests/{}", &path[pos + 7..])
    } else if let Some(pos) = path.rfind('/') {
        path[pos + 1..].to_string()
    } else {
        path
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::io::Write;
    use tempfile::TempDir;

    #[test]
    fn test_detect_formatter_from_explicit_arg() {
        let args = vec!["black".to_string(), "--check".to_string()];
        let formatter = detect_formatter(&args);
        assert_eq!(formatter, "black");

        let args = vec!["prettier".to_string(), ".".to_string()];
        let formatter = detect_formatter(&args);
        assert_eq!(formatter, "prettier");

        let args = vec!["ruff".to_string(), "format".to_string()];
        let formatter = detect_formatter(&args);
        assert_eq!(formatter, "ruff");
    }

    #[test]
    fn test_detect_formatter_from_pyproject_black() {
        let temp_dir = TempDir::new().unwrap();
        let pyproject_path = temp_dir.path().join("pyproject.toml");
        let mut file = fs::File::create(&pyproject_path).unwrap();
        writeln!(file, "[tool.black]\nline-length = 88").unwrap();

        let formatter = detect_formatter_in_dir(&[], temp_dir.path());
        assert_eq!(formatter, "black");
    }

    #[test]
    fn test_detect_formatter_from_pyproject_ruff() {
        let temp_dir = TempDir::new().unwrap();
        let pyproject_path = temp_dir.path().join("pyproject.toml");
        let mut file = fs::File::create(&pyproject_path).unwrap();
        writeln!(file, "[tool.ruff.format]\nindent-width = 4").unwrap();

        let formatter = detect_formatter_in_dir(&[], temp_dir.path());
        assert_eq!(formatter, "ruff");
    }

    #[test]
    fn test_detect_formatter_from_package_json() {
        let temp_dir = TempDir::new().unwrap();
        let package_path = temp_dir.path().join("package.json");
        let mut file = fs::File::create(&package_path).unwrap();
        writeln!(file, "{{\"name\": \"test\"}}").unwrap();

        let formatter = detect_formatter_in_dir(&[], temp_dir.path());
        assert_eq!(formatter, "prettier");
    }

    #[test]
    fn test_filter_black_all_formatted() {
        let output = "All done! ✨ 🍰 ✨\n5 files left unchanged.";
        let result = filter_black_output(output);
        assert!(result.contains("✓ Format (black)"));
        assert!(result.contains("All files formatted"));
        assert!(result.contains("5 files checked"));
    }

    #[test]
    fn test_filter_black_needs_formatting() {
        let output = r#"would reformat: src/main.py
would reformat: tests/test_utils.py
Oh no! 💥 💔 💥
2 files would be reformatted, 3 files would be left unchanged."#;

        let result = filter_black_output(output);
        assert!(result.contains("2 files need formatting"));
        assert!(result.contains("main.py"));
        assert!(result.contains("test_utils.py"));
        assert!(result.contains("3 files already formatted"));
        assert!(result.contains("Run `black .`"));
    }

    #[test]
    fn test_compact_path() {
        assert_eq!(
            compact_path("/Users/foo/project/src/main.py"),
            "src/main.py"
        );
        assert_eq!(compact_path("/home/user/app/lib/utils.py"), "lib/utils.py");
        assert_eq!(
            compact_path("C:\\Users\\foo\\project\\tests\\test.py"),
            "tests/test.py"
        );
        assert_eq!(compact_path("relative/file.py"), "file.py");
    }
}
