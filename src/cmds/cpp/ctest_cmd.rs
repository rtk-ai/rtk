//! Filters ctest output — keep GoogleTest failures and final summaries.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("ctest");
    for a in args {
        cmd.arg(a);
    }
    if verbose > 0 {
        eprintln!("Running: ctest {}", args.join(" "));
    }
    runner::run_filtered(
        cmd,
        "ctest",
        &args.join(" "),
        filter_ctest_output,
        RunOptions::with_tee("ctest"),
    )
}

pub(crate) fn filter_ctest_output(output: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let mut current_test: Option<String> = None;
    let mut emitting_failure_block = false;
    let mut emitted_test_header_for_block = false;

    for line in output.lines() {
        let trimmed = line.trim_end();
        let t = trimmed.trim();
        let m = normalize_ctest_verbose_prefix(t);

        if t.is_empty() {
            if emitting_failure_block {
                out.push(String::new());
            }
            continue;
        }

        if let Some(name) = parse_gtest_run(m) {
            current_test = Some(name);
            emitting_failure_block = false;
            emitted_test_header_for_block = false;
            continue;
        }

        if is_gtest_ok(m) {
            emitting_failure_block = false;
            emitted_test_header_for_block = false;
            current_test = None;
            continue;
        }

        if is_gtest_failure_header(m) {
            if !emitted_test_header_for_block {
                if let Some(name) = current_test.as_deref() {
                    out.push(name.to_string());
                }
                emitted_test_header_for_block = true;
            }
            emitting_failure_block = true;
            out.push(trimmed.to_string());
            continue;
        }

        if is_gtest_failed_summary_line(m) {
            emitting_failure_block = false;
            emitted_test_header_for_block = false;
            out.push(trimmed.to_string());
            continue;
        }

        if is_ctest_summary_line(m) {
            emitting_failure_block = false;
            emitted_test_header_for_block = false;
            out.push(trimmed.to_string());
            continue;
        }

        if is_important_non_gtest_line(m) {
            out.push(trimmed.to_string());
            continue;
        }

        if emitting_failure_block {
            if is_gtest_failed_test_line(m) {
                out.push(trimmed.to_string());
                emitting_failure_block = false;
                emitted_test_header_for_block = false;
                current_test = None;
                continue;
            }

            if is_noisy_separator(m) {
                continue;
            }

            out.push(trimmed.to_string());
        }
    }

    compact_lines(out).join("\n")
}

fn normalize_ctest_verbose_prefix(line: &str) -> &str {
    let s = line.trim_start();
    let mut i = 0;
    for (idx, ch) in s.char_indices() {
        if ch.is_ascii_digit() {
            i = idx + ch.len_utf8();
            continue;
        }
        i = idx;
        break;
    }
    if i == 0 {
        return line;
    }
    let rest = &s[i..];
    if !rest.starts_with(':') {
        return line;
    }
    let rest = &rest[1..];
    let rest = rest.strip_prefix(' ').unwrap_or(rest);
    if rest.is_empty() {
        line
    } else {
        rest
    }
}

fn parse_gtest_run(line: &str) -> Option<String> {
    if !line.starts_with("[ RUN") {
        return None;
    }
    let idx = line.find("]")?;
    let rest = line[idx + 1..].trim();
    if rest.is_empty() {
        None
    } else {
        Some(rest.to_string())
    }
}

fn is_gtest_ok(line: &str) -> bool {
    line.starts_with("[       OK ]")
}

fn is_gtest_failure_header(line: &str) -> bool {
    line.ends_with(": Failure")
}

fn is_gtest_failed_test_line(line: &str) -> bool {
    line.starts_with("[  FAILED  ]") && line.contains('(')
}

fn is_gtest_failed_summary_line(line: &str) -> bool {
    if line.starts_with("[  FAILED  ]") {
        return true;
    }
    if line.starts_with("[  PASSED  ]") && line.contains("tests") {
        return true;
    }
    false
}

fn is_ctest_summary_line(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("tests failed")
        || l.contains("% tests passed")
        || l.contains("the following tests failed")
        || l.contains("errors while running ctest")
        || l.contains("ctest error")
}

fn is_important_non_gtest_line(line: &str) -> bool {
    let l = line.to_lowercase();
    l.contains("addresssanitizer")
        || l.contains("runtime error")
        || l.contains("undefined behavior")
        || l.contains("segmentation fault")
        || l.contains("abort")
        || l.starts_with("error:")
        || l.contains("fatal error")
        || l.contains("cmake error")
        || l.contains("clang: error")
        || l.contains("gcc: error")
        || l.contains("ld: error")
}

fn is_noisy_separator(line: &str) -> bool {
    line.chars().all(|c| c == '=' || c == '-' || c == '─' || c == '═')
}

fn compact_lines(mut lines: Vec<String>) -> Vec<String> {
    while lines.first().is_some_and(|l| l.trim().is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|l| l.trim().is_empty()) {
        lines.pop();
    }

    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut last_blank = false;
    for l in lines.drain(..) {
        let blank = l.trim().is_empty();
        if blank {
            if last_blank {
                continue;
            }
            last_blank = true;
            out.push(String::new());
        } else {
            last_blank = false;
            out.push(l);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gtest_all_passed_compacts() {
        let input = r#"
[==========] Running 2 tests from 1 test suite.
[ RUN      ] Foo.Pass
[       OK ] Foo.Pass (1 ms)
[ RUN      ] Foo.Pass2
[       OK ] Foo.Pass2 (1 ms)
[==========] 2 tests from 1 test suite ran. (2 ms total)
[  PASSED  ] 2 tests.
"#;
        let out = filter_ctest_output(input);
        assert!(!out.contains("Foo.Pass"));
        assert!(!out.contains("[ RUN"));
        assert!(out.contains("[  PASSED  ] 2 tests."));
    }

    #[test]
    fn gtest_one_failed_keeps_block_and_summary() {
        let input = r#"
[==========] Running 3 tests from 1 test suite.
[ RUN      ] Foo.Pass
[       OK ] Foo.Pass (1 ms)
[ RUN      ] Foo.Fail
/path/foo_test.cc:42: Failure
Expected equality of these values:
  actual
  expected
[  FAILED  ] Foo.Fail (0 ms)
[ RUN      ] Foo.Pass2
[       OK ] Foo.Pass2 (1 ms)
[==========] 3 tests from 1 test suite ran.
[  PASSED  ] 2 tests.
[  FAILED  ] 1 test, listed below:
[  FAILED  ] Foo.Fail
"#;
        let out = filter_ctest_output(input);
        assert!(out.contains("Foo.Fail"));
        assert!(out.contains("/path/foo_test.cc:42: Failure"));
        assert!(out.contains("Expected equality of these values:"));
        assert!(out.contains("[  FAILED  ] 1 test, listed below:"));
        assert!(out.contains("[  FAILED  ] Foo.Fail"));
        assert!(!out.contains("Foo.Pass"));
        assert!(!out.contains("Foo.Pass2"));
    }

    #[test]
    fn gtest_multiple_failed_keeps_both_blocks() {
        let input = r#"
[ RUN      ] A.Fail
/p/a.cc:1: Failure
boom
[  FAILED  ] A.Fail (0 ms)
[ RUN      ] B.Fail
/p/b.cc:2: Failure
kaboom
[  FAILED  ] B.Fail (0 ms)
[  FAILED  ] 2 tests, listed below:
[  FAILED  ] A.Fail
[  FAILED  ] B.Fail
"#;
        let out = filter_ctest_output(input);
        assert!(out.contains("A.Fail"));
        assert!(out.contains("/p/a.cc:1: Failure"));
        assert!(out.contains("B.Fail"));
        assert!(out.contains("/p/b.cc:2: Failure"));
        assert!(out.contains("[  FAILED  ] A.Fail"));
        assert!(out.contains("[  FAILED  ] B.Fail"));
    }

    #[test]
    fn gtest_parameterized_names_survive() {
        let input = r#"
[ RUN      ] FooTest/0.DoesThing
/p/t.cc:3: Failure
nope
[  FAILED  ] FooTest/0.DoesThing (0 ms)
[ RUN      ] FooSuite/BarTest.DoesThing/1
/p/u.cc:4: Failure
nope2
[  FAILED  ] FooSuite/BarTest.DoesThing/1 (0 ms)
[  FAILED  ] 2 tests, listed below:
[  FAILED  ] FooTest/0.DoesThing
[  FAILED  ] FooSuite/BarTest.DoesThing/1
"#;
        let out = filter_ctest_output(input);
        assert!(out.contains("FooTest/0.DoesThing"));
        assert!(out.contains("FooSuite/BarTest.DoesThing/1"));
    }

    #[test]
    fn keeps_non_gtest_important_lines() {
        let input = r#"
[ RUN      ] Foo.Fail
/p/t.cc:3: Failure
nope
AddressSanitizer: heap-use-after-free
[  FAILED  ] Foo.Fail (0 ms)
"#;
        let out = filter_ctest_output(input);
        assert!(out.contains("AddressSanitizer: heap-use-after-free"));
        assert!(out.contains("/p/t.cc:3: Failure"));
    }

    #[test]
    fn ctest_verbose_prefix_gtest_failure_survives_and_pass_noise_drops() {
        let input = r#"
1: [ RUN      ] Foo.Pass
1: [       OK ] Foo.Pass (1 ms)
1: [ RUN      ] Foo.Fail
1: /path/foo_test.cc:42: Failure
1: Expected equality of these values:
1:   actual
1:   expected
1: [  FAILED  ] Foo.Fail (0 ms)
1: [ RUN      ] Foo.Pass2
1: [       OK ] Foo.Pass2 (1 ms)
1: [  FAILED  ] 1 test, listed below:
1: [  FAILED  ] Foo.Fail
"#;
        let out = filter_ctest_output(input);
        assert!(out.contains("Foo.Fail"));
        assert!(out.contains("1: /path/foo_test.cc:42: Failure"));
        assert!(out.contains("1: Expected equality of these values:"));
        assert!(out.contains("1: [  FAILED  ] Foo.Fail"));
        assert!(!out.contains("Foo.Pass"));
        assert!(!out.contains("Foo.Pass2"));
    }

    #[test]
    fn ctest_verbose_prefix_normalizer_requires_colon() {
        assert_eq!(normalize_ctest_verbose_prefix("123 tests failed"), "123 tests failed");
        assert_eq!(normalize_ctest_verbose_prefix("404 error happened"), "404 error happened");
    }
}
