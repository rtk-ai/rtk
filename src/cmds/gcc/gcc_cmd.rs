use crate::core::utils::{resolved_command, strip_ansi};
use crate::core::{runner, tracking};
use anyhow::{Context, Result};
use regex::Regex;
use std::collections::HashMap;
use std::ffi::OsString;
use std::sync::LazyLock;

// file:line:col: severity: message [-Wflag]
static DIAG: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"^([^:\n]+):(\d+):\d+:\s*(fatal error|error|warning|note):\s+(.+?)(?:\s+\[(-\S+)\])?$",
    )
    .unwrap()
});

// "In file included from …" / "In function 'foo':" / "At top level:"
static CONTEXT_HDR: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:In file included from|In function|At top level|from)\b").unwrap()
});

// Driver/tool errors without a source location:
// "cc1: fatal error: x.c: No such file or directory", "gcc: error: …"
static TOOL_ERR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^[^\s:]+: (?:fatal error|error): ").unwrap());

// make: *** [...] Error N
static MAKE_ERR: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^make(?:\[\d+\])?: \*\*\*").unwrap());

/// Flags that make the compiler write its real output (preprocessed source,
/// dependency rules, `-o -`) to stdout, which the diagnostic summary would replace.
fn writes_to_stdout(args: &[String]) -> bool {
    args.iter()
        .any(|a| matches!(a.as_str(), "-E" | "-M" | "-MM" | "-o-"))
        || args.windows(2).any(|w| w[0] == "-o" && w[1] == "-")
}

pub fn run(compiler: &str, args: &[String], verbose: u8) -> Result<i32> {
    if writes_to_stdout(args) {
        let os_args: Vec<OsString> = args.iter().map(OsString::from).collect();
        return runner::run_passthrough(compiler, &os_args, verbose);
    }

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
            "error" | "fatal error" => Some(Severity::Error),
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
    /// Notes that follow this diagnostic (candidates, fix-it hints)
    notes: Vec<String>,
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
    let lines: Vec<&str> = output.lines().collect();
    let mut diagnostics: Vec<GccDiagnostic> = Vec::new();
    let mut make_errors: Vec<String> = Vec::new();
    let mut tool_errors: Vec<String> = Vec::new();

    for line in &lines {
        if MAKE_ERR.is_match(line) {
            make_errors.push(line.to_string());
            continue;
        }
        if TOOL_ERR.is_match(line) {
            tool_errors.push(line.to_string());
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
            let file = &caps[1];
            let line_no: usize = caps[2].parse().unwrap_or(0);
            if severity == Severity::Note {
                // Attach to the diagnostic it explains; drop orphan notes
                if let Some(parent) = diagnostics.last_mut() {
                    let note = if parent.file == file {
                        format!("L{}: {}", line_no, &caps[4])
                    } else {
                        format!("{}:{}: {}", file, line_no, &caps[4])
                    };
                    parent.notes.push(note);
                }
                continue;
            }
            diagnostics.push(GccDiagnostic {
                file: file.to_string(),
                line: line_no,
                severity,
                message: caps[4].to_string(),
                flag: caps.get(5).map(|m| m.as_str().to_string()),
                notes: Vec::new(),
            });
        }
    }

    // No located diagnostic (linker, driver or make output): pass through raw,
    // since the summary below would drop the lines that explain the failure
    if diagnostics.is_empty() {
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
    if errors.is_empty() && tool_errors.is_empty() && make_errors.is_empty() {
        if warnings.is_empty() {
            return "gcc: Build succeeded with no warnings".to_string();
        }
        return format_warnings_only(&warnings);
    }

    // Build failed — show errors + summary + warnings summary
    format_build_failed(&errors, &warnings, &tool_errors, &make_errors)
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
            push_notes(&mut result, d);
        }
        result.push('\n');
    }

    result.trim().to_string()
}

fn push_notes(result: &mut String, d: &GccDiagnostic) {
    for note in &d.notes {
        result.push_str(&format!("    note {}\n", note));
    }
}

fn format_build_failed(
    errors: &[&GccDiagnostic],
    warnings: &[&GccDiagnostic],
    tool_errors: &[String],
    make_errors: &[String],
) -> String {
    let mut by_file: HashMap<&str, Vec<&GccDiagnostic>> = HashMap::new();
    for e in errors {
        by_file.entry(e.file.as_str()).or_default().push(e);
    }

    let error_count = errors.len() + tool_errors.len();
    let mut result = format!(
        "gcc: BUILD FAILED — {} error{}, {} warning{}\n",
        error_count,
        if error_count == 1 { "" } else { "s" },
        warnings.len(),
        if warnings.len() == 1 { "" } else { "s" },
    );
    result.push_str("═══════════════════════════════════════\n");

    for line in tool_errors {
        result.push_str(line);
        result.push('\n');
    }
    if !tool_errors.is_empty() {
        result.push('\n');
    }

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
            push_notes(&mut result, d);
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

    // ── Regression tests (real clang output) ────────────────────────────────

    #[test]
    fn test_fatal_error_kept_next_to_warning() {
        let input = "\
b.c:2:18: warning: unused variable 'unused' [-Wunused-variable]
    2 | int f(void){ int unused; return 1; }
      |                  ^~~~~~
1 warning generated.
a.c:2:10: fatal error: 'missing.h' file not found
    2 | #include \"missing.h\"
      |          ^~~~~~~~~~~
1 error generated.
";
        let result = filter_gcc_output(input);
        assert!(result.contains("BUILD FAILED"), "got:\n{}", result);
        assert!(
            result.contains("'missing.h' file not found"),
            "fatal error must be shown; got:\n{}",
            result
        );
    }

    #[test]
    fn test_notes_attached_to_error() {
        let input = "\
c.cpp:3:13: error: no matching function for call to 'g'
    3 | int main(){ g(\"x\"); }
      |             ^
c.cpp:1:6: note: candidate function not viable: no known conversion from 'const char[2]' to 'int' for 1st argument
    1 | void g(int);
      |      ^ ~~~
c.cpp:2:6: note: candidate function not viable: no known conversion from 'const char[2]' to 'double' for 1st argument
    2 | void g(double);
      |      ^ ~~~~~~
1 error generated.
";
        let result = filter_gcc_output(input);
        assert!(
            result.contains("1 error,"),
            "notes are not errors; got:\n{}",
            result
        );
        assert!(
            result.contains("note L1: candidate function not viable"),
            "got:\n{}",
            result
        );
        assert!(result.contains("to 'double'"), "got:\n{}", result);
    }

    #[test]
    fn test_note_from_other_file_keeps_path() {
        let input = "\
src/main.c:9:5: error: conflicting types for 'parse'
src/parser.h:3:5: note: previous declaration is here
";
        let result = filter_gcc_output(input);
        assert!(
            result.contains("note src/parser.h:3: previous declaration is here"),
            "got:\n{}",
            result
        );
    }

    #[test]
    fn test_stdout_modes_pass_through() {
        let args = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(writes_to_stdout(&args(&["-E", "w.c"])));
        assert!(writes_to_stdout(&args(&["-M", "a.c"])));
        assert!(writes_to_stdout(&args(&["-MM", "a.c"])));
        assert!(!writes_to_stdout(&args(&["-MMD", "-c", "a.c"])));
        assert!(!writes_to_stdout(&args(&["-Wall", "-c", "a.c"])));
        assert!(writes_to_stdout(&args(&["-S", "-o", "-", "a.c"])));
        assert!(writes_to_stdout(&args(&["-S", "-o-", "a.c"])));
        assert!(!writes_to_stdout(&args(&["-o", "a.out", "a.c"])));
    }

    #[test]
    fn test_driver_error_kept_next_to_warning() {
        // GNU gcc 16: missing input file next to a file that only warns
        let input = "\
cc1: fatal error: nofile.c: No such file or directory
compilation terminated.
b.c: In function 'f':
b.c:1:18: warning: unused variable 'unused' [-Wunused-variable]
    1 | int f(void){ int unused; return 1; }
      |                  ^~~~~~
";
        let result = filter_gcc_output(input);
        assert!(
            result.contains("BUILD FAILED — 1 error"),
            "got:\n{}",
            result
        );
        assert!(
            result.contains("cc1: fatal error: nofile.c: No such file or directory"),
            "got:\n{}",
            result
        );
    }

    #[test]
    fn test_linker_failure_passes_through() {
        let input = "\
Undefined symbols for architecture arm64:
  \"_nope\", referenced from:
      _main in l-9a32a6.o
ld: symbol(s) not found for architecture arm64
clang: error: linker command failed with exit code 1 (use -v to see invocation)
";
        let result = filter_gcc_output(input);
        assert!(
            result.contains("\"_nope\", referenced from:"),
            "got:\n{}",
            result
        );
        assert!(result.contains("linker command failed"), "got:\n{}", result);
    }
}
