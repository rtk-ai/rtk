//! Shared compiler-diagnostic classification used by all build-tool filters.

#![allow(dead_code)]
//!
//! Provides a single source of truth for GCC/Clang, MSVC, and linker diagnostic
//! detection, warning-flag extraction, message dedup, and line-continuation logic.

use crate::core::utils::strip_ansi;

/// Regex patterns compiled once per pattern.
macro_rules! lazy_re {
    ($re:literal) => {{
        lazy_static::lazy_static! {
            static ref RE: regex::Regex = regex::Regex::new($re).unwrap();
        }
        &*RE
    }};
}
#[allow(unused_imports)]
pub(crate) use lazy_re;

// ── Normalization ──

/// Strip ANSI escape sequences AND carriage returns (CR).
/// Call this FIRST before any classification.
pub fn normalize(line: &str) -> String {
    strip_ansi(line).replace('\r', "")
}

// ── GCC/Clang Diagnostic Detection ──

/// Check if a line is a GCC/Clang diagnostic: `file:line:col: severity:` or `file:line: severity:`.
/// Severity ∈ {error, warning, note, fatal error, remark}.
///
/// # Examples
/// - `src/main.cpp:42:13: error: 'foo' was not declared`
/// - `src/main.cpp:42: error: 'foo' was not declared` (no column)
/// - `src/main.cpp:42:10: warning: unused parameter [-Wunused-parameter]`
pub fn is_gcc_diag(line: &str) -> bool {
    let trimmed = line.trim_start();
    if let Some(colon) = trimmed.find(':') {
        let before_first = &trimmed[..colon];
        // Must not contain spaces (simple file path)
        if before_first.contains(' ') {
            return false;
        }
        // After first colon, expect digits
        let after = &trimmed[colon + 1..];
        if let Some(second_colon) = after.find(':') {
            let digits_part = &after[..second_colon];
            if digits_part.parse::<usize>().is_ok() {
                // Could be: file:line:
                let after_second = &after[second_colon + 1..];
                if let Some(third_colon) = after_second.find(':') {
                    let col_part = &after_second[..third_colon];
                    if col_part.parse::<usize>().is_ok() {
                        let after_third = after_second[third_colon + 1..].trim_start();
                        return after_third.starts_with("error")
                            || after_third.starts_with("warning")
                            || after_third.starts_with("note")
                            || after_third.starts_with("fatal error");
                    }
                }
                // Maybe just file:line: severity (no column)
                let after_second = after_second.trim_start();
                return after_second.starts_with("error")
                    || after_second.starts_with("warning")
                    || after_second.starts_with("note")
                    || after_second.starts_with("fatal error");
            }
        }
    }
    false
}

// ── MSVC Diagnostic Detection ──

/// Check MSVC format: `file(line[,col]): severity code: message`.
///
/// # Examples
/// - `src\main.cpp(42): error C2065: 'x' : undeclared identifier`
/// - `src/main.cpp(42,5): error C2039: 'visit' is not a member`
/// - `src\main.cpp(42): warning C4100: 'x': unreferenced formal parameter`
pub fn is_msvc_diag(line: &str) -> bool {
    if let Some(paren) = line.find('(') {
        if let Some(close_paren) = line.find(')') {
            let line_num = &line[paren + 1..close_paren];
            // Handle optional column: "42,5" → "42"
            let first_num = if let Some(comma) = line_num.find(',') {
                &line_num[..comma]
            } else {
                line_num
            };
            if first_num.parse::<usize>().is_ok() {
                let after = line[close_paren + 1..].trim_start();
                if after.starts_with(": ") || after.starts_with(" : ") {
                    let after_colon = after.trim_start_matches(':').trim_start();
                    return after_colon.starts_with("error")
                        || after_colon.starts_with("warning")
                        || after_colon.starts_with("note")
                        || after_colon.starts_with("fatal error");
                }
            }
        }
    }
    false
}

// ── Unified Diagnostic Detection ──

/// Returns true if line matches GCC/Clang OR MSVC diagnostic format.
pub fn is_compiler_diag(line: &str) -> bool {
    is_gcc_diag(line) || is_msvc_diag(line)
}

/// Returns the severity if the line is a compiler diagnostic with that severity.
/// Handles both GCC/Clang (`: severity:`) and MSVC (`file(line): severity CODE:`).
pub fn diag_has_severity(line: &str, severity: &str) -> bool {
    let lower = line.to_lowercase();
    // Try MSVC format first: file(line): severity ...
    if let Some(paren) = line.find('(') {
        if let Some(close_paren) = line.find(')') {
            if line[paren + 1..close_paren]
                .split(',')
                .next()
                .unwrap_or("")
                .parse::<usize>()
                .is_ok()
            {
                let after = line[close_paren + 1..].trim_start();
                let after_colon = after.trim_start_matches(':').trim_start();
                if after_colon.to_lowercase().starts_with(severity) {
                    return true;
                }
            }
        }
    }
    // GCC/Clang format: the severity word appears right before a colon
    // e.g. "file:line:col: severity: message" or "file:line: severity: message"
    let target = format!(": {}:", severity);
    lower.contains(&target)
}

// ── Linker Error Detection ──

/// Check for linker errors: `undefined reference to`, `cannot find -l`,
/// `ld returned N exit status`, `error LNK\d+`, `fatal error LNK\d+`,
/// `relocation ... against ... can not be used`.
///
/// # Patterns matched
/// - `/usr/bin/ld: <obj>: undefined reference to '<sym>'`
/// - `<file>:(.text+0x1e): undefined reference to ...`
/// - `cannot find -l<lib>`
/// - `collect2: error: ld returned 1 exit status`
/// - `ld.lld: error: <msg>`
/// - `error LNK2001: unresolved external symbol <sym>` (MSVC linker)
/// - `fatal error LNK1120: N unresolved externals` (MSVC linker)
/// - `relocation R_X86_64_32 against ... can not be used when making a PIE object`
pub fn is_linker_error(line: &str) -> bool {
    let trimmed = line.trim();
    // GNU ld / lld / gold / mold
    if trimmed.contains("undefined reference to")
        || trimmed.contains("cannot find -l")
        || trimmed.contains("ld returned")
        || trimmed.starts_with("ld.")
        || trimmed.starts_with("collect2:")
    {
        return true;
    }
    // MSVC linker
    if lazy_re!(r"\b(error|fatal error) LNK\d+:").is_match(trimmed) {
        return true;
    }
    // Linker relocation errors
    if trimmed.contains("relocation ") && trimmed.contains(" against ") {
        return true;
    }
    false
}

// ── Warning Flag Extraction ──

/// Extract warning flag from `[-Wflag]` (GCC/Clang) or `CXXXX` (MSVC).
/// Returns `None` if no flag found.
///
/// # Examples
/// - `[-Wunused-parameter]` → `Some("-Wunused-parameter")`
/// - `warning C4100:` → `Some("C4100")`
pub fn extract_warning_flag(line: &str) -> Option<String> {
    // Match patterns like [-Wunused-parameter] (GCC/Clang)
    if let Some(start) = line.rfind('[') {
        if let Some(end) = line[start..].find(']') {
            let flag = &line[start + 1..start + end];
            if flag.starts_with("-W") {
                return Some(flag.to_string());
            }
        }
    }
    // MSVC warning codes: CXXXX
    extract_msvc_code(line)
}

/// Extract MSVC warning/error code: `CXXXX` from `warning CXXXX:` or `error CXXXX:`.
pub fn extract_msvc_code(line: &str) -> Option<String> {
    let upper = line.to_uppercase();
    for prefix in &["WARNING C", "ERROR C"] {
        if let Some(pos) = upper.find(prefix) {
            let after = &upper[pos + prefix.len()..];
            let code: String = after.chars().take_while(|c| c.is_alphanumeric()).collect();
            if code.len() >= 4 {
                return Some(format!("C{}", code));
            }
        }
    }
    None
}

// ── Message Extraction for Dedup ──

/// Extract the core message body from a diagnostic line (after file:line:col: severity:).
/// Used for deduplication: identical messages across translation units are collapsed.
pub fn extract_diag_message(line: &str) -> String {
    let line = strip_ansi(line);

    // Try MSVC format first: file(line): severity code: message
    if let Some(close_paren) = line.find(')') {
        let after = line[close_paren + 1..].trim_start();
        // after = "error C2039: 'visit' is not a member of 'luisa::compute::dx::DXCodegen'"
        let after_colon = after.trim_start_matches(':').trim_start();
        if let Some(first_colon) = after_colon.find(':') {
            let code_part = &after_colon[..first_colon];
            if code_part.contains("error")
                || code_part.contains("warning")
                || code_part.contains("note")
                || code_part.contains("fatal")
            {
                let msg = after_colon[first_colon + 1..].trim();
                if !msg.is_empty() {
                    return msg.to_string();
                }
            }
        }
    }

    // Try GCC/Clang format: file:line:col: severity: message
    // Find the severity token from the right side to avoid splitting on
    // drive-letter colons (e.g. "C:/foo/bar.cpp:42:13: error: msg").
    let severities = [": fatal error:", ": error:", ": warning:", ": note:"];
    let mut best_pos: Option<(usize, &str)> = None;
    for sev in &severities {
        if let Some(pos) = line.rfind(sev) {
            let candidate = (pos, *sev);
            if best_pos.is_none_or(|(p, _)| candidate.0 > p) {
                best_pos = Some(candidate);
            }
        }
    }
    if let Some((pos, sev)) = best_pos {
        let msg = line[pos + sev.len()..].trim();
        if !msg.is_empty() {
            return msg.to_string();
        }
        return sev.trim_start_matches(": ").to_string();
    }

    line
}

// ── Line Continuation Detection ──

/// Returns true if this line is a continuation of a preceding diagnostic block.
///
/// Covers: indented lines, `In file included from`, `from ` (include stack),
/// caret/underline lines (`|`, `^`, `~`), `required from` (template backtrace),
/// `Call Stack`, and blank-line separators inside multi-line messages.
pub fn is_diag_continuation(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return true; // blank line separates multi-line messages
    }
    line.starts_with(' ')
        || line.starts_with('\t')
        || trimmed.starts_with("In file")
        || trimmed.starts_with("from ")
        || trimmed.starts_with('|')
        || trimmed.starts_with('^')
        || trimmed.starts_with('~')
        || trimmed.starts_with("--")
        || trimmed.starts_with("required from")
        || trimmed.starts_with("note:")
        || trimmed.starts_with("Call Stack")
}

// ── Noise Line Detection ──

/// MSVC `/showIncludes` flood: `Note: including file: <path>`.
pub fn is_show_includes(line: &str) -> bool {
    let trimmed = line.trim();
    trimmed.starts_with("Note: including file:")
}

/// MSVC `cl.exe` bare filename echo before diagnostics (when `/nologo` absent).
/// Matches lines that are just a source filename (no colon, no other content).
/// "Just a filename" means: no spaces after trimming, ends with `.cpp`/`.cxx`/`.c`/`.cc`.
pub fn is_bare_source_file(line: &str) -> bool {
    let trimmed = line.trim();
    if trimmed.is_empty() || trimmed.contains(' ') {
        return false;
    }
    lazy_re!(r"\.(cpp|cxx|c|cc|h|hpp|hxx)$").is_match(trimmed)
}

#[cfg(test)]
mod tests {
    use super::*;

    // ── normalize ──

    #[test]
    fn test_normalize_ansi_stripped() {
        assert_eq!(normalize("\x1b[31merror\x1b[0m"), "error");
    }

    #[test]
    fn test_normalize_cr_stripped() {
        assert_eq!(normalize("line\r\n"), "line\n");
    }

    #[test]
    fn test_normalize_both() {
        assert_eq!(normalize("\x1b[31merror\r\n\x1b[0m"), "error\n");
    }

    // ── is_gcc_diag ──

    #[test]
    fn test_gcc_error_with_col() {
        assert!(is_gcc_diag(
            "src/core/hash.h:42:13: error: static assertion failed: Hash must be specialized"
        ));
    }

    #[test]
    fn test_gcc_error_no_col() {
        assert!(is_gcc_diag("src/main.cpp:42: error: no matching function"));
    }

    #[test]
    fn test_gcc_warning() {
        assert!(is_gcc_diag(
            "src/api/api.cpp:42:10: warning: unused parameter 'device_id' [-Wunused-parameter]"
        ));
    }

    #[test]
    fn test_gcc_note() {
        assert!(is_gcc_diag(
            "src/core/hash.h:42:13: note: in instantiation of template class"
        ));
    }

    #[test]
    fn test_gcc_fatal() {
        assert!(is_gcc_diag(
            "src/main.cpp:1:1: fatal error: No such file or directory"
        ));
    }

    #[test]
    fn test_gcc_not_diag() {
        assert!(!is_gcc_diag(
            "FAILED: src/core/CMakeFiles/lc_core.dir/hash.cpp.o"
        ));
        assert!(!is_gcc_diag("[1/456] Building CXX object ..."));
        assert!(!is_gcc_diag("ninja: build stopped: subcommand failed."));
        assert!(!is_gcc_diag(""));
    }

    #[test]
    fn test_gcc_not_windows_path_with_space() {
        // A Windows path like "C:\Program Files\..." with a space should not match
        assert!(!is_gcc_diag(
            "C:\\Program Files\\foo\\bar.cpp:42: error: something"
        ));
    }

    // ── is_msvc_diag ──

    #[test]
    fn test_msvc_error() {
        assert!(is_msvc_diag(
            "src/backends/dx/dx_codegen.cpp(88): error C2039: 'visit': is not a member"
        ));
    }

    #[test]
    fn test_msvc_warning() {
        assert!(is_msvc_diag(
            "src\\main.cpp(42): warning C4100: 'x': unreferenced formal parameter"
        ));
    }

    #[test]
    fn test_msvc_with_col() {
        assert!(is_msvc_diag(
            "src/main.cpp(42,5): error C2039: 'visit' is not a member"
        ));
    }

    #[test]
    fn test_msvc_not_diag() {
        assert!(!is_msvc_diag("FAILED: src/core/hash.cpp.o"));
        assert!(!is_msvc_diag("[1/456] Building CXX object ..."));
    }

    #[test]
    fn test_msvc_windows_path() {
        assert!(is_msvc_diag(
            "C:\\Users\\test\\project\\src\\main.cpp(42): error C2065: 'x' : undeclared identifier"
        ));
    }

    // ── is_compiler_diag ──

    #[test]
    fn test_unified_gcc() {
        assert!(is_compiler_diag(
            "src/core/hash.h:42:13: error: static assertion failed"
        ));
    }

    #[test]
    fn test_unified_msvc() {
        assert!(is_compiler_diag(
            "src/main.cpp(88): error C2039: 'visit': is not a member"
        ));
    }

    #[test]
    fn test_unified_not() {
        assert!(!is_compiler_diag("FAILED: src/core/hash.cpp.o"));
        assert!(!is_compiler_diag("[1/456] Building CXX object ..."));
        assert!(!is_compiler_diag(""));
    }

    // ── is_linker_error ──

    #[test]
    fn test_ld_undefined_ref() {
        assert!(is_linker_error(
            "clock.cpp:(.text+0x1e): undefined reference to 'start_impl'"
        ));
    }

    #[test]
    fn test_ld_cannot_find_l() {
        assert!(is_linker_error("/usr/bin/ld: cannot find -lfoo"));
    }

    #[test]
    fn test_ld_returned() {
        assert!(is_linker_error(
            "collect2: error: ld returned 1 exit status"
        ));
    }

    #[test]
    fn test_msvc_lnk2001() {
        assert!(is_linker_error(
            "error LNK2001: unresolved external symbol __imp_MessageBoxA"
        ));
    }

    #[test]
    fn test_msvc_lnk1120() {
        assert!(is_linker_error(
            "fatal error LNK1120: 1 unresolved externals"
        ));
    }

    #[test]
    fn test_relocation_error() {
        assert!(is_linker_error(
            "relocation R_X86_64_32 against `.rodata' can not be used when making a PIE object"
        ));
    }

    #[test]
    fn test_not_linker_error() {
        assert!(!is_linker_error(
            "src/main.cpp:42:13: error: static assertion failed"
        ));
    }

    // ── extract_warning_flag ──

    #[test]
    #[allow(non_snake_case)]
    fn test_gcc_Wflag() {
        assert_eq!(
            extract_warning_flag(
                "src/api/api.cpp:42:10: warning: unused parameter [-Wunused-parameter]"
            ),
            Some("-Wunused-parameter".to_string())
        );
    }

    #[test]
    #[allow(non_snake_case)]
    fn test_msvc_Ccode() {
        assert_eq!(
            extract_warning_flag("warning C4100: 'x': unreferenced formal parameter"),
            Some("C4100".to_string())
        );
    }

    #[test]
    fn test_no_flag() {
        assert_eq!(
            extract_warning_flag("just a regular line without flag"),
            None
        );
    }

    // ── extract_diag_message ──

    #[test]
    fn test_gcc_message() {
        let msg = extract_diag_message(
            "src/core/hash.h:42:13: error: static assertion failed: Hash must be specialized",
        );
        assert_eq!(
            msg, "static assertion failed: Hash must be specialized",
            "should capture full message including colons, got: '{}'",
            msg
        );
    }

    #[test]
    fn test_msvc_message() {
        let msg = extract_diag_message(
            "src/backends/dx/dx_codegen.cpp(88): error C2039: 'visit' is not a member of 'luisa::compute::dx::DXCodegen'"
        );
        assert_eq!(
            msg, "'visit' is not a member of 'luisa::compute::dx::DXCodegen'",
            "should capture full message including ::, got: '{}'",
            msg
        );
    }

    #[test]
    fn test_msvc_empty_message() {
        let msg = extract_diag_message("src/main.cpp(42): warning C4100:");
        assert!(!msg.is_empty(), "should not be empty, got: '{}'", msg);
    }

    // ── is_diag_continuation ──

    #[test]
    fn test_indent_space() {
        assert!(is_diag_continuation("  detail line"));
    }

    #[test]
    fn test_indent_tab() {
        assert!(is_diag_continuation("\tdetail line"));
    }

    #[test]
    fn test_in_file_included() {
        assert!(is_diag_continuation("In file included from a.h:1,"));
    }

    #[test]
    fn test_from_continuation() {
        assert!(is_diag_continuation("from b.cpp:7:"));
    }

    #[test]
    fn test_caret_line() {
        assert!(is_diag_continuation("      |   ^~~"));
    }

    #[test]
    fn test_required_from() {
        assert!(is_diag_continuation(
            "required from 'void foo() [with T = int]'"
        ));
    }

    #[test]
    fn test_call_stack() {
        assert!(is_diag_continuation("Call Stack (most recent call first):"));
    }

    #[test]
    fn test_blank_line() {
        assert!(is_diag_continuation(""));
    }

    #[test]
    fn test_note_standalone() {
        assert!(is_diag_continuation("note: candidate function not viable"));
    }

    #[test]
    fn test_not_continuation() {
        assert!(!is_diag_continuation("FAILED: src/core/hash.cpp.o"));
    }

    // ── is_show_includes ──

    #[test]
    fn test_show_includes() {
        assert!(is_show_includes(
            "Note: including file: C:\\Program Files\\foo\\bar.h"
        ));
    }

    #[test]
    fn test_not_show_includes() {
        assert!(!is_show_includes("note: candidate function not viable"));
    }

    // ── is_bare_source_file ──

    #[test]
    fn test_bare_source_file_cpp() {
        assert!(is_bare_source_file("clock.cpp"));
    }

    #[test]
    fn test_bare_source_file_with_path() {
        // Has no spaces and ends with .cpp
        assert!(is_bare_source_file("src/core/clock.cpp"));
    }

    #[test]
    fn test_not_bare_source_file_with_spaces() {
        assert!(!is_bare_source_file("cl.exe /c clock.cpp"));
    }

    #[test]
    fn test_not_bare_source_file_empty() {
        assert!(!is_bare_source_file(""));
    }
}
