//! Filters cmake build/configure output — keep diagnostics, drop progress noise.

use super::failure_fallback;
use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use regex::Regex;
use std::sync::LazyLock;

static GCC_DIAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    // GCC/Clang diagnostic: file.cpp:line:col: error|warning|note: message
    Regex::new(r"^[^:\s].*:\d+:\d+:\s+(?:error|warning|note|fatal error):").unwrap()
});
static MSVC_DIAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^.+\(\d+(?:,\d+)?\):\s+(?:warning|error|fatal error)\s+[A-Z]+\d+:").unwrap()
});
static LINK_DIAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^(?:LINK|.+\.obj)\s*:\s+(?:warning|error|fatal error)\s+LNK\d+:").unwrap()
});
static RC_DIAG_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^.+\.rc\(\d+(?:,\d+)?\):\s+(?:warning|error|fatal error)\s+RC\d+:").unwrap()
});
    // make[N]: *** error
static MAKE_ERR_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^make(\[\d+\])?:\s+\*\*\*").unwrap());
    // [ N%] Building CXX object ... or [ N%] Linking ...
static CMAKE_PROGRESS_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\[\s*\d+%\]\s+(Building|Linking|Built target|Generating|Built)").unwrap()
});
    // ninja-style progress: [N/M] Building ...
static NINJA_PROGRESS_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\[\d+/\d+\]\s+(Building|Linking|Generating)").unwrap());
    // CMake configure noise lines
static CMAKE_PROBE_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(
        r"^-- (Check for working|Detecting|Looking for|Found|Performing Test|Checking)"
    ).unwrap());

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("cmake");
    for a in args {
        cmd.arg(a);
    }

    if verbose > 0 {
        eprintln!("Running: cmake {}", args.join(" "));
    }

    let is_build = args.iter().any(|a| a == "--build");
    let args_owned = args.to_vec();
    runner::run_filtered_with_exit(
        cmd,
        "cmake",
        &args.join(" "),
        move |raw, exit_code| {
            if is_build {
                filter_build(raw, &args_owned, exit_code)
            } else {
                filter_configure(raw, exit_code)
            }
        },
        RunOptions::with_tee("cmake").preserve_filtered_failure_output(),
    )
}

fn filter_build(raw: &str, args: &[String], exit_code: i32) -> String {
    let mut out = Vec::new();
    let mut diag_context = 0usize;
    let mut emitted_diag = false;
    let mut file_count = 0usize;

    for line in raw.lines() {
        if (CMAKE_PROGRESS_RE.is_match(line) || NINJA_PROGRESS_RE.is_match(line))
            && !line.trim_start().starts_with("FAILED:")
        {
            file_count += 1;
            continue;
        }
        if line.contains("Entering directory") || line.contains("Leaving directory") {
            continue;
        }

        if GCC_DIAG_RE.is_match(line)
            || MSVC_DIAG_RE.is_match(line)
            || LINK_DIAG_RE.is_match(line)
            || RC_DIAG_RE.is_match(line)
        {
            out.push(line.to_string());
            diag_context = 3;
            emitted_diag = true;
            continue;
        }
        if MAKE_ERR_RE.is_match(line) {
            out.push(line.to_string());
            emitted_diag = true;
            diag_context = 0;
            continue;
        }
        if line.contains("error:")
            || line.contains("undefined reference")
            || line.trim_start().starts_with("FAILED:")
            || line.to_ascii_lowercase().contains("build stopped: subcommand failed")
        {
            out.push(line.to_string());
            emitted_diag = true;
            diag_context = 2;
            continue;
        }
        if diag_context > 0 {
            // Source context lines (typical clang/gcc): ' 42 | code'
            //                                            '    | ^~~~'
            let trimmed = line.trim_start();
            if trimmed.is_empty()
                || trimmed.starts_with('|')
                || line.starts_with(' ')
                || line.starts_with('\t')
                || trimmed.chars().take_while(|c| c.is_ascii_digit()).count() > 0
            {
                out.push(line.to_string());
                diag_context -= 1;
                continue;
            }
            diag_context = 0;
        }
    }

    if !emitted_diag {
        if exit_code != 0 {
            return failure_fallback("cmake", exit_code, raw);
        }
        let target = args
            .iter()
            .position(|a| a == "--target")
            .and_then(|i| args.get(i + 1))
            .map(String::as_str)
            .unwrap_or_else(|| {
                args.iter()
                    .position(|a| a == "--build")
                    .and_then(|i| args.get(i + 1))
                    .map(|s| s.trim_start_matches("./"))
                    .unwrap_or("")
            });
        if target.is_empty() {
            return format!("cmake: ok  ({} files)", file_count);
        }
        return format!("cmake: ok  {}  ({} files)", target, file_count);
    }

    out.join("\n")
}

fn filter_configure(raw: &str, exit_code: i32) -> String {
    let mut out = Vec::new();
    let mut error_context = 0usize;
    let mut emitted_failure = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("CMake Error") || trimmed.starts_with("CMake Warning") {
            out.push(line.to_string());
            emitted_failure |= trimmed.starts_with("CMake Error");
            error_context = if trimmed.starts_with("CMake Error") { 6 } else { 0 };
            continue;
        }
        if error_context > 0
            && (line.starts_with(' ') || line.starts_with('\t') || trimmed.starts_with('|'))
        {
            out.push(line.to_string());
            error_context -= 1;
            continue;
        }
        error_context = 0;
        if line.starts_with("ERROR")
            || line.contains("error:")
            || MSVC_DIAG_RE.is_match(line)
            || LINK_DIAG_RE.is_match(line)
            || RC_DIAG_RE.is_match(line)
            || trimmed.contains("Configuring incomplete")
        {
            out.push(line.to_string());
            emitted_failure = true;
            continue;
        }
        if let Some(rest) = line.strip_prefix("-- ") {
            if CMAKE_PROBE_RE.is_match(line) {
                continue;
            }
            // Keep notable lines: Configuring done, Build files written, Build type, Install prefix, etc.
            if rest.starts_with("Configuring done")
                || rest.starts_with("Generating done")
                || rest.starts_with("Build files have been written")
                || rest.starts_with("Build type")
                || rest.starts_with("Install prefix")
                || rest.starts_with("Could NOT find")
                || rest.starts_with("The C compiler identification")
                || rest.starts_with("The CXX compiler identification")
                || rest.starts_with("Configuring incomplete")
            {
                out.push(line.to_string());
            }
            continue;
        }
    }

    if exit_code != 0 && !emitted_failure {
        return failure_fallback("cmake", exit_code, raw);
    }
    if out.is_empty() {
        return "cmake: configure ok".to_string();
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_build_success_summary() {
        let raw = "[ 10%] Building CXX object CMakeFiles/myapp.dir/main.cpp.o\n\
                   [ 50%] Building CXX object CMakeFiles/myapp.dir/util.cpp.o\n\
                   [100%] Linking CXX executable myapp\n\
                   [100%] Built target myapp\n";
        let args = vec!["--build".to_string(), "build".to_string()];
        let out = filter_build(raw, &args, 0);
        assert!(out.starts_with("cmake: ok"));
        assert!(out.contains("4 files"));
    }

    #[test]
    fn test_build_failure_keeps_diag() {
        let raw = "[ 50%] Building CXX object CMakeFiles/x.dir/main.cpp.o\n\
                   /tmp/main.cpp:3:5: error: 'foo' was not declared in this scope\n\
                       3 |     foo();\n\
                         |     ^~~\n\
                   make[2]: *** [CMakeFiles/x.dir/main.cpp.o] Error 1\n\
                   make[1]: *** [CMakeFiles/x.dir/all] Error 2\n";
        let args = vec!["--build".to_string(), "build".to_string()];
        let out = filter_build(raw, &args, 1);
        assert!(out.contains("error: 'foo'"));
        assert!(out.contains("make[2]: ***"));
        assert!(!out.contains("Building CXX"));
    }

    #[test]
    fn test_configure_strips_probes() {
        let raw = "-- The C compiler identification is GNU 13\n\
                   -- Detecting C compiler ABI info\n\
                   -- Detecting C compiler ABI info - done\n\
                   -- Check for working C compiler: /usr/bin/cc\n\
                   -- Looking for sys/types.h\n\
                   -- Looking for sys/types.h - found\n\
                   -- Configuring done\n\
                   -- Generating done\n\
                   -- Build files have been written to: /tmp/build\n";
        let out = filter_configure(raw, 0);
        assert!(out.contains("Configuring done"));
        assert!(out.contains("Build files have been written"));
        assert!(!out.contains("Detecting"));
        assert!(!out.contains("Looking for"));
    }

    #[test]
    fn test_configure_keeps_errors() {
        let raw = "-- Configuring incomplete, errors occurred!\n\
                   CMake Error at CMakeLists.txt:5 (find_package):\n\
                     Could not find FooBar.\n";
        let out = filter_configure(raw, 1);
        assert!(out.contains("CMake Error"));
        assert!(out.contains("Configuring incomplete"));
    }

    #[test]
    fn test_fixture_build_success() {
        let raw = include_str!("../../../tests/fixtures/cpp/cmake_build_success.txt");
        let args = vec!["--build".to_string(), "build".to_string()];
        let out = filter_build(raw, &args, 0);
        assert!(out.starts_with("cmake: ok"));
        let savings =
            100.0 - (count_tokens(&out) as f64 / count_tokens(raw) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60%, got {:.1}%", savings);
    }

    #[test]
    fn test_fixture_build_failure() {
        let raw = include_str!("../../../tests/fixtures/cpp/cmake_build_failure.txt");
        let args = vec!["--build".to_string(), "build".to_string()];
        let out = filter_build(raw, &args, 1);
        assert!(out.contains("error:"));
        assert!(out.contains("make[2]: ***"));
        assert!(!out.contains("Building CXX object"));
    }

    #[test]
    fn test_fixture_configure() {
        let raw = include_str!("../../../tests/fixtures/cpp/cmake_configure.txt");
        let out = filter_configure(raw, 0);
        assert!(out.contains("Configuring done"));
        assert!(!out.contains("Detecting"));
        assert!(!out.contains("Looking for"));
    }

    #[test]
    fn configure_keeps_missing_optional_dependency() {
        let raw = "-- Looking for ZLIB\n-- Could NOT find ZLIB (missing: ZLIB_LIBRARY)\n-- Configuring done\n-- Build files have been written to: build\n";
        let out = filter_configure(raw, 0);
        assert!(out.contains("Could NOT find ZLIB"));
        assert!(!out.contains("Looking for ZLIB"));
    }

    #[test]
    fn test_savings_build_success() {
        let raw = (0..50)
            .map(|i| format!("[{:>3}%] Building CXX object CMakeFiles/lib.dir/file{}.cpp.o", i * 2, i))
            .collect::<Vec<_>>()
            .join("\n");
        let args = vec!["--build".to_string(), "build".to_string()];
        let out = filter_build(&raw, &args, 0);
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(&raw) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60%, got {:.1}%", savings);
    }

    #[test]
    fn msvc_and_link_diagnostics_survive() {
        let raw = "C:\\src\\main.cpp(42,7): error C2065: name\nLINK : fatal error LNK1104: missing.lib\n";
        let out = filter_build(raw, &["--build".into(), "build".into()], 1);
        assert!(out.contains("C2065"));
        assert!(out.contains("LNK1104"));
        assert!(!out.contains("cmake: ok"));
    }

    #[test]
    fn unknown_or_empty_nonzero_build_is_failure() {
        let args = vec!["--build".into(), "build".into()];
        assert!(filter_build("unrecognized failure", &args, 2).contains("cmake: failed (exit 2)"));
        assert_eq!(filter_build("", &args, 2), "cmake: failed (exit 2)");
    }
}
