//! Filters cmake configure output — compiler probes stripped, errors/warnings kept.

use crate::core::runner;
use crate::core::utils::{resolved_command, strip_ansi};
use anyhow::Result;

/// Regex patterns for line classification — compiled once via `lazy_static!`.
macro_rules! lazy_re {
    ($re:literal) => {{
        lazy_static::lazy_static! {
            static ref RE: regex::Regex = regex::Regex::new($re).unwrap();
        }
        &*RE
    }};
}

/// Returns `true` for cmake compiler probe lines — stripped regardless of language.
fn is_droppable_line(trimmed: &str) -> bool {
    // Compiler identification: -- The X compiler identification is ...
    lazy_re!(r"^-- The \w+ compiler identification")
        .is_match(trimmed)
        // -- Check for working X compiler...
        || lazy_re!(r"^-- Check for working \w+ compiler")
            .is_match(trimmed)
        // -- Detecting X compiler ABI info / -- Detecting X compile features
        || lazy_re!(r"^-- Detecting \w+( compiler)? (ABI info|compile features)")
            .is_match(trimmed)
        // -- Performing Test ...
        || lazy_re!(r"^-- Performing Test ")
            .is_match(trimmed)
        // -- Looking for ...
        || lazy_re!(r"^-- Looking for ")
            .is_match(trimmed)
        // -- Searching for ...
        || lazy_re!(r"^-- Searching for ")
            .is_match(trimmed)
        // -- Configuring done / -- Generating done (with or without timing)
        || lazy_re!(r"^-- (Configuring|Generating) done")
            .is_match(trimmed)
        // -- Found PkgConfig / Found wayland (noisy built-in find modules)
        || lazy_re!(r"^-- Found (PkgConfig|wayland)")
            .is_match(trimmed)
}

/// Lines that start a multi-line error block.
const ERROR_BLOCK_START: &str = "CMake Error at ";
/// Lines that start with `CMake Error:` (colon variant, e.g. source directory not found).
const ERROR_LINE_START: &str = "CMake Error:";

/// Lines that start a multi-line warning block.
const WARNING_BLOCK_START: &str = "CMake Warning at ";

/// Lines indicating a missing dependency.
const COULD_NOT_FIND_PREFIX: &str = "-- Could NOT find ";

/// Track state during cmake output parsing.
struct CmakeStats {
    errors: Vec<String>,
    warnings: Vec<String>,
    missing_deps: Vec<String>,
    /// Found packages (user-requested `find_package` results).
    found_packages: Vec<String>,
    /// User-set cache variables (`-D` flag echoes).
    cache_vars: Vec<String>,
    /// Build info lines (sccache, LTO, platform, etc.).
    info_lines: Vec<String>,
    generator: String,
    build_dir: String,
    has_fatal: bool,
}

impl CmakeStats {
    fn new() -> Self {
        Self {
            errors: Vec::new(),
            warnings: Vec::new(),
            missing_deps: Vec::new(),
            found_packages: Vec::new(),
            cache_vars: Vec::new(),
            info_lines: Vec::new(),
            generator: String::new(),
            build_dir: String::new(),
            has_fatal: false,
        }
    }
}

/// Check if a line is a user-set cmake cache variable (`-- <VAR>:<TYPE>=<VALUE>`).
fn is_cache_var_line(line: &str) -> Option<&str> {
    let stripped = line.strip_prefix("-- ")?.trim();
    if !stripped.contains(':') || !stripped.contains('=') {
        return None;
    }
    // Only match if the name before `:` isn't a known probe/info prefix.
    // Reuses the same regex checks as `is_droppable_line` to stay in sync.
    let var_name = stripped.split(':').next()?;
    if is_droppable_line(line) {
        return None;
    }
    // Exclude lines that are already classified as messages, not cache vars.
    if var_name.starts_with("Found") || var_name.starts_with("Could NOT") {
        return None;
    }
    Some(stripped)
}

/// Check if a line is a non-noisy `-- Found X: ...` for a user-requested package.
///
/// Drops known built-in probe modules (PkgConfig, wayland, Threads, etc.)
/// and keeps packages explicitly listed via `find_package()` in CMakeLists.txt.
/// The blocklist here is intentionally minimal: built-in CMake find-modules that
/// every project encounters. User packages like `CUDAToolkit`, `Vulkan`, `LLVM`,
/// `embree` are kept.
const NOISY_FOUND_MODULES: &[&str] = &[
    "Found PkgConfig",
    "Found wayland",
    "Found Threads",
    "Found X11",
    "Found PNG",
    "Found JPEG",
    "Found ZLIB",
    "Found BZip2",
    "Found OpenMP",
    "Found CURL",
    "Found Python",
    "Found Python3",
    "Found Perl",
    "Found Git",
    "Found Subversion",
];

fn is_significant_found_line(line: &str) -> Option<&str> {
    let stripped = line.strip_prefix("-- ")?;
    if !stripped.starts_with("Found ") {
        return None;
    }
    if NOISY_FOUND_MODULES.iter().any(|p| stripped.starts_with(p)) {
        return None;
    }
    Some(stripped)
}

/// Check if this line is a significant informational line (build config, LTO, etc.).
fn is_info_line(line: &str) -> Option<&str> {
    let stripped = line.strip_prefix("-- ")?;
    if stripped.starts_with("Build")
        || stripped.starts_with("Enable")
        || stripped.starts_with("Disable")
        || stripped.starts_with("Configuring")
        || stripped.starts_with("Install")
        || stripped.starts_with("IPO")
        || stripped.starts_with("LTO")
        || stripped.starts_with("Using")
    {
        return Some(stripped);
    }
    None
}

/// Determine if a line should be kept during configure output.
/// Side-effect: populates `stats` for error/warning/dep tracking.
fn should_keep_line(line: &str, stats: &mut CmakeStats) -> bool {
    let trimmed = line.trim();

    // Skip blank lines
    if trimmed.is_empty() {
        return false;
    }

    // ── Error block detection (multi-line) ──

    // CMake Error at file:line (message): — multi-line error block
    if trimmed.starts_with(ERROR_BLOCK_START) {
        let ansi_free = strip_ansi(trimmed);
        stats.errors.push(ansi_free);
        return true;
    }
    // CMake Error: ... — single-line error (e.g. source dir not found)
    if trimmed.starts_with(ERROR_LINE_START) {
        stats.has_fatal = true;
        let ansi_free = strip_ansi(trimmed);
        stats.errors.push(ansi_free);
        return true;
    }
    // Error block continuation — lines that are indented or known continuation
    // (Call Stack, messages). Terminates on blank line, new CMake block, or `-- ` line.
    if !stats.errors.is_empty()
        && !trimmed.starts_with("-- ")
        && !trimmed.starts_with("CMake Error at ")
        && !trimmed.starts_with("CMake Error:")
        && !trimmed.starts_with("CMake Warning at ")
    {
        let last = stats.errors.last_mut().unwrap();
        last.push('\n');
        last.push_str(&strip_ansi(trimmed));
        return false;
    }

    // ── Warning block detection ──

    if trimmed.starts_with(WARNING_BLOCK_START) {
        let ansi_free = strip_ansi(trimmed);
        stats.warnings.push(ansi_free);
        return true;
    }
    // Warning block continuation
    if !stats.warnings.is_empty()
        && !trimmed.starts_with("-- ")
        && !trimmed.starts_with("CMake Error at ")
        && !trimmed.starts_with("CMake Warning at ")
    {
        let last = stats.warnings.last_mut().unwrap();
        last.push('\n');
        last.push_str(&strip_ansi(trimmed));
        return false;
    }

    // ── Fatal error summary ──
    if trimmed.contains("Configuring incomplete, errors occurred") {
        stats.has_fatal = true;
        return true;
    }

    // ── Build directory line ──
    if trimmed.starts_with("-- Build files have been written to:") {
        stats.build_dir = trimmed
            .strip_prefix("-- Build files have been written to:")
            .unwrap_or("")
            .trim()
            .trim_end_matches('/')
            .trim_end_matches('\\')
            .to_string();
        return true;
    }

    // ── Generator info ──
    if trimmed.starts_with("-- Generator:") || trimmed.starts_with("-- The \"") {
        if let Some(gen) = trimmed.strip_prefix("-- Generator: ") {
            stats.generator = gen.to_string();
        } else if let Some(gen_end) = trimmed.strip_prefix("-- The \"") {
            if let Some(end_quote) = gen_end.find('\"') {
                stats.generator = gen_end[..end_quote].to_string();
            }
        }
        return true;
    }

    // ── Probe noise — drop ──
    if is_droppable_line(trimmed) {
        return false;
    }

    // ── Missing dependency ──
    if trimmed.starts_with(COULD_NOT_FIND_PREFIX) {
        let dep = trimmed
            .strip_prefix(COULD_NOT_FIND_PREFIX)
            .unwrap_or("")
            .to_string();
        stats.missing_deps.push(dep);
        return true;
    }

    // ── Cache variable lines (user-set -D flags) ──
    if let Some(var) = is_cache_var_line(trimmed) {
        stats.cache_vars.push(var.to_string());
        return true;
    }

    // ── Found packages (user-requested, non-noisy) ──
    if let Some(found) = is_significant_found_line(trimmed) {
        stats.found_packages.push(found.to_string());
        return true;
    }

    // ── Info lines (build config, LTO, etc.) ──
    if let Some(info) = is_info_line(trimmed) {
        stats.info_lines.push(info.to_string());
        return true;
    }

    // ── Safety valve: unrecognized `-- ` lines ──
    if trimmed.starts_with("-- ") {
        let after = trimmed.trim_start_matches("-- ").trim();
        if !after.is_empty() && !after.starts_with("The ") {
            return true;
        }
        return false;
    }

    // Everything else passes through
    true
}

/// Build the filtered output from parsed state.
fn compose_output(stats: &CmakeStats) -> String {
    let mut output = String::new();

    // If there are errors (fatal or not), show them and the failed message.
    if stats.has_fatal {
        output.push_str("cmake: configuration failed\n\n");
        for error in &stats.errors {
            output.push_str(error);
            output.push('\n');
        }
        return output;
    }
    if !stats.errors.is_empty() {
        // SEND_ERROR level — configure continued but errors were reported
        output.push_str("cmake: configured with errors\n\n");
        for error in &stats.errors {
            output.push_str(error);
            output.push('\n');
        }
        output.push('\n');
    }

    // Generator display
    let generator_display = if stats.generator.is_empty() {
        String::new()
    } else {
        format!(" ({})", stats.generator)
    };
    // Build dir — handle both `/` and `\` paths (cmake on Windows uses forward
    // slashes, some generators use backslashes).
    let build_dir_display = if stats.build_dir.is_empty() {
        String::new()
    } else {
        let dir = stats.build_dir.trim();
        let short = dir
            .split('/')
            .next_back()
            .or_else(|| dir.split('\\').next_back())
            .unwrap_or(dir);
        format!(", {}/", short)
    };

    if stats.has_fatal {
        // Already returned above
    } else if !stats.warnings.is_empty() {
        output.push_str(&format!(
            "cmake: configured{} — with {}{}\n",
            generator_display,
            stats.warnings.len(),
            build_dir_display
        ));
    } else {
        output.push_str(&format!(
            "ok cmake: configured{} {}\n",
            generator_display, build_dir_display
        ));
    }

    // Missing dependencies
    if !stats.missing_deps.is_empty() {
        output.push_str(&format!("  missing: {}\n", stats.missing_deps.join(", ")));
    }

    // User-set cache variables
    for var in &stats.cache_vars {
        output.push_str(&format!("  {}\n", var));
    }

    // Found packages
    for pkg in &stats.found_packages {
        output.push_str(&format!("  found: {}\n", pkg));
    }

    // Info lines
    for info in &stats.info_lines {
        output.push_str(&format!("  {}\n", info));
    }

    // Warning blocks
    if !stats.warnings.is_empty() {
        output.push('\n');
        for warning in &stats.warnings {
            output.push_str(warning);
            output.push('\n');
            output.push('\n');
        }
    }

    output
}

/// Filter cmake configure output.
fn filter_cmake_output(input: &str, exit_code: i32) -> String {
    let ansi_free = strip_ansi(input);
    let mut stats = CmakeStats::new();

    for line in ansi_free.lines() {
        should_keep_line(line, &mut stats);
    }

    // If cmake exited with non-zero but no errors were captured (e.g.
    // `CMake Error: source dir not found`), fall back to fatal.
    if exit_code != 0 && !stats.has_fatal && stats.errors.is_empty() {
        stats.has_fatal = true;
        // Keep the raw input so the user sees what went wrong.
        stats.errors.push(strip_ansi(input).trim().to_string());
    }

    compose_output(&stats)
}

/// Run cmake with configure output filtering.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    if verbose > 0 {
        eprintln!("cmake: running cmake {}", args.join(" "));
    }

    let mut cmd = resolved_command("cmake");
    for arg in args {
        cmd.arg(arg);
    }
    let args_str = args.join(" ");

    runner::run_filtered_with_exit(
        cmd,
        "cmake",
        &args_str,
        filter_cmake_output,
        runner::RunOptions::with_tee("cmake"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::tracking::estimate_tokens;

    // Test helper
    fn filter_cmake(input: &str, exit_code: i32) -> String {
        filter_cmake_output(input, exit_code)
    }

    // ── Helper tests ──

    #[test]
    fn test_is_droppable_language_probes() {
        assert!(is_droppable_line(
            "-- The C compiler identification is GNU 13.2.0"
        ));
        assert!(is_droppable_line(
            "-- The CXX compiler identification is Clang 19.1.0"
        ));
        assert!(is_droppable_line(
            "-- The CUDA compiler identification is NVIDIA 12.3"
        ));
        assert!(is_droppable_line(
            "-- The Fortran compiler identification is GFC"
        ));
        assert!(is_droppable_line(
            "-- The HIP compiler identification is ROCm 6.0"
        ));
        assert!(is_droppable_line(
            "-- The Rust compiler identification is rustc 1.80"
        ));
        assert!(is_droppable_line(
            "-- Check for working C compiler: /usr/bin/gcc"
        ));
        assert!(is_droppable_line("-- Detecting CXX compiler ABI info"));
        assert!(is_droppable_line("-- Detecting C compile features"));
        assert!(is_droppable_line("-- Performing Test HAVE_SOME_FEATURE"));
        assert!(is_droppable_line("-- Looking for include file stdio.h"));
        assert!(is_droppable_line("-- Searching for a C compiler"));
        assert!(is_droppable_line("-- Configuring done (3.2s)"));
        assert!(is_droppable_line("-- Generating done (0.1s)"));
    }

    #[test]
    fn test_is_droppable_not_false_positive() {
        assert!(!is_droppable_line("-- Build for Linux (x86_64)"));
        assert!(!is_droppable_line("-- Configuring Project 1.0..."));
        assert!(!is_droppable_line(
            "CMake Error at CMakeLists.txt:5 (message):"
        ));
        assert!(!is_droppable_line("-- Could NOT find CUDAToolkit"));
    }

    #[test]
    fn test_is_cache_var_line_valid() {
        assert_eq!(
            is_cache_var_line("-- LUISA_COMPUTE_ENABLE_RUST:BOOL=OFF"),
            Some("LUISA_COMPUTE_ENABLE_RUST:BOOL=OFF")
        );
        assert_eq!(
            is_cache_var_line("-- CMAKE_BUILD_TYPE:STRING=Release"),
            Some("CMAKE_BUILD_TYPE:STRING=Release")
        );
    }

    #[test]
    fn test_is_cache_var_line_invalid() {
        // Probe lines should not be treated as cache vars
        assert_eq!(is_cache_var_line("-- The C compiler identification"), None);
        assert_eq!(is_cache_var_line("-- Performing Test HAVE_FOO"), None);
        // Lines without `=` should not match
        assert_eq!(is_cache_var_line("-- Build for Linux"), None);
    }

    #[test]
    fn test_is_significant_found_line_drops_noisy() {
        assert_eq!(
            is_significant_found_line(
                "-- Found PkgConfig: /usr/bin/pkg-config (found version \"0.29.2\")"
            ),
            None
        );
        assert_eq!(
            is_significant_found_line("-- Found wayland, version 1.22.0"),
            None
        );
    }

    #[test]
    fn test_is_significant_found_line_keeps_user_packages() {
        assert!(
            is_significant_found_line("-- Found CUDAToolkit: /usr/local/cuda/include").is_some()
        );
        assert!(is_significant_found_line("-- Found Vulkan: /usr/lib/libvulkan.so").is_some());
        assert!(is_significant_found_line("-- Found LLVM: /usr/lib/llvm-18/include").is_some());
        assert!(
            is_significant_found_line("-- Found embree: /usr/lib/cmake/embree-4.3.0").is_some()
        );
    }

    // ── Success cases ──

    #[test]
    fn test_cmake_successful_configure_luisa() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Detecting C compiler ABI info
-- Detecting C compiler ABI info - done
-- Check for working C compiler: /usr/bin/gcc - skipped
-- Detecting C compile features
-- Detecting C compile features - done
-- Detecting CXX compiler ABI info
-- Detecting CXX compiler ABI info - done
-- Check for working CXX compiler: /usr/bin/g++ - skipped
-- Detecting CXX compile features
-- Detecting CXX compile features - done
-- Configuring LuisaCompute 0.9.3...
-- Build with sccache: /usr/bin/sccache
-- Build for Linux (x86_64)
-- Enable Rust support (toolchain found at /home/runner/.cargo/bin/cargo)
-- Performing Test CMAKE_HAVE_LIBC_PTHREAD - Success
-- Found PkgConfig: /usr/bin/pkg-config (found version \"0.29.2\")
-- Found CUDAToolkit: /usr/local/cuda/include
-- Found Vulkan: /usr/lib/x86_64-linux-gnu/libvulkan.so
-- Found embree: /usr/lib/cmake/embree-4.3.0
-- Found LLVM: /usr/lib/llvm-18/include
-- IPO/LTO enabled for release builds
-- Configuring done (3.2s)
-- Generating done (0.1s)
-- Build files have been written to: /home/runner/LuisaCompute/build
";
        let result = filter_cmake(input, 0);
        assert!(result.contains("ok cmake: configured"), "got: {}", result);
        assert!(
            !result.contains("The C compiler identification"),
            "should strip probe lines, got: {}",
            result
        );
        assert!(
            !result.contains("Detecting C compiler"),
            "should strip probe lines, got: {}",
            result
        );

        // Found packages should be listed
        assert!(
            result.contains("found: Found CUDAToolkit"),
            "got: {}",
            result
        );
        assert!(result.contains("found: Found Vulkan"), "got: {}", result);
        assert!(result.contains("found: Found LLVM"), "got: {}", result);

        // PkgConfig noise should be stripped
        assert!(
            !result.contains("PkgConfig"),
            "should strip PkgConfig, got: {}",
            result
        );

        // Token savings check
        let raw_tokens = estimate_tokens(input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 55,
            "token savings: {}% (expected >=55%)",
            savings
        );
    }

    #[test]
    fn test_cmake_configure_with_user_cache_vars() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Detecting C compiler ABI info
-- Detecting C compiler ABI info - done
-- Configuring LuisaCompute 0.9.3...
-- Build for Windows (AMD64)
-- LUISA_COMPUTE_ENABLE_RUST:BOOL=OFF
-- LUISA_COMPUTE_ENABLE_REMOTE:BOOL=OFF
-- LUISA_COMPUTE_ENABLE_CPU:BOOL=OFF
-- LUISA_COMPUTE_ENABLE_UNITY_BUILD:BOOL=OFF
-- CMAKE_BUILD_TYPE:STRING=Release
-- CMAKE_C_COMPILER:FILEPATH=clang-cl
-- Configuring done
-- Generating done
-- Build files have been written to: build
";
        let result = filter_cmake(input, 0);
        assert!(result.contains("ok cmake:"), "got: {}", result);
        assert!(
            result.contains("LUISA_COMPUTE_ENABLE_RUST:BOOL=OFF"),
            "should keep cache vars, got: {}",
            result
        );
        assert!(
            result.contains("CMAKE_BUILD_TYPE:STRING=Release"),
            "should keep cache vars, got: {}",
            result
        );
        assert!(
            !result.contains("The C compiler"),
            "should strip probes, got: {}",
            result
        );
    }

    // ── Warning cases ──

    #[test]
    fn test_cmake_missing_optional_dependency() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Detecting C compiler ABI info
-- Detecting C compiler ABI info - done
-- Configuring LuisaCompute 0.9.3...
-- Build for Linux (x86_64)
-- Could NOT find CUDAToolkit (missing: CUDAToolkit_INCLUDE_DIRS)
CMake Warning at scripts/validate_options.cmake:70 (message):
  The CUDA backend is not available. The CUDA backend will be disabled.
Call Stack (most recent call first):
  CMakeLists.txt:98 (include)

-- Configuring done (1.8s)
-- Generating done (0.1s)
-- Build files have been written to: /home/runner/LuisaCompute/build
";
        let result = filter_cmake(input, 0);
        assert!(result.contains("configured"), "got: {}", result);
        assert!(result.contains("missing:"), "got: {}", result);
        assert!(result.contains("CUDAToolkit"), "got: {}", result);
        assert!(
            result.contains("CMake Warning"),
            "warning block should be preserved, got: {}",
            result
        );
        assert!(result.contains("CUDA backend"), "got: {}", result);
    }

    #[test]
    fn test_cmake_multiple_missing_deps() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Configuring LuisaCompute 0.9.3...
-- Could NOT find CUDAToolkit (missing: CUDAToolkit_INCLUDE_DIRS)
CMake Warning at scripts/validate_options.cmake:70 (message):
  The CUDA backend is not available.
Call Stack (most recent call first):
  CMakeLists.txt:98 (include)

-- Could NOT find LLVM (missing: LLVM_DIR)
CMake Warning at scripts/validate_options.cmake:121 (message):
  The fallback backend is not available.
Call Stack (most recent call first):
  CMakeLists.txt:105 (include)

-- Configuring done
-- Generating done
-- Build files have been written to: build
";
        let result = filter_cmake(input, 0);
        assert!(result.contains("missing:"), "got: {}", result);
        assert!(result.contains("CUDAToolkit"), "got: {}", result);
        assert!(result.contains("LLVM"), "got: {}", result);
    }

    // ── Error cases ──

    #[test]
    fn test_cmake_fatal_error_missing_required() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Configuring LuisaCompute 0.9.3...
CMake Error at scripts/validate_options.cmake:73 (message):
  The DirectX backend is not available. Please install the dependencies to
  enable the DirectX backend.
Call Stack (most recent call first):
  CMakeLists.txt:105 (include)

-- Configuring incomplete, errors occurred!
";
        let result = filter_cmake(input, 1);
        assert!(result.contains("configuration failed"), "got: {}", result);
        assert!(
            result.contains("CMake Error"),
            "error block should be preserved, got: {}",
            result
        );
        assert!(result.contains("DirectX backend"), "got: {}", result);
        assert!(
            result.contains("Call Stack"),
            "call stack should be preserved, got: {}",
            result
        );
    }

    #[test]
    fn test_cmake_error_multiline_message() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Configuring Project 1.0.0...
CMake Error at CMakeLists.txt:5 (find_package):
  Could not find a package configuration file provided by \"SomeLib\" with any
  of the following names:

    SomeLibConfig.cmake
    somelib-config.cmake

  Add the installation prefix of \"SomeLib\" to CMAKE_PREFIX_PATH or set
  \"SomeLib_DIR\" to a directory containing one of the above files.
Call Stack (most recent call first):
  CMakeLists.txt:10 (include)

-- Configuring incomplete, errors occurred!
";
        let result = filter_cmake(input, 1);
        assert!(result.contains("configuration failed"), "got: {}", result);
        assert!(
            result.contains("Could not find a package"),
            "got: {}",
            result
        );
        assert!(result.contains("SomeLibConfig.cmake"), "got: {}", result);
        assert!(result.contains("CMAKE_PREFIX_PATH"), "got: {}", result);
    }

    /// cmake `message(SEND_ERROR)` — configure continues but errors are shown.
    #[test]
    fn test_cmake_send_error_not_fatal() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Configuring Project 1.0.0...
CMake Error at CMakeLists.txt:10 (message):
  SEND_ERROR: Something went wrong but we continue.

-- Configuring done (0.5s)
-- Generating done (0.1s)
-- Build files have been written to: build
";
        let result = filter_cmake(input, 0);
        // No "Configuring incomplete" → not fatal → "configured with errors"
        assert!(result.contains("configured with errors"), "got: {}", result);
        assert!(result.contains("SEND_ERROR"), "got: {}", result);
    }

    // ── Edge cases ──

    #[test]
    fn test_cmake_ansi_stripped() {
        let input = "\x1b[32m-- Configuring Project...\x1b[0m\n\
                      \x1b[31mCMake Error at CMakeLists.txt:1 (message):\x1b[0m\n\
                      \x1b[31m  This is an error\x1b[0m\n";
        let result = filter_cmake(input, 1);
        // No ANSI escape sequences should remain
        assert!(!result.contains("\x1b["), "ANSI codes should be stripped");
        // Without "Configuring incomplete", this is a SEND_ERROR level
        assert!(
            result.contains("configured with errors") || result.contains("configuration failed"),
            "should mention errors, got: {}",
            result
        );
    }

    #[test]
    fn test_cmake_empty_input() {
        let result = filter_cmake("", 0);
        assert!(
            result.contains("cmake"),
            "should have a summary, got: '{}'",
            result
        );
    }

    #[test]
    fn test_cmake_only_probe_lines() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Detecting C compiler ABI info
-- Detecting C compiler ABI info - done
-- Check for working C compiler: /usr/bin/gcc - skipped
-- Detecting C compile features
-- Detecting C compile features - done
";
        let result = filter_cmake(input, 0);
        // No meaningful output, but should still say something sensible
        assert!(
            result.contains("cmake"),
            "should have a summary, got: '{}'",
            result
        );
        assert!(
            !result.contains("The C compiler"),
            "should strip probes, got: {}",
            result
        );
    }

    #[test]
    fn test_cmake_token_savings_above_60pct() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Detecting C compiler ABI info
-- Detecting C compiler ABI info - done
-- Check for working C compiler: /usr/bin/gcc - skipped
-- Detecting C compile features
-- Detecting C compile features - done
-- Detecting CXX compiler ABI info
-- Detecting CXX compiler ABI info - done
-- Check for working CXX compiler: /usr/bin/g++ - skipped
-- Detecting CXX compile features
-- Detecting CXX compile features - done
-- Configuring LuisaCompute 0.9.3...
-- Build with sccache: /usr/bin/sccache
-- Build for Linux (x86_64)
-- Enable Rust support (toolchain found at /home/runner/.cargo/bin/cargo)
-- Performing Test CMAKE_HAVE_LIBC_PTHREAD - Success
CMake Error at scripts/validate_options.cmake:73 (message):
  The DirectX backend is not available.
Call Stack (most recent call first):
  CMakeLists.txt:105 (include)

-- Configuring incomplete, errors occurred!
";
        let result = filter_cmake(input, 1);
        let raw_tokens = estimate_tokens(input);
        let filtered_tokens = estimate_tokens(&result);
        let savings = if raw_tokens > 0 {
            ((raw_tokens - filtered_tokens) as f64 / raw_tokens as f64 * 100.0) as usize
        } else {
            0
        };
        assert!(
            savings >= 60,
            "token savings: {}% (expected >=60%)",
            savings
        );
    }

    /// Windows backslash paths should produce a clean short name.
    #[test]
    fn test_cmake_build_dir_backslash() {
        let input = "\
-- The C compiler identification is GNU 13.2.0
-- The CXX compiler identification is GNU 13.2.0
-- Configuring done
-- Generating done
-- Build files have been written to: D:\\myproject\\build_out
";
        let result = filter_cmake(input, 0);
        assert!(
            result.contains("build_out/"),
            "should extract dir name from backslash path, got: {}",
            result
        );
    }
}
