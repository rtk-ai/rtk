//! Filters TypeScript compiler errors, grouping them by file and error code.

use crate::core::runner;
use crate::core::stream::{BlockHandler, BlockStreamFilter};
use crate::core::truncate::{CAP_WARNINGS, reduced};
use crate::core::utils::{MissingTool, exec_runner, strip_ansi, tool_exec, tool_exists, truncate};
use anyhow::Result;
use regex::Regex;
use std::borrow::Cow;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::LazyLock;

/// Cap on the non-empty lines kept when RTK cannot parse failure output. With
/// tee disabled (`RTK_TEE=0`, `config.tee.enabled = false`) these lines are the
/// only surviving copy of the failure.
const MAX_UNPARSED_LINES: usize = CAP_WARNINGS;
/// tsc and npx print the cause first (`Unknown compiler option`, `This is not
/// the tsc command`) and boilerplate after it, so spending the whole cap on a
/// tail would drop the cause.
const MAX_UNPARSED_HEAD_LINES: usize = reduced(MAX_UNPARSED_LINES, 5);

static TSC_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?)\((\d+),(\d+)\):\s+(error|warning)\s+(TS\d+):\s+(.+)$").unwrap()
});
/// `--pretty` layout: `file:line:col - error TSxxxx: message` (ANSI already stripped).
static TSC_PRETTY_ERROR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(.+?):(\d+):(\d+)\s+-\s+(error|warning)\s+(TS\d+):\s+(.+)$").unwrap()
});
static TSC_GLOBAL_ERROR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(error|warning)\s+(TS\d+):\s+(.+)$").unwrap());

struct Diagnostic<'a> {
    file: Option<&'a str>,
    line: Option<&'a str>,
    code: &'a str,
    message: &'a str,
}

/// Match default, `--pretty`, and file-less TypeScript diagnostics without allocating.
fn parse_diagnostic(line: &str) -> Option<Diagnostic<'_>> {
    if let Some(caps) = TSC_ERROR.captures(line) {
        return Some(Diagnostic {
            file: caps.get(1).map(|value| value.as_str()),
            line: caps.get(2).map(|value| value.as_str()),
            code: caps.get(5)?.as_str(),
            message: caps.get(6)?.as_str(),
        });
    }
    if let Some(caps) = TSC_PRETTY_ERROR.captures(line) {
        return Some(Diagnostic {
            file: caps.get(1).map(|value| value.as_str()),
            line: caps.get(2).map(|value| value.as_str()),
            code: caps.get(5)?.as_str(),
            message: caps.get(6)?.as_str(),
        });
    }
    let caps = TSC_GLOBAL_ERROR.captures(line)?;
    Some(Diagnostic {
        file: None,
        line: None,
        code: caps.get(2)?.as_str(),
        message: caps.get(3)?.as_str(),
    })
}

fn push_dump_line(summary: &mut String, line: &str) {
    summary.push_str(&truncate(line, 120));
    summary.push('\n');
}

fn clean_line(line: &str) -> Cow<'_, str> {
    if line.contains('\x1b') {
        Cow::Owned(strip_ansi(line))
    } else {
        Cow::Borrowed(line)
    }
}

/// `runner` is the package runner the user named (`bunx tsc`, `npx tsc`), or
/// None for a bare `rtk tsc` where nothing was specified and detection applies.
pub fn run(runner: Option<&str>, args: &[String], verbose: u8) -> Result<i32> {
    let tsc_exists = tool_exists("tsc");

    // Fetch, not Fail: `npx tsc` fetched before this routing existed, and
    // rtk filters output rather than changing what a command does.
    let mut cmd = tool_exec(runner, "tsc", MissingTool::Fetch);

    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        let via = if tsc_exists {
            "tsc".to_string()
        } else {
            format!("{} tsc", exec_runner(runner, MissingTool::Fetch))
        };
        eprintln!("Running: {} {}", via, args.join(" "));
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
    /// `--pretty` wraps every field in ANSI escapes; strip them once so
    /// matching and the emitted block both see plain text.
    fn normalize_line<'a>(&self, line: &'a str) -> Cow<'a, str> {
        clean_line(line)
    }

    fn should_skip(&mut self, line: &str) -> bool {
        line.starts_with("Found ")
    }

    fn is_block_start(&mut self, line: &str) -> bool {
        if let Some(diagnostic) = parse_diagnostic(line) {
            self.error_count += 1;
            if let Some(file) = diagnostic.file {
                self.files.insert(file.to_string());
            }
            *self
                .code_counts
                .entry(diagnostic.code.to_string())
                .or_insert(0) += 1;
            true
        } else {
            false
        }
    }

    fn is_block_continuation(&mut self, line: &str, _block: &[String]) -> bool {
        line.starts_with("  ") || line.starts_with('\t')
    }

    fn format_summary(&self, exit_code: i32, raw: &str) -> Option<String> {
        if self.error_count == 0 {
            if exit_code == 0 {
                return Some("TypeScript: No errors found\n".to_string());
            }
            // tsc failed without one diagnostic RTK understands (missing binary,
            // no project, unparseable output). "No errors found" would be a
            // false green, so surface the failure with both ends of the raw
            // output.
            let mut summary = format!(
                "TypeScript: compiler exited with code {exit_code}, but RTK parsed no diagnostics\n"
            );

            if raw.len() < crate::core::tee::MIN_TEE_SIZE {
                for line in raw.lines() {
                    let line = clean_line(line);
                    if line.trim().is_empty() {
                        continue;
                    }
                    push_dump_line(&mut summary, line.as_ref());
                }
                return Some(summary);
            }

            let head_len = MAX_UNPARSED_HEAD_LINES.min(MAX_UNPARSED_LINES);
            let tail_len = MAX_UNPARSED_LINES.saturating_sub(head_len);
            let mut head: Vec<Cow<'_, str>> = Vec::with_capacity(head_len);
            let mut tail: VecDeque<Cow<'_, str>> = VecDeque::with_capacity(tail_len);
            let mut total = 0;

            for line in raw.lines() {
                let line = clean_line(line);
                if line.trim().is_empty() {
                    continue;
                }
                total += 1;
                if head.len() < head_len {
                    head.push(line);
                    continue;
                }
                if tail_len == 0 {
                    continue;
                }
                if tail.len() == tail_len {
                    tail.pop_front();
                }
                tail.push_back(line);
            }

            let hidden = total - head.len() - tail.len();
            for line in head {
                push_dump_line(&mut summary, line.as_ref());
            }
            if hidden > 0 {
                summary.push_str(&format!("... +{hidden} more lines\n"));
            }
            for line in tail {
                push_dump_line(&mut summary, line.as_ref());
            }
            return Some(summary);
        }

        let mut result = if self.files.is_empty() {
            format!("TypeScript: {} errors\n", self.error_count)
        } else {
            format!(
                "TypeScript: {} errors in {} files\n",
                self.error_count,
                self.files.len()
            )
        };

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
    struct TsError {
        file: Option<String>,
        line: Option<usize>,
        code: String,
        message: String,
        context_lines: Vec<String>,
    }

    let mut errors: Vec<TsError> = Vec::new();
    let clean_output = clean_line(output);
    let lines: Vec<&str> = clean_output.lines().collect();
    let mut i = 0;

    while i < lines.len() {
        let line = lines[i];
        if let Some(diagnostic) = parse_diagnostic(line) {
            let mut err = TsError {
                file: diagnostic.file.map(str::to_string),
                line: diagnostic.line.and_then(|value| value.parse().ok()),
                code: diagnostic.code.to_string(),
                message: diagnostic.message.to_string(),
                context_lines: Vec::new(),
            };

            // Capture continuation lines (indented context from tsc)
            i += 1;
            while i < lines.len() {
                let next = lines[i];
                if !next.is_empty()
                    && (next.starts_with("  ") || next.starts_with('\t'))
                    && parse_diagnostic(next).is_none()
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
        if clean_output.contains("Found 0 errors") {
            return "TypeScript: No errors found".to_string();
        }
        return "TypeScript compilation completed".to_string();
    }

    let global_errors: Vec<&TsError> = errors.iter().filter(|err| err.file.is_none()).collect();

    // Group file-scoped diagnostics without counting the global bucket as a file.
    let mut by_file: HashMap<String, Vec<&TsError>> = HashMap::new();
    for err in &errors {
        if let Some(file) = &err.file {
            by_file.entry(file.clone()).or_default().push(err);
        }
    }

    // Count by error code for summary
    let mut by_code: HashMap<String, usize> = HashMap::new();
    for err in &errors {
        *by_code.entry(err.code.clone()).or_insert(0) += 1;
    }

    let mut result = if by_file.is_empty() {
        format!("TypeScript: {} errors\n", errors.len())
    } else {
        format!(
            "TypeScript: {} errors in {} files\n",
            errors.len(),
            by_file.len()
        )
    };

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

    if !global_errors.is_empty() {
        result.push_str(&format!("global ({} errors)\n", global_errors.len()));
        for err in global_errors {
            result.push_str(&format!("  {} {}\n", err.code, truncate(&err.message, 120)));
            for ctx in &err.context_lines {
                result.push_str(&format!("    {}\n", truncate(ctx, 120)));
            }
        }
        result.push('\n');
    }

    // Files sorted by error count (most errors first)
    let mut files_sorted: Vec<_> = by_file.iter().collect();
    files_sorted.sort_by_key(|b| std::cmp::Reverse(b.1.len()));

    // Show every error per file — no limits
    for (file, file_errors) in &files_sorted {
        result.push_str(&format!("{} ({} errors)\n", file, file_errors.len()));

        for err in *file_errors {
            result.push_str(&format!(
                "  L{}: {} {}\n",
                err.line.unwrap_or(0),
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
    fn test_filter_tsc_output_pretty_ansi_error() {
        let output = include_str!("../../../tests/fixtures/tsc_pretty_raw.txt");
        let result = filter_tsc_output(output);
        assert!(result.contains("TypeScript: 3 errors in 1 files"));
        assert!(result.contains("src/index.ts (3 errors)"));
        assert!(result.contains("L1:"));
        assert_eq!(result.matches("L4:").count(), 2);
        assert!(result.contains("TS2322"));
        assert!(!result.contains("\x1b["));
    }

    #[test]
    fn test_filter_tsc_output_global_error() {
        let result =
            filter_tsc_output("error TS5058: The specified path does not exist: 'tsconfig.json'.");
        assert!(result.contains("TS5058"));
        assert!(result.contains("global (1 errors)"));
        assert!(!result.contains("completed"));
    }

    #[test]
    fn test_filter_tsc_output_global_error_continuation() {
        let output = include_str!("../../../tests/fixtures/tsc_global_config_error_raw.txt");
        let result = filter_tsc_output(output);
        assert!(result.contains("TS2688"));
        assert!(result.contains("The file is in the program because:"));
        assert!(result.contains("global (1 errors)"));
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

    #[test]
    fn test_filter_no_errors_with_ansi() {
        let output = "\x1b[32mFound 0 errors.\x1b[0m Watching for file changes.";
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
    fn test_tsc_stream_pretty_ansi_errors() {
        let input = include_str!("../../../tests/fixtures/tsc_pretty_raw.txt");
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, input, 2);
        assert!(
            result.contains("src/index.ts:1:7 - error TS2322:"),
            "got: {}",
            result
        );
        assert!(
            result.contains("src/index.ts:4:8 - error TS2322:"),
            "got: {}",
            result
        );
        assert!(
            result.contains("src/index.ts:4:16 - error TS2322:"),
            "got: {}",
            result
        );
        assert!(result.contains("3 errors in 1 files"), "got: {}", result);
        assert!(!result.contains("Found 3 errors"), "got: {}", result);
        assert!(
            !result.contains("\x1b["),
            "emitted block keeps escapes: {}",
            result
        );
        // Real pretty output separates code frames with a blank line; the compact filter drops them.
        assert!(!result.contains("const x: number"), "got: {}", result);
        assert!(
            !result.contains("The expected type comes from"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_tsc_stream_global_error_with_continuation() {
        let input = include_str!("../../../tests/fixtures/tsc_global_config_error_raw.txt");
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, input, 2);
        assert!(result.contains("TS2688"), "got: {}", result);
        assert!(
            result.contains("The file is in the program because:"),
            "got: {}",
            result
        );
        assert_eq!(result.lines().last(), Some("TypeScript: 1 errors"));
        assert!(!result.contains("parsed no diagnostics"), "got: {}", result);
        assert!(!result.contains("No errors found"), "got: {}", result);
    }

    #[test]
    fn test_tsc_stream_mixed_global_and_file_errors() {
        let input = "\
error TS5023: Unknown compiler option 'foo'.
src/app.ts(1,7): error TS2322: Type 'string' is not assignable to type 'number'.
";
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, input, 2);
        assert!(result.contains("TS5023"), "got: {}", result);
        assert!(result.contains("TS2322"), "got: {}", result);
        assert!(result.contains("2 errors in 1 files"), "got: {}", result);
    }

    #[test]
    fn test_tsc_stream_pretty_global_error() {
        let input = "\x1b[91merror\x1b[0m\x1b[90m TS5058: \x1b[0mThe specified path does not exist: 'nope.json'.";
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, input, 1);
        assert!(result.contains("error TS5058:"), "got: {}", result);
        assert!(result.contains("1 errors"), "got: {}", result);
        assert!(!result.contains("\x1b["), "got: {}", result);
    }

    #[test]
    fn test_tsc_stream_failed_unparsed_output_keeps_head_and_tail() {
        let padding = "x".repeat(30);
        let input: String = (1..=30)
            .map(|i| format!("junk line {i} {padding}\n"))
            .collect();
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, &input, 2);
        assert!(
            result.contains("compiler exited with code 2"),
            "got: {}",
            result
        );
        for i in 1..=5 {
            assert!(
                result.contains(&format!("junk line {i} {padding}\n")),
                "got: {}",
                result
            );
        }
        assert!(result.contains("... +20 more lines"), "got: {}", result);
        for i in 26..=30 {
            assert!(
                result.contains(&format!("junk line {i} {padding}\n")),
                "got: {}",
                result
            );
        }
        assert!(
            !result.contains(&format!("junk line 6 {padding}\n")),
            "got: {}",
            result
        );
        assert!(
            !result.contains(&format!("junk line 25 {padding}\n")),
            "got: {}",
            result
        );
        assert_eq!(result.lines().count(), 1 + 5 + 1 + 5);
    }

    #[test]
    fn test_tsc_stream_failed_unparsed_output_ignores_escape_only_lines() {
        let padding = "x".repeat(30);
        let mut input = "\x1b[0m\n\x1b[32m\x1b[0m\n\x1b[0m\n".to_string();
        for i in 1..=30 {
            input.push_str(&format!("real line {i} {padding}\n"));
        }

        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, &input, 2);
        for i in 1..=5 {
            assert!(
                result.contains(&format!("real line {i} {padding}\n")),
                "got: {}",
                result
            );
        }
        assert!(result.contains("... +20 more lines"), "got: {}", result);
        for i in 26..=30 {
            assert!(
                result.contains(&format!("real line {i} {padding}\n")),
                "got: {}",
                result
            );
        }
        assert!(!result.contains("\n\n"), "got: {}", result);
        assert_eq!(result.lines().count(), 1 + 5 + 1 + 5);
    }

    #[test]
    fn test_tsc_stream_failed_unparsed_output_under_tee_floor_is_complete() {
        let input: String = (1..=15).map(|i| format!("short {i}\n")).collect();
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, &input, 1);

        for i in 1..=15 {
            assert!(result.contains(&format!("short {i}\n")), "got: {}", result);
        }
        assert!(!result.contains("... +"), "got: {}", result);
    }

    #[test]
    fn test_tsc_stream_failed_unparsed_output_caps_line_width() {
        let long_line = "z".repeat(300);
        let padding = "x".repeat(30);
        let mut input = format!("{long_line}\n");
        for i in 1..=10 {
            input.push_str(&format!("padding line {i} {padding}\n"));
        }

        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, &input, 1);
        let emitted_long_line = result.lines().nth(1).expect("long line should be emitted");
        assert!(emitted_long_line.chars().count() <= 120, "got: {}", result);
        assert!(emitted_long_line.ends_with("..."), "got: {}", result);
    }

    #[test]
    fn test_tsc_stream_failed_without_project_keeps_cause() {
        let input = include_str!("../../../tests/fixtures/tsc_no_project_raw.txt");
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, input, 1);
        assert!(
            result.contains("compiler exited with code 1"),
            "got: {}",
            result
        );
        assert!(result.contains("Version 6.0.3"), "got: {}", result);
        assert!(
            result.contains("tsc: The TypeScript Compiler"),
            "got: {}",
            result
        );
        assert!(result.contains("... +"), "got: {}", result);
    }

    #[test]
    fn test_tsc_stream_failed_unparsed_output_not_success() {
        let input = "\
\x1b[31mThis is not the tsc command you are looking for\x1b[0m
Use npm install typescript before using npx
";
        let mut f = BlockStreamFilter::new(TscHandler::new());
        let result = run_block_filter(&mut f, input, 1);
        assert!(
            result.contains("compiler exited with code 1"),
            "got: {}",
            result
        );
        assert!(
            result.contains("This is not the tsc command you are looking for"),
            "got: {}",
            result
        );
        assert!(!result.contains("No errors found"), "got: {}", result);
        assert!(!result.contains("\x1b["), "got: {}", result);
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
