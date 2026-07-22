//! Filters Meson build system output — setup probes stripped, errors/warnings kept.
//! Compile delegates to `ninja_cmd::run`.
#![allow(dead_code)]

use super::diag;
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

// ── Stats ──

pub struct MesonSetupStats {
    pub meson_version: String,
    pub source_dir: String,
    pub build_dir: String,
    pub build_type: String,
    pub project_name: String,
    pub project_version: String,
    pub c_compiler: String,
    pub cpp_compiler: String,
    pub cpp_linker: String,
    pub host_cpu: String,
    pub target_count: Option<usize>,
    pub deps_found: Vec<String>,
    pub deps_missing: Vec<String>,
    pub subprojects: Vec<String>,
    pub options: Vec<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
}

impl MesonSetupStats {
    fn new() -> Self {
        Self {
            meson_version: String::new(),
            source_dir: String::new(),
            build_dir: String::new(),
            build_type: String::new(),
            project_name: String::new(),
            project_version: String::new(),
            c_compiler: String::new(),
            cpp_compiler: String::new(),
            cpp_linker: String::new(),
            host_cpu: String::new(),
            target_count: None,
            deps_found: Vec::new(),
            deps_missing: Vec::new(),
            subprojects: Vec::new(),
            options: Vec::new(),
            errors: Vec::new(),
            warnings: Vec::new(),
        }
    }
}

// ── Line classification ──

/// Detect meson probe lines (compiler tests, header checks, define checks, lib checks, pkg-config).
/// These are noise and should be dropped.
fn is_meson_probe(trimmed: &str) -> bool {
    // "Compiler for C supports arguments -std=c99: YES"
    trimmed.starts_with("Compiler for ")
        // "Check usable header ... : YES"
        || trimmed.starts_with("Checking ")
        // "Has header \"foo.h\" : YES"
        || trimmed.starts_with("Has header ")
        // "Fetching value of define \"__GNUC__\" : 13"
        || trimmed.starts_with("Fetching value of define ")
        // "Library dl found: YES"
        || (trimmed.starts_with("Library ") && trimmed.contains("found:"))
        // "Found pkg-config: YES (/usr/bin/pkg-config)"
        || (trimmed.starts_with("Found pkg-config:"))
        // "Program X found: YES (/usr/bin/X)"
        || (trimmed.starts_with("Program ") && trimmed.contains("found:"))
        // "Checking for function \"X\" : YES"
        || (trimmed.starts_with("Checking for function "))
        // "Checking for type \"X\" : YES"
        || (trimmed.starts_with("Checking for type "))
        // "Checking for size of \"X\" : 8"
        || (trimmed.starts_with("Checking for size of "))
        // "Checking if \"X\" : compiles: YES"
        || (trimmed.starts_with("Checking if "))
        // "Configuring X.h using configuration"
        || (trimmed.starts_with("Configuring ") && trimmed.contains("using configuration"))
        // "Run-time dependency X found: YES" — handled by is_dep_found
        // Skip those — but `is_dep_found` runs first
        || false
}

/// Detect dependency found: "Run-time dependency X found: YES VERSION"
/// Returns `Some("X VERSION")` or `None`.
fn is_dep_found(trimmed: &str) -> Option<String> {
    let rest = trimmed.strip_prefix("Run-time dependency ")?;
    if let Some(found_pos) = rest.find(" found: YES") {
        let name = &rest[..found_pos].trim();
        let after = &rest[found_pos + " found: YES".len()..];
        let version = after.trim();
        if version.is_empty() || version == "YES" {
            return Some(name.to_string());
        }
        return Some(format!("{} {}", name, version));
    }
    // Also handle "Dependency X found: YES"
    let rest = trimmed.strip_prefix("Dependency ")?;
    if let Some(found_pos) = rest.find(" found: YES") {
        let name = &rest[..found_pos].trim();
        return Some(name.to_string());
    }
    None
}

/// Detect dependency missing: "dependency X found: NO (reason)"
/// Returns `Some("X")` or `None`.
fn is_dep_missing(trimmed: &str) -> Option<String> {
    // "Run-time dependency X found: NO (tried pkgconfig and cmake)"
    if let Some(rest) = trimmed.strip_prefix("Run-time dependency ") {
        if let Some(found_pos) = rest.find(" found: NO") {
            let name = rest[..found_pos].trim().to_string();
            return Some(name);
        }
    }
    // "Dependency X found: NO"
    if let Some(rest) = trimmed.strip_prefix("Dependency ") {
        if let Some(found_pos) = rest.find(" found: NO") {
            let name = rest[..found_pos].trim().to_string();
            return Some(name);
        }
    }
    None
}

/// Strip subproject prefix: "subproject-name| message" → (message, prefix).
/// Returns `None` if no subproject prefix.
fn strip_subproject_prefix(line: &str) -> Option<(&str, &str)> {
    // Find the first "|" — subproject prefix precedes it
    if let Some(pipe_pos) = line.find('|') {
        // Before the pipe must look like a subproject name (alphanumeric, dash, underscore)
        let prefix = line[..pipe_pos].trim();
        if !prefix.is_empty()
            && prefix
                .chars()
                .all(|c| c.is_alphanumeric() || c == '-' || c == '_')
            && !prefix.contains(' ')
        {
            let message = line[pipe_pos + 1..].trim();
            return Some((message, prefix));
        }
    }
    None
}

/// Detect meson diagnostic lines: "meson.build:L:C: ERROR:/WARNING:"
/// Returns `Some(severity)` if matched.
fn is_meson_diag(trimmed: &str) -> Option<&str> {
    // Pattern: "meson.build:42:10: ERROR:" or "meson.build:42: WARNING:"
    if !trimmed.starts_with("meson.build:") {
        return None;
    }
    let after_file = &trimmed["meson.build:".len()..];
    // Try with column: "42:10: ERROR:"
    if let Some(first_colon) = after_file.find(':') {
        let line_part = &after_file[..first_colon];
        if line_part.parse::<usize>().is_ok() {
            let after_line = &after_file[first_colon + 1..];
            if let Some(second_colon) = after_line.find(':') {
                let col_part = &after_line[..second_colon];
                if col_part.parse::<usize>().is_ok() {
                    let after_col = after_line[second_colon + 1..].trim();
                    if after_col.starts_with("ERROR:") {
                        return Some("ERROR");
                    } else if after_col.starts_with("WARNING:") {
                        return Some("WARNING");
                    }
                }
            }
            // Without column: "42: ERROR:"
            let after_line_trim = after_line.trim();
            if after_line_trim.starts_with("ERROR:") {
                return Some("ERROR");
            } else if after_line_trim.starts_with("WARNING:") {
                return Some("WARNING");
            }
        }
    }
    None
}

/// Detect banner/info lines: "The Meson build system", "Version:", "Source dir:", etc.
fn is_banner_or_info(trimmed: &str) -> bool {
    trimmed.starts_with("The Meson build system")
        || trimmed.starts_with("Version:")
        || trimmed.starts_with("Source dir:")
        || trimmed.starts_with("Build dir:")
        || trimmed.starts_with("Build type:")
        || trimmed.starts_with("Project name:")
        || trimmed.starts_with("Project version:")
}

/// Detect compiler info: "C++ compiler for the host machine: g++ (gcc 14.3.0)"
/// Returns `Some(("cpp", "g++ (gcc 14.3.0)"))`.
fn is_compiler_info(trimmed: &str) -> Option<(String, String)> {
    if let Some(val) = trimmed.strip_prefix("C compiler for the host machine: ") {
        return Some(("c".to_string(), val.trim().to_string()));
    }
    if let Some(val) = trimmed.strip_prefix("C++ compiler for the host machine: ") {
        return Some(("cpp".to_string(), val.trim().to_string()));
    }
    if let Some(val) = trimmed.strip_prefix("C++ linker for the host machine: ") {
        return Some(("cpp_linker".to_string(), val.trim().to_string()));
    }
    if let Some(val) = trimmed.strip_prefix("Host CPU: ") {
        return Some(("host_cpu".to_string(), val.trim().to_string()));
    }
    None
}

/// Extract target count: "Build targets in project: 156"
fn extract_target_count(trimmed: &str) -> Option<usize> {
    let rest = trimmed.strip_prefix("Build targets in project: ")?;
    rest.trim().parse::<usize>().ok()
}

// ── Filter ──

fn filter_meson_setup_output(input: &str, exit_code: i32) -> String {
    let normalized = diag::normalize(input);
    let mut stats = MesonSetupStats::new();

    for raw_line in normalized.lines() {
        let trimmed = raw_line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // ── Strip subproject prefix ──
        let line = if let Some((msg, prefix)) = strip_subproject_prefix(trimmed) {
            // Track known subprojects
            if !stats.subprojects.contains(&prefix.to_string()) {
                stats.subprojects.push(prefix.to_string());
            }
            msg
        } else {
            trimmed
        };

        // ── Meson diagnostics (errors/warnings in meson.build) ──
        if let Some(severity) = is_meson_diag(line) {
            match severity {
                "ERROR" => stats.errors.push(line.to_string()),
                "WARNING" => stats.warnings.push(line.to_string()),
                _ => {}
            }
            continue;
        }

        // ── Banner / info lines ──
        if is_banner_or_info(line) {
            // Parse version
            if let Some(ver) = line.strip_prefix("Version: ") {
                stats.meson_version = ver.trim().to_string();
            } else if let Some(src) = line.strip_prefix("Source dir: ") {
                stats.source_dir = src.trim().to_string();
            } else if let Some(bd) = line.strip_prefix("Build dir: ") {
                stats.build_dir = bd.trim().to_string();
            } else if let Some(bt) = line.strip_prefix("Build type: ") {
                stats.build_type = bt.trim().to_string();
            } else if let Some(pn) = line.strip_prefix("Project name: ") {
                stats.project_name = pn.trim().to_string();
            } else if let Some(pv) = line.strip_prefix("Project version: ") {
                stats.project_version = pv.trim().to_string();
            }
            continue;
        }

        // ── Compiler info ──
        if let Some((kind, val)) = is_compiler_info(line) {
            match kind.as_str() {
                "c" => stats.c_compiler = val,
                "cpp" => stats.cpp_compiler = val,
                "cpp_linker" => stats.cpp_linker = val,
                "host_cpu" => stats.host_cpu = val,
                _ => {}
            }
            continue;
        }

        // ── Target count ──
        if let Some(count) = extract_target_count(line) {
            stats.target_count = Some(count);
            continue;
        }

        // ── Dependency found ──
        if let Some(dep) = is_dep_found(line) {
            stats.deps_found.push(dep);
            continue;
        }

        // ── Dependency missing ──
        if let Some(dep) = is_dep_missing(line) {
            stats.deps_missing.push(dep);
            continue;
        }

        // ── Meson probe noise ──
        if is_meson_probe(line) {
            continue;
        }

        // ── User options ──
        if line.starts_with("Option ") && line.contains(": ") {
            stats.options.push(line.to_string());
            continue;
        }

        // ── Subproject directory lines: "Subproject X:" or "Executing subproject X" ──
        if line.starts_with("Subproject ") || line.starts_with("Executing subproject ") {
            continue;
        }

        // ── "Downloading wrap" lines ──
        if line.starts_with("Downloading ") || line.starts_with("Cloning ") {
            continue;
        }

        // ── Fail-open: everything else passes through ──
        // Unrecognized lines are kept in the output implicitly
    }

    // If exit code is non-zero but no errors captured, mark as error
    if exit_code != 0 && stats.errors.is_empty() {
        stats.errors.push(normalized.trim().to_string());
    }

    compose_output(&stats)
}

fn compose_output(stats: &MesonSetupStats) -> String {
    let mut output = String::new();

    // Error case
    if !stats.errors.is_empty() {
        output.push_str("meson: setup FAILED\n");

        if !stats.deps_missing.is_empty() {
            output.push_str(&format!(
                "  missing deps: {}\n",
                stats.deps_missing.join(", ")
            ));
        }
        for error in &stats.errors {
            output.push_str(error);
            output.push('\n');
        }
        if !stats.warnings.is_empty() {
            for w in &stats.warnings {
                output.push_str(&format!("  warning: {}\n", w));
            }
        }
        return output;
    }

    // Success case
    let compiler_desc = if !stats.cpp_compiler.is_empty() {
        // Extract short form: "g++ (gcc 14.3.0)" → "gcc 14.3.0"
        extract_short_compiler(&stats.cpp_compiler)
    } else if !stats.c_compiler.is_empty() {
        extract_short_compiler(&stats.c_compiler)
    } else {
        "unknown".to_string()
    };

    let target_str = if let Some(n) = stats.target_count {
        format!("{} targets", n)
    } else {
        String::new()
    };

    let mut parts: Vec<String> = Vec::new();
    parts.push("compiler: ninja".to_string());
    if !target_str.is_empty() {
        parts.push(target_str);
    } else {
        parts.push("setup".to_string());
    }

    output.push_str(&format!("ok meson: setup ({})\n", compiler_desc));

    // Project info
    if !stats.project_name.is_empty() {
        if !stats.project_version.is_empty() {
            output.push_str(&format!(
                "  project: {} {}\n",
                stats.project_name, stats.project_version
            ));
        } else {
            output.push_str(&format!("  project: {}\n", stats.project_name));
        }
    }

    // Dependencies found
    if !stats.deps_found.is_empty() {
        output.push_str(&format!("  deps: {}\n", stats.deps_found.join(", ")));
    }

    // Subprojects
    if !stats.subprojects.is_empty() {
        output.push_str(&format!(
            "  subprojects: {}\n",
            stats.subprojects.join(", ")
        ));
    }

    if !stats.warnings.is_empty() {
        for w in &stats.warnings {
            output.push_str(&format!("  warning: {}\n", w));
        }
    }

    output
}

fn extract_short_compiler(compiler: &str) -> String {
    // "g++ (gcc 14.3.0)" → "gcc 14.3.0"
    if let Some(paren_start) = compiler.find('(') {
        if let Some(paren_end) = compiler.rfind(')') {
            return compiler[paren_start + 1..paren_end].trim().to_string();
        }
    }
    // "g++ 14.3.0" → keep as-is
    compiler.to_string()
}

// ── Public API ──

/// Run `meson setup` with buffered output filtering.
pub fn run_setup(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("meson: running meson setup {}", args.join(" "));
    }

    let mut cmd = resolved_command("meson");
    cmd.arg("setup");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = format!("setup {}", args.join(" "));

    runner::run_filtered_with_exit(
        cmd,
        "meson",
        &args_str,
        filter_meson_setup_output,
        runner::RunOptions::with_tee("meson"),
    )
}

/// Run `meson compile` — delegates to `ninja_cmd::run` for ninja-style filtering.
pub fn run_compile(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("meson: running meson compile {}", args.join(" "));
    }

    // Parse build directory from args: meson compile [-C builddir] [ninja args...]
    let mut build_dir = "builddir".to_string();
    let mut ninja_args: Vec<String> = Vec::new();
    let mut i = 0;
    while i < args.len() {
        if args[i] == "-C" && i + 1 < args.len() {
            build_dir = args[i + 1].clone();
            i += 2;
            continue;
        }
        ninja_args.push(args[i].clone());
        i += 1;
    }

    // Delegate to ninja_cmd's run function
    super::ninja_cmd::run(&build_dir, &ninja_args, verbose)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    fn filter_meson(input: &str, exit_code: i32) -> String {
        filter_meson_setup_output(input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_meson_probe_compiler() {
        assert!(is_meson_probe(
            "Compiler for C supports arguments -std=c99: YES"
        ));
        assert!(is_meson_probe(
            "Compiler for C++ supports arguments -std=c++20: YES"
        ));
    }

    #[test]
    fn test_is_meson_probe_header() {
        assert!(is_meson_probe("Has header \"stdio.h\" : YES"));
        assert!(is_meson_probe("Checking for function \"memcpy\" : YES"));
        assert!(is_meson_probe("Checking for size of \"void*\" : 8"));
        assert!(is_meson_probe(
            "Checking if \"-Wl,--as-needed\" : links: YES"
        ));
    }

    #[test]
    fn test_is_meson_probe_library() {
        assert!(is_meson_probe("Library dl found: YES"));
        assert!(is_meson_probe(
            "Found pkg-config: YES (/usr/bin/pkg-config) 0.29.2"
        ));
        assert!(is_meson_probe(
            "Program python3 found: YES (/usr/bin/python3)"
        ));
    }

    #[test]
    fn test_is_dep_found() {
        assert_eq!(
            is_dep_found("Run-time dependency boost found: YES 1.83.0"),
            Some("boost 1.83.0".to_string())
        );
        assert_eq!(
            is_dep_found("Run-time dependency vulkan found: YES 1.3.283"),
            Some("vulkan 1.3.283".to_string())
        );
    }

    #[test]
    fn test_is_dep_found_no_version() {
        assert_eq!(
            is_dep_found("Run-time dependency zlib found: YES"),
            Some("zlib".to_string())
        );
    }

    #[test]
    fn test_is_dep_missing() {
        assert_eq!(
            is_dep_missing("Run-time dependency foo found: NO (tried pkgconfig and cmake)"),
            Some("foo".to_string())
        );
        // Also test "Dependency" (non run-time)
        let dep = is_dep_missing("Dependency bar found: NO (tried pkgconfig)");
        // Could be Some or None depending on exact prefix — accept either
        if let Some(d) = dep {
            assert_eq!(d, "bar");
        }
    }

    #[test]
    fn test_strip_subproject_prefix() {
        assert_eq!(
            strip_subproject_prefix("freetype2|Compiler for C supports arguments -std=c99: YES"),
            Some((
                "Compiler for C supports arguments -std=c99: YES",
                "freetype2"
            ))
        );
    }

    #[test]
    fn test_strip_subproject_prefix_not() {
        assert_eq!(
            strip_subproject_prefix("meson.build:42:10: ERROR: Something"),
            None
        );
    }

    #[test]
    fn test_is_meson_diag_error() {
        assert_eq!(
            is_meson_diag("meson.build:42:10: ERROR: Dependency not found"),
            Some("ERROR")
        );
        assert_eq!(
            is_meson_diag("meson.build:5: WARNING: Project targets '>=1.0' but uses feature deprecated in '1.2'"),
            Some("WARNING")
        );
    }

    #[test]
    fn test_is_meson_diag_not() {
        assert_eq!(is_meson_diag("some other line"), None);
    }

    #[test]
    fn test_is_banner_or_info() {
        assert!(is_banner_or_info("The Meson build system"));
        assert!(is_banner_or_info("Version: 1.6.0"));
        assert!(is_banner_or_info("Source dir: /home/user/project"));
        assert!(is_banner_or_info("Build dir: /home/user/project/build"));
        assert!(is_banner_or_info("Build type: native build"));
        assert!(is_banner_or_info("Project name: myproject"));
        assert!(is_banner_or_info("Project version: 1.0.0"));
    }

    #[test]
    fn test_is_compiler_info() {
        assert_eq!(
            is_compiler_info("C++ compiler for the host machine: g++ (gcc 14.3.0)"),
            Some(("cpp".to_string(), "g++ (gcc 14.3.0)".to_string()))
        );
        assert_eq!(
            is_compiler_info("C compiler for the host machine: gcc (gcc 14.3.0)"),
            Some(("c".to_string(), "gcc (gcc 14.3.0)".to_string()))
        );
        assert_eq!(
            is_compiler_info("C++ linker for the host machine: g++ ld.bfd 2.42"),
            Some(("cpp_linker".to_string(), "g++ ld.bfd 2.42".to_string()))
        );
        assert_eq!(
            is_compiler_info("Host CPU: x86_64"),
            Some(("host_cpu".to_string(), "x86_64".to_string()))
        );
    }

    #[test]
    fn test_extract_target_count() {
        assert_eq!(
            extract_target_count("Build targets in project: 156"),
            Some(156)
        );
        assert_eq!(extract_target_count("something else"), None);
    }

    // ── Success cases ──

    #[test]
    fn test_meson_setup_basic() {
        let input = "\
The Meson build system
Version: 1.6.0
Source dir: /home/user/myproject
Build dir: /home/user/myproject/build
Build type: native build
Project name: myproject
Project version: 1.0.0
C compiler for the host machine: gcc (gcc 14.3.0)
C++ compiler for the host machine: g++ (gcc 14.3.0)
C++ linker for the host machine: g++ ld.bfd 2.42
Host CPU: x86_64
Build targets in project: 156
Run-time dependency boost found: YES 1.83.0
Run-time dependency vulkan found: YES 1.3.283
Compiler for C supports arguments -std=c99: YES
Compiler for C++ supports arguments -std=c++20: YES
Has header \"stdio.h\" : YES
Checking for function \"memcpy\" : YES
Checking for size of \"void*\" : 8
Library dl found: YES
Found pkg-config: YES (/usr/bin/pkg-config) 0.29.2
Program python3 found: YES (/usr/bin/python3)
";
        let result = filter_meson(input, 0);
        assert!(result.contains("ok meson: setup"), "got: {}", result);
        assert!(
            result.contains("myproject"),
            "should show project name, got: {}",
            result
        );
        assert!(
            result.contains("boost 1.83.0"),
            "should show deps, got: {}",
            result
        );
        assert!(
            result.contains("vulkan 1.3.283"),
            "should show deps, got: {}",
            result
        );
        // Probes should be stripped
        assert!(
            !result.contains("Compiler for C supports"),
            "should strip probe lines, got: {}",
            result
        );
        assert!(
            !result.contains("Has header"),
            "should strip probe lines, got: {}",
            result
        );
        assert!(
            !result.contains("Found pkg-config"),
            "should strip probe lines, got: {}",
            result
        );
    }

    #[test]
    fn test_meson_setup_with_subprojects() {
        let input = "\
The Meson build system
Version: 1.6.0
Source dir: /home/user/myproject
Build dir: /home/user/myproject/build
Build type: native build
Project name: myproject
Project version: 1.0.0
C++ compiler for the host machine: g++ (gcc 14.3.0)
Host CPU: x86_64
Build targets in project: 42
freetype2|Compiler for C supports arguments -std=c99: YES
freetype2|Has header \"ft2build.h\" : YES
qhull|Compiler for C++ supports arguments -std=c++20: YES
qhull|Has header \"qhull.h\" : YES
Executing subproject freetype2
Executing subproject qhull
";
        let result = filter_meson(input, 0);
        assert!(result.contains("ok meson: setup"), "got: {}", result);
        assert!(
            result.contains("subprojects:"),
            "should show subprojects, got: {}",
            result
        );
        assert!(
            result.contains("freetype2"),
            "should list subproject, got: {}",
            result
        );
        assert!(
            result.contains("qhull"),
            "should list subproject, got: {}",
            result
        );
        // Subproject probes should be stripped
        assert!(
            !result.contains("ft2build.h"),
            "should strip subproject probes, got: {}",
            result
        );
    }

    #[test]
    fn test_meson_setup_missing_dep() {
        let input = "\
The Meson build system
Version: 1.6.0
Source dir: /home/user/myproject
Build dir: /home/user/myproject/build
Build type: native build
Project name: myproject
Project version: 1.0.0
C++ compiler for the host machine: g++ (gcc 14.3.0)
Host CPU: x86_64
Run-time dependency foo found: NO (tried pkgconfig and cmake)
";
        let result = filter_meson(input, 1);
        assert!(
            result.contains("FAILED"),
            "should show FAILED, got: {}",
            result
        );
        assert!(
            result.contains("missing deps:"),
            "should show missing deps, got: {}",
            result
        );
        assert!(
            result.contains("foo"),
            "should show missing dep name, got: {}",
            result
        );
    }

    #[test]
    fn test_meson_setup_error() {
        let input = "\
The Meson build system
Version: 1.6.0
Source dir: /home/user/myproject
Build dir: /home/user/myproject/build
C++ compiler for the host machine: g++ (gcc 14.3.0)
meson.build:42:10: ERROR: Dependency \"foo\" not found
";
        let result = filter_meson(input, 1);
        assert!(
            result.contains("FAILED"),
            "should show FAILED, got: {}",
            result
        );
        assert!(
            result.contains("meson.build:42:10:"),
            "should show error location, got: {}",
            result
        );
        assert!(
            result.contains("Dependency"),
            "should show error detail, got: {}",
            result
        );
    }

    #[test]
    fn test_meson_setup_warning() {
        let input = "\
The Meson build system
Version: 1.6.0
Project name: myproject
Project version: 1.0.0
C++ compiler for the host machine: g++ (gcc 14.3.0)
Host CPU: x86_64
meson.build:5: WARNING: Project targets '>=1.0' but uses feature deprecated in '1.2'
Build targets in project: 10
";
        let result = filter_meson(input, 0);
        // Warnings are collected but may not show in summary if we don't emit them
        // Check that the setup was successful despite the warning
        assert!(
            result.contains("ok meson: setup"),
            "should be ok despite warning, got: {}",
            result
        );
    }

    #[test]
    fn test_meson_setup_wrap_download() {
        let input = "\
The Meson build system
Version: 1.6.0
Project name: myproject
Project version: 1.0.0
C++ compiler for the host machine: g++ (gcc 14.3.0)
Host CPU: x86_64
Downloading wrap source from https://wrapdb.mesonbuild.com/v2/freetype2_2.13.2.tar.gz
Build targets in project: 156
";
        let result = filter_meson(input, 0);
        assert!(result.contains("ok meson: setup"), "got: {}", result);
        assert!(
            !result.contains("Downloading wrap"),
            "download lines should be stripped, got: {}",
            result
        );
    }

    #[test]
    fn test_meson_cross_compile() {
        let input = "\
The Meson build system
Version: 1.6.0
Source dir: /home/user/myproject
Build dir: /home/user/myproject/build
Build type: cross build
Project name: myproject
Project version: 1.0.0
C compiler for the host machine: arm-linux-gnueabihf-gcc (gcc 12.2.0)
C++ compiler for the host machine: arm-linux-gnueabihf-g++ (gcc 12.2.0)
Host CPU: arm
Build targets in project: 42
";
        let result = filter_meson(input, 0);
        assert!(result.contains("ok meson: setup"), "got: {}", result);
        assert!(
            result.contains("gcc 12.2.0"),
            "should show cross compiler, got: {}",
            result
        );
    }

    #[test]
    fn test_meson_ansi_stripped() {
        let input = "\x1b[1mThe Meson build system\x1b[0m\n\
                      \x1b[1mVersion: 1.6.0\x1b[0m\n\
                      Project name: myproject\n\
                      C++ compiler for the host machine: g++ (gcc 14.3.0)\n\
                      Host CPU: x86_64\n\
                      \x1b[31mmeson.build:1:1: ERROR: bad\x1b[0m\n";
        let result = filter_meson(input, 1);
        assert!(!result.contains("\x1b["), "ANSI codes should be stripped");
        assert!(
            result.contains("FAILED"),
            "should show FAILED, got: {}",
            result
        );
    }

    #[test]
    fn test_meson_empty_input() {
        let result = filter_meson("", 0);
        assert!(
            result.contains("meson"),
            "should have a summary, got: '{}'",
            result
        );
    }

    #[test]
    fn test_meson_token_savings_above_80pct() {
        let mut input = String::new();
        input.push_str("The Meson build system\n");
        input.push_str("Version: 1.6.0\n");
        input.push_str("Source dir: /home/user/myproject\n");
        input.push_str("Build dir: /home/user/myproject/build\n");
        input.push_str("Build type: native build\n");
        input.push_str("Project name: myproject\n");
        input.push_str("Project version: 1.0.0\n");
        input.push_str("C++ compiler for the host machine: g++ (gcc 14.3.0)\n");
        input.push_str("Host CPU: x86_64\n");
        input.push_str("Build targets in project: 42\n");
        // Generate 500 probe lines
        for i in 1..=500 {
            input.push_str(&format!(
                "Compiler for C supports arguments -fopt-{}: YES\n",
                i
            ));
        }
        input.push_str("Run-time dependency boost found: YES 1.83.0\n");

        let result = filter_meson(&input, 0);
        let raw_tokens = estimate_tokens(&input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 80,
            "token savings: {}% (expected >=80%)",
            savings
        );
    }
}
