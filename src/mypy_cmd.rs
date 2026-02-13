use crate::tracking;
use crate::utils::{strip_ansi, truncate};
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = if which_command("mypy").is_some() {
        Command::new("mypy")
    } else {
        let mut c = Command::new("python3");
        c.arg("-m").arg("mypy");
        c
    };

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: mypy {}", args.join(" "));
    }

    let output = cmd
        .output()
        .context("Failed to run mypy. Is it installed? Try: pip install mypy")?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);
    let clean = strip_ansi(&raw);

    let filtered = filter_mypy_output(&clean);

    println!("{}", filtered);

    timer.track(
        &format!("mypy {}", args.join(" ")),
        &format!("rtk mypy {}", args.join(" ")),
        &raw,
        &filtered,
    );

    std::process::exit(output.status.code().unwrap_or(1));
}

fn which_command(cmd: &str) -> Option<String> {
    Command::new("which")
        .arg(cmd)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8(o.stdout).ok())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

struct MypyError {
    file: String,
    line: usize,
    code: String,
    message: String,
    context_lines: Vec<String>,
}

fn filter_mypy_output(output: &str) -> String {
    lazy_static::lazy_static! {
        // file.py:12: error: Message [error-code]
        // file.py:12:5: error: Message [error-code]
        static ref MYPY_DIAG: Regex = Regex::new(
            r"^(.+?):(\d+)(?::\d+)?: (error|warning|note): (.+?)(?:\s+\[(.+)\])?$"
        ).unwrap();
    }

    let lines: Vec<&str> = output.lines().collect();
    let mut errors: Vec<MypyError> = Vec::new();
    let mut fileless_lines: Vec<String> = Vec::new();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        // Skip mypy's own summary line
        if line.starts_with("Found ") && line.contains(" error") {
            i += 1;
            continue;
        }
        // Skip "Success: no issues found"
        if line.starts_with("Success:") {
            i += 1;
            continue;
        }

        if let Some(caps) = MYPY_DIAG.captures(line) {
            let severity = &caps[3];
            let file = caps[1].to_string();
            let line_num: usize = caps[2].parse().unwrap_or(0);
            let message = caps[4].to_string();
            let code = caps
                .get(5)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();

            if severity == "note" {
                // Attach note to preceding error if same file and line
                if let Some(last) = errors.last_mut() {
                    if last.file == file {
                        last.context_lines.push(message);
                        i += 1;
                        continue;
                    }
                }
                // Standalone note with no parent -- display as fileless
                fileless_lines.push(line.to_string());
                i += 1;
                continue;
            }

            let mut err = MypyError {
                file,
                line: line_num,
                code,
                message,
                context_lines: Vec::new(),
            };

            // Capture continuation note lines
            i += 1;
            while i < lines.len() {
                if let Some(next_caps) = MYPY_DIAG.captures(lines[i]) {
                    if &next_caps[3] == "note" && next_caps[1] == err.file {
                        let note_msg = next_caps[4].to_string();
                        err.context_lines.push(note_msg);
                        i += 1;
                        continue;
                    }
                }
                break;
            }

            errors.push(err);
        } else if line.contains("error:") && !line.trim().is_empty() {
            // File-less error (config errors, import errors)
            fileless_lines.push(line.to_string());
            i += 1;
        } else {
            i += 1;
        }
    }

    // No errors at all
    if errors.is_empty() && fileless_lines.is_empty() {
        if output.contains("Success: no issues found") || output.contains("no issues found") {
            return "mypy: No issues found".to_string();
        }
        return "mypy: No issues found".to_string();
    }

    // Group by file
    let mut by_file: HashMap<String, Vec<&MypyError>> = HashMap::new();
    for err in &errors {
        by_file.entry(err.file.clone()).or_default().push(err);
    }

    // Count by error code
    let mut by_code: HashMap<String, usize> = HashMap::new();
    for err in &errors {
        if !err.code.is_empty() {
            *by_code.entry(err.code.clone()).or_insert(0) += 1;
        }
    }

    let mut result = String::new();

    // File-less errors first
    for line in &fileless_lines {
        result.push_str(line);
        result.push('\n');
    }
    if !fileless_lines.is_empty() && !errors.is_empty() {
        result.push('\n');
    }

    if !errors.is_empty() {
        result.push_str(&format!(
            "mypy: {} errors in {} files\n",
            errors.len(),
            by_file.len()
        ));
        result.push_str("═══════════════════════════════════════\n");

        // Top error codes summary (only when 2+ distinct codes)
        let mut code_counts: Vec<_> = by_code.iter().collect();
        code_counts.sort_by(|a, b| b.1.cmp(a.1));

        if code_counts.len() > 1 {
            let codes_str: Vec<String> = code_counts
                .iter()
                .take(5)
                .map(|(code, count)| format!("{} ({}x)", code, count))
                .collect();
            result.push_str(&format!("Top codes: {}\n\n", codes_str.join(", ")));
        }

        // Files sorted by error count (most errors first)
        let mut files_sorted: Vec<_> = by_file.iter().collect();
        files_sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        for (file, file_errors) in &files_sorted {
            result.push_str(&format!("{} ({} errors)\n", file, file_errors.len()));

            for err in *file_errors {
                if err.code.is_empty() {
                    result.push_str(&format!(
                        "  L{}: {}\n",
                        err.line,
                        truncate(&err.message, 120)
                    ));
                } else {
                    result.push_str(&format!(
                        "  L{}: [{}] {}\n",
                        err.line,
                        err.code,
                        truncate(&err.message, 120)
                    ));
                }
                for ctx in &err.context_lines {
                    result.push_str(&format!("    {}\n", truncate(ctx, 120)));
                }
            }
            result.push('\n');
        }
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_mypy_errors_grouped_by_file() {
        let output = "\
src/server/auth.py:12: error: Incompatible return value type (got \"str\", expected \"int\")  [return-value]
src/server/auth.py:15: error: Argument 1 has incompatible type \"int\"; expected \"str\"  [arg-type]
src/models/user.py:8: error: Name \"foo\" is not defined  [name-defined]
src/models/user.py:10: error: Incompatible types in assignment  [assignment]
src/models/user.py:20: error: Missing return statement  [return]
Found 5 errors in 2 files (checked 10 source files)
";
        let result = filter_mypy_output(output);
        assert!(result.contains("mypy: 5 errors in 2 files"));
        // user.py has 3 errors, auth.py has 2 -- user.py should come first
        let user_pos = result.find("user.py").unwrap();
        let auth_pos = result.find("auth.py").unwrap();
        assert!(
            user_pos < auth_pos,
            "user.py (3 errors) should appear before auth.py (2 errors)"
        );
        assert!(result.contains("user.py (3 errors)"));
        assert!(result.contains("auth.py (2 errors)"));
    }

    #[test]
    fn test_filter_mypy_with_column_numbers() {
        let output = "\
src/api.py:10:5: error: Incompatible return value type  [return-value]
";
        let result = filter_mypy_output(output);
        assert!(result.contains("L10:"));
        assert!(result.contains("[return-value]"));
        assert!(result.contains("Incompatible return value type"));
    }

    #[test]
    fn test_filter_mypy_top_codes_summary() {
        let output = "\
a.py:1: error: Error one  [return-value]
a.py:2: error: Error two  [return-value]
a.py:3: error: Error three  [return-value]
b.py:1: error: Error four  [name-defined]
c.py:1: error: Error five  [arg-type]
Found 5 errors in 3 files
";
        let result = filter_mypy_output(output);
        assert!(result.contains("Top codes:"));
        assert!(result.contains("return-value (3x)"));
        assert!(result.contains("name-defined (1x)"));
        assert!(result.contains("arg-type (1x)"));
    }

    #[test]
    fn test_filter_mypy_single_code_no_summary() {
        let output = "\
a.py:1: error: Error one  [return-value]
a.py:2: error: Error two  [return-value]
b.py:1: error: Error three  [return-value]
Found 3 errors in 2 files
";
        let result = filter_mypy_output(output);
        assert!(
            !result.contains("Top codes:"),
            "Top codes should not appear with only one distinct code"
        );
    }

    #[test]
    fn test_filter_mypy_every_error_shown() {
        let output = "\
src/api.py:10: error: Type \"str\" not assignable to \"int\"  [assignment]
src/api.py:20: error: Missing return statement  [return]
src/api.py:30: error: Name \"bar\" is not defined  [name-defined]
";
        let result = filter_mypy_output(output);
        assert!(result.contains("Type \"str\" not assignable to \"int\""));
        assert!(result.contains("Missing return statement"));
        assert!(result.contains("Name \"bar\" is not defined"));
        assert!(result.contains("L10:"));
        assert!(result.contains("L20:"));
        assert!(result.contains("L30:"));
    }

    #[test]
    fn test_filter_mypy_note_continuation() {
        let output = "\
src/app.py:10: error: Incompatible types in assignment  [assignment]
src/app.py:10: note: Expected type \"int\"
src/app.py:10: note: Got type \"str\"
src/app.py:20: error: Missing return statement  [return]
";
        let result = filter_mypy_output(output);
        assert!(result.contains("Incompatible types in assignment"));
        assert!(result.contains("Expected type \"int\""));
        assert!(result.contains("Got type \"str\""));
        assert!(result.contains("L10:"));
        assert!(result.contains("L20:"));
    }

    #[test]
    fn test_filter_mypy_fileless_errors() {
        let output = "\
mypy: error: No module named 'nonexistent'
src/api.py:10: error: Name \"foo\" is not defined  [name-defined]
Found 1 error in 1 file
";
        let result = filter_mypy_output(output);
        // File-less error should appear verbatim before grouped output
        assert!(result.contains("mypy: error: No module named 'nonexistent'"));
        assert!(result.contains("api.py (1 error"));
        let fileless_pos = result.find("No module named").unwrap();
        let grouped_pos = result.find("api.py").unwrap();
        assert!(
            fileless_pos < grouped_pos,
            "File-less errors should appear before grouped file errors"
        );
    }

    #[test]
    fn test_filter_mypy_no_errors() {
        let output = "Success: no issues found in 5 source files\n";
        let result = filter_mypy_output(output);
        assert_eq!(result, "mypy: No issues found");
    }

    #[test]
    fn test_filter_mypy_no_file_limit() {
        let mut output = String::new();
        for i in 1..=15 {
            output.push_str(&format!(
                "src/file{}.py:{}: error: Error in file {}.  [assignment]\n",
                i, i, i
            ));
        }
        output.push_str("Found 15 errors in 15 files\n");
        let result = filter_mypy_output(&output);
        assert!(result.contains("15 errors in 15 files"));
        for i in 1..=15 {
            assert!(
                result.contains(&format!("file{}.py", i)),
                "file{}.py missing from output",
                i
            );
        }
    }
}
