//! Filters SCons build output — phase messages / configure checks stripped,
//! errors/warnings/tracebacks kept.
#![allow(dead_code)]

use super::diag;
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

// ── Stats ──

/// Accumulated statistics during SCons output filtering.
struct SconsStats {
    /// Number of successfully built targets.
    targets_built: usize,
    /// Number of up-to-date targets.
    targets_up_to_date: usize,
    /// Collected configure-check lines.
    configure_checks: Vec<String>,
    /// Collected error lines.
    errors: Vec<String>,
    /// Collected warning lines.
    warnings: Vec<String>,
    /// Whether `scons: building terminated because of errors.` was seen.
    has_terminated: bool,
    /// Traceback lines (Python exception stack).
    traceback: Vec<String>,
    /// Whether we are inside a traceback block.
    in_traceback: bool,
}

impl SconsStats {
    fn new() -> Self {
        Self {
            targets_built: 0,
            targets_up_to_date: 0,
            configure_checks: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
            has_terminated: false,
            traceback: Vec::new(),
            in_traceback: false,
        }
    }
}

// ── Line classification ──

/// Check for SCons phase lines:
/// - `scons: Reading SConscript files ...`
/// - `scons: done reading SConscript files.`
/// - `scons: Building targets ...`
/// - `scons: done building targets.`
fn is_phase_line(trimmed: &str) -> bool {
    if !trimmed.starts_with("scons: ") {
        return false;
    }
    trimmed.starts_with("scons: Reading SConscript files ")
        || trimmed.starts_with("scons: done reading SConscript files.")
        || trimmed.starts_with("scons: Building targets ")
        || trimmed.starts_with("scons: done building targets.")
}

/// Check for "up to date" messages: `scons: 'X' is up to date.`
fn is_up_to_date(trimmed: &str) -> bool {
    trimmed.starts_with("scons: ") && trimmed.contains("is up to date")
}

/// Check for configure-check lines: `scons: Configure: Checking for ...`
/// Returns the check description or None.
fn is_configure_check(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("scons: Configure: Checking for ")?;
    Some(format!("Checking for {}", rest.trim()))
}

/// Check for SCons error lines: `scons: *** [target] Error N`
fn is_scons_error(trimmed: &str) -> bool {
    diag::lazy_re!(r"^scons: \*{1,3} \[.+\] Error \d+").is_match(trimmed)
}

/// Check for terminated message: `scons: building terminated because of errors.`
fn is_scons_terminated(trimmed: &str) -> bool {
    trimmed.starts_with("scons: building terminated because of errors.")
}

/// Check if a line looks like a Python exception (e.g. `TypeError: ...`).
fn is_python_exception_line(trimmed: &str) -> bool {
    // Match patterns like "TypeError:", "ValueError:", "AttributeError:", etc.
    if let Some(colon) = trimmed.find(':') {
        let before = &trimmed[..colon];
        // No spaces in the exception name, and it looks like an exception type
        if !before.contains(' ') && before.len() > 3 {
            let looks_like_exception = before.ends_with("Error")
                || before.ends_with("Exception")
                || before.ends_with("Warning")
                || before == "TypeError"
                || before == "ValueError"
                || before == "KeyError"
                || before == "NameError"
                || before == "OSError"
                || before == "IOError"
                || before == "SyntaxError"
                || before == "ImportError"
                || before == "IndexError";
            return looks_like_exception;
        }
    }
    false
}

/// Check for Python traceback start: `Traceback (most recent call last):` or `  File "..."`.
/// These checks are done on the *untrimmed* raw line to preserve indentation.
fn is_traceback_start(raw: &str) -> bool {
    raw.starts_with("Traceback (most recent call last):") || raw.starts_with("  File \"")
}

/// Check if a raw line is likely a traceback continuation (indented Python stack frame).
fn is_traceback_continuation(raw: &str) -> bool {
    raw.starts_with("    ") || raw.starts_with('\t')
}

/// Check for SCons command-string short forms: the actual shell command executed.
/// Example: `g++ -o build/main.o -c src/main.cpp`
/// We detect these by looking for known tool names at the start of the line.
fn is_command_string(trimmed: &str) -> bool {
    // Skip if it's a compiler diagnostic
    if diag::is_compiler_diag(trimmed) {
        return false;
    }
    // Known tools that SCons echoes as command strings
    const KNOWN_TOOLS: &[&str] = &[
        "gcc", "g++", "clang", "clang++", "cc", "c++", "cl", "cl.exe", "link", "link.exe", "ar",
        "ld", "as", "nasm", "yasm", "ranlib", "nvcc", "icc", "icx", "icpx", "windres", "rc", "mt",
        "python", "python3", "install", "mkdir", "rm", "cp", "mv", "sed", "awk", "touch", "swig",
        "lex", "yacc", "bison", "flex",
    ];
    let first = match trimmed.split_whitespace().next() {
        Some(w) => w,
        None => return false,
    };
    // Check if first word is a known tool (including path-prefixed variants)
    if let Some(fname) = std::path::Path::new(first).file_name() {
        if let Some(name) = fname.to_str() {
            if KNOWN_TOOLS.contains(&name) {
                return true;
            }
        }
    }
    // Path-like first word (contains slash or backslash)
    (first.contains('/') || first.contains('\\')) && first.len() > 2
}

// ── Filter logic ──

/// Filter SCons output: keep errors/warnings/tracebacks, drop phase noise and command echoes.
fn filter_scons_output(input: &str, exit_code: i32) -> String {
    let normalized = diag::normalize(input);
    let mut stats = SconsStats::new();
    let mut kept_lines: Vec<String> = Vec::new();

    for raw_line in normalized.lines() {
        let trimmed = raw_line.trim();

        // ── Blank lines ──
        if trimmed.is_empty() {
            continue;
        }

        // ── Traceback handling (use raw_line to preserve indentation) ──
        if stats.in_traceback {
            if is_traceback_continuation(raw_line) || is_traceback_start(raw_line) {
                stats.traceback.push(raw_line.to_string());
                continue;
            }
            // Specific SCons error line after traceback
            if trimmed.starts_with("scons: ") {
                stats.traceback.push(trimmed.to_string());
                continue;
            }
            // End of traceback
            stats.in_traceback = false;
        }

        // Python exception line may start a traceback without `Traceback (...)` header
        if is_traceback_start(raw_line) || is_python_exception_line(trimmed) {
            stats.in_traceback = true;
            stats.traceback.push(raw_line.to_string());
            continue;
        }

        // ── Phase lines ──
        if is_phase_line(trimmed) {
            continue;
        }

        // ── Up to date ──
        if is_up_to_date(trimmed) {
            stats.targets_up_to_date += 1;
            continue;
        }

        // ── Configure checks ──
        if let Some(check) = is_configure_check(trimmed) {
            stats.configure_checks.push(check);
            continue;
        }

        // ── SCons errors ──
        if is_scons_error(trimmed) {
            stats.errors.push(trimmed.to_string());
            continue;
        }

        // ── Terminated ──
        if is_scons_terminated(trimmed) {
            stats.has_terminated = true;
            continue;
        }

        // ── Compiler warning diagnostics ──
        if diag::diag_has_severity(trimmed, "warning") {
            stats.warnings.push(trimmed.to_string());
            continue;
        }

        // ── Compiler error diagnostics ──
        if diag::diag_has_severity(trimmed, "error")
            || diag::diag_has_severity(trimmed, "fatal error")
        {
            stats.errors.push(trimmed.to_string());
            continue;
        }

        // ── Linker errors ──
        if diag::is_linker_error(trimmed) {
            stats.errors.push(trimmed.to_string());
            continue;
        }

        // ── Command string echoes (compile/link commands) ──
        if is_command_string(trimmed) {
            stats.targets_built += 1;
            continue;
        }

        // ── SCons install/copy messages ──
        if trimmed.starts_with("Install ") || trimmed.starts_with("Copy(") {
            stats.targets_built += 1;
            continue;
        }

        // ── Fail-open: unrecognised lines pass through ──
        kept_lines.push(trimmed.to_string());
    }

    // If exit code indicates failure but no errors were captured, fall back.
    if exit_code != 0 && stats.errors.is_empty() && stats.traceback.is_empty() {
        stats.errors.push(normalized.trim().to_string());
    }

    compose_output(&stats, &kept_lines)
}

/// Build the final filtered output string.
fn compose_output(stats: &SconsStats, kept_lines: &[String]) -> String {
    let mut output = String::new();

    // ── Traceback (SConscript error) ──
    if !stats.traceback.is_empty() {
        output.push_str("scons: SConscript error\n");
        output.push_str("Traceback.\n");
        for line in &stats.traceback {
            output.push_str(line);
            output.push('\n');
        }
        return output;
    }

    // ── Errors ──
    if !stats.errors.is_empty() {
        output.push_str("scons: build failed\n");
        for err in &stats.errors {
            output.push_str(err);
            output.push('\n');
        }
        if stats.has_terminated {
            output.push_str("scons: building terminated because of errors.\n");
        }
        return output;
    }

    // ── Warnings ──
    if !stats.warnings.is_empty() {
        output.push_str(&format!("scons: {} warning(s)\n", stats.warnings.len()));
        for warn in &stats.warnings {
            output.push_str(warn);
            output.push('\n');
        }
        return output;
    }

    // ── Success ──
    let total = stats.targets_built + stats.targets_up_to_date;
    if total > 0 {
        output.push_str(&format!("ok scons: {} targets", total));
    } else {
        output.push_str("ok scons: up to date");
    }

    // Add any kept (unrecognised) lines
    if !kept_lines.is_empty() {
        output.push('\n');
        for line in kept_lines {
            output.push_str(line);
            output.push('\n');
        }
    }

    output
}

// ── Public API ──

/// Run `scons` with buffered output filtering.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("scons: running scons {}", args.join(" "));
    }

    let mut cmd = resolved_command("scons");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_filtered_with_exit(
        cmd,
        "scons",
        &args_str,
        filter_scons_output,
        runner::RunOptions::with_tee("scons"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    fn filter_scons(input: &str, exit_code: i32) -> String {
        filter_scons_output(input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_phase_line() {
        assert!(is_phase_line("scons: Reading SConscript files ..."));
        assert!(is_phase_line("scons: done reading SConscript files."));
        assert!(is_phase_line("scons: Building targets ..."));
        assert!(is_phase_line("scons: done building targets."));
        assert!(!is_phase_line(
            "scons: Configure: Checking for C library m..."
        ));
        assert!(!is_phase_line("scons: *** [target] Error 1"));
    }

    #[test]
    fn test_is_up_to_date() {
        assert!(is_up_to_date("scons: 'build/main.o' is up to date."));
        assert!(!is_up_to_date("scons: Building targets ..."));
    }

    #[test]
    fn test_is_configure_check() {
        assert_eq!(
            is_configure_check("scons: Configure: Checking for C library m..."),
            Some("Checking for C library m...".to_string())
        );
        assert_eq!(is_configure_check("scons: Configure: no"), None);
    }

    #[test]
    fn test_is_scons_error() {
        assert!(is_scons_error("scons: *** [build/main.o] Error 1"));
        assert!(is_scons_error("scons: *** [target] Error 2"));
        assert!(!is_scons_error("scons: done building targets."));
    }

    #[test]
    fn test_is_scons_terminated() {
        assert!(is_scons_terminated(
            "scons: building terminated because of errors."
        ));
        assert!(!is_scons_terminated("scons: done building targets."));
    }

    #[test]
    fn test_is_traceback_start() {
        assert!(is_traceback_start("Traceback (most recent call last):"));
        assert!(is_traceback_start(
            "  File \"SConstruct\", line 42, in <module>"
        ));
        assert!(!is_traceback_start("scons: Building targets ..."));
    }

    #[test]
    fn test_is_command_string() {
        assert!(is_command_string("g++ -o build/main.o -c src/main.cpp"));
        assert!(is_command_string("clang++ -std=c++20 -c file.cpp"));
        assert!(is_command_string("ar cr libfoo.a file1.o file2.o"));
        assert!(is_command_string("cl.exe /c /nologo /EHsc file.cpp"));
        assert!(!is_command_string("src/main.cpp:42:13: error: bad"));
        assert!(!is_command_string(""));
        assert!(!is_command_string("scons: Building targets ..."));
    }

    // ── Success cases ──

    #[test]
    fn test_scons_successful_build() {
        let input = "\
scons: Reading SConscript files ...
scons: done reading SConscript files.
scons: Building targets ...
g++ -o build/main.o -c src/main.cpp
g++ -o build/util.o -c src/util.cpp
g++ -o build/app build/main.o build/util.o
scons: done building targets.
";
        let result = filter_scons(input, 0);
        assert!(result.contains("ok scons: 3 targets"), "got: {}", result);
        assert!(
            !result.contains("scons: Reading SConscript"),
            "phase lines should be stripped, got: {}",
            result
        );
        assert!(
            !result.contains("g++ -o build/main.o"),
            "command strings should be stripped, got: {}",
            result
        );
    }

    #[test]
    fn test_scons_up_to_date() {
        let input = "\
scons: Reading SConscript files ...
scons: done reading SConscript files.
scons: Building targets ...
scons: 'build/main.o' is up to date.
scons: 'build/util.o' is up to date.
scons: 'build/app' is up to date.
scons: done building targets.
";
        let result = filter_scons(input, 0);
        assert!(result.contains("ok scons: 3 targets"), "got: {}", result);
    }

    #[test]
    fn test_scons_no_targets() {
        let input = "\
scons: Reading SConscript files ...
scons: done reading SConscript files.
scons: Building targets ...
scons: done building targets.
";
        let result = filter_scons(input, 0);
        assert!(result.contains("ok scons: up to date"), "got: {}", result);
    }

    #[test]
    fn test_scons_configure_check() {
        let input = "\
scons: Reading SConscript files ...
scons: done reading SConscript files.
scons: Building targets ...
scons: Configure: Checking for C library m...
scons: Configure: Checking for C++ header cstdint...
scons: Configure: Checking for sizeof(void*)...
g++ -o build/main.o -c src/main.cpp
scons: done building targets.
";
        let result = filter_scons(input, 0);
        assert!(result.contains("ok scons: 1 targets"), "got: {}", result);
        assert!(
            !result.contains("Checking for C library m"),
            "configure checks should be stripped, got: {}",
            result
        );
    }

    // ── Failure cases ──

    #[test]
    fn test_scons_build_error() {
        let input = "\
scons: Reading SConscript files ...
scons: done reading SConscript files.
scons: Building targets ...
g++ -o build/main.o -c src/main.cpp
g++ -o build/bad.o -c src/bad.cpp
src/bad.cpp:5:13: error: 'x' was not declared in this scope
src/bad.cpp:5:13: note: suggested alternative: 'y'
scons: *** [build/bad.o] Error 1
scons: building terminated because of errors.
";
        let result = filter_scons(input, 1);
        assert!(result.contains("scons: build failed"), "got: {}", result);
        assert!(
            result.contains("error: 'x' was not declared"),
            "got: {}",
            result
        );
        assert!(
            result.contains("scons: *** [build/bad.o] Error 1"),
            "got: {}",
            result
        );
        assert!(
            !result.contains("scons: Reading SConscript"),
            "phase lines should be stripped, got: {}",
            result
        );
    }

    #[test]
    fn test_scons_traceback() {
        let input = "\
scons: Reading SConscript files ...
scons: done reading SConscript files.
TypeError: 'NoneType' object is not iterable:
  File \"/home/user/project/SConstruct\", line 42:
    env = Environment()
  File \"/usr/lib/scons/SCons/Environment.py\", line 1234:
    some_func()
";
        let result = filter_scons(input, 1);
        assert!(
            result.contains("scons: SConscript error"),
            "got: {}",
            result
        );
        assert!(result.contains("Traceback."), "got: {}", result);
        assert!(result.contains("TypeError"), "got: {}", result);
    }

    #[test]
    fn test_scons_ansi_stripped() {
        let input = "\x1b[1mscons: Reading SConscript files ...\x1b[0m\n\
                      \x1b[1mscons: done reading SConscript files.\x1b[0m\n\
                      \x1b[32mg++ -o build/main.o -c src/main.cpp\x1b[0m\n\
                      \x1b[31msrc/main.cpp:1:1: error: bad code\x1b[0m\n\
                      scons: *** [build/main.o] Error 1\n\
                      scons: building terminated because of errors.\n";
        let result = filter_scons(input, 1);
        assert!(!result.contains("\x1b["), "ANSI codes should be stripped");
        assert!(result.contains("error: bad code"), "got: {}", result);
    }

    #[test]
    fn test_scons_empty_input() {
        let result = filter_scons("", 0);
        assert!(
            result.contains("ok scons"),
            "should have a summary, got: '{}'",
            result
        );
    }

    #[test]
    fn test_scons_token_savings_above_70pct() {
        let mut input = String::new();
        input.push_str("scons: Reading SConscript files ...\n");
        input.push_str("scons: done reading SConscript files.\n");
        input.push_str("scons: Building targets ...\n");
        // 200 command-string lines
        for i in 1..=200 {
            input.push_str(&format!(
                "g++ -o build/file{}.o -c src/file{}.cpp -Iinclude -I/usr/local/include -std=c++20 -O2 -Wall -Wextra -DNDEBUG\n",
                i, i
            ));
        }
        // One error
        input.push_str("src/file42.cpp:10:13: error: 'bad_func' was not declared in this scope\n");
        input.push_str("scons: *** [build/file42.o] Error 1\n");
        input.push_str("scons: building terminated because of errors.\n");

        let result = filter_scons(&input, 1);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 70,
            "token savings: {}% (expected >=70%)",
            savings
        );
    }
}
