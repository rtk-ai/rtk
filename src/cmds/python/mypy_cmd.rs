//! Filters mypy type-checking output, grouping errors by file.

use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi, tool_exists};
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::sync::LazyLock;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = if tool_exists("mypy") {
        resolved_command("mypy")
    } else {
        let mut c = resolved_command("python3");
        c.arg("-m").arg("mypy");
        c
    };

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: mypy {}", args.join(" "));
    }

    runner::run_filtered_with_exit(
        cmd,
        "mypy",
        &args.join(" "),
        |raw, exit_code| {
            let clean = strip_ansi(raw);
            let filtered = filter_mypy_output(&clean);
            // Nothing recognised on a failed run means mypy never type-checked.
            if exit_code != 0 && filtered == MYPY_CLEAN {
                return clean.trim().to_string();
            }
            filtered
        },
        runner::RunOptions::default(),
    )
}

const MYPY_CLEAN: &str = "mypy: No issues found";

struct MypyError {
    file: String,
    line: usize,
    column: Option<usize>,
    severity: String,
    code: String,
    message: String,
    context_lines: Vec<MypyNote>,
}

struct MypyNote {
    line: usize,
    column: Option<usize>,
    code: String,
    message: String,
}

pub fn filter_mypy_output(output: &str) -> String {
    // file.py:12: error: Message [error-code]
    // file.py:12:5: error: Message [error-code]
    static MYPY_DIAG: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"^(.+?):(\d+)(?::(\d+))?: (error|warning|note): (.+?)(?:\s+\[(.+)\])?$",
        )
        .unwrap()
    });

    let lines: Vec<&str> = output.lines().collect();
    let mut errors: Vec<MypyError> = Vec::new();
    let mut fileless_lines: Vec<String> = Vec::new();
    let mut native_summary: Option<String> = None;
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];

        if line.starts_with("Found ") && line.contains(" error") {
            native_summary = Some(format!("mypy: {}", line.trim_start_matches("Found ")));
            i += 1;
            continue;
        }
        if line.starts_with("Success:") {
            let detail = line.trim_start_matches("Success:").trim_start();
            let detail = detail
                .strip_prefix("no issues found")
                .map(|rest| format!("No issues found{}", rest))
                .unwrap_or_else(|| detail.to_string());
            native_summary = Some(format!("mypy: {}", detail));
            i += 1;
            continue;
        }

        if let Some(caps) = MYPY_DIAG.captures(line) {
            let severity = &caps[4];
            let file = caps[1].to_string();
            let line_num: usize = caps[2].parse().unwrap_or(0);
            let column = caps.get(3).and_then(|m| m.as_str().parse().ok());
            let message = caps[5].to_string();
            let code = caps
                .get(6)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();

            if severity == "note" {
                // Attach note to preceding error if same file and line
                if let Some(last) = errors.last_mut() {
                    if last.file == file {
                        last.context_lines.push(MypyNote {
                            line: line_num,
                            column,
                            code,
                            message,
                        });
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
                column,
                severity: severity.to_string(),
                code,
                message,
                context_lines: Vec::new(),
            };

            // Capture continuation note lines
            i += 1;
            while i < lines.len() {
                if let Some(next_caps) = MYPY_DIAG.captures(lines[i]) {
                    if &next_caps[4] == "note" && next_caps[1] == err.file {
                        err.context_lines.push(MypyNote {
                            line: next_caps[2].parse().unwrap_or(0),
                            column: next_caps.get(3).and_then(|m| m.as_str().parse().ok()),
                            code: next_caps
                                .get(6)
                                .map(|m| m.as_str().to_string())
                                .unwrap_or_default(),
                            message: next_caps[5].to_string(),
                        });
                        i += 1;
                        continue;
                    }
                }
                break;
            }

            errors.push(err);
        } else if ["error:", "warning:", "note:"]
            .iter()
            .any(|marker| line.contains(marker))
            && !line.trim().is_empty()
        {
            // File-less diagnostic (config errors, import errors, global warnings)
            fileless_lines.push(line.to_string());
            i += 1;
        } else {
            i += 1;
        }
    }

    // No errors at all
    if errors.is_empty() && fileless_lines.is_empty() {
        return native_summary.unwrap_or_else(|| MYPY_CLEAN.to_string());
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

    if errors.is_empty() {
        if let Some(summary) = native_summary {
            result.push_str(&summary);
        }
        return result.trim().to_string();
    }

    if !errors.is_empty() {
        let summary = native_summary.unwrap_or_else(|| {
            let error_count = errors.iter().filter(|d| d.severity == "error").count();
            let warning_count = errors.iter().filter(|d| d.severity == "warning").count();
            match (error_count, warning_count) {
                (errors, 0) => format!("mypy: {} errors in {} files", errors, by_file.len()),
                (0, warnings) => {
                    format!("mypy: {} warnings in {} files", warnings, by_file.len())
                }
                (errors, warnings) => format!(
                    "mypy: {} errors, {} warnings in {} files",
                    errors,
                    warnings,
                    by_file.len()
                ),
            }
        });
        result.push_str(&summary);
        result.push('\n');

        // Top error codes summary (only when 2+ distinct codes)
        let mut code_counts: Vec<_> = by_code.iter().collect();
        code_counts.sort_by(|a, b| b.1.cmp(a.1).then_with(|| a.0.cmp(b.0)));

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
        files_sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then_with(|| a.0.cmp(b.0)));

        for (file, file_errors) in &files_sorted {
            let error_count = file_errors
                .iter()
                .filter(|d| d.severity == "error")
                .count();
            let warning_count = file_errors
                .iter()
                .filter(|d| d.severity == "warning")
                .count();
            let counts = match (error_count, warning_count) {
                (errors, 0) => format!("{} errors", errors),
                (0, warnings) => format!("{} warnings", warnings),
                (errors, warnings) => format!("{} errors, {} warnings", errors, warnings),
            };
            result.push_str(&format!("{} ({})\n", file, counts));

            for err in *file_errors {
                let location = err
                    .column
                    .map(|column| format!("L{}:C{}", err.line, column))
                    .unwrap_or_else(|| format!("L{}", err.line));
                let code = if err.code.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", err.code)
                };
                result.push_str(&format!(
                    "  {}: {}{} {}\n",
                    location, err.severity, code, err.message
                ));
                for ctx in &err.context_lines {
                    let location = ctx
                        .column
                        .map(|column| format!("L{}:C{}", ctx.line, column))
                        .unwrap_or_else(|| format!("L{}", ctx.line));
                    let code = if ctx.code.is_empty() {
                        String::new()
                    } else {
                        format!(" [{}]", ctx.code)
                    };
                    result.push_str(&format!(
                        "    {}: note{} {}\n",
                        location, code, ctx.message
                    ));
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
        assert!(result.contains("checked 10 source files"));
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
        assert!(result.contains("L10:C5: error"));
        assert!(result.contains("[return-value]"));
        assert!(result.contains("Incompatible return value type"));
    }

    #[test]
    fn test_filter_mypy_preserves_diagnostic_severity_location_code_and_message() {
        let long_message = format!("Unexpected value {}", "detail ".repeat(30));
        let output = format!(
            "src/api.py:10:5: warning: Deprecated call  [deprecated]\n\
             src/api.py:11:7: error: {long_message} [assignment]\n\
             src/api.py:11:7: note: Expected type \"int\"  [note-code]\n\
             Found 1 error in 1 file (checked 27 source files)\n"
        );

        let result = filter_mypy_output(&output);

        assert!(result.contains("L10:C5: warning [deprecated] Deprecated call"));
        assert!(result.contains("L11:C7: error [assignment]"));
        assert!(result.contains("L11:C7: note [note-code] Expected type \"int\""));
        assert!(result.contains(long_message.trim()));
        assert!(result.contains("checked 27 source files"));
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
    fn test_filter_mypy_equal_counts_have_stable_order() {
        let output = "\
zeta.py:1: error: Error one  [return-value]
zeta.py:2: error: Error two  [assignment]
alpha.py:1: error: Error three  [return-value]
alpha.py:2: error: Error four  [assignment]
Found 4 errors in 2 files
";
        let result = filter_mypy_output(output);

        assert!(result.contains("Top codes: assignment (2x), return-value (2x)"));
        assert!(result.find("alpha.py (2 errors)") < result.find("zeta.py (2 errors)"));
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
mypy: warning: Global configuration is deprecated
src/api.py:10: error: Name \"foo\" is not defined  [name-defined]
Found 1 error in 1 file
";
        let result = filter_mypy_output(output);
        // File-less error should appear verbatim before grouped output
        assert!(result.contains("mypy: error: No module named 'nonexistent'"));
        assert!(result.contains("mypy: warning: Global configuration is deprecated"));
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
        assert_eq!(result, "mypy: No issues found in 5 source files");
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
