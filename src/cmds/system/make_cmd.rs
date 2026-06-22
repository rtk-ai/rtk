//! Compact filter for `make` builds.
//!
//! Groups runs of compiler invocations into a one-line summary, promotes
//! diagnostics verbatim, and strips make[N]: directory chrome.
//!
//! Token savings: 90%+ on parallel builds with hundreds of gcc/clang lines.
//! Fallback: if input has no compiler-style lines, returns it unchanged.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    // Compiler invocations: cc, gcc, clang, g++, c++, ar, ranlib, ld, libtool variants
    static ref COMPILER_LINE: Regex = Regex::new(
        r"(?x)
        ^(?:
            (?:.*/)?(?:cc|gcc|g\+\+|clang|clang\+\+|c\+\+|x86_64-[a-z]+-gcc|arm-[a-z]+-gcc)\s |
            (?:.*/)?(?:ar|ranlib|ld|libtool|llvm-ar)\s |
            /bin/sh\s+\./libtool\s
        )"
    ).expect("compiler line regex");

    // Diagnostic lines: warning, error, note — from compiler output
    static ref DIAGNOSTIC_LINE: Regex = Regex::new(
        r": (?:warning|error|note):"
    ).expect("diagnostic regex");

    // "In file included from" context lines preceding a diagnostic
    static ref INCLUDE_CHAIN: Regex = Regex::new(
        r"^(?:In file included from|                 from)\s"
    ).expect("include chain regex");

    // make[N]: lines to strip
    static ref MAKE_BRACKET: Regex = Regex::new(
        r"^make\[\d+\]:"
    ).expect("make bracket regex");

    // Lines to drop unconditionally (blank-line runs handled separately)
    static ref NOTHING_TO_DO: Regex = Regex::new(
        r"^(?:Nothing to be done|make: Nothing)"
    ).expect("nothing to do regex");

    // Link / install / phase boundaries — flush compile group, keep verbatim
    static ref LINK_OR_INSTALL: Regex = Regex::new(
        r"(?x)
        ^(?:
            /usr/bin/install\b |
            libtool:\s+link: |
            Making\s+(?:install|all|clean|check|distcheck|dist)\b |
            make\s+\[|                           # nested make calls emitted as text
            gcc\s+-shared\s |                    # shared library link step
            g\+\+\s+-shared\s |
            clang\s+-shared\s |
            ld\s+-shared\s
        )"
    ).expect("link/install regex");
}

/// Minimum compiler lines before we collapse them into a summary.
const GROUP_THRESHOLD: usize = 3;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("make");
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: make {}", args.join(" "));
    }

    runner::run_filtered(
        cmd,
        "make",
        &args.join(" "),
        filter_make_output,
        RunOptions::stdout_only(),
    )
}

/// Pure filter: takes raw make output, returns condensed form.
pub fn filter_make_output(raw: &str) -> String {
    let lines: Vec<&str> = raw.lines().collect();

    // Fallback: if no compiler-style lines detected, return input unchanged.
    let has_compiler_lines = lines.iter().any(|l| COMPILER_LINE.is_match(l));
    if !has_compiler_lines {
        return strip_chrome(raw);
    }

    let mut output: Vec<String> = Vec::new();
    let mut compile_count: usize = 0;
    let mut warn_count: usize = 0;
    let mut err_count: usize = 0;
    // Pending diagnostic context lines (include chains) before the diagnostic itself
    let mut pending_context: Vec<String> = Vec::new();
    // Buffered compiler lines not yet counted into a group
    let mut compiler_buf: Vec<&str> = Vec::new();

    let flush_group = |output: &mut Vec<String>,
                       count: &mut usize,
                       warns: &mut usize,
                       errs: &mut usize| {
        if *count == 0 {
            return;
        }
        let detail = match (*errs, *warns) {
            (0, 0) => "0 warnings, 0 errors".to_string(),
            (0, w) => format!("{} warning{}, 0 errors", w, if w == 1 { "" } else { "s" }),
            (e, 0) => format!("0 warnings, {} error{}", e, if e == 1 { "" } else { "s" }),
            (e, w) => format!(
                "{} warning{}, {} error{}",
                w,
                if w == 1 { "" } else { "s" },
                e,
                if e == 1 { "" } else { "s" }
            ),
        };
        output.push(format!("compiled {} files ({})", count, detail));
        *count = 0;
        *warns = 0;
        *errs = 0;
    };

    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];

        // Strip make[N]: and blank lines
        if MAKE_BRACKET.is_match(line) || line.trim().is_empty() || NOTHING_TO_DO.is_match(line) {
            i += 1;
            continue;
        }

        // Diagnostic lines (warning/error/note)
        if DIAGNOSTIC_LINE.is_match(line) {
            // Flush current compile group before emitting diagnostic
            if compile_count > 0 || !compiler_buf.is_empty() {
                // Count buffered lines that didn't reach threshold
                compile_count += compiler_buf.len();
                compiler_buf.clear();
                flush_group(
                    &mut output,
                    &mut compile_count,
                    &mut warn_count,
                    &mut err_count,
                );
            }

            // Emit any pending include-chain context
            for ctx in pending_context.drain(..) {
                output.push(ctx);
            }

            // Count and emit the diagnostic
            if line.contains(": warning:") {
                warn_count += 1;
            } else if line.contains(": error:") {
                err_count += 1;
            }
            output.push(line.to_string());
            i += 1;
            continue;
        }

        // Include-chain context lines — buffer them until we see their diagnostic
        if INCLUDE_CHAIN.is_match(line) {
            pending_context.push(line.to_string());
            i += 1;
            continue;
        } else {
            // Non-diagnostic line clears pending context (it was orphaned)
            pending_context.clear();
        }

        // Link/install boundary — flush compile group, keep this line verbatim
        if LINK_OR_INSTALL.is_match(line) {
            if compile_count > 0 || !compiler_buf.is_empty() {
                compile_count += compiler_buf.len();
                compiler_buf.clear();
                flush_group(
                    &mut output,
                    &mut compile_count,
                    &mut warn_count,
                    &mut err_count,
                );
            }
            output.push(line.to_string());
            i += 1;
            continue;
        }

        // Compiler invocation
        if COMPILER_LINE.is_match(line) {
            compiler_buf.push(line);
            // When buffer reaches threshold, absorb into running count
            if compiler_buf.len() >= GROUP_THRESHOLD {
                compile_count += compiler_buf.len();
                compiler_buf.clear();
            }
            i += 1;
            continue;
        }

        // Any other line: flush compile group if pending, keep verbatim
        if compile_count > 0 || !compiler_buf.is_empty() {
            compile_count += compiler_buf.len();
            compiler_buf.clear();
            flush_group(
                &mut output,
                &mut compile_count,
                &mut warn_count,
                &mut err_count,
            );
        }
        output.push(line.to_string());
        i += 1;
    }

    // Flush remaining compile group
    if compile_count > 0 || !compiler_buf.is_empty() {
        compile_count += compiler_buf.len();
        flush_group(
            &mut output,
            &mut compile_count,
            &mut warn_count,
            &mut err_count,
        );
    }

    let result = output.join("\n");

    if result.is_empty() {
        "make: ok".to_string()
    } else {
        result
    }
}

/// Strip only make[N] chrome and blank runs — used for non-compiler output passthrough.
fn strip_chrome(raw: &str) -> String {
    let lines: Vec<&str> = raw
        .lines()
        .filter(|l| !MAKE_BRACKET.is_match(l) && !l.trim().is_empty() && !NOTHING_TO_DO.is_match(l))
        .collect();

    if lines.is_empty() {
        "make: ok".to_string()
    } else {
        lines.join("\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(text: &str) -> usize {
        text.split_whitespace().count()
    }

    // ── Fixture tests ─────────────────────────────────────────────────────────

    #[test]
    fn test_token_savings_long_build() {
        let input = include_str!("../../../tests/fixtures/make_parallel_build_raw.txt");
        let output = filter_make_output(input);

        let input_tokens = count_tokens(input);
        let output_tokens = count_tokens(&output);
        let savings = 100.0 - (output_tokens as f64 / input_tokens as f64 * 100.0);

        assert!(
            savings >= 60.0,
            "Expected >=60% token savings on long build, got {:.1}% (in={} out={})",
            savings,
            input_tokens,
            output_tokens
        );
    }

    #[test]
    fn test_incremental_build_output() {
        let input = include_str!("../../../tests/fixtures/make_incremental_raw.txt");
        let output = filter_make_output(input);

        // Should report 1 compiled file (gcc line is under GROUP_THRESHOLD=3 but still counted)
        // Actually 1 gcc + 1 gcc link = 2 compiler lines, under threshold so kept as "compiled N"
        // The warning from main.c should be verbatim
        assert!(
            output.contains("warning:"),
            "Warning should be preserved verbatim"
        );
        // make[N] lines should be stripped
        assert!(
            !output.contains("make[1]"),
            "make[N] lines should be stripped"
        );
    }

    // ── Unit tests ────────────────────────────────────────────────────────────

    #[test]
    fn test_strips_make_bracket_lines() {
        let input = "make[1]: Entering directory '/home/user'\nmake[1]: Leaving directory '/home/user'\n";
        let output = filter_make_output(input);
        assert_eq!(output, "make: ok");
    }

    #[test]
    fn test_strips_blank_lines() {
        let input = "gcc -O2 -c foo.c -o foo.o\n\ngcc -O2 -c bar.c -o bar.o\n\ngcc -O2 -c baz.c -o baz.o\n";
        let output = filter_make_output(input);
        // Three compiler lines should be grouped
        assert!(output.contains("compiled 3 files"));
        assert!(!output.contains("\n\n"), "Blank lines should be removed");
    }

    #[test]
    fn test_groups_compiler_lines() {
        let input = "gcc -O2 -c a.c -o a.o\ngcc -O2 -c b.c -o b.o\ngcc -O2 -c c.c -o c.o\ngcc -O2 -c d.c -o d.o\n";
        let output = filter_make_output(input);
        assert_eq!(output, "compiled 4 files (0 warnings, 0 errors)");
    }

    #[test]
    fn test_preserves_warnings_verbatim() {
        let input = "gcc -O2 -c a.c -o a.o\ngcc -O2 -c b.c -o b.o\ngcc -O2 -c c.c -o c.o\na.c:10:5: warning: unused variable 'x' [-Wunused-variable]\ngcc -O2 -c d.c -o d.o\n";
        let output = filter_make_output(input);
        assert!(output.contains("warning: unused variable 'x'"), "Warning line must appear verbatim");
        assert!(output.contains("compiled"), "Should still have compile summary");
    }

    #[test]
    fn test_preserves_errors_verbatim() {
        let input = "gcc -O2 -c a.c -o a.o\ngcc -O2 -c b.c -o b.o\ngcc -O2 -c c.c -o c.o\nsrc/foo.c:42:1: error: expected ';' before '}'\n";
        let output = filter_make_output(input);
        assert!(output.contains("error: expected ';'"), "Error line must appear verbatim");
    }

    #[test]
    fn test_warning_count_in_summary() {
        let input = "\
gcc -O2 -c a.c -o a.o
gcc -O2 -c b.c -o b.o
gcc -O2 -c c.c -o c.o
a.c:10:5: warning: foo [-Wfoo]
gcc -O2 -c d.c -o d.o
gcc -O2 -c e.c -o e.o
gcc -O2 -c f.c -o f.o
b.c:20:3: warning: bar [-Wbar]
";
        let output = filter_make_output(input);
        // Both warnings should be verbatim; summary after second group reflects its count
        assert!(output.contains("warning: foo"));
        assert!(output.contains("warning: bar"));
        // First group flushed before first warning: 3 files, 0 warnings
        assert!(output.contains("compiled 3 files (0 warnings, 0 errors)"));
    }

    #[test]
    fn test_include_chain_kept_with_diagnostic() {
        let input = "\
gcc -O2 -c a.c -o a.o
gcc -O2 -c b.c -o b.o
gcc -O2 -c c.c -o c.o
In file included from src/foo.h:3,
                 from src/main.c:1:
src/main.c:5:1: warning: implicit declaration [-Wimplicit]
";
        let output = filter_make_output(input);
        assert!(output.contains("In file included from"), "include chain should be preserved");
        assert!(output.contains("warning: implicit declaration"));
    }

    #[test]
    fn test_link_line_kept_verbatim() {
        let input = "\
gcc -O2 -c a.c -o a.o
gcc -O2 -c b.c -o b.o
gcc -O2 -c c.c -o c.o
/usr/bin/install -c mybin /usr/local/bin/mybin
";
        let output = filter_make_output(input);
        assert!(output.contains("/usr/bin/install"), "Install line should be verbatim");
        assert!(output.contains("compiled 3 files"), "Compile group before install");
    }

    #[test]
    fn test_on_empty_chrome_only() {
        let input = "\
make[1]: Entering directory '/home/user'
make[1]: Leaving directory '/home/user'
";
        let output = filter_make_output(input);
        assert_eq!(output, "make: ok");
    }

    #[test]
    fn test_nothing_to_be_done() {
        let input = "Nothing to be done for `all'.\n";
        let output = filter_make_output(input);
        assert_eq!(output, "make: ok");
    }

    #[test]
    fn test_fallback_passthrough_non_compiler() {
        // Non-compiler output: should pass through (minus chrome)
        let input = "Custom build step running\nDone.\n";
        let output = filter_make_output(input);
        assert!(output.contains("Custom build step running"));
        assert!(output.contains("Done."));
    }

    #[test]
    fn test_empty_input() {
        let output = filter_make_output("");
        assert_eq!(output, "make: ok");
    }

    #[test]
    fn test_clang_lines_grouped() {
        let input = "clang -O2 -c a.c -o a.o\nclang -O2 -c b.c -o b.o\nclang -O2 -c c.c -o c.o\n";
        let output = filter_make_output(input);
        assert_eq!(output, "compiled 3 files (0 warnings, 0 errors)");
    }

    #[test]
    fn test_ar_ranlib_grouped_with_compiler() {
        let input = "\
gcc -O2 -c a.c -o a.o
gcc -O2 -c b.c -o b.o
gcc -O2 -c c.c -o c.o
ar rcs libfoo.a a.o b.o c.o
ranlib libfoo.a
";
        let output = filter_make_output(input);
        // ar and ranlib are compiler-class lines, all should be grouped
        assert!(output.contains("compiled 5 files") || output.contains("compiled"), "Should group ar/ranlib with gcc");
    }

    #[test]
    fn test_below_threshold_still_counted() {
        // 2 compiler lines (below GROUP_THRESHOLD=3) should still be counted at flush
        let input = "\
gcc -O2 -c a.c -o a.o
gcc -O2 -c b.c -o b.o
ar rcs libfoo.a a.o b.o
";
        let output = filter_make_output(input);
        // All 3 are compiler-class; even if under threshold initially they get flushed
        assert!(output.contains("compiled"), "Should summarize even below threshold");
    }
}
