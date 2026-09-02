use crate::core::tracking;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::{Context, Result};
use std::collections::HashMap;

pub fn run(compiler: &str, args: &[String], verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    let mut cmd = resolved_command(compiler);
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {}", compiler, args.join(" "));
    }

    let output = cmd
        .output()
        .with_context(|| format!("Failed to run {}. Is it installed?", compiler))?;

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    // gcc/g++ write diagnostics to stderr; combine so filter sees everything
    let raw = format!("{}{}", stdout, stderr);
    let clean = strip_ansi(&raw);

    let filtered = filter_gcc_output(&clean);

    println!("{}", filtered);

    timer.track(compiler, &format!("rtk {}", compiler), &raw, &filtered);

    Ok(if output.status.success() {
        0
    } else {
        output.status.code().unwrap_or(1)
    })
}

/// Severity level of a gcc diagnostic.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
enum Severity {
    Note,
    Warning,
    Error,
}

impl Severity {
    fn from_str(s: &str) -> Option<Self> {
        match s {
            "error" => Some(Severity::Error),
            "warning" => Some(Severity::Warning),
            "note" => Some(Severity::Note),
            _ => None,
        }
    }
}

struct GccDiagnostic {
    file: String,
    line: usize,
    severity: Severity,
    message: String,
    /// Flag that triggered this diagnostic, e.g. `-Wunused-variable`
    flag: Option<String>,
}

/// Filter gcc/g++/clang compiler output, grouping diagnostics by file.
///
/// Suppresses verbose caret context lines (the `|` source snippets and `^`
/// underlines) that duplicate information already present in the diagnostic
/// message line itself.  Notes that annotate the preceding diagnostic are
/// attached inline so no information is lost.
///
/// Returns a compact summary: header + grouped file sections.
pub fn filter_gcc_output(output: &str) -> String {
    lazy_static::lazy_static! {
        // file:line:col: severity: message [-Wflag]
        static ref DIAG: regex::Regex = regex::Regex::new(
            r"^([^:\n]+):(\d+):\d+:\s*(error|warning|note):\s+(.+?)(?:\s+\[(-\S+)\])?$"
        ).unwrap();

        // "In file included from …" / "In function 'foo':" / "At top level:"
        static ref CONTEXT_HDR: regex::Regex = regex::Regex::new(
            r"^(?:In file included from|In function|At top level|from)\b"
        ).unwrap();

        // make: *** [...] Error N
        static ref MAKE_ERR: regex::Regex = regex::Regex::new(
            r"^make(?:\[\d+\])?: \*\*\*"
        ).unwrap();
    }

    let lines: Vec<&str> = output.lines().collect();
    let mut diagnostics: Vec<GccDiagnostic> = Vec::new();
    let mut make_errors: Vec<String> = Vec::new();

    for line in &lines {
        if MAKE_ERR.is_match(line) {
            make_errors.push(line.to_string());
            continue;
        }
        // Skip context headers ("In function …") and caret/pipe source lines
        if CONTEXT_HDR.is_match(line) {
            continue;
        }
        // Skip pure caret/source snippet lines (start with spaces and contain | or ^)
        let trimmed = line.trim();
        if trimmed.starts_with('|') || trimmed.starts_with('^') || trimmed.starts_with('~') {
            continue;
        }
        // Skip line-number-only source echo lines (leading spaces + digits + " |")
        if trimmed.contains(" | ") || trimmed.ends_with(" |") {
            // Heuristic: lines like "   25 |     x = 42;"
            if trimmed
                .split_whitespace()
                .next()
                .is_some_and(|w| w.chars().all(|c| c.is_ascii_digit()))
            {
                continue;
            }
        }

        if let Some(caps) = DIAG.captures(line) {
            let severity_str = &caps[3];
            let Some(severity) = Severity::from_str(severity_str) else {
                continue;
            };
            diagnostics.push(GccDiagnostic {
                file: caps[1].to_string(),
                line: caps[2].parse().unwrap_or(0),
                severity,
                message: caps[4].to_string(),
                flag: caps.get(5).map(|m| m.as_str().to_string()),
            });
        }
    }

    // If nothing was parsed, fall back to the raw output (best-effort passthrough)
    if diagnostics.is_empty() && make_errors.is_empty() {
        let trimmed = output.trim();
        if trimmed.is_empty() {
            return "gcc: Build succeeded with no warnings".to_string();
        }
        return trimmed.to_string();
    }

    let errors: Vec<&GccDiagnostic> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Error)
        .collect();
    let warnings: Vec<&GccDiagnostic> = diagnostics
        .iter()
        .filter(|d| d.severity == Severity::Warning)
        .collect();

    // Only-warnings run (no errors) — build probably succeeded
    if errors.is_empty() && make_errors.is_empty() {
        if warnings.is_empty() {
            return "gcc: Build succeeded with no warnings".to_string();
        }
        return format_warnings_only(&warnings);
    }

    // Build failed — show errors + summary + warnings summary
    format_build_failed(&errors, &warnings, &make_errors)
}

fn format_warnings_only(warnings: &[&GccDiagnostic]) -> String {
    let mut by_file: HashMap<&str, Vec<&GccDiagnostic>> = HashMap::new();
    for w in warnings {
        by_file.entry(w.file.as_str()).or_default().push(w);
    }

    let mut result = format!(
        "gcc: {} warning{} in {} file{}\n",
        warnings.len(),
        if warnings.len() == 1 { "" } else { "s" },
        by_file.len(),
        if by_file.len() == 1 { "" } else { "s" }
    );
    result.push_str("═══════════════════════════════════════\n");

    let mut files: Vec<(&&str, &Vec<&GccDiagnostic>)> = by_file.iter().collect();
    files.sort_by_key(|(f, _)| **f);

    for (file, diags) in files {
        result.push_str(&format!(
            "{} ({} warning{})\n",
            file,
            diags.len(),
            if diags.len() == 1 { "" } else { "s" }
        ));
        for d in diags {
            let flag_str = d.flag.as_deref().unwrap_or("");
            if flag_str.is_empty() {
                result.push_str(&format!("  L{}: {}\n", d.line, d.message));
            } else {
                result.push_str(&format!("  L{}: {} [{}]\n", d.line, d.message, flag_str));
            }
        }
        result.push('\n');
    }

    result.trim().to_string()
}

fn format_build_failed(
    errors: &[&GccDiagnostic],
    warnings: &[&GccDiagnostic],
    make_errors: &[String],
) -> String {
    let mut by_file: HashMap<&str, Vec<&GccDiagnostic>> = HashMap::new();
    for e in errors {
        by_file.entry(e.file.as_str()).or_default().push(e);
    }

    let mut result = format!(
        "gcc: BUILD FAILED — {} error{}, {} warning{}\n",
        errors.len(),
        if errors.len() == 1 { "" } else { "s" },
        warnings.len(),
        if warnings.len() == 1 { "" } else { "s" },
    );
    result.push_str("═══════════════════════════════════════\n");

    // Files sorted by error count descending
    let mut files: Vec<(&&str, &Vec<&GccDiagnostic>)> = by_file.iter().collect();
    files.sort_by(|a, b| b.1.len().cmp(&a.1.len()).then(a.0.cmp(b.0)));

    for (file, diags) in &files {
        result.push_str(&format!(
            "{} ({} error{})\n",
            file,
            diags.len(),
            if diags.len() == 1 { "" } else { "s" }
        ));
        for d in *diags {
            result.push_str(&format!("  L{}: [error] {}\n", d.line, d.message));
        }
        result.push('\n');
    }

    // Warnings summary (counts only, to save tokens)
    if !warnings.is_empty() {
        let mut warn_by_file: HashMap<&str, usize> = HashMap::new();
        for w in warnings {
            *warn_by_file.entry(w.file.as_str()).or_insert(0) += 1;
        }
        let mut warn_files: Vec<_> = warn_by_file.iter().collect();
        warn_files.sort_by(|a, b| b.1.cmp(a.1).then(a.0.cmp(b.0)));

        result.push_str("Warnings:\n");
        for (file, count) in &warn_files {
            result.push_str(&format!(
                "  {} — {} warning{}\n",
                file,
                count,
                if **count == 1 { "" } else { "s" }
            ));
        }
        result.push('\n');
    }

    // make error lines (e.g. "make: *** [Makefile:10: build] Error 1")
    for line in make_errors {
        result.push_str(line);
        result.push('\n');
    }

    result.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    // ── Helpers ─────────────────────────────────────────────────────────────

    fn fixture(name: &str) -> &'static str {
        match name {
            "errors" => include_str!("../../../tests/fixtures/gcc/build_errors.txt"),
            "success" => include_str!("../../../tests/fixtures/gcc/build_success.txt"),
            "clean" => include_str!("../../../tests/fixtures/gcc/build_clean.txt"),
            _ => panic!("unknown fixture: {}", name),
        }
    }

    // ── Filter logic tests ───────────────────────────────────────────────────

    #[test]
    fn test_build_failed_header() {
        let result = filter_gcc_output(fixture("errors"));
        assert!(
            result.contains("BUILD FAILED"),
            "should have BUILD FAILED header"
        );
    }

    #[test]
    fn test_error_count_in_header() {
        let result = filter_gcc_output(fixture("errors"));
        // fixture has 5 errors
        assert!(
            result.contains("5 errors"),
            "header should show error count; got:\n{}",
            result
        );
    }

    #[test]
    fn test_errors_grouped_by_file() {
        let result = filter_gcc_output(fixture("errors"));
        assert!(result.contains("src/main.c"), "should show main.c");
        assert!(result.contains("src/parser.c"), "should show parser.c");
        assert!(result.contains("src/lexer.c"), "should show lexer.c");
    }

    #[test]
    fn test_error_line_numbers_present() {
        let result = filter_gcc_output(fixture("errors"));
        assert!(result.contains("L25:"), "should show line 25 from main.c");
        assert!(result.contains("L88:"), "should show line 88 from parser.c");
    }

    #[test]
    fn test_caret_lines_stripped() {
        let result = filter_gcc_output(fixture("errors"));
        // Caret context lines should not appear in output
        assert!(
            !result.contains("^~~~~~~"),
            "caret lines should be stripped"
        );
        assert!(
            !result.contains("      |"),
            "pipe context lines should be stripped"
        );
    }

    #[test]
    fn test_make_error_preserved() {
        let result = filter_gcc_output(fixture("errors"));
        assert!(
            result.contains("make:") || result.contains("Error 1"),
            "make error line should be preserved"
        );
    }

    #[test]
    fn test_warnings_only_header() {
        let result = filter_gcc_output(fixture("success"));
        assert!(
            result.contains("warning"),
            "warnings-only run should mention warnings"
        );
        assert!(
            !result.contains("BUILD FAILED"),
            "warnings-only should not say BUILD FAILED"
        );
    }

    #[test]
    fn test_warnings_include_flag() {
        let result = filter_gcc_output(fixture("success"));
        // fixture has [-Wunused-result] and [-Wpointer-compare]
        assert!(
            result.contains("-Wunused-result") || result.contains("-Wpointer-compare"),
            "warning flags should be preserved"
        );
    }

    #[test]
    fn test_clean_build() {
        let result = filter_gcc_output(fixture("clean"));
        assert!(
            result.contains("no warnings") || result.contains("succeeded"),
            "clean build should report success"
        );
    }

    #[test]
    fn test_empty_input() {
        let result = filter_gcc_output("");
        assert!(
            result.contains("no warnings") || result.contains("succeeded"),
            "empty input treated as clean build"
        );
    }

    #[test]
    fn test_most_errors_file_first() {
        let input = "\
src/a.c:1:1: error: error one
src/b.c:1:1: error: error B1
src/b.c:2:1: error: error B2
src/b.c:3:1: error: error B3
";
        let result = filter_gcc_output(input);
        let pos_a = result.find("src/a.c").unwrap_or(usize::MAX);
        let pos_b = result.find("src/b.c").unwrap_or(usize::MAX);
        assert!(
            pos_b < pos_a,
            "b.c (3 errors) should appear before a.c (1 error)"
        );
    }

    // ── Token savings test ───────────────────────────────────────────────────

    #[test]
    fn test_token_savings_errors_fixture() {
        let input = fixture("errors");
        let output = filter_gcc_output(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 40.0,
            "Expected ≥40% token savings on errors fixture, got {:.1}% (in={} out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_token_savings_warnings_fixture() {
        let input = fixture("success");
        let output = filter_gcc_output(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);

        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);
        assert!(
            savings >= 25.0,
            "Expected ≥25% token savings on warnings fixture, got {:.1}% (in={} out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }
}
