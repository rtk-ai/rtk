//! Filters xmake build/configure output — compiler command lines stripped,
//! progress lines counted, errors/warnings kept verbatim.
//!
//! Cross-platform: handles MSVC (Windows), GCC (Linux), and Clang (macOS) formats.
//! Uses structural heuristics (length > 200, tool invocation patterns, flag patterns)
//! to detect full compiler/linker command lines without hard-coding tool paths.

use super::diag;
use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;
use std::collections::HashMap;

/// Track state during xmake output parsing.
struct XmakeStats {
    /// Section tracking
    section: String, // "config" or "build"

    /// Counters
    compile_count: usize,
    archive_count: usize,
    link_count: usize,
    unity_gen_count: usize,
    generate_count: usize,

    /// Target tracking: target_name -> (compile, archive, link)
    targets: HashMap<String, (usize, usize, usize)>,

    /// Error lines (verbatim)
    errors: Vec<String>,
    in_error_block: bool,

    /// Warning tracking: flag -> count
    warning_counts: HashMap<String, usize>,

    /// Dedup tracking for repeated diagnostics
    seen_diagnostics: HashMap<String, usize>,

    /// Exit codes
    config_exit_code: Option<i32>,
    build_exit_code: Option<i32>,

    /// Section headers
    section_headers: Vec<String>,

    /// Platform & toolchain info
    platform: String,
    compiler_family: String,
    build_modes: Vec<String>, // "debug", "release", etc.

    /// Raw lines kept for the output
    kept_lines: Vec<String>,

    /// Verbosity level (for per-target detail)
    verbose: u8,
}

impl XmakeStats {
    fn new() -> Self {
        Self {
            section: String::new(),
            compile_count: 0,
            archive_count: 0,
            link_count: 0,
            unity_gen_count: 0,
            generate_count: 0,
            targets: HashMap::new(),
            errors: Vec::new(),
            in_error_block: false,
            warning_counts: HashMap::new(),
            seen_diagnostics: HashMap::new(),
            config_exit_code: None,
            build_exit_code: None,
            section_headers: Vec::new(),
            platform: String::new(),
            compiler_family: String::new(),
            build_modes: Vec::new(),
            kept_lines: Vec::new(),
            verbose: 0,
        }
    }

    fn track_dedup(&mut self, diag_line: &str) -> bool {
        let msg = diag::extract_diag_message(diag_line);
        let count = self.seen_diagnostics.entry(msg).or_insert(0);
        *count += 1;
        *count <= 3
    }
}

// ─── Line Classification Helpers ───

/// Returns `true` if the line is an xmake section header like `=== XMAKE DEBUG CONFIG ===`.
fn is_section_header(trimmed: &str) -> bool {
    trimmed.starts_with("=== XMAKE ") && trimmed.ends_with(" ===")
}

/// Returns `true` if the line is an xmake probe line (`checking for ...`).
/// These are typically noise, but some contain extractable info (platform, compiler).
fn is_probe_line(trimmed: &str) -> bool {
    trimmed.starts_with("checking for ")
}

/// Extract platform info from `checking for platform ... <os> (<arch>)`.
fn extract_platform(trimmed: &str) -> Option<String> {
    if !trimmed.starts_with("checking for platform ... ") {
        return None;
    }
    let after = trimmed.strip_prefix("checking for platform ... ")?;
    // Format: "windows (x64)", "linux (x86_64)", "macosx (arm64)"
    let platform = after.trim();
    if platform.is_empty() || platform.contains("...") {
        return None;
    }
    Some(platform.to_string())
}

/// Extract compiler family from probe line like
/// `checking for Microsoft C/C++ Compiler ... ok` (MSVC)
/// `checking for gcc ... /usr/bin/gcc` (GCC)
/// `checking for clang ... /usr/bin/clang` (Clang)
fn extract_compiler(trimmed: &str) -> Option<String> {
    if trimmed.starts_with("checking for Microsoft C/C++ Compiler") {
        Some("msvc".to_string())
    } else if trimmed.starts_with("checking for gcc") {
        Some("gcc".to_string())
    } else if trimmed.starts_with("checking for clang") {
        Some("clang".to_string())
    } else {
        None
    }
}

/// Returns `true` for `generating.unityfile <path>` lines.
fn is_unity_gen_line(trimmed: &str) -> bool {
    trimmed.starts_with("generating.unityfile")
}

/// Returns `true` for `xmake lua cli.binutils.bin2obj ...` tool invocations.
fn is_bin2obj_command(trimmed: &str) -> bool {
    trimmed.starts_with("xmake lua cli.binutils.bin2obj ")
}

/// Returns `true` for bin2obj internal output lines that should be dropped:
/// - `running imported module cli.binutils.bin2obj ...`
/// - `converting binary file ... to coff object file ...`
/// - `.obj generated!` / `.h.obj generated!` completion lines
/// - Numbered argument dump lines (` 1: "-i"`, ` 2: "-o"`, etc.)
fn is_bin2obj_noise(trimmed: &str) -> bool {
    // Module execution noise: "running imported module ..."
    if trimmed.starts_with("running imported module ") {
        return true;
    }
    // "with args:" continuation (from interleaved parallel output)
    if trimmed.starts_with("with args:") {
        return true;
    }
    // "converting binary file ... to coff object file ..."
    if trimmed.starts_with("converting binary file ") && trimmed.contains(" to coff object file ") {
        return true;
    }
    // ".obj generated!" or similar completion messages
    if trimmed.ends_with(" generated!") {
        return true;
    }
    // Numbered argument dumps: " 1: \"-i\"", " 2: \"-o\"", etc.
    diag::lazy_re!("^\\s*\\d+:\\s*\"").is_match(trimmed)
}

/// Returns `true` for probe test lines like `> cl.exe` with flags.
/// These are compiler/linker probe diagnostic lines that xmake emits during
/// `checking for flags (...)` sequences.
fn is_probe_test_line(trimmed: &str) -> bool {
    trimmed.starts_with("> ")
        && trimmed.len() > 2
        // Ensure it's not just "> " with nothing meaningful
        && !trimmed.starts_with("> [")
}

/// Information extracted from a progress line.
struct ProgressInfo {
    target: String,
    action: String, // "compiling", "archiving", "linking"
    _mode: String,  // "debug" or "release"
}

/// Parse `[  N%]: <target> action.mode file_path`
fn is_progress_line(trimmed: &str) -> Option<ProgressInfo> {
    // Match: [  N%]: <target> action.mode ...
    // Examples:
    // [  7%]: <mimalloc> compiling.debug path/to/file.c
    // [ 25%]: <lc-yyjson> archiving.debug luisa-ext-lc-yyjson.lib
    // [ 26%]: <glfw> linking.debug luisa-ext-glfw.dll
    let re = diag::lazy_re!(
        r"^\[\s*\d+%\]:\s*<([^>]+)>\s+(compiling|archiving|linking|generating)\.(\w+)\s"
    );
    let caps = re.captures(trimmed)?;
    Some(ProgressInfo {
        target: caps.get(1)?.as_str().to_string(),
        action: caps.get(2)?.as_str().to_string(),
        _mode: caps.get(3)?.as_str().to_string(),
    })
}

/// Cross-platform full command line detection.
/// Returns `true` for compiler/linker/archiver command lines that should be dropped.
///
/// Three gates (all must pass):
/// 1. Length > 200 chars
/// 2. Looks like a tool invocation (starts with quoted path or bare tool binary)
/// 3. Contains compiler/linker flag patterns
fn is_full_command_line(trimmed: &str) -> bool {
    // Gate 1: Length gate
    if trimmed.len() <= 200 {
        return false;
    }

    // Gate 2: Tool invocation gate
    let looks_like_tool = trimmed.starts_with('"')
        || diag::lazy_re!(r"^(/[A-Za-z_][A-Za-z0-9_\-/]*)?\b(g\+\+|gcc|clang\+\+|clang|cc|c\+\+|cl\.exe|link\.exe|ld|lld|ar)\b").is_match(trimmed)
        || diag::lazy_re!(r"^[A-Za-z]:\\").is_match(trimmed) // Windows path start like C:\
            && trimmed.contains("cl.exe")
        || diag::lazy_re!(r"^[A-Za-z]:\\").is_match(trimmed)
            && trimmed.contains("link.exe");

    if !looks_like_tool {
        return false;
    }

    // Gate 3: Flags gate — must contain compiler/linker flag patterns
    // Also matches archiver flags (rcs) and miscellaneous build flags
    diag::lazy_re!(r"(?:\s-[DIo]\b|\s-Fo\b|\s-lib\b|\s-dll\b|\s-shared\b|\s/EHs|\s/GS|\s/Gd|\s/Zc:|\s-std=c\+\+|\s-std=c\b|\s-fPIC\b|\s-fPIE\b|\s-nologo\b|\s-MD\b|\srcs\b)").is_match(trimmed)
}

/// Check if a line is an error line (xmake aggregate or compiler diagnostic).
fn is_error_line(trimmed: &str) -> bool {
    // xmake aggregate error header: "error: <file>"
    if trimmed.starts_with("error:") {
        return true;
    }
    diag::is_compiler_diag(trimmed) && diag::diag_has_severity(trimmed, "error")
}

/// Check if a line is a warning line (compiler diagnostic).
fn is_warning_line(trimmed: &str) -> bool {
    diag::is_compiler_diag(trimmed) && diag::diag_has_severity(trimmed, "warning")
}

/// Check if a line is a note line (compiler diagnostic continuation).
fn is_note_line(trimmed: &str) -> bool {
    diag::is_compiler_diag(trimmed) && diag::diag_has_severity(trimmed, "note")
}

/// Check if line is an exit code line: `Config exit code: N` or `Build exit code: N`.
fn is_exit_code_line(trimmed: &str) -> Option<(String, i32)> {
    if let Some(code_str) = trimmed.strip_prefix("Config exit code: ") {
        let code = code_str.trim().parse::<i32>().ok()?;
        return Some(("config".to_string(), code));
    }
    if let Some(code_str) = trimmed.strip_prefix("Build exit code: ") {
        let code = code_str.trim().parse::<i32>().ok()?;
        return Some(("build".to_string(), code));
    }
    None
}

// ─── Main Filter Function ───

/// Filter xmake build/configure output.
#[allow(dead_code)]
fn filter_xmake_output(input: &str, _exit_code: i32) -> String {
    filter_xmake_output_verbose(input, _exit_code, 0)
}

/// Filter xmake build/configure output with verbosity control.
fn filter_xmake_output_verbose(input: &str, _exit_code: i32, verbose: u8) -> String {
    let ansi_free = strip_ansi(input);
    let mut stats = XmakeStats::new();
    stats.verbose = verbose;

    for line in ansi_free.lines() {
        let trimmed = line.trim();

        // Skip blank lines
        if trimmed.is_empty() {
            continue;
        }

        // Section headers
        if is_section_header(trimmed) {
            stats.section_headers.push(trimmed.to_string());
            if trimmed.contains("CONFIG") {
                stats.section = "config".to_string();
            } else if trimmed.contains("BUILD") {
                stats.section = "build".to_string();
            }
            continue;
        }

        // Exit code lines
        if let Some((section, code)) = is_exit_code_line(trimmed) {
            match section.as_str() {
                "config" => stats.config_exit_code = Some(code),
                "build" => stats.build_exit_code = Some(code),
                _ => {}
            }
            continue;
        }

        // Error lines — keep verbatim regardless of section
        if is_error_line(trimmed) {
            stats.in_error_block = true;
            // Track warnings inside error blocks too
            if trimmed.to_lowercase().contains("warning:") {
                if let Some(flag) = diag::extract_warning_flag(trimmed) {
                    *stats.warning_counts.entry(flag).or_insert(0) += 1;
                }
            }
            if stats.track_dedup(trimmed) {
                stats.errors.push(trimmed.to_string());
                stats.kept_lines.push(trimmed.to_string());
            }
            continue;
        }

        // Warning lines — keep verbatim, dedup by flag
        if is_warning_line(trimmed) {
            stats.in_error_block = false;
            if let Some(flag) = diag::extract_warning_flag(trimmed) {
                *stats.warning_counts.entry(flag).or_insert(0) += 1;
            }
            if stats.track_dedup(trimmed) {
                stats.kept_lines.push(trimmed.to_string());
            }
            continue;
        }

        // Note lines — keep as continuation of error/warning
        if is_note_line(trimmed) {
            if stats.track_dedup(trimmed) {
                stats.kept_lines.push(trimmed.to_string());
            }
            continue;
        }

        // Platform extraction from probe lines
        if let Some(platform) = extract_platform(trimmed) {
            stats.platform = platform;
            continue;
        }

        // Compiler extraction
        if let Some(compiler) = extract_compiler(trimmed) {
            stats.compiler_family = compiler;
            continue;
        }

        // Probe lines — drop
        if is_probe_line(trimmed) {
            continue;
        }

        // Unity generation lines — count and drop
        if is_unity_gen_line(trimmed) {
            stats.unity_gen_count += 1;
            continue;
        }

        // Bin2obj command lines — drop (progress line already counts the action)
        if is_bin2obj_command(trimmed) {
            continue;
        }

        // Bin2obj internal noise — drop
        if is_bin2obj_noise(trimmed) {
            continue;
        }

        // Probe test lines ("> cl.exe ...") — drop
        if is_probe_test_line(trimmed) {
            continue;
        }

        // Full command lines — drop
        if is_full_command_line(trimmed) {
            continue;
        }

        // Progress lines — extract target info, count, drop
        if let Some(info) = is_progress_line(trimmed) {
            match info.action.as_str() {
                "compiling" => {
                    stats.compile_count += 1;
                    let entry = stats
                        .targets
                        .entry(info.target.clone())
                        .or_insert((0, 0, 0));
                    entry.0 += 1;
                }
                "archiving" => {
                    stats.archive_count += 1;
                    let entry = stats
                        .targets
                        .entry(info.target.clone())
                        .or_insert((0, 0, 0));
                    entry.1 += 1;
                }
                "linking" => {
                    stats.link_count += 1;
                    let entry = stats
                        .targets
                        .entry(info.target.clone())
                        .or_insert((0, 0, 0));
                    entry.2 += 1;
                }
                "generating" => {
                    stats.generate_count += 1;
                    let entry = stats
                        .targets
                        .entry(info.target.clone())
                        .or_insert((0, 0, 0));
                    entry.0 += 1;
                }
                _ => {}
            }
            continue;
        }
    }

    compose_output(&stats)
}

// ─── Output Composition ───

/// Build the compact output from parsed state.
fn compose_output(stats: &XmakeStats) -> String {
    let mut output = String::new();

    // Determine if build or config phase
    let is_build = !stats.section_headers.iter().any(|h| h.contains("CONFIG"))
        || stats.section_headers.iter().any(|h| h.contains("BUILD"));

    // Build mode string
    let mode_str = if stats.build_modes.is_empty() {
        if is_build {
            "debug".to_string()
        } else {
            String::new()
        }
    } else {
        stats.build_modes.join(",")
    };

    // Platform string
    let platform_str = if stats.platform.is_empty() {
        String::new()
    } else {
        format!(", {}", stats.platform)
    };

    // Compiler string
    let compiler_str = if stats.compiler_family.is_empty() {
        String::new()
    } else {
        format!(", {}", stats.compiler_family)
    };

    let total_targets = stats.targets.len();

    // ── Error case ──
    if !stats.errors.is_empty()
        || stats.build_exit_code == Some(1)
        || stats.config_exit_code == Some(1)
    {
        output.push_str(&format!(
            "xmake: {} failed ({}{}{})\n",
            if is_build { "build" } else { "configure" },
            mode_str,
            platform_str,
            compiler_str,
        ));

        // Show error lines from kept_lines
        for line in &stats.kept_lines {
            output.push_str(line);
            output.push('\n');
        }

        // Per-target detail (verbose mode) — even on failure
        if stats.verbose > 0 && !stats.targets.is_empty() {
            let mut targets: Vec<_> = stats.targets.iter().collect();
            targets.sort_by(|a, b| a.0.cmp(b.0));
            for (target, &(comp, arch, link)) in &targets {
                let mut parts = Vec::new();
                if comp > 0 {
                    parts.push(format!("{} compiled", comp));
                }
                if arch > 0 {
                    parts.push(format!("{} archived", arch));
                }
                if link > 0 {
                    parts.push(format!("{} linked", link));
                }
                output.push_str(&format!(" <{}>: {}\n", target, parts.join(", ")));
            }
        }

        return output;
    }

    // ── Success case ──
    if is_build || stats.compile_count > 0 || stats.archive_count > 0 || stats.link_count > 0 {
        let status = if !stats.warning_counts.is_empty() {
            format!(
                " — with {} warnings",
                stats.warning_counts.values().sum::<usize>()
            )
        } else {
            String::new()
        };

        output.push_str(&format!(
            "ok xmake: build ({}{}{}){}\n",
            mode_str, platform_str, compiler_str, status,
        ));

        // Target stats — include generate_count when non-zero
        let mut stat_parts: Vec<String> = Vec::new();
        stat_parts.push(format!("{} compiled", stats.compile_count));
        if stats.generate_count > 0 {
            stat_parts.push(format!("{} generated", stats.generate_count));
        }
        stat_parts.push(format!("{} archived", stats.archive_count));
        stat_parts.push(format!("{} linked", stats.link_count));
        output.push_str(&format!(
            " {} ({} targets)\n",
            stat_parts.join(", "),
            total_targets,
        ));
    } else {
        // Config only
        output.push_str(&format!(
            "ok xmake: configured ({}{})\n",
            platform_str, compiler_str,
        ));
    }

    // Warning summary
    if !stats.warning_counts.is_empty() {
        let mut warnings: Vec<_> = stats.warning_counts.iter().collect();
        warnings.sort_by(|a, b| b.1.cmp(a.1));
        let warn_parts: Vec<String> = warnings
            .iter()
            .map(|(flag, count)| format!("{} ×{}", flag, count))
            .collect();
        output.push_str(&format!("  warnings: {}\n", warn_parts.join(", ")));
    }

    // Per-target detail (verbose mode)
    if stats.verbose > 0 && !stats.targets.is_empty() {
        let mut targets: Vec<_> = stats.targets.iter().collect();
        targets.sort_by(|a, b| a.0.cmp(b.0));
        for (target, &(comp, arch, link)) in &targets {
            let mut parts = Vec::new();
            if comp > 0 {
                parts.push(format!("{} compiled", comp));
            }
            if arch > 0 {
                parts.push(format!("{} archived", arch));
            }
            if link > 0 {
                parts.push(format!("{} linked", link));
            }
            output.push_str(&format!(" <{}>: {}\n", target, parts.join(", ")));
        }
    }

    output
}

// ─── Public API ───

/// Run xmake with output filtering.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("xmake: running xmake {}", args.join(" "));
    }

    let mut cmd = resolved_command("xmake");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_filtered_with_exit(
        cmd,
        "xmake",
        &args_str,
        move |input, exit_code| filter_xmake_output_verbose(input, exit_code, verbose),
        runner::RunOptions::with_tee("xmake"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    // Test helper
    fn filter_xmake(input: &str, exit_code: i32) -> String {
        filter_xmake_output(input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_progress_line_compiling() {
        let result = is_progress_line("[  7%]: <mimalloc> compiling.debug path\\to\\file.c");
        assert!(result.is_some(), "should parse MSVC-style progress line");
        if let Some(info) = result {
            assert_eq!(info.target, "mimalloc");
            assert_eq!(info.action, "compiling");
        }
    }

    #[test]
    fn test_is_progress_line_gcc() {
        let result = is_progress_line("[ 25%]: <lc-core> compiling.debug path/to/file.cpp");
        assert!(result.is_some(), "should parse GCC-style progress line");
        if let Some(info) = result {
            assert_eq!(info.target, "lc-core");
            assert_eq!(info.action, "compiling");
        }
    }

    #[test]
    fn test_is_progress_line_release() {
        let result = is_progress_line("[ 50%]: <lc-core> compiling.release path/to/file.cpp");
        assert!(result.is_some(), "should parse release build progress line");
        if let Some(info) = result {
            assert_eq!(info.target, "lc-core");
            assert_eq!(info.action, "compiling");
        }
    }

    #[test]
    fn test_is_progress_line_archiving() {
        let result =
            is_progress_line("[ 75%]: <lc-yyjson> archiving.debug luisa-ext-lc-yyjson.lib");
        assert!(result.is_some());
        if let Some(info) = result {
            assert_eq!(info.target, "lc-yyjson");
            assert_eq!(info.action, "archiving");
        }
    }

    #[test]
    fn test_is_progress_line_linking() {
        let result = is_progress_line("[ 99%]: <glfw> linking.debug luisa-ext-glfw.dll");
        assert!(result.is_some());
        if let Some(info) = result {
            assert_eq!(info.target, "glfw");
            assert_eq!(info.action, "linking");
        }
    }

    #[test]
    fn test_is_progress_line_not() {
        assert!(is_progress_line("error: unity_16.cpp").is_none());
        assert!(is_progress_line("checking for platform ... windows (x64)").is_none());
        assert!(is_progress_line("").is_none());
    }

    #[test]
    fn test_is_section_header_config() {
        assert!(is_section_header("=== XMAKE DEBUG CONFIG ==="));
    }

    #[test]
    fn test_is_section_header_build() {
        assert!(is_section_header("=== XMAKE DEBUG BUILD (rebuild) ==="));
    }

    #[test]
    fn test_is_section_header_not() {
        assert!(!is_section_header(
            "[  7%]: <mimalloc> compiling.debug file.c"
        ));
        assert!(!is_section_header("error: something"));
    }

    #[test]
    fn test_is_unity_gen_line() {
        assert!(is_unity_gen_line(
            "generating.unityfile path\\to\\unity_16.cpp"
        ));
        assert!(is_unity_gen_line(
            "generating.unityfile path/to/unity_16.cpp"
        ));
        assert!(!is_unity_gen_line(
            "[  7%]: <mimalloc> compiling.debug file.c"
        ));
    }

    #[test]
    fn test_is_full_command_line_msvc() {
        // MSVC compiler command line
        let line = "\"C:\\Program Files\\Microsoft Visual Studio\\2022\\Community\\VC\\Tools\\MSVC\\14.38.33130\\bin\\Hostx64\\x64\\cl.exe\" -c -nologo -MDd -Zi -FS -Fd\"build\\.objs\\luisa-compute\\windows\\x64\\debug\\lc-core\\src\\core\\clock.cpp.pdb\" -Fo\"build\\.objs\\luisa-compute\\windows\\x64\\debug\\lc-core\\src\\core\\clock.cpp.obj\" -I\"src\" -I\"include\" -std:c++20 -DUNITY_BUILD -DLC_BACKEND_DX_ENABLED=1 \"-FAsrc\\core\\clock.cpp.asm\" -showIncludes \"src\\core\\clock.cpp\"";
        assert!(
            is_full_command_line(line),
            "MSVC compiler line should be detected"
        );
    }

    #[test]
    fn test_is_full_command_line_msvc_link_dll() {
        let line = "\"C:\\Program Files\\Microsoft Visual Studio\\2022\\Community\\VC\\Tools\\MSVC\\14.38.33130\\bin\\Hostx64\\x64\\link.exe\" -dll -nologo -machine:x64 \"build\\.objs\\luisa-compute\\windows\\x64\\debug\\lc-yyjson\\src\\json.cpp.obj\" -out:\"build\\windows\\x64\\debug\\lc-yyjson.dll\"";
        assert!(
            is_full_command_line(line),
            "MSVC link DLL line should be detected"
        );
    }

    #[test]
    fn test_is_full_command_line_msvc_link_lib() {
        let line = "\"C:\\Program Files\\Microsoft Visual Studio\\2022\\Community\\VC\\Tools\\MSVC\\14.38.33130\\bin\\Hostx64\\x64\\link.exe\" -lib -nologo -machine:x64 \"build\\.objs\\luisa-compute\\windows\\x64\\debug\\lc-core\\src\\core\\clock.cpp.obj\" -out:\"build\\windows\\x64\\debug\\lc-core.lib\"";
        assert!(
            is_full_command_line(line),
            "MSVC link lib line should be detected"
        );
    }

    #[test]
    fn test_is_full_command_line_gcc() {
        let line = "\"/usr/bin/g++\" -c -g -std=c++20 -fPIC -I\"src/core\" -I\"src/include\" -I\"src/third_party/abseil-cpp\" -DUNITY_BUILD -DBUILD_TESTING=1 -DBUILD_BENCHMARKS=1 -o\"build/.objs/lc-core/src/core/clock.cpp.o\" \"src/core/clock.cpp\"";
        assert!(
            is_full_command_line(line),
            "GCC compiler line should be detected"
        );
    }

    #[test]
    fn test_is_full_command_line_clang() {
        let line = "\"/usr/bin/clang++\" -c -g -std=c++20 -I\"src/core\" -I\"src/include\" -I\"src/vendor/abseil-cpp/absl\" -I\"src/vendor/googletest/googletest/include\" -DUNITY_BUILD -DLC_BACKEND_DX_ENABLED=1 -o\"build/.objs/lc-core/src/core/clock.cpp.o\" \"src/core/clock.cpp\"";
        assert!(
            is_full_command_line(line),
            "Clang compiler line should be detected"
        );
    }

    #[test]
    fn test_is_full_command_line_archiver_gcc() {
        let line = "\"/usr/bin/ar\" rcs \"build/lib/liblc-core.a\" build/.objs/lc-core/src/core/clock.cpp.o build/.objs/lc-core/src/core/hash.cpp.o build/.objs/lc-core/src/core/arena.cpp.o build/.objs/lc-core/src/core/buffer.cpp.o build/.objs/lc-core/src/core/context.cpp.o";
        assert!(
            is_full_command_line(line),
            "GCC archiver line should be detected"
        );
    }

    #[test]
    fn test_is_full_command_line_linker_gcc() {
        let line = "\"/usr/bin/g++\" -shared -o\"build/lib/liblc-yyjson.so\" \"build/.objs/lc-yyjson/src/json.cpp.o\" \"build/.objs/lc-yyjson/src/parse.cpp.o\" \"build/.objs/lc-yyjson/src/stringify.cpp.o\" \"build/.objs/lc-yyjson/src/sax.cpp.o\" \"build/.objs/lc-yyjson/src/utf8.cpp.o\"";
        assert!(
            is_full_command_line(line),
            "GCC linker line should be detected"
        );
    }

    #[test]
    fn test_is_not_full_command_line() {
        // Progress line — short and no flags
        assert!(!is_full_command_line(
            "[  7%]: <mimalloc> compiling.debug path/to/file.c"
        ));
        // Short line
        assert!(!is_full_command_line("error: unity_16.cpp"));
        // Regular message
        assert!(!is_full_command_line(
            "checking for platform ... windows (x64)"
        ));
        // Empty
        assert!(!is_full_command_line(""));
    }

    #[test]
    fn test_is_error_line_msvc() {
        assert!(is_error_line(
            "src\\scalar_evolution.h(118): error C2375: function_name: redefinition"
        ));
    }

    #[test]
    fn test_is_error_line_gcc() {
        assert!(is_error_line(
            "src/core/hash.h:42:13: error: static assertion failed: Hash must be specialized"
        ));
    }

    #[test]
    fn test_is_error_line_xmake_header() {
        assert!(is_error_line("error: unity_16.cpp"));
    }

    #[test]
    fn test_is_error_line_not() {
        assert!(!is_error_line("[  7%]: <mimalloc> compiling.debug file.c"));
        assert!(!is_error_line("checking for platform ... windows (x64)"));
    }

    #[test]
    fn test_is_warning_line_msvc() {
        assert!(is_warning_line(
            "src\\main.cpp(42): warning C4100: 'x': unreferenced formal parameter"
        ));
    }

    #[test]
    fn test_is_warning_line_gcc() {
        assert!(is_warning_line(
            "src/api/api.cpp:42:10: warning: unused parameter 'device_id' [-Wunused-parameter]"
        ));
    }

    #[test]
    fn test_is_warning_line_not() {
        assert!(!is_warning_line("error: unity_16.cpp"));
        assert!(!is_warning_line(
            "[ 50%]: <lc-core> compiling.release file.cpp"
        ));
    }

    #[test]
    fn test_extract_warning_flag_msvc() {
        assert_eq!(
            diag::extract_warning_flag("warning C4100: 'x': unreferenced formal parameter"),
            Some("C4100".to_string())
        );
    }

    #[test]
    fn test_extract_warning_flag_gcc() {
        assert_eq!(
            diag::extract_warning_flag(
                "src/api/api.cpp:42:10: warning: unused parameter 'device_id' [-Wunused-parameter]"
            ),
            Some("-Wunused-parameter".to_string())
        );
    }

    #[test]
    fn test_extract_platform_windows() {
        assert_eq!(
            extract_platform("checking for platform ... windows (x64)"),
            Some("windows (x64)".to_string())
        );
    }

    #[test]
    fn test_extract_platform_linux() {
        assert_eq!(
            extract_platform("checking for platform ... linux (x86_64)"),
            Some("linux (x86_64)".to_string())
        );
    }

    #[test]
    fn test_extract_platform_macos() {
        assert_eq!(
            extract_platform("checking for platform ... macosx (arm64)"),
            Some("macosx (arm64)".to_string())
        );
    }

    #[test]
    fn test_is_probe_line_true() {
        assert!(is_probe_line(
            "checking for Microsoft C/C++ Compiler ... ok"
        ));
        assert!(is_probe_line("checking for gcc ... /usr/bin/gcc"));
        assert!(is_probe_line("checking for clang ... /usr/bin/clang"));
        assert!(is_probe_line(
            "checking for the c++ compiler (cxx) ... clang++"
        ));
        assert!(is_probe_line("checking for flags (-FS) ... ok"));
        assert!(is_probe_line("checking for link.exe ... /usr/bin/ld"));
    }

    #[test]
    fn test_is_probe_line_false() {
        // Platform lines start with "checking for" so they ARE probe lines
        assert!(is_probe_line("checking for platform ... windows (x64)"));
        // But we keep them because they carry platform info
        assert!(!is_probe_line("[  7%]: <mimalloc> compiling.debug file.c"));
        assert!(!is_probe_line("error: something"));
    }

    #[test]
    fn test_extract_compiler_msvc() {
        assert_eq!(
            extract_compiler("checking for Microsoft C/C++ Compiler ... ok"),
            Some("msvc".to_string())
        );
    }

    #[test]
    fn test_extract_compiler_gcc() {
        assert_eq!(
            extract_compiler("checking for gcc ... /usr/bin/gcc"),
            Some("gcc".to_string())
        );
    }

    #[test]
    fn test_extract_compiler_clang() {
        assert_eq!(
            extract_compiler("checking for clang ... /usr/bin/clang"),
            Some("clang".to_string())
        );
    }

    #[test]
    fn test_is_exit_code_line() {
        assert_eq!(
            is_exit_code_line("Config exit code: 0"),
            Some(("config".to_string(), 0))
        );
        assert_eq!(
            is_exit_code_line("Build exit code: 1"),
            Some(("build".to_string(), 1))
        );
        assert_eq!(is_exit_code_line("not an exit code line"), None);
    }

    #[test]
    fn test_is_compiler_diag_gcc_error() {
        assert!(diag::is_compiler_diag(
            "src/core/hash.h:42:13: error: static assertion failed: Hash must be specialized"
        ));
    }

    #[test]
    fn test_is_compiler_diag_msvc_error() {
        assert!(diag::is_compiler_diag(
            "src/backends/dx/dx_codegen.cpp(88): error C2039: 'visit': is not a member"
        ));
    }

    #[test]
    fn test_is_compiler_diag_warning() {
        assert!(diag::is_compiler_diag(
            "src/api/api.cpp:42:10: warning: unused parameter 'device_id' [-Wunused-parameter]"
        ));
    }

    #[test]
    fn test_is_compiler_diag_note() {
        assert!(diag::is_compiler_diag(
            "src/core/hash.h:42:13: note: in instantiation of template class"
        ));
    }

    #[test]
    fn test_is_compiler_diag_not() {
        assert!(!diag::is_compiler_diag(
            "[  7%]: <mimalloc> compiling.debug file.c"
        ));
        assert!(!diag::is_compiler_diag(
            "checking for platform ... windows (x64)"
        ));
        assert!(!diag::is_compiler_diag(""));
    }

    // ── Success cases ──

    #[test]
    fn test_xmake_successful_build_msvc() {
        let input = "\
=== XMAKE DEBUG CONFIG ===
checking for platform ... windows (x64)
checking for Microsoft C/C++ Compiler ... ok
checking for the c++ compiler (cxx) ... cl.exe
checking for flags (-FS) ... ok
checking for link.exe ... link.exe
Config exit code: 0
=== XMAKE DEBUG BUILD (rebuild) ===
generating.unityfile src\\core\\unity_16.cpp
generating.unityfile src\\core\\unity_17.cpp
[  7%]: <mimalloc> compiling.debug src\\mimalloc\\alloc.c
[ 25%]: <lc-core> compiling.debug src\\core\\clock.cpp
[ 37%]: <lc-core> compiling.debug src\\core\\hash.cpp
[ 50%]: <lc-yyjson> compiling.debug src\\json\\parse.cpp
[ 62%]: <lc-yyjson> archiving.debug lc-yyjson.lib
[ 75%]: <glfw> compiling.debug src\\glfw\\init.cpp
[ 87%]: <glfw> linking.debug glfw.dll
[100%]: <lc-core> linking.debug lc-core.dll
Build exit code: 0
";
        let result = filter_xmake(input, 0);
        assert!(
            result.contains("ok xmake: build (debug, windows (x64), msvc)"),
            "got: {}",
            result
        );
        assert!(
            result.contains("5 compiled, 1 archived, 2 linked (4 targets)"),
            "got: {}",
            result
        );
        assert!(
            !result.contains("generating.unityfile"),
            "should strip unity gen lines, got: {}",
            result
        );
        assert!(
            !result.contains("[  7%]"),
            "should strip progress lines, got: {}",
            result
        );
        assert!(
            !result.contains("checking for"),
            "should strip probe lines, got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_successful_build_gcc() {
        let input = "\
=== XMAKE DEBUG CONFIG ===
checking for platform ... linux (x86_64)
checking for gcc ... /usr/bin/gcc
checking for the c++ compiler (cxx) ... g++
Config exit code: 0
=== XMAKE DEBUG BUILD (rebuild) ===
[  7%]: <mimalloc> compiling.debug src/mimalloc/alloc.c
[ 25%]: <lc-core> compiling.debug src/core/clock.cpp
[ 50%]: <lc-yyjson> compiling.debug src/json/parse.cpp
[ 62%]: <lc-yyjson> archiving.debug liblc-yyjson.a
[ 75%]: <glfw> compiling.debug src/glfw/init.cpp
[100%]: <lc-core> linking.debug liblc-core.so
Build exit code: 0
";
        let result = filter_xmake(input, 0);
        assert!(
            result.contains("ok xmake: build (debug, linux (x86_64), gcc)"),
            "got: {}",
            result
        );
        assert!(
            result.contains("4 compiled, 1 archived, 1 linked (4 targets)"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_configure_only() {
        let input = "\
=== XMAKE DEBUG CONFIG ===
checking for platform ... windows (x64)
checking for Microsoft C/C++ Compiler ... ok
Config exit code: 0
";
        let result = filter_xmake(input, 0);
        assert!(result.contains("ok xmake: configured"), "got: {}", result);
        assert!(
            !result.contains("build"),
            "should not mention build, got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_config_and_build() {
        let input = "\
=== XMAKE DEBUG CONFIG ===
checking for platform ... linux (x86_64)
checking for gcc ... /usr/bin/gcc
Config exit code: 0
=== XMAKE DEBUG BUILD (rebuild) ===
[ 50%]: <lc-core> compiling.debug src/core/main.cpp
[100%]: <lc-core> linking.debug lc-core
Build exit code: 0
";
        let result = filter_xmake(input, 0);
        assert!(result.contains("ok xmake: build"), "got: {}", result);
        assert!(
            result.contains("1 compiled, 0 archived, 1 linked (1 targets)"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_token_savings_above_90pct() {
        // Simulate a build with many progress lines and command lines (785-line log simulation)
        let mut input = String::new();
        input.push_str("=== XMAKE DEBUG CONFIG ===\n");
        input.push_str("checking for platform ... windows (x64)\n");
        input.push_str("checking for Microsoft C/C++ Compiler ... ok\n");
        input.push_str("Config exit code: 0\n");
        input.push_str("=== XMAKE DEBUG BUILD (rebuild) ===\n");

        // 100 progress lines
        for i in 1..=100 {
            input.push_str(&format!(
                "[{:3}%]: <lc-core> compiling.debug src/core/file_{}.cpp\n",
                i, i
            ));
            // Each progress line followed by a long command line
            input.push_str(&format!(
                "\"C:\\cl.exe\" -c -nologo -MDd -Zi -FS -Fd\"build\\file_{}.pdb\" -Fo\"build\\file_{}.obj\" -I\"src\" -std:c++20 -DUNITY_BUILD \"src\\core\\file_{}.cpp\"\n",
                i, i, i
            ));
        }

        // 20 archive/link lines with their command lines
        for i in 1..=10 {
            input.push_str(&format!(
                "[{:3}%]: <lc-core> archiving.debug lc-core.lib\n",
                80 + i
            ));
            input.push_str(&format!(
                "\"C:\\link.exe\" -lib -nologo -machine:x64 \"build\\file_{}.obj\" -out:\"build\\lc-core.lib\"\n",
                i
            ));
        }
        for i in 1..=5 {
            input.push_str(&format!(
                "[{:3}%]: <lc-core> linking.debug lc-core.dll\n",
                90 + i
            ));
            input.push_str(&format!(
                "\"C:\\link.exe\" -dll -nologo -machine:x64 \"build\\file_{}.obj\" -out:\"build\\lc-core.dll\"\n",
                i
            ));
        }

        input.push_str("Build exit code: 0\n");

        let result = filter_xmake(&input, 0);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 90,
            "token savings: {}% (expected >=90%)",
            savings
        );
    }

    #[test]
    fn test_xmake_token_savings_above_90pct_gcc() {
        let mut input = String::new();
        input.push_str("=== XMAKE DEBUG CONFIG ===\n");
        input.push_str("checking for platform ... linux (x86_64)\n");
        input.push_str("checking for gcc ... /usr/bin/gcc\n");
        input.push_str("Config exit code: 0\n");
        input.push_str("=== XMAKE DEBUG BUILD (rebuild) ===\n");

        for i in 1..=100 {
            input.push_str(&format!(
                "[{:3}%]: <lc-core> compiling.debug src/core/file_{}.cpp\n",
                i, i
            ));
            input.push_str(&format!(
                "\"/usr/bin/g++\" -c -g -std=c++20 -fPIC -I\"src\" -o\"build/file_{}.o\" \"src/core/file_{}.cpp\"\n",
                i, i
            ));
        }

        for i in 1..=10 {
            input.push_str(&format!(
                "[{:3}%]: <lc-core> archiving.debug liblc-core.a\n",
                80 + i
            ));
            input.push_str(&format!(
                "\"/usr/bin/ar\" rcs \"build/liblc-core.a\" build/file_{}.o\n",
                i
            ));
        }

        input.push_str("Build exit code: 0\n");

        let result = filter_xmake(&input, 0);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 90,
            "token savings: {}% (expected >=90%)",
            savings
        );
    }

    // ── Warning cases ──

    #[test]
    fn test_xmake_warnings_msvc() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===
[ 50%]: <lc-core> compiling.debug src/core/clock.cpp
src\\core\\clock.cpp(42): warning C4100: 'x': unreferenced formal parameter
src\\core\\clock.cpp(55): warning C4244: '=': conversion from 'size_t' to 'int', possible loss of data
[100%]: <lc-core> linking.debug lc-core.dll
Build exit code: 0
";
        let result = filter_xmake(input, 0);
        assert!(result.contains("ok xmake: build"), "got: {}", result);
        assert!(
            result.contains("warnings: C4100 ×1, C4244 ×1")
                || result.contains("warnings: C4244 ×1, C4100 ×1"),
            "should show warning counts, got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_warnings_gcc() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===
[ 50%]: <lc-core> compiling.debug src/core/clock.cpp
src/api/api.cpp:42:10: warning: unused parameter 'device_id' [-Wunused-parameter]
src/core/clock.cpp:15:20: warning: variable 'old_val' set but not used [-Wunused-but-set-variable]
[100%]: <lc-core> linking.debug lc-core
Build exit code: 0
";
        let result = filter_xmake(input, 0);
        assert!(
            result.contains("warnings: -Wunused-parameter ×1, -Wunused-but-set-variable ×1")
                || result.contains("warnings: -Wunused-but-set-variable ×1, -Wunused-parameter ×1"),
            "should show GCC warning flags, got: {}",
            result
        );
    }

    // ── Error cases ──

    #[test]
    fn test_xmake_error_msvc() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===
[ 50%]: <lc-core> compiling.debug src/core/unity_16.cpp
src\\core\\scalar_evolution.h(118): error C2375: function_name: redefinition
src\\core\\scalar_evolution.h(100): note: previous definition
src\\core\\indvar_simplify.cpp(138): error C3861: function_name: undeclared identifier
Build exit code: 1
";
        let result = filter_xmake(input, 1);
        assert!(result.contains("xmake: build failed"), "got: {}", result);
        assert!(result.contains("error C2375"), "got: {}", result);
        assert!(result.contains("error C3861"), "got: {}", result);
        assert!(
            result.contains("note: previous definition"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_error_gcc() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===
[ 50%]: <lc-core> compiling.debug src/core/unity_1.cpp
src/core/hash.h:118:5: error: redefinition of 'scev_pass_run_on_function'
src/core/hash.h:100:5: note: previous definition is here
src/core/transform.cpp:138:5: error: use of undeclared identifier
Build exit code: 1
";
        let result = filter_xmake(input, 1);
        assert!(result.contains("xmake: build failed"), "got: {}", result);
        assert!(result.contains("error: redefinition"), "got: {}", result);
        assert!(
            result.contains("note: previous definition"),
            "got: {}",
            result
        );
        assert!(
            result.contains("error: use of undeclared"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_error_clang() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===
[ 50%]: <lc-core> compiling.debug src/core/main.cpp
src/core/main.cpp:42:5: error: use of undeclared identifier 'foo'; did you mean 'bar'?
src/core/main.cpp:10:5: note: 'bar' declared here
Build exit code: 1
";
        let result = filter_xmake(input, 1);
        assert!(result.contains("xmake: build failed"), "got: {}", result);
        assert!(
            result.contains("error: use of undeclared"),
            "got: {}",
            result
        );
        assert!(result.contains("did you mean"), "got: {}", result);
    }

    #[test]
    fn test_xmake_error_with_note_context() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===
[ 50%]: <lc-core> compiling.debug src/core/unity_1.cpp
src/core/hash.h:118:5: error: redefinition of 'scev_pass_run_on_function'
src/core/hash.h:100:5: note: previous definition is here
Build exit code: 1
";
        let result = filter_xmake(input, 1);
        assert!(result.contains("error: redefinition of"), "got: {}", result);
        assert!(
            result.contains("note: previous definition"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_nonzero_exit() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===
[100%]: <lc-core> compiling.debug src/core/main.cpp
src/core/main.cpp:1:1: error: fatal error: No such file or directory
Build exit code: 2
";
        let result = filter_xmake(input, 2);
        assert!(result.contains("xmake: build failed"), "got: {}", result);
        assert!(result.contains("fatal error"), "got: {}", result);
    }

    // ── Edge cases ──

    #[test]
    fn test_xmake_ansi_stripped() {
        let input = "\x1b[32m=== XMAKE DEBUG BUILD (rebuild) ===\x1b[0m\n\
                      [ 50%]: <lc-core> compiling.debug src/core/main.cpp\n\
                      \x1b[31msrc/core/main.cpp:1:1: error: fatal error\x1b[0m\n\
                      Build exit code: 1\n";
        let result = filter_xmake(input, 1);
        // No ANSI escape sequences should remain
        assert!(
            !result.contains("\x1b["),
            "ANSI codes should be stripped, got: {}",
            result
        );
        assert!(result.contains("xmake: build failed"), "got: {}", result);
    }

    #[test]
    fn test_xmake_empty_input() {
        let result = filter_xmake("", 0);
        assert!(
            result.contains("xmake"),
            "should have a summary, got: '{}'",
            result
        );
    }

    #[test]
    fn test_xmake_no_progress_lines() {
        // Only config, no build
        let input = "\
=== XMAKE DEBUG CONFIG ===
checking for platform ... windows (x64)
checking for Microsoft C/C++ Compiler ... ok
Config exit code: 0
";
        let result = filter_xmake(input, 0);
        assert!(result.contains("ok xmake: configured"), "got: {}", result);
    }

    #[test]
    fn test_xmake_cross_compiler() {
        let line = "\"/opt/toolchains/arm-gcc/bin/arm-none-eabi-g++\" -c -g -std=c++17 -fPIC -I\"src/arm\" -I\"src/arm/include\" -I\"src/arm/third_party/freetype/include\" -DARM_TARGET=1 -DCMAKE_BUILD_TYPE=Release -o\"build/arm/file.o\" \"src/arm/file.cpp\"";
        assert!(
            is_full_command_line(line),
            "Cross-compiler command line should be detected"
        );
    }

    #[test]
    fn test_xmake_extract_diag_message_gcc() {
        let msg = diag::extract_diag_message(
            "src/core/hash.h:42:13: error: static assertion failed: Hash must be specialized",
        );
        assert_eq!(
            msg, "static assertion failed: Hash must be specialized",
            "should capture full message including colons, got: '{}'",
            msg
        );
    }

    #[test]
    fn test_xmake_extract_diag_message_msvc() {
        let msg = diag::extract_diag_message(
            "src/backends/dx/dx_codegen.cpp(88): error C2039: 'visit' is not a member of 'luisa::compute::dx::DXCodegen'"
        );
        assert_eq!(
            msg, "'visit' is not a member of 'luisa::compute::dx::DXCodegen'",
            "should capture full message including ::, got: '{}'",
            msg
        );
    }

    #[test]
    fn test_xmake_successful_build_clang() {
        let input = "\
=== XMAKE DEBUG CONFIG ===\n\
checking for platform ... macosx (arm64)\n\
checking for clang ... /usr/bin/clang\n\
checking for the c++ compiler (cxx) ... clang++\n\
Config exit code: 0\n\
=== XMAKE DEBUG BUILD (rebuild) ===\n\
[ 25%]: <lc-core> compiling.debug src/core/main.cpp\n\
[ 50%]: <lc-yyjson> compiling.debug src/json/parse.cpp\n\
[ 75%]: <lc-yyjson> archiving.debug liblc-yyjson.a\n\
[100%]: <lc-core> linking.debug liblc-core.dylib\n\
Build exit code: 0\n";
        let result = filter_xmake(input, 0);
        assert!(
            result.contains("ok xmake: build (debug, macosx (arm64), clang)"),
            "got: {}",
            result
        );
        assert!(
            result.contains("2 compiled, 1 archived, 1 linked (2 targets)"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_mixed_debug_release() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===\n\
[ 25%]: <lc-core> compiling.debug src/core/main.cpp\n\
[ 50%]: <lc-core> compiling.release src/core/main.cpp\n\
[100%]: <lc-core> linking.debug lc-core\n\
Build exit code: 0\n";
        let result = filter_xmake(input, 0);
        assert!(
            result.contains("ok xmake: build (debug)"),
            "got: {}",
            result
        );
        assert!(
            result.contains("2 compiled, 0 archived, 1 linked (1 targets)"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_multiline_error() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===\n\
[ 50%]: <lc-core> compiling.debug src/core/main.cpp\n\
src/core/main.cpp:42:5: error: use of undeclared identifier 'foo'\n\
src/core/main.cpp:10:5: note: in instantiation of template class 'vector<int>'\n\
src/core/main.cpp:11:5: note: while compiling class template member function\n\
src/core/main.cpp:100:5: error: use of undeclared identifier 'bar'\n\
Build exit code: 1\n";
        let result = filter_xmake(input, 1);
        assert!(result.contains("xmake: build failed"), "got: {}", result);
        assert!(
            result.contains("use of undeclared identifier 'foo'"),
            "got: {}",
            result
        );
        assert!(
            result.contains("use of undeclared identifier 'bar'"),
            "got: {}",
            result
        );
        assert!(
            result.contains("note: in instantiation of template"),
            "got: {}",
            result
        );
        assert!(
            result.contains("note: while compiling class"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_verbose_mode_shows_per_target() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===\n\
[ 25%]: <lc-core> compiling.debug src/core/main.cpp\n\
[ 50%]: <lc-core> compiling.debug src/core/utils.cpp\n\
[ 75%]: <lc-core> linking.debug lc-core\n\
[100%]: <glfw> compiling.debug src/glfw/init.cpp\n\
Build exit code: 0\n";
        let result = filter_xmake_output_verbose(input, 0, 1);
        assert!(
            result.contains("ok xmake: build (debug)"),
            "got: {}",
            result
        );
        assert!(
            result.contains("3 compiled, 0 archived, 1 linked (2 targets)"),
            "got: {}",
            result
        );
        assert!(
            result.contains("<lc-core>: 2 compiled, 1 linked"),
            "should show per-target detail, got: {}",
            result
        );
        assert!(
            result.contains("<glfw>: 1 compiled"),
            "should show per-target detail, got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_verbose_mode_shows_per_target_on_failure() {
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===\n\
[ 50%]: <lc-core> compiling.debug src/core/main.cpp\n\
src/core/main.cpp:42:5: error: use of undeclared identifier 'foo'\n\
Build exit code: 1\n";
        let result = filter_xmake_output_verbose(input, 1, 1);
        assert!(result.contains("xmake: build failed"), "got: {}", result);
        assert!(
            result.contains("<lc-core>: 1 compiled"),
            "should show per-target detail even on failure, got: {}",
            result
        );
    }

    // ── New filter rules tests ──

    #[test]
    fn test_is_progress_line_generating_bin2obj() {
        let result = is_progress_line(
            "[ 16%]: <cuda-builtin> generating.bin2obj thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu"
        );
        assert!(
            result.is_some(),
            "should parse generating.bin2obj progress line"
        );
        if let Some(info) = result {
            assert_eq!(info.target, "cuda-builtin");
            assert_eq!(info.action, "generating");
            assert_eq!(info._mode, "bin2obj");
        }
    }

    #[test]
    fn test_is_bin2obj_command_true() {
        assert!(is_bin2obj_command(
            "xmake lua cli.binutils.bin2obj -i thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu -o build\\.objs\\cuda-builtin\\windows\\x64\\debug\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu.obj -f coff -a x64 -p windows"
        ));
    }

    #[test]
    fn test_is_bin2obj_command_false() {
        assert!(!is_bin2obj_command(
            "[ 16%]: <cuda-builtin> generating.bin2obj file.cu"
        ));
        assert!(!is_bin2obj_command("xmake build"));
        assert!(!is_bin2obj_command(""));
    }

    #[test]
    fn test_is_bin2obj_noise_running_imported_module() {
        assert!(is_bin2obj_noise(
            "running imported module cli.binutils.bin2obj with args:"
        ));
    }

    #[test]
    fn test_is_bin2obj_noise_with_args() {
        assert!(is_bin2obj_noise("with args:"));
    }

    #[test]
    fn test_is_bin2obj_noise_converting_binary() {
        assert!(is_bin2obj_noise(
            "converting binary file C:\\dev\\myproject\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_device_math.h to coff object file C:\\dev\\myproject\\build\\.objs\\cuda-builtin\\windows\\x64\\debug\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_device_math.h.obj .."
        ));
    }

    #[test]
    fn test_is_bin2obj_noise_generated() {
        assert!(is_bin2obj_noise(
            "C:\\dev\\myproject\\build\\.objs\\cuda-builtin\\windows\\x64\\debug\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu.obj generated!"
        ));
        assert!(is_bin2obj_noise(
            "C:\\dev\\myproject\\build\\.objs\\cuda-builtin\\windows\\x64\\debug\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_device_half.h.obj generated!"
        ));
    }

    #[test]
    fn test_is_bin2obj_noise_numbered_args() {
        assert!(is_bin2obj_noise(" 1: \"-i\""));
        assert!(is_bin2obj_noise(" 2: \"-o\""));
        assert!(is_bin2obj_noise(" 5: \"-f\""));
        assert!(is_bin2obj_noise("10: \"windows\""));
        assert!(!is_bin2obj_noise("1: not a quoted arg"));
        assert!(!is_bin2obj_noise(
            "[  7%]: <mimalloc> compiling.debug file.c"
        ));
    }

    #[test]
    fn test_is_probe_test_line_true() {
        assert!(is_probe_test_line(
            "> cl.exe \"-FS\" \"-FdC:\\Users\\user\\AppData\\Local\\Temp\\.xmake\\240101\\_a1b2c3d4e5f6a7b8c9d0e1f2a3b4c5d6.pdb\" \"-nologo\""
        ));
        assert!(is_probe_test_line("> cl.exe \"-std:c++20\" \"-nologo\""));
        assert!(is_probe_test_line(
            "> cl.exe \"/sourceDependencies\" \"C:\\Users\\...\\_a3c4f1b719d59ba2eb8030ef5e51fbe1.json\" \"-nologo\""
        ));
    }

    #[test]
    fn test_is_probe_test_line_false() {
        assert!(!is_probe_test_line("checking for flags (-FS) ... ok"));
        assert!(!is_probe_test_line(
            "[  7%]: <mimalloc> compiling.debug file.c"
        ));
        assert!(!is_probe_test_line("> [some output]"));
        assert!(!is_probe_test_line(">"));
        assert!(!is_probe_test_line(""));
    }

    // ── Integration tests with real log patterns ──

    #[test]
    fn test_xmake_with_bin2obj_actions() {
        // Simulates a build that includes generating.bin2obj steps (e.g., CUDA backend)
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===\n\
[  7%]: <mimalloc> compiling.debug thirdparty\\SomeLib\\src\\ext\\EASTL\\packages\\mimalloc\\src\\static.c\n\
[ 16%]: <cuda-builtin> generating.bin2obj thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu\n\
xmake lua cli.binutils.bin2obj -i thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu -o build\\.objs\\cuda-builtin\\windows\\x64\\debug\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu.obj -f coff -a x64 -p windows\n\
running imported module cli.binutils.bin2obj with args:\n\
 1: \"-i\"\n\
 2: \"thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu\"\n\
 3: \"-o\"\n\
 4: \"build\\.objs\\cuda-builtin\\windows\\x64\\debug\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu.obj\"\n\
 5: \"-f\"\n\
 6: \"coff\"\n\
 7: \"-a\"\n\
 8: \"x64\"\n\
 9: \"-p\"\n\
10: \"windows\"\n\
converting binary file C:\\dev\\myproject\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu to coff object file C:\\dev\\myproject\\build\\.objs\\cuda-builtin\\windows\\x64\\debug\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu.obj ..\n\
C:\\dev\\myproject\\build\\.objs\\cuda-builtin\\windows\\x64\\debug\\thirdparty\\SomeLib\\src\\backends\\cuda\\cuda_builtin\\cuda_builtin_kernels.cu.obj generated!\n\
[ 50%]: <lc-core> compiling.debug src\\core\\clock.cpp\n\
[100%]: <lc-core> linking.debug lc-core.dll\n\
Build exit code: 0\n";
        let result = filter_xmake(input, 0);
        assert!(result.contains("ok xmake: build"), "got: {}", result);
        assert!(
            result.contains("2 compiled, 1 generated, 0 archived, 1 linked"),
            "should show generate count, got: {}",
            result
        );
        assert!(
            !result.contains("xmake lua cli.binutils.bin2obj"),
            "should strip bin2obj commands, got: {}",
            result
        );
        assert!(
            !result.contains("running imported module"),
            "should strip bin2obj noise, got: {}",
            result
        );
        assert!(
            !result.contains("converting binary file"),
            "should strip conversion lines, got: {}",
            result
        );
        assert!(
            !result.contains("generated!"),
            "should strip generated completion, got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_with_probe_test_lines() {
        // Simulates probe test lines mixed with config output
        let input = "\
=== XMAKE DEBUG CONFIG ===\n\
checking for platform ... windows (x64)\n\
checking for Microsoft C/C++ Compiler ... ok\n\
checking for flags (-FS) ... ok\n\
> cl.exe \"-FS\" \"-FdC:\\Users\\user\\AppData\\Local\\Temp\\.xmake\\_test.pdb\" \"-nologo\"\n\
checking for flags (-std:c++20) ... ok\n\
> cl.exe \"-std:c++20\" \"-nologo\"\n\
Config exit code: 0\n";
        let result = filter_xmake(input, 0);
        assert!(result.contains("ok xmake: configured"), "got: {}", result);
        assert!(
            !result.contains("> cl.exe"),
            "should strip probe test lines, got: {}",
            result
        );
        assert!(
            result.contains("windows (x64)"),
            "should keep platform info, got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_token_savings_with_bin2obj_noise() {
        // Verify token savings stay >90% even with heavy bin2obj noise
        let mut input = String::new();
        input.push_str("=== XMAKE DEBUG CONFIG ===\n");
        input.push_str("checking for platform ... windows (x64)\n");
        input.push_str("checking for Microsoft C/C++ Compiler ... ok\n");
        input.push_str("Config exit code: 0\n");
        input.push_str("=== XMAKE DEBUG BUILD (rebuild) ===\n");

        // 50 normal compile lines with command lines
        for i in 1..=50 {
            input.push_str(&format!(
                "[{:3}%]: <lc-core> compiling.debug src/core/file_{}.cpp\n",
                i, i
            ));
            input.push_str(&format!(
                "\"C:\\cl.exe\" -c -nologo -MDd -Zi -FS -Fd\"build\\file_{}.pdb\" -Fo\"build\\file_{}.obj\" -I\"src\" -std:c++20 \"src\\core\\file_{}.cpp\"\n",
                i, i, i
            ));
        }

        // 30 bin2obj noise blocks (simulating heavy CUDA compilation)
        for i in 1..=30 {
            input.push_str(&format!(
                "[{:3}%]: <lc-cuda-builtin> generating.bin2obj cuda_builtin_{}.cu\n",
                50 + i,
                i
            ));
            input.push_str(&format!(
                "xmake lua cli.binutils.bin2obj -i cuda_builtin_{}.cu -o cuda_builtin_{}.obj -f coff -a x64 -p windows\n",
                i, i
            ));
            input.push_str("running imported module cli.binutils.bin2obj with args:\n");
            for j in 1..=10 {
                input.push_str(&format!(" {:2}: \"-arg_{}_{}\"\n", j, i, j));
            }
            input.push_str(&format!(
                "converting binary file cuda_builtin_{}.cu to coff object file cuda_builtin_{}.obj ..\n",
                i, i
            ));
            input.push_str(&format!("cuda_builtin_{}.obj generated!\n", i));
        }

        input.push_str("Build exit code: 0\n");

        let result = filter_xmake(&input, 0);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 90,
            "token savings: {}% (expected >=90%), raw={}, filtered={}",
            savings,
            raw_tokens,
            filtered_tokens
        );
        // Verify generate count shown
        assert!(
            result.contains("generated"),
            "should show generate count when bin2obj present, got: {}",
            result
        );
    }

    #[test]
    fn test_xmake_no_generate_count_when_zero() {
        // Existing behaviour: no "generated" when there are no bin2obj steps
        let input = "\
=== XMAKE DEBUG BUILD (rebuild) ===\n\
[ 50%]: <lc-core> compiling.debug src/core/main.cpp\n\
[100%]: <lc-core> linking.debug lc-core.dll\n\
Build exit code: 0\n";
        let result = filter_xmake(input, 0);
        assert!(
            !result.contains("generated"),
            "should NOT show 'generated' when count is 0, got: {}",
            result
        );
        assert!(
            result.contains("1 compiled, 0 archived, 1 linked"),
            "should show standard format, got: {}",
            result
        );
    }
}
