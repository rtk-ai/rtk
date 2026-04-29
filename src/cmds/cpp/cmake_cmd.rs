//! Filters cmake build/configure output — keep diagnostics, drop progress noise.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

lazy_static! {
    // GCC/Clang diagnostic: file.cpp:line:col: error|warning|note: message
    static ref GCC_DIAG_RE: Regex =
        Regex::new(r"^[^:\s].*:\d+:\d+:\s+(?:error|warning|note|fatal error):").unwrap();
    // make[N]: *** error
    static ref MAKE_ERR_RE: Regex = Regex::new(r"^make(\[\d+\])?:\s+\*\*\*").unwrap();
    // [ N%] Building CXX object ... or [ N%] Linking ...
    static ref CMAKE_PROGRESS_RE: Regex =
        Regex::new(r"^\[\s*\d+%\]\s+(Building|Linking|Built target|Generating|Built)").unwrap();
    // ninja-style progress: [N/M] Building ...
    static ref NINJA_PROGRESS_RE: Regex =
        Regex::new(r"^\[\d+/\d+\]\s+(Building|Linking|Generating)").unwrap();
    // CMake configure noise lines
    static ref CMAKE_PROBE_RE: Regex = Regex::new(
        r"^-- (Check for working|Detecting|Looking for|Found|Performing Test|Checking|Could NOT find)"
    )
    .unwrap();
}

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
    runner::run_filtered(
        cmd,
        "cmake",
        &args.join(" "),
        move |raw| {
            if is_build {
                filter_build(raw, &args_owned)
            } else {
                filter_configure(raw)
            }
        },
        RunOptions::with_tee("cmake"),
    )
}

fn filter_build(raw: &str, args: &[String]) -> String {
    let mut out = Vec::new();
    let mut diag_context = 0usize;
    let mut emitted_diag = false;
    let mut file_count = 0usize;

    for line in raw.lines() {
        if CMAKE_PROGRESS_RE.is_match(line) || NINJA_PROGRESS_RE.is_match(line) {
            file_count += 1;
            continue;
        }
        if line.contains("Entering directory") || line.contains("Leaving directory") {
            continue;
        }

        if GCC_DIAG_RE.is_match(line) {
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
        if line.contains("error:") || line.contains("undefined reference") {
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

fn filter_configure(raw: &str) -> String {
    let mut out = Vec::new();
    let mut had_error = false;

    for line in raw.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with("CMake Error") || trimmed.starts_with("CMake Warning") {
            out.push(line.to_string());
            had_error = trimmed.starts_with("CMake Error") || had_error;
            continue;
        }
        if line.starts_with("ERROR") || line.contains("error:") {
            out.push(line.to_string());
            had_error = true;
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
                || rest.starts_with("The C compiler identification")
                || rest.starts_with("The CXX compiler identification")
                || rest.starts_with("Configuring incomplete")
            {
                out.push(line.to_string());
            }
            continue;
        }
    }

    if out.is_empty() {
        let _ = had_error;
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
        let out = filter_build(raw, &args);
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
        let out = filter_build(raw, &args);
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
        let out = filter_configure(raw);
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
        let out = filter_configure(raw);
        assert!(out.contains("CMake Error"));
        assert!(out.contains("Configuring incomplete"));
    }

    #[test]
    fn test_fixture_build_success() {
        let raw = include_str!("../../../tests/fixtures/cpp/cmake_build_success.txt");
        let args = vec!["--build".to_string(), "build".to_string()];
        let out = filter_build(raw, &args);
        assert!(out.starts_with("cmake: ok"));
        let savings =
            100.0 - (count_tokens(&out) as f64 / count_tokens(raw) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60%, got {:.1}%", savings);
    }

    #[test]
    fn test_fixture_build_failure() {
        let raw = include_str!("../../../tests/fixtures/cpp/cmake_build_failure.txt");
        let args = vec!["--build".to_string(), "build".to_string()];
        let out = filter_build(raw, &args);
        assert!(out.contains("error:"));
        assert!(out.contains("make[2]: ***"));
        assert!(!out.contains("Building CXX object"));
    }

    #[test]
    fn test_fixture_configure() {
        let raw = include_str!("../../../tests/fixtures/cpp/cmake_configure.txt");
        let out = filter_configure(raw);
        assert!(out.contains("Configuring done"));
        assert!(!out.contains("Detecting"));
        assert!(!out.contains("Looking for"));
    }

    #[test]
    fn test_savings_build_success() {
        let raw = (0..50)
            .map(|i| format!("[{:>3}%] Building CXX object CMakeFiles/lib.dir/file{}.cpp.o", i * 2, i))
            .collect::<Vec<_>>()
            .join("\n");
        let args = vec!["--build".to_string(), "build".to_string()];
        let out = filter_build(&raw, &args);
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(&raw) as f64 * 100.0);
        assert!(savings >= 60.0, "expected >=60%, got {:.1}%", savings);
    }
}
