#![allow(dead_code)]

//! Filters GNU Make build output — recipe echoes dropped, errors/warnings kept.
//!
//! Uses `BlockStreamFilter` + `BlockHandler` for streaming line-at-a-time
//! filtering. Recipe echoes, Entering/Leaving directory messages, automake
//! short-forms, and progress lines are stripped. Compiler diagnostics, linker
//! errors, and make error lines are kept in diagnostic blocks.

use super::diag;
use crate::core::runner;
use crate::core::stream::{BlockHandler, BlockStreamFilter};
use crate::core::utils::resolved_command;
use anyhow::Result;
use std::collections::HashMap;

// ── Helper functions ──

/// Recognised tool names for recipe-echo detection.
const KNOWN_TOOLS: &[&str] = &[
    "gcc", "g++", "clang", "clang++", "cc", "c++", "cl", "cl.exe", "link", "link.exe", "ar", "ld",
    "ld.lld", "lld", "lld-link", "as", "nasm", "yasm", "strip", "objcopy", "objdump", "ranlib",
    "dlltool", "windres", "rc", "mt", "nvcc", "icx", "icpx", "ifort", "icc", "rustc", "cargo",
    "go", "javac", "kotlinc", "cmake", "cd", "mkdir", "rm", "cp", "mv", "echo", "install", "ln",
    "touch", "sed", "awk", "python", "python3", "perl", "bash", "sh", "ninja", "make", "nmake",
    "msbuild", "xmake",
];

/// Check if `trimmed` starts with a quoted string (e.g. `"C:\Program Files\..."`).
fn starts_with_quote(s: &str) -> bool {
    s.starts_with('"') || s.starts_with('\'')
}

/// Check if a line is a recipe echo — a shell command printed by make before
/// execution.  Detects lines that start with a known tool, a path-like first
/// word, or a quoted path.
pub fn is_recipe_echo(trimmed: &str) -> bool {
    if trimmed.is_empty() {
        return false;
    }
    // Quoted command path
    if starts_with_quote(trimmed) {
        return true;
    }
    // Exclude GCC/Clang diagnostic lines (they contain ':' patterns and '/'
    // in file paths but are not recipe echoes).
    if diag::is_compiler_diag(trimmed) {
        return false;
    }
    // First word
    let first = match trimmed.split_whitespace().next() {
        Some(w) => w,
        None => return false,
    };
    // Known tool by basename
    if let Some(fname) = std::path::Path::new(first).file_name() {
        if let Some(name) = fname.to_str() {
            if KNOWN_TOOLS.contains(&name) {
                return true;
            }
        }
    }
    // Path-like (contains slash or backslash in first word), but not Windows
    // drive letter alone (e.g. "C:").
    if (first.contains('/') || first.contains('\\')) && first.len() > 2 {
        return true;
    }
    false
}

/// Check for `make[N]: Entering directory '...'` or `make[N]: Leaving directory '...'`.
pub fn is_enter_leave(trimmed: &str) -> bool {
    diag::lazy_re!(r"^make\[\d+\]: (Entering|Leaving) directory").is_match(trimmed)
}

/// Check for `make[N]: *** [target] Error N` (or `make[N]: [target] Error N`).
pub fn is_make_error(trimmed: &str) -> bool {
    diag::lazy_re!(r"^make\[\d+\]: \*?\*?\*?\s*\[[^\]]*\] Error \d+").is_match(trimmed)
}

/// Check for `make: *** [target] Error N` — the final make-level error line.
pub fn is_make_final_error(trimmed: &str) -> bool {
    diag::lazy_re!(r"^make: \*{1,3} \[[^\]]*\] Error \d+").is_match(trimmed)
}

/// Check for "No rule to make target '...'".
pub fn is_no_rule_target(trimmed: &str) -> bool {
    trimmed.contains("No rule to make target")
        || diag::lazy_re!(r"^make\[\d+\]: \*{1,3} No rule to make target").is_match(trimmed)
}

/// Check for "Nothing to be done for '...'" or "'...' is up to date".
pub fn is_nothing_to_do(trimmed: &str) -> bool {
    trimmed.contains("Nothing to be done")
        || trimmed.contains("is up to date")
        || diag::lazy_re!(r"^make\[\d+\]: Nothing to be done").is_match(trimmed)
        || diag::lazy_re!(r"^make: Nothing to be done").is_match(trimmed)
}

/// Automake silent-rule short forms: `  CC  src/file.o`, `  CXX  ...`,
/// `  CXXLD  myapp`, `  LD  lib.so`, `  AR  lib.a`.
pub fn is_automake_short(trimmed: &str) -> bool {
    diag::lazy_re!(r"^  (CC|CXX|CXXLD|LD|AR|FC|F77|F90|GOC|GPC|GCJ|VALAC|GEN|INSTALL|MKDIR|LN|CP|RM|MV|TAR|GZIP|LEX|YACC|MAKEINFO|TEXI2DVI|TEXI2PDF|DVIPS|DVIPDF)\s").is_match(trimmed)
}

/// Progress line: `[N/M] ...` (same format as ninja/cmake progress).
pub fn is_progress_line(line: &str) -> bool {
    line.starts_with('[') && {
        if let Some(end_bracket) = line.find(']') {
            let inner = &line[1..end_bracket];
            if let Some(slash) = inner.find('/') {
                inner[..slash].parse::<usize>().is_ok()
                    && inner[slash + 1..].parse::<usize>().is_ok()
            } else {
                false
            }
        } else {
            false
        }
    }
}

/// Parse `[N/M]` from a progress line.
pub fn parse_progress(line: &str) -> Option<(usize, usize)> {
    if !line.starts_with('[') {
        return None;
    }
    let end_bracket = line.find(']')?;
    let inner = &line[1..end_bracket];
    let slash = inner.find('/')?;
    let n = inner[..slash].parse::<usize>().ok()?;
    let m = inner[slash + 1..].parse::<usize>().ok()?;
    Some((n, m))
}

/// Extract the directory path from an Entering/Leaving message.
fn extract_dir(trimmed: &str) -> Option<String> {
    // Look for `'...'` or `...`
    if let Some(start) = trimmed.find('\'') {
        let after = &trimmed[start + 1..];
        if let Some(end) = after.find('\'') {
            return Some(after[..end].to_string());
        }
    }
    // Fallback: take everything after "directory " (non-quoted)
    if let Some(pos) = trimmed.find("directory ") {
        let after = &trimmed[pos + "directory ".len()..];
        let dir = after.trim();
        if !dir.is_empty() {
            return Some(dir.to_string());
        }
    }
    None
}

// ── Handler ──

/// BlockHandler for GNU Make build output.
pub struct MakeBuildHandler {
    /// Number of recipe lines executed (recipe echoes skipped).
    recipes_executed: usize,
    /// Number of failed targets.
    targets_failed: usize,
    /// Directory stack from Entering/Leaving messages.
    dir_stack: Vec<String>,
    /// Error cascade — target + error code for each failed target.
    error_cascade: Vec<String>,
    /// Whether we are inside a recipe-echo block (being skipped).
    in_recipe_block: bool,
    /// Whether we are inside a diagnostic block.
    in_diag_block: bool,
    /// Warning flag → count.
    warning_counts: HashMap<String, usize>,
    /// Dedup state: message body → count.
    seen_diagnostics: HashMap<String, usize>,
    /// Targets that failed due to "No rule to make target".
    no_rule_targets: Vec<String>,
    /// Number of "up to date" messages suppressed.
    up_to_date_count: usize,
    /// True if the only output was "Nothing to be done".
    nothing_to_do: bool,
    /// Maximum total edges seen in progress lines.
    edges_total: usize,
    /// Current progress edge.
    edges_built: usize,
}

impl MakeBuildHandler {
    pub fn new() -> Self {
        Self {
            recipes_executed: 0,
            targets_failed: 0,
            dir_stack: Vec::new(),
            error_cascade: Vec::new(),
            in_recipe_block: false,
            in_diag_block: false,
            warning_counts: HashMap::new(),
            seen_diagnostics: HashMap::new(),
            no_rule_targets: Vec::new(),
            up_to_date_count: 0,
            nothing_to_do: false,
            edges_total: 0,
            edges_built: 0,
        }
    }

    fn track_dedup(&mut self, diag_line: &str) -> bool {
        let msg = diag::extract_diag_message(diag_line);
        let count = self.seen_diagnostics.entry(msg).or_insert(0);
        *count += 1;
        *count <= 3 // Show first 3 occurrences, collapse rest
    }

    fn track_warning(&mut self, line: &str) {
        if let Some(flag) = diag::extract_warning_flag(line) {
            *self.warning_counts.entry(flag).or_insert(0) += 1;
        } else {
            *self.warning_counts.entry("other".to_string()).or_insert(0) += 1;
        }
    }
}

impl BlockHandler for MakeBuildHandler {
    fn should_skip(&mut self, line: &str) -> bool {
        let normalized = diag::normalize(line);
        let trimmed = normalized.trim();

        if trimmed.is_empty() {
            return true;
        }

        // ── Progress lines: [N/M] ... ──
        if is_progress_line(trimmed) {
            self.edges_built += 1;
            if let Some((_n, m)) = parse_progress(trimmed) {
                self.edges_total = self.edges_total.max(m);
            }
            return true;
        }

        // ── Entering/Leaving directory ──
        if is_enter_leave(trimmed) {
            if let Some(dir) = extract_dir(trimmed) {
                if trimmed.contains("Entering") {
                    self.dir_stack.push(dir);
                } else if trimmed.contains("Leaving") {
                    // Pop matching directory
                    if self.dir_stack.last() == Some(&dir) {
                        self.dir_stack.pop();
                    }
                }
            }
            return true;
        }

        // ── Nothing to be done / up to date ──
        if is_nothing_to_do(trimmed) {
            self.nothing_to_do = true;
            self.up_to_date_count += 1;
            return true;
        }

        // ── Automake short forms ──
        if is_automake_short(trimmed) {
            self.recipes_executed += 1;
            return true;
        }

        // ── Recipe echoes ──
        if is_recipe_echo(trimmed) {
            self.recipes_executed += 1;
            self.in_recipe_block = true;
            return true;
        }

        // Continuation of a recipe echo (continuation lines after `\`)
        if self.in_recipe_block {
            if trimmed.ends_with('\\') || trimmed.starts_with(' ') || trimmed.starts_with('\t') {
                return true;
            }
            self.in_recipe_block = false;
        }

        false
    }

    fn is_block_start(&mut self, line: &str) -> bool {
        let normalized = diag::normalize(line);
        let trimmed = normalized.trim();

        if trimmed.is_empty() {
            return false;
        }

        // ── No rule to make target (check before is_make_error since
        //    `make[N]: *** No rule to make target` matches both patterns) ──
        if is_no_rule_target(trimmed) {
            self.targets_failed += 1;
            self.in_diag_block = true;
            self.no_rule_targets.push(trimmed.to_string());
            return true;
        }

        // ── Make sub-error: `make[N]: *** [target] Error N` ──
        if is_make_error(trimmed) {
            self.targets_failed += 1;
            self.in_diag_block = true;
            self.error_cascade.push(trimmed.to_string());
            return true;
        }

        // ── Make final error: `make: *** [target] Error N` ──
        if is_make_final_error(trimmed) {
            self.targets_failed += 1;
            self.in_diag_block = true;
            self.error_cascade.push(trimmed.to_string());
            return true;
        }

        // ── Compiler diagnostic ──
        if diag::is_compiler_diag(trimmed) {
            self.in_diag_block = true;
            if trimmed.contains("warning:") || trimmed.to_lowercase().contains("warning") {
                self.track_warning(trimmed);
            }
            return self.track_dedup(trimmed);
        }

        // ── Linker error ──
        if diag::is_linker_error(trimmed) {
            self.in_diag_block = true;
            return true;
        }

        false
    }

    fn is_block_continuation(&mut self, line: &str, _block: &[String]) -> bool {
        let normalized = diag::normalize(line);
        let trimmed = normalized.trim();

        // End of diag block on various conditions
        if self.in_diag_block {
            // Blank line ends a diag block
            if trimmed.is_empty() {
                self.in_diag_block = false;
                return false;
            }
            // A new make error/section starts
            if is_make_error(trimmed) || is_make_final_error(trimmed) || is_no_rule_target(trimmed)
            {
                self.in_diag_block = false;
                return false;
            }
            // Progress line ends diag block
            if is_progress_line(trimmed) {
                self.in_diag_block = false;
                return false;
            }
            // Continuation lines (indentation, include stack, etc.)
            if diag::is_diag_continuation(trimmed) {
                return true;
            }
            // Error cascade continuation: lines starting with "make[" or "make:"
            if trimmed.starts_with("make[") || trimmed.starts_with("make:") {
                return true;
            }
            // Linker error continuations (e.g., ld's multi-line output)
            if trimmed.starts_with("  ") || trimmed.starts_with('\t') {
                return true;
            }
            // End of diag block
            self.in_diag_block = false;
            return false;
        }

        // Generic continuation: indented lines that follow an error cascade
        false
    }

    fn format_summary(&self, exit_code: i32, _raw: &str) -> Option<String> {
        let mut lines = Vec::new();

        if self.targets_failed == 0 && exit_code == 0 {
            if self.nothing_to_do && self.recipes_executed == 0 {
                lines.push("ok make: nothing to do".to_string());
            } else {
                let total = if self.edges_total > 0 {
                    self.edges_total
                } else {
                    self.recipes_executed
                };
                if self.dir_stack.is_empty() {
                    lines.push(format!("ok make: {} recipes, 0 failed", total));
                } else {
                    let dir = self.dir_stack.last().unwrap();
                    lines.push(format!("ok make: {} recipes, 0 failed  ({})", total, dir));
                }
            }
        } else {
            // Failure summary
            if !self.error_cascade.is_empty() {
                let last_error = self.error_cascade.last().unwrap();
                lines.push(format!(
                    "make: {} failed at {}",
                    self.targets_failed, last_error
                ));
                // Show cascade (up to 5 levels)
                let cascade_start = if self.error_cascade.len() > 5 {
                    self.error_cascade.len() - 5
                } else {
                    0
                };
                for err in &self.error_cascade[cascade_start..] {
                    lines.push(format!("  {}", err));
                }
            } else if !self.no_rule_targets.is_empty() {
                lines.push(format!(
                    "make: {} no-rule target(s)",
                    self.no_rule_targets.len()
                ));
                for tgt in &self.no_rule_targets {
                    lines.push(format!("  {}", tgt));
                }
            } else if exit_code != 0 {
                lines.push(format!(
                    "make: exited with code {} (no specific errors captured)",
                    exit_code
                ));
            } else {
                lines.push(format!("make: {} targets failed", self.targets_failed));
            }
        }

        // Warning summary
        if !self.warning_counts.is_empty() {
            let mut warnings: Vec<_> = self.warning_counts.iter().collect();
            warnings.sort_by(|a, b| b.1.cmp(a.1));
            let warn_parts: Vec<String> = warnings
                .iter()
                .map(|(flag, count)| format!("{} ×{}", flag, count))
                .collect();
            lines.push(format!("  warnings: {}", warn_parts.join(", ")));
        }

        Some(lines.join("\n") + "\n")
    }
}

// ── Public API ──

/// Run GNU Make with filtered output.
pub fn run(directory: Option<&str>, args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        let dir_str = directory.unwrap_or(".");
        eprintln!("make: running make -C {} {}", dir_str, args.join(" "));
    }

    let mut cmd = resolved_command("make");
    if let Some(dir) = directory {
        cmd.arg("-C").arg(dir);
    }
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = if let Some(dir) = directory {
        format!("-C {} {}", dir, args.join(" "))
    } else {
        args.join(" ")
    };

    runner::run_streamed(
        cmd,
        "make",
        &args_str,
        Box::new(BlockStreamFilter::new(MakeBuildHandler::new())),
        runner::RunOptions::with_tee("make"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::stream::StreamFilter;
    use crate::core::tracking::estimate_tokens;

    // Helper to run a filter through the BlockStreamFilter
    fn run_filter(filter: &mut dyn StreamFilter, input: &str, exit_code: i32) -> String {
        let mut output = String::new();
        for line in input.lines() {
            if let Some(s) = filter.feed_line(line) {
                output.push_str(&s);
            }
        }
        output.push_str(&filter.flush());
        if let Some(post) = filter.on_exit(exit_code, input) {
            output.push_str(&post);
        }
        output
    }

    fn filter_make(input: &str, exit_code: i32) -> String {
        let handler = MakeBuildHandler::new();
        let mut filter = BlockStreamFilter::new(handler);
        run_filter(&mut filter, input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_recipe_echo_gcc() {
        assert!(is_recipe_echo("gcc -c -o src/file.o src/file.c"));
        assert!(is_recipe_echo("g++ -std=c++20 -c file.cpp"));
    }

    #[test]
    fn test_is_recipe_echo_clang() {
        assert!(is_recipe_echo("clang++ -c foo.cpp -o foo.o"));
        assert!(is_recipe_echo("clang -c bar.c"));
    }

    #[test]
    fn test_is_recipe_echo_msvc() {
        assert!(is_recipe_echo("cl.exe /c /nologo /EHsc file.cpp"));
        assert!(is_recipe_echo("link.exe /OUT:prog.exe file.obj"));
    }

    #[test]
    fn test_is_recipe_echo_quoted_path() {
        assert!(is_recipe_echo(
            "\"C:/Program Files/LLVM/bin/clang-cl.exe\" /c file.cpp"
        ));
    }

    #[test]
    fn test_is_recipe_echo_path_like() {
        assert!(is_recipe_echo("/usr/bin/g++ -c file.cpp"));
    }

    #[test]
    fn test_is_recipe_echo_not() {
        assert!(!is_recipe_echo("make[1]: Entering directory '/src'"));
        assert!(!is_recipe_echo(
            "src/main.cpp:42:13: error: something wrong"
        ));
        assert!(!is_recipe_echo(""));
    }

    #[test]
    fn test_is_enter_leave_entering() {
        assert!(is_enter_leave(
            "make[1]: Entering directory '/home/user/project/build'"
        ));
    }

    #[test]
    fn test_is_enter_leave_leaving() {
        assert!(is_enter_leave(
            "make[2]: Leaving directory '/home/user/project/build/subdir'"
        ));
    }

    #[test]
    fn test_is_enter_leave_not() {
        assert!(!is_enter_leave("make: *** [target] Error 1"));
        assert!(!is_enter_leave("gcc -c file.c"));
    }

    #[test]
    fn test_is_make_error_normal() {
        assert!(is_make_error("make[1]: *** [src/file.o] Error 1"));
    }

    #[test]
    fn test_is_make_error_no_stars() {
        assert!(is_make_error("make[2]: [all] Error 2"));
    }

    #[test]
    fn test_is_make_error_not() {
        assert!(!is_make_error("gcc -c file.c"));
        assert!(!is_make_error("make: *** [Makefile:42] Error 1"));
    }

    #[test]
    fn test_is_make_final_error_normal() {
        assert!(is_make_final_error(
            "make: *** [Makefile:42: target] Error 2"
        ));
    }

    #[test]
    fn test_is_make_final_error_not() {
        assert!(!is_make_final_error("make[1]: *** [target] Error 1"));
        assert!(!is_make_final_error("make: Nothing to be done for 'all'"));
    }

    #[test]
    fn test_is_no_rule_target_explicit() {
        assert!(is_no_rule_target(
            "make[1]: *** No rule to make target 'missing.h', needed by 'file.o'.  Stop."
        ));
    }

    #[test]
    fn test_is_no_rule_target_plain() {
        assert!(is_no_rule_target("No rule to make target 'libfoo.a'"));
    }

    #[test]
    fn test_is_no_rule_target_not() {
        assert!(!is_no_rule_target("make[1]: *** [target] Error 1"));
    }

    #[test]
    fn test_is_automake_short_cc() {
        assert!(is_automake_short("  CC       src/file.o"));
    }

    #[test]
    fn test_is_automake_short_cxxld() {
        assert!(is_automake_short("  CXXLD    myapp"));
    }

    #[test]
    fn test_is_automake_short_ar() {
        assert!(is_automake_short("  AR       libfoo.a"));
    }

    #[test]
    fn test_is_automake_short_not() {
        assert!(!is_automake_short("gcc -c file.c"));
        assert!(!is_automake_short("  some random indent"));
    }

    // ── Success cases ──

    #[test]
    fn test_make_successful_build() {
        let input = "\
make[1]: Entering directory '/home/user/project/build'
gcc -c src/file1.c -o src/file1.o
g++ -c src/file2.cpp -o src/file2.o
g++ -c src/file3.cpp -o src/file3.o
ar cr libfoo.a src/file1.o src/file2.o src/file3.o
g++ -o myapp src/main.cpp.o -lfoo
make[1]: Leaving directory '/home/user/project/build'
";
        let result = filter_make(input, 0);
        assert!(
            result.contains("ok make: 5 recipes, 0 failed"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_make_nothing_to_do() {
        let input = "make: Nothing to be done for 'all'.\n";
        let result = filter_make(input, 0);
        assert!(result.contains("nothing to do"), "got: {}", result);
    }

    #[test]
    fn test_make_success_recursive() {
        let input = "\
make[1]: Entering directory '/project/build'
make[2]: Entering directory '/project/build/sub'
gcc -c sub.c -o sub.o
ar cr libsub.a sub.o
make[2]: Leaving directory '/project/build/sub'
gcc -c main.c -o main.o
gcc -o prog main.o sub/libsub.a
make[1]: Leaving directory '/project/build'
";
        let result = filter_make(input, 0);
        assert!(
            result.contains("ok make: 4 recipes, 0 failed"),
            "got: {}",
            result
        );
    }

    // ── Failure cases ──

    #[test]
    fn test_make_single_error() {
        let input = "\
make[1]: Entering directory '/project/build'
gcc -c good.c -o good.o
gcc -c bad.c -o bad.o
bad.c:5:13: error: 'x' undeclared (first use in this function)
bad.c:5:13: note: each undeclared identifier is reported only once
make[1]: *** [bad.o] Error 1
make[1]: Leaving directory '/project/build'
make: *** [all] Error 2
";
        let result = filter_make(input, 1);
        assert!(result.contains("failed at"), "got: {}", result);
        assert!(result.contains("error: 'x' undeclared"), "got: {}", result);
        assert!(
            !result.contains("gcc -c good.c"),
            "recipe echoes should be skipped, got: {}",
            result
        );
    }

    #[test]
    fn test_make_error_cascade() {
        let input = "\
make[1]: Entering directory '/build'
gcc -c a.c -o a.o
a.c:1:1: error: first error in a.c
make[1]: *** [a.o] Error 1
make[1]: *** Waiting for unfinished jobs....
gcc -c b.c -o b.o
b.c:2:1: error: error in b.c
make[1]: *** [b.o] Error 1
make[1]: Leaving directory '/build'
make: *** [all] Error 2
";
        let result = filter_make(input, 1);
        assert!(result.contains("failed"), "got: {}", result);
        assert!(result.contains("first error in a.c"), "got: {}", result);
        assert!(result.contains("error in b.c"), "got: {}", result);
    }

    #[test]
    fn test_make_no_rule_target() {
        let input = "\
make[1]: Entering directory '/build'
make[1]: *** No rule to make target 'config.h', needed by 'main.o'.  Stop.
make[1]: Leaving directory '/build'
";
        let result = filter_make(input, 1);
        assert!(result.contains("no-rule target"), "got: {}", result);
    }

    #[test]
    fn test_make_linker_error() {
        let input = "\
gcc -c main.c -o main.o
gcc -o prog main.o
/usr/bin/ld: main.o: in function `main':
main.c:(.text+0x1e): undefined reference to `missing_func'
collect2: error: ld returned 1 exit status
make: *** [prog] Error 1
";
        let result = filter_make(input, 1);
        assert!(
            result.contains("undefined reference to `missing_func'"),
            "got: {}",
            result
        );
    }

    // ── Edge cases ──

    #[test]
    fn test_make_ansi_stripped() {
        let input = "\x1b[31mgcc -c bad.c -o bad.o\x1b[0m\n\
                      \x1b[1;31mbad.c:1:1: error: bad code\x1b[0m\n\
                      make: *** [bad.o] Error 1\n";
        let result = filter_make(input, 1);
        // The error content should be captured (ANSI may pass through on
        // block-start lines, matching the existing ninja handler behaviour).
        assert!(result.contains("error: bad code"), "got: {}", result);
        assert!(
            result.contains("make: *** [bad.o] Error 1"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_make_empty_input() {
        let result = filter_make("", 0);
        assert!(
            result.contains("ok make"),
            "should have a summary, got: '{}'",
            result
        );
    }

    #[test]
    fn test_make_keep_going() {
        let input = "\
gcc -c a.c -o a.o
a.c:1:1: error: error in a
make[1]: *** [a.o] Error 1
make[1]: Target 'all' not remade because of errors.
gcc -c b.c -o b.o
b.c:1:1: error: error in b
make[1]: *** [b.o] Error 1
make: *** [all] Error 2
";
        let result = filter_make(input, 1);
        assert!(
            result.contains("failed"),
            "keep-going should report failures, got: {}",
            result
        );
        assert!(result.contains("error: error in a"), "got: {}", result);
        assert!(result.contains("error: error in b"), "got: {}", result);
    }

    #[test]
    fn test_make_output_sync() {
        // With output-sync, lines may be reordered but recipe echoes and
        // diagnostics still appear.
        let input = "\
make[1]: Entering directory '/build'
gcc -c a.c -o a.o
make[1]: Leaving directory '/build'
a.c:1:1: error: delayed error
make: *** [a.o] Error 1
";
        let result = filter_make(input, 1);
        assert!(result.contains("error: delayed error"), "got: {}", result);
        assert!(
            !result.contains("gcc -c a.c"),
            "recipe echo should be skipped, got: {}",
            result
        );
    }

    #[test]
    fn test_make_token_savings_above_70pct() {
        let mut input = String::new();
        // Generate many recipe echoes
        for i in 1..=200 {
            input.push_str(&format!(
                "gcc -c -o src/file{}.o src/file{}.cpp -Iinclude -I/usr/local/include -std=c++20 -O2 -Wall -Wextra -DNDEBUG\n",
                i, i
            ));
        }
        // Add a few errors
        input.push_str("src/file42.cpp:10:13: error: 'bad_func' was not declared in this scope\n");
        input.push_str("src/file42.cpp:10:13: note: suggested alternative: 'good_func'\n");
        input.push_str("make[1]: *** [src/file42.o] Error 1\n");
        input.push_str("make: *** [all] Error 2\n");

        let result = filter_make(&input, 1);
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
