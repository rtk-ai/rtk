//! Filters TypeScript compiler errors, grouping them by file and error code.

use crate::core::runner;
use crate::core::stream::{BlockHandler, BlockStreamFilter};
use crate::core::utils::{resolved_command, tool_exists, truncate};
use anyhow::Result;
use regex::Regex;
use std::collections::{HashMap, HashSet};

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let tsc_exists = tool_exists("tsc");

    let mut cmd = if tsc_exists {
        resolved_command("tsc")
    } else {
        let mut c = resolved_command("npx");
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

    runner::run_streamed(
        cmd,
        "tsc",
        &args.join(" "),
        Box::new(BlockStreamFilter::new(TscHandler::new())),
        runner::RunOptions::with_tee("tsc"),
    )
}

struct TscHandler {
    error_count: usize,
    files: HashSet<String>,
    code_counts: HashMap<String, usize>,
}

impl TscHandler {
    fn new() -> Self {
        Self {
            error_count: 0,
            files: HashSet::new(),
            code_counts: HashMap::new(),
        }
    }
}

impl BlockHandler for TscHandler {
    fn should_skip(&mut self, line: &str) -> bool {
        line.starts_with("Found ")
    }

    fn is_block_start(&mut self, line: &str) -> bool {
        lazy_static::lazy_static! {
            static ref TSC_ERROR: Regex = Regex::new(
                r"^(.+?)\((\d+),(\d+)\):\s+(error|warning)\s+(TS\d+):\s+(.+)$"
            ).unwrap();
        }
        if let Some(caps) = TSC_ERROR.captures(line) {
            self.error_count += 1;
            self.files.insert(caps[1].to_string());
            *self.code_counts.entry(caps[5].to_string()).or_insert(0) += 1;
            true
        } else {
            false
        }
    }

    fn is_block_continuation(&mut self, line: &str, _block: &[String]) -> bool {
        line.starts_with("  ") || line.starts_with('\t')
    }

    fn format_summary(&self, _exit_code: i32, _raw: &str) -> Option<String> {
        if self.error_count == 0 {
            return Some("TypeScript: No errors found\n".to_string());
        }

        let mut result = format!(
            "═══════════════════════════════════════\nTypeScript: {} errors in {} files\n",
            self.error_count,
            self.files.len()
        );

        if self.code_counts.len() > 1 {
            let mut counts: Vec<_> = self.code_counts.iter().collect();
            counts.sort_by(|a, b| b.1.cmp(a.1));
            let codes_str: Vec<String> = counts
                .iter()
                .take(5)
                .map(|(code, count)| format!("{} ({}x)", code, count))
                .collect();
            result.push_str(&format!("Top codes: {}\n", codes_str.join(", ")));
        }

        Some(result)
    }
}

pub(crate) fn filter_tsc_output(output: &str) -> String {
    lazy_static::lazy_static! {
        // Pattern: src/file.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.
        static ref TSC_ERROR: Regex = Regex::new(
            r"^(.+?)\((\d+),(\d+)\):\s+(error|warning)\s+(TS\d+):\s+(.+)$"
        ).unwrap();
    }

    struct TsError {
        file: String,
        line: usize,
        code: String,
        message: String,
        context_lines: Vec<String>,
    }

    let mut errors: Vec<TsError> = Vec::new();
    let lines: Vec<&str> = output.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if let Some(caps) = TSC_ERROR.captures(line) {
            let mut err = TsError {
                file: caps[1].to_string(),
                line: caps[2].parse().unwrap_or(0),
                code: caps[5].to_string(),
                message: caps[6].to_string(),
                context_lines: Vec::new(),
            };

            // Capture continuation lines (indented context from tsc)
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                if !next.is_empty()
                    && (next.starts_with("  ") || next.starts_with('\t'))
                    && !TSC_ERROR.is_match(next)
                {
                    err.context_lines.push(next.trim().to_string());
                    i += 1;
                } else {
                    break;
                }
            }

            errors.push(err);
        } else {
            i += 1;
        }
    }

    if errors.is_empty() {
        if output.contains("Found 0 errors") {
            return "TypeScript: No errors found".to_string();
        }
        return "TypeScript compilation completed".to_string();
    }

    // Group by file
    let mut by_file: HashMap<String, Vec<&TsError>> = HashMap::new();
    for err in &errors {
        by_file.entry(err.file.clone()).or_default().push(err);
    }

    // Count by error code for summary
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

    // Top error codes summary (compact, one line)
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

    // Show every error per file — no limits
    for (file, file_errors) in &files_sorted {
        result.push_str(&format!("{} ({} errors)\n", file, file_errors.len()));

        for err in *file_errors {
            result.push_str(&format!(
                "  L{}: {} {}\n",
                err.line,
                err.code,
                truncate(&err.message, 120)
            ));
            for ctx in &err.context_lines {
                result.push_str(&format!("    {}\n", truncate(ctx, 120)));
            }
        }
        result.push('\n');
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
    fn test_every_error_message_shown() {
        let output = "\
src/api.ts(10,5): error TS2322: Type 'string' is not assignable to type 'number'.
src/api.ts(20,5): error TS2322: Type 'boolean' is not assignable to type 'string'.
src/api.ts(30,5): error TS2322: Type 'null' is not assignable to type 'object'.
";
        let result = filter_tsc_output(output);
        // Each error message must be individually visible, not collapsed
        assert!(result.contains("Type 'string' is not assignable to type 'number'"));
        assert!(result.contains("Type 'boolean' is not assignable to type 'string'"));
        assert!(result.contains("Type 'null' is not assignable to type 'object'"));
        assert!(result.contains("L10:"));
        assert!(result.contains("L20:"));
        assert!(result.contains("L30:"));
    }

    #[test]
    fn test_continuation_lines_preserved() {
        let output = "\
src/app.tsx(10,3): error TS2322: Type '{ children: Element; }' is not assignable to type 'Props'.
  Property 'children' does not exist on type 'Props'.
src/app.tsx(20,5): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.
";
        let result = filter_tsc_output(output);
        assert!(result.contains("Property 'children' does not exist on type 'Props'"));
        assert!(result.contains("L10:"));
        assert!(result.contains("L20:"));
    }

    #[test]
    fn test_no_file_limit() {
        // 15 files with errors — all must appear
        let mut output = String::new();
        for i in 1..=15 {
            output.push_str(&format!(
                "src/file{}.ts({},1): error TS2322: Error in file {}.\n",
                i, i, i
            ));
        }
        let result = filter_tsc_output(&output);
        assert!(result.contains("15 errors in 15 files"));
        for i in 1..=15 {
            assert!(
                result.contains(&format!("file{}.ts", i)),
                "file{}.ts missing from output",
                i
            );
        }
    }

    #[test]
    fn test_filter_no_errors() {
        let output = "Found 0 errors. Watching for file changes.";
        let result = filter_tsc_output(output);
        assert!(result.contains("No errors found"));
    }

    // --- Streaming handler tests ---

    use crate::core::stream::tests::run_block_filter;

    #[test]
    fn test_tsc_stream_errors() {
        let input = "\
src/server/api/auth.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.
src/server/api/auth.ts(15,10): error TS2345: Argument of type 'number' is not assignable to parameter of type 'string'.
src/components/Button.tsx(8,3): error TS2339: Property 'onClick' does not exist on type 'ButtonProps'.

Found 3 errors in 2 files.
";
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, input, 1);
        assert!(result.contains("TS2322"), "got: {}", result);
        assert!(result.contains("TS2345"), "got: {}", result);
        assert!(result.contains("3 errors in 2 files"), "got: {}", result);
        assert!(!result.contains("Found 3"), "got: {}", result);
    }

    #[test]
    fn test_tsc_stream_no_errors() {
        let input = "Found 0 errors. Watching for file changes.\n";
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, input, 0);
        assert!(result.contains("No errors found"), "got: {}", result);
    }

    #[test]
    fn test_tsc_stream_continuation_lines() {
        let input = "\
src/app.tsx(10,3): error TS2322: Type '{ children: Element; }' is not assignable to type 'Props'.
  Property 'children' does not exist on type 'Props'.
src/app.tsx(20,5): error TS2345: Argument of type 'number' is not assignable.
";
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, input, 1);
        assert!(
            result.contains("Property 'children' does not exist"),
            "got: {}",
            result
        );
        assert!(result.contains("TS2322"), "got: {}", result);
        assert!(result.contains("TS2345"), "got: {}", result);
    }
}
