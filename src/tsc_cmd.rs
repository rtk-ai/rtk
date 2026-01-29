use crate::tracking;
use crate::utils::truncate;
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::process::Command;

pub fn run(args: &[String], verbose: u8) -> Result<()> {
    // Try tsc directly first, fallback to npx if not found
    let tsc_exists = Command::new("which")
        .arg("tsc")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false);

    let mut cmd = if tsc_exists {
        Command::new("tsc")
    } else {
        let mut c = Command::new("npx");
        c.arg("tsc");
        c
    };

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        let tool = if tsc_exists { "tsc" } else { "npx tsc" };
        eprintln!("Running: {} {}", tool, args.join(" "));
    }

    let output = cmd.output().context("Failed to run tsc (try: npm install -g typescript)")?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let raw = format!("{}\n{}", stdout, stderr);

    let filtered = filter_tsc_output(&raw);

    println!("{}", filtered);

    tracking::track(
        &format!("tsc {}", args.join(" ")),
        &format!("rtk tsc {}", args.join(" ")),
        &raw,
        &filtered,
    );

    // Preserve tsc exit code for CI/CD compatibility
    std::process::exit(output.status.code().unwrap_or(1));
}

/// Filter TypeScript compiler output - group errors by file and error code
fn filter_tsc_output(output: &str) -> String {
    lazy_static::lazy_static! {
        // Pattern: src/file.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.
        static ref TSC_ERROR: Regex = Regex::new(
            r"^(.+?)\((\d+),(\d+)\):\s+(error|warning)\s+(TS\d+):\s+(.+)$"
        ).unwrap();
    }

    #[derive(Debug)]
    struct TsError {
        file: String,
        line: usize,
        col: usize,
        _severity: String,
        code: String,
        message: String,
    }

    let mut errors: Vec<TsError> = Vec::new();
    let mut other_lines: Vec<String> = Vec::new();

    for line in output.lines() {
        if let Some(caps) = TSC_ERROR.captures(line) {
            errors.push(TsError {
                file: caps[1].to_string(),
                line: caps[2].parse().unwrap_or(0),
                col: caps[3].parse().unwrap_or(0),
                _severity: caps[4].to_string(),
                code: caps[5].to_string(),
                message: caps[6].to_string(),
            });
        } else if !line.trim().is_empty() {
            // Keep summary lines and other important info
            if line.contains("error") || line.contains("warning") || line.contains("Found") {
                other_lines.push(line.to_string());
            }
        }
    }

    if errors.is_empty() {
        // No TypeScript errors found
        if output.contains("Found 0 errors") {
            return "✓ TypeScript: No errors found".to_string();
        }
        return "TypeScript compilation completed".to_string();
    }

    // Group errors by file
    let mut by_file: HashMap<String, Vec<&TsError>> = HashMap::new();
    for err in &errors {
        by_file
            .entry(err.file.clone())
            .or_default()
            .push(err);
    }

    // Group all errors by error code for global summary
    let mut by_code: HashMap<String, usize> = HashMap::new();
    for err in &errors {
        *by_code.entry(err.code.clone()).or_insert(0) += 1;
    }

    let mut result = String::new();
    result.push_str(&format!(
        "TypeScript: {} errors in {} files\n",
        errors.len(),
        by_file.len()
    ));
    result.push_str("═══════════════════════════════════════\n");

    // Show top error codes
    let mut code_counts: Vec<_> = by_code.iter().collect();
    code_counts.sort_by(|a, b| b.1.cmp(a.1));

    if code_counts.len() > 1 {
        result.push_str("Top error codes:\n");
        for (code, count) in code_counts.iter().take(5) {
            result.push_str(&format!("  {} ({}x)\n", code, count));
        }
        result.push('\n');
    }

    // Show errors grouped by file (limit to top 10 files by error count)
    let mut files_sorted: Vec<_> = by_file.iter().collect();
    files_sorted.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

    for (file, file_errors) in files_sorted.iter().take(10) {
        result.push_str(&format!("{} ({} errors)\n", file, file_errors.len()));

        // Group errors in this file by error code
        let mut file_by_code: HashMap<String, Vec<&TsError>> = HashMap::new();
        for err in *file_errors {
            file_by_code
                .entry(err.code.clone())
                .or_default()
                .push(err);
        }

        // Show grouped by error code
        let mut file_codes: Vec<_> = file_by_code.iter().collect();
        file_codes.sort_by(|a, b| b.1.len().cmp(&a.1.len()));

        for (code, code_errors) in file_codes.iter().take(3) {
            if code_errors.len() == 1 {
                let err = code_errors[0];
                result.push_str(&format!(
                    "  {} ({}:{}): {}\n",
                    err.code,
                    err.line,
                    err.col,
                    truncate(&err.message, 60)
                ));
            } else {
                result.push_str(&format!(
                    "  {} ({}x): {}\n",
                    code,
                    code_errors.len(),
                    truncate(&code_errors[0].message, 60)
                ));
            }
        }

        if file_errors.len() > 3 {
            result.push_str(&format!("  ... +{} more errors\n", file_errors.len() - 3));
        }

        result.push('\n');
    }

    if by_file.len() > 10 {
        result.push_str(&format!(
            "... ({} more files with errors)\n",
            by_file.len() - 10
        ));
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_filter_tsc_output() {
        let output = r#"
src/server/api/auth.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.
src/server/api/auth.ts(15,10): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.
src/components/Button.tsx(8,3): error TS2339: Property 'onClick' does not exist on type 'ButtonProps'.
src/components/Button.tsx(10,5): error TS2322: Type 'string' is not assignable to type 'number'.

Found 4 errors in 2 files.
"#;
        let result = filter_tsc_output(output);
        assert!(result.contains("TypeScript: 4 errors in 2 files"));
        assert!(result.contains("auth.ts (2 errors)"));
        assert!(result.contains("Button.tsx (2 errors)"));
        assert!(result.contains("TS2322"));
        assert!(!result.contains("Found 4 errors")); // Summary line should be replaced
    }

    #[test]
    fn test_filter_no_errors() {
        let output = "Found 0 errors. Watching for file changes.";
        let result = filter_tsc_output(output);
        assert!(result.contains("No errors found"));
    }
}
