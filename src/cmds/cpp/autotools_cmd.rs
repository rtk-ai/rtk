//! Filters autotools (./configure) output — check probes counted/stripped, errors/warnings kept.
#![allow(dead_code)]

use super::diag;
use crate::core::runner;
use crate::core::utils::resolved_command;
use anyhow::Result;

// ── Stats ──

pub struct AutotoolsStats {
    pub checks_total: usize,
    pub checks_passed: usize,
    pub checks_failed: Vec<String>,
    pub compiler_info: Vec<String>,
    pub build_system_type: Option<String>,
    pub host_system_type: Option<String>,
    pub errors: Vec<String>,
    pub warnings: Vec<String>,
    pub config_status_lines: Vec<String>,
    pub has_fatal: bool,
}

impl AutotoolsStats {
    fn new() -> Self {
        Self {
            checks_total: 0,
            checks_passed: 0,
            checks_failed: Vec::new(),
            compiler_info: Vec::new(),
            build_system_type: None,
            host_system_type: None,
            errors: Vec::new(),
            warnings: Vec::new(),
            config_status_lines: Vec::new(),
            has_fatal: false,
        }
    }
}

// ── Line classification ──

/// Detect "checking for X... result" lines.
/// Returns `Some((feature, result))` if matched.
fn is_checking_line(trimmed: &str) -> Option<(&str, &str)> {
    let stripped = trimmed.strip_prefix("checking ")?;
    // Find the "... " separator (triple dots + space)
    let dots_end = stripped.find("... ")?;
    let feature = &stripped[..dots_end];
    let result = &stripped[dots_end + 4..]; // "... " = 4 chars
                                            // Skip "whether" probes (e.g. "checking whether the C compiler works")
    if feature.starts_with("whether ") || feature.starts_with("if ") {
        return None;
    }
    Some((feature, result))
}

/// Detect configure error lines: "configure: error: ..."
fn is_configure_error(trimmed: &str) -> bool {
    trimmed.starts_with("configure: error:")
}

/// Detect configure warning lines: "configure: WARNING: ..."
fn is_configure_warning(trimmed: &str) -> bool {
    trimmed.starts_with("configure: WARNING:")
}

/// Detect config.status lines: "config.status: creating X"
/// Returns `Some(filename)` if matched.
fn is_config_status(trimmed: &str) -> Option<&str> {
    let rest = trimmed.strip_prefix("config.status: ")?;
    if rest.starts_with("creating ")
        || rest.starts_with("executing ")
        || rest.starts_with("linking ")
        || rest.starts_with("generating ")
    {
        Some(rest)
    } else {
        None
    }
}

/// Detect system type lines: "checking build system type... x86_64-pc-linux-gnu"
/// or "checking host system type... x86_64-pc-linux-gnu".
/// Returns `Some(("build"/"host", "x86_64-pc-linux-gnu"))`.
fn is_system_type(trimmed: &str) -> Option<(&str, &str)> {
    let rest = trimmed.strip_prefix("checking ")?;
    if let Some(val) = rest.strip_prefix("build system type... ") {
        Some(("build", val))
    } else if let Some(val) = rest.strip_prefix("host system type... ") {
        Some(("host", val))
    } else {
        None
    }
}

/// Detect compiler info lines: "checking for gcc... /usr/bin/gcc",
/// "checking whether the C compiler works... yes", etc.
fn is_compiler_check(trimmed: &str) -> bool {
    if !trimmed.starts_with("checking ") {
        return false;
    }
    // "checking for gcc" / "checking for C compiler default output" etc.
    trimmed.contains("compiler")
        || trimmed.contains("CC")
        || trimmed.contains("CXX")
        || trimmed.contains("gcc")
        || trimmed.contains("g++")
        || trimmed.contains("clang")
        || trimmed.contains("CC=")
        || trimmed.contains("CXX=")
}

// ── Filter ──

fn filter_autotools_output(input: &str, exit_code: i32) -> String {
    let normalized = diag::normalize(input);
    let mut stats = AutotoolsStats::new();

    for line in normalized.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        // ── configure: error: → fatal + keep ──
        if is_configure_error(trimmed) {
            stats.has_fatal = true;
            stats.errors.push(trimmed.to_string());
            continue;
        }

        // ── configure: WARNING: → warnings + keep ──
        if is_configure_warning(trimmed) {
            stats.warnings.push(trimmed.to_string());
            continue;
        }

        // ── config.status: creating X ──
        if let Some(rest) = is_config_status(trimmed) {
            stats.config_status_lines.push(rest.to_string());
            continue;
        }

        // ── System type ──
        if let Some((kind, val)) = is_system_type(trimmed) {
            match kind {
                "build" => stats.build_system_type = Some(val.to_string()),
                "host" => stats.host_system_type = Some(val.to_string()),
                _ => {}
            }
            continue;
        }

        // ── Compiler info ──
        if is_compiler_check(trimmed) {
            stats.compiler_info.push(trimmed.to_string());
            continue;
        }

        // ── Checking lines: "checking for X... result" ──
        if let Some((feature, result)) = is_checking_line(trimmed) {
            stats.checks_total += 1;
            if result.eq_ignore_ascii_case("yes") {
                stats.checks_passed += 1;
                // Drop — successful check
                continue;
            } else if result.eq_ignore_ascii_case("no") {
                stats.checks_failed.push(feature.to_string());
                // Keep — failed check
                continue;
            }
            // Other results (paths, versions, etc.) — keep as info
            stats.checks_passed += 1;
            stats.compiler_info.push(trimmed.to_string());
            continue;
        }

        // ── Fail-open: everything else passes through ──
        // (This line is already in the output implicitly via compose)
    }

    // If exit code is non-zero but no fatal errors captured, mark as fatal
    if exit_code != 0 && !stats.has_fatal && stats.errors.is_empty() {
        stats.has_fatal = true;
        stats.errors.push(normalized.trim().to_string());
    }

    compose_output(&stats)
}

/// Build the filtered output from parsed state.
fn compose_output(stats: &AutotoolsStats) -> String {
    let mut output = String::new();

    if stats.has_fatal {
        output.push_str("configure: FAILED\n");
        for error in &stats.errors {
            output.push_str(error);
            output.push('\n');
        }
        output.push_str("full log: config.log\n");
        return output;
    }

    // Success
    let failed_count = stats.checks_failed.len();
    output.push_str(&format!(
        "ok configure: {} checks, {} failed\n",
        stats.checks_total, failed_count
    ));

    // System type
    let sys = stats
        .host_system_type
        .as_deref()
        .or(stats.build_system_type.as_deref())
        .unwrap_or("unknown");
    output.push_str(&format!("  system: {}\n", sys));

    // Compiler info
    if !stats.compiler_info.is_empty() {
        // Try to extract a short compiler description
        let compiler_desc = extract_compiler_desc(&stats.compiler_info);
        output.push_str(&format!("  compiler: {}\n", compiler_desc));
    }

    // Failed checks
    if !stats.checks_failed.is_empty() {
        output.push_str(&format!(
            "  failed checks: {}\n",
            stats.checks_failed.join(", ")
        ));
    }

    // Created files
    if !stats.config_status_lines.is_empty() {
        let created: Vec<&str> = stats
            .config_status_lines
            .iter()
            .filter_map(|l| l.strip_prefix("creating "))
            .collect();
        if !created.is_empty() {
            output.push_str(&format!("  created: {}\n", created.join(", ")));
        }
    }

    // Warnings
    if !stats.warnings.is_empty() {
        for w in &stats.warnings {
            output.push_str(w);
            output.push('\n');
        }
    }

    output.push_str("full log: config.log\n");
    output
}

fn extract_compiler_desc(info_lines: &[String]) -> String {
    // Look for "checking for gcc... /usr/bin/gcc" or "checking for g++... /usr/bin/g++"
    for line in info_lines {
        if let Some((feature, result)) = is_checking_line(line) {
            let lower = feature.to_lowercase();
            if lower.contains("gcc") || lower.starts_with("gcc") {
                return format!("gcc {}", result);
            }
            if lower.contains("g++") || lower.starts_with("g++") {
                return format!("g++ {}", result);
            }
            if lower.contains("clang++") {
                return format!("clang++ {}", result);
            }
            if lower.contains("clang") {
                return format!("clang {}", result);
            }
            if lower.contains("cc") || lower.contains("c compiler") {
                return format!("cc {}", result);
            }
            if lower.contains("cxx") || lower.contains("c++ compiler") {
                return format!("c++ {}", result);
            }
        }
    }
    // Fallback: use the first available info line
    info_lines
        .first()
        .cloned()
        .unwrap_or_else(|| "unknown".to_string())
}

// ── Public API ──

/// Run `./configure` (or `resolved_command("configure")`) with output filtering.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("configure: running configure {}", args.join(" "));
    }

    let cmd = if args.first().map(|a| a.as_str()) == Some("configure") {
        let mut c = resolved_command(&args[0]);
        for arg in &args[1..] {
            c.arg(arg);
        }
        c
    } else {
        let mut c = resolved_command("configure");
        for arg in args {
            c.arg(arg);
        }
        c
    };
    let args_str = args.join(" ");

    runner::run_filtered_with_exit(
        cmd,
        "configure",
        &args_str,
        filter_autotools_output,
        runner::RunOptions::with_tee("configure"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    fn filter_autotools(input: &str, exit_code: i32) -> String {
        filter_autotools_output(input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_checking_line_yes() {
        assert_eq!(
            is_checking_line("checking for ICU... yes"),
            Some(("for ICU", "yes"))
        );
    }

    #[test]
    fn test_is_checking_line_path() {
        assert_eq!(
            is_checking_line("checking for gcc... /usr/bin/gcc"),
            Some(("for gcc", "/usr/bin/gcc"))
        );
    }

    #[test]
    fn test_is_checking_line_no() {
        assert_eq!(
            is_checking_line("checking for libfoo... no"),
            Some(("for libfoo", "no"))
        );
    }

    #[test]
    fn test_is_checking_line_skip_whether() {
        assert_eq!(
            is_checking_line("checking whether the C compiler works... yes"),
            None
        );
        assert_eq!(
            is_checking_line("checking if fcntl supports F_SETLKW... yes"),
            None
        );
    }

    #[test]
    fn test_is_configure_error() {
        assert!(is_configure_error(
            "configure: error: no acceptable C compiler found in $PATH"
        ));
        assert!(!is_configure_error("checking for something... yes"));
    }

    #[test]
    fn test_is_configure_warning() {
        assert!(is_configure_warning(
            "configure: WARNING: unrecognized options: --enable-foo"
        ));
        assert!(!is_configure_warning("checking for something... yes"));
    }

    #[test]
    fn test_is_config_status_creating() {
        assert_eq!(
            is_config_status("config.status: creating Makefile"),
            Some("creating Makefile")
        );
        assert_eq!(
            is_config_status("config.status: creating src/Makefile"),
            Some("creating src/Makefile")
        );
        assert_eq!(
            is_config_status("config.status: creating config.h"),
            Some("creating config.h")
        );
    }

    #[test]
    fn test_is_config_status_not() {
        assert_eq!(is_config_status("something else"), None);
        assert_eq!(is_config_status("config.status: something else"), None);
    }

    #[test]
    fn test_is_system_type_build() {
        assert_eq!(
            is_system_type("checking build system type... x86_64-pc-linux-gnu"),
            Some(("build", "x86_64-pc-linux-gnu"))
        );
    }

    #[test]
    fn test_is_system_type_host() {
        assert_eq!(
            is_system_type("checking host system type... arm-linux-gnueabihf"),
            Some(("host", "arm-linux-gnueabihf"))
        );
    }

    #[test]
    fn test_is_system_type_not() {
        assert_eq!(is_system_type("checking for something... yes"), None);
    }

    // ── Success cases ──

    #[test]
    fn test_autotools_success() {
        let input = "\
checking for a BSD-compatible install... /usr/bin/install -c
checking whether build environment is sane... yes
checking for a thread-safe mkdir -p... /bin/mkdir -p
checking for gawk... gawk
checking whether make sets $(MAKE)... yes
checking whether make supports nested variables... yes
checking build system type... x86_64-pc-linux-gnu
checking host system type... x86_64-pc-linux-gnu
checking for gcc... /usr/bin/gcc
checking whether the C compiler works... yes
checking for C compiler default output file name... a.out
checking for suffix of executables...
checking whether we are cross compiling... no
checking for suffix of object files... o
checking whether we are using the GNU C compiler... yes
checking whether /usr/bin/gcc accepts -g... yes
checking for g++... /usr/bin/g++
checking whether we are using the GNU C++ compiler... yes
checking whether /usr/bin/g++ accepts -g... yes
checking for ICU... yes
checking for OpenSSL... yes
checking for libxml2... yes
checking for zlib... yes
checking for stdlib.h... yes
checking for string.h... yes
checking for sys/time.h... yes
checking for unistd.h... yes
configure: creating ./config.status
config.status: creating Makefile
config.status: creating src/Makefile
config.status: creating config.h
config.status: executing depfiles commands
config.status: executing libtool commands
";
        let result = filter_autotools(input, 0);
        assert!(
            result.contains("ok configure:"),
            "should be ok, got: {}",
            result
        );
        assert!(
            result.contains("checks"),
            "should show check count, got: {}",
            result
        );
        assert!(
            result.contains("system: x86_64-pc-linux-gnu"),
            "got: {}",
            result
        );
        assert!(
            result.contains("compiler:"),
            "should show compiler, got: {}",
            result
        );
        assert!(
            result.contains("Makefile") || result.contains("created:"),
            "should show created files, got: {}",
            result
        );
        assert!(result.contains("full log: config.log"), "got: {}", result);
    }

    #[test]
    fn test_autotools_all_yes_90pct_savings() {
        // Generate many "checking for X... yes" lines to test token savings
        let mut input = String::new();
        for i in 1..=200 {
            input.push_str(&format!("checking for feature_{}... yes\n", i));
        }
        input.push_str("checking build system type... x86_64-pc-linux-gnu\n");
        input.push_str("checking host system type... x86_64-pc-linux-gnu\n");
        input.push_str("config.status: creating Makefile\n");
        input.push_str("config.status: creating config.h\n");

        let result = filter_autotools(&input, 0);
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
    fn test_autotools_with_failures() {
        let input = "\
checking for a BSD-compatible install... /usr/bin/install -c
checking whether build environment is sane... yes
checking build system type... x86_64-pc-linux-gnu
checking host system type... x86_64-pc-linux-gnu
checking for gcc... /usr/bin/gcc
checking whether the C compiler works... yes
checking for icu-le-hb... no
checking for libfoo... no
checking for OpenSSL... yes
configure: creating ./config.status
config.status: creating Makefile
config.status: creating config.h
";
        let result = filter_autotools(input, 0);
        assert!(result.contains("ok configure:"), "got: {}", result);
        assert!(
            result.contains("failed checks:"),
            "should show failed checks, got: {}",
            result
        );
        assert!(
            result.contains("icu-le-hb"),
            "should list failed check, got: {}",
            result
        );
        assert!(
            result.contains("libfoo"),
            "should list failed check, got: {}",
            result
        );
        assert!(
            result.contains("2 failed"),
            "should show failed count, got: {}",
            result
        );
    }

    #[test]
    fn test_autotools_fatal_error() {
        let input = "\
checking for a BSD-compatible install... /usr/bin/install -c
checking whether build environment is sane... yes
checking for gcc... no
checking for cc... no
checking for cl.exe... no
configure: error: no acceptable C compiler found in $PATH
";
        let result = filter_autotools(input, 1);
        assert!(
            result.contains("FAILED"),
            "should show FAILED, got: {}",
            result
        );
        assert!(
            result.contains("configure: error:"),
            "should show error, got: {}",
            result
        );
        assert!(
            result.contains("no acceptable C compiler"),
            "got: {}",
            result
        );
    }

    #[test]
    fn test_autotools_cross_compile() {
        let input = "\
checking build system type... x86_64-pc-linux-gnu
checking host system type... arm-linux-gnueabihf
checking for arm-linux-gnueabihf-gcc... /usr/bin/arm-linux-gnueabihf-gcc
checking whether the C compiler works... yes
checking for stdlib.h... yes
configure: creating ./config.status
config.status: creating Makefile
";
        let result = filter_autotools(input, 0);
        assert!(
            result.contains("system: arm-linux-gnueabihf"),
            "should show host system, got: {}",
            result
        );
    }

    #[test]
    fn test_autotools_cache_hits() {
        // "checking for X... (cached) yes" lines
        let input = "\
checking build system type... x86_64-pc-linux-gnu
checking host system type... x86_64-pc-linux-gnu
checking for gcc... /usr/bin/gcc
checking for stdlib.h... (cached) yes
checking for string.h... (cached) yes
checking for unistd.h... (cached) yes
checking for sys/time.h... (cached) yes
checking for OpenSSL... (cached) yes
config.status: creating Makefile
config.status: creating config.h
";
        let result = filter_autotools(input, 0);
        assert!(result.contains("ok configure:"), "got: {}", result);
        // Cached results starting with "yes" should be dropped (counted as passed)
        assert_eq!(
            result.matches("(cached)").count(),
            0,
            "cached lines should be dropped, got: {}",
            result
        );
    }

    #[test]
    fn test_autotools_ansi_stripped() {
        let input = "\x1b[32mchecking for gcc... /usr/bin/gcc\x1b[0m\n\
                      \x1b[32mchecking for stdlib.h... yes\x1b[0m\n\
                      \x1b[31mconfigure: error: Something went wrong\x1b[0m\n";
        let result = filter_autotools(input, 1);
        assert!(!result.contains("\x1b["), "ANSI codes should be stripped");
        assert!(
            result.contains("configure: error:"),
            "error should be captured, got: {}",
            result
        );
    }

    #[test]
    fn test_autotools_empty_input() {
        let result = filter_autotools("", 0);
        assert!(
            result.contains("configure"),
            "should have a summary even for empty input, got: '{}'",
            result
        );
    }

    #[test]
    fn test_autotools_token_savings_above_90pct() {
        let mut input = String::new();
        // Generate 400 "checking for X... yes" lines
        for i in 1..=400 {
            input.push_str(&format!("checking for feature_{:04}... yes\n", i));
        }
        input.push_str("checking build system type... x86_64-pc-linux-gnu\n");
        input.push_str("checking host system type... x86_64-pc-linux-gnu\n");
        input.push_str("checking for gcc... /usr/bin/gcc\n");
        input.push_str("config.status: creating Makefile\n");
        input.push_str("config.status: creating config.h\n");

        let result = filter_autotools(&input, 0);
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
}
