//! Helpers shared by the Perl tool filters.

use regex::Regex;
use std::sync::LazyLock;

use crate::core::truncate::CAP_LIST;

/// Longest list of passing test files printed by name; past it only the count is printed.
const MAX_PASSED_NAMES: usize = CAP_LIST;

/// `not ok 2 - name` (TAP) or `not ok - name` (yath's `[  FAIL  ]` line).
static NOT_OK_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*not ok(?: \d+)?(?: - (.*))?$").unwrap());
/// `#   Failed test 'name'`, Test::More and Test2; yath prints it without the `#`.
static FAILED_TEST_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)(?:#\s+)?Failed test '(.*)'$").unwrap());
/// `#   at t/x.t line 10.`, the line after a named `Failed test`.
static AT_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\s*(?:#\s+)?at (.+ line \d+)\.$").unwrap());
/// `#   Failed test at t/x.t line 10.`, an unnamed test.
static FAILED_UNNAMED_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^(\s*)(?:#\s+)?Failed test at (.+ line \d+)\.$").unwrap());

/// One line naming the test files that passed, so a reader can tell "passed" from "not run".
/// A long list becomes a count: a passing file needs no further reading, and the count is
/// not a truncated list, so there is nothing to recall.
pub fn passed_files_line(files: &[String]) -> Option<String> {
    match files.len() {
        0 => None,
        n if n > MAX_PASSED_NAMES => Some(format!("Passed: {} files", n)),
        _ => Some(format!("Passed: {}", files.join(", "))),
    }
}

/// Fold a failure's `Failed test 'name'` / `at FILE line N.` diagnostic into one location.
///
/// When the `not ok` line for the same test is just above, the location joins it:
/// `not ok 2 - name (t/x.t line 10)`. Otherwise (quiet runs print no `not ok` lines) the
/// diagnostic becomes `# Failed test 'name' (t/x.t line 10)`. Both forms say the same as the
/// two or three lines they replace.
pub fn merge_failure_locations(lines: Vec<String>) -> Vec<String> {
    let mut out: Vec<String> = Vec::with_capacity(lines.len());
    let mut iter = lines.into_iter().peekable();
    // Whether the last line in `out` is a `not ok` that already took a location.
    let mut last_located = false;
    while let Some(line) = iter.next() {
        let (indent, name, location) = if let Some(caps) = FAILED_UNNAMED_RE.captures(&line) {
            (caps[1].to_string(), None, caps[2].to_string())
        } else if let Some(caps) = FAILED_TEST_RE.captures(&line) {
            let Some(location) = iter
                .peek()
                .and_then(|next| AT_RE.captures(next))
                .map(|c| c[1].to_string())
            else {
                out.push(line);
                last_located = false;
                continue;
            };
            iter.next();
            (caps[1].to_string(), Some(caps[2].to_string()), location)
        } else {
            out.push(line);
            last_located = false;
            continue;
        };

        let joins_not_ok = !last_located
            && out
                .last()
                .and_then(|prev| NOT_OK_RE.captures(prev))
                .is_some_and(|c| c.get(1).map(|m| m.as_str()) == name.as_deref());
        if joins_not_ok {
            if let Some(prev) = out.last_mut() {
                prev.push_str(&format!(" ({})", location));
            }
            last_located = true;
            continue;
        }
        last_located = false;
        match name {
            Some(name) => out.push(format!("{}# Failed test '{}' ({})", indent, name, location)),
            None => out.push(format!("{}# Failed test ({})", indent, location)),
        }
    }
    out
}

/// `line` with the working directory's path removed, so an error in `lib/Foo.pm` reads as
/// that rather than as an absolute path the reader has to strip in their head.
pub fn relative_to(line: &str, cwd: Option<&str>) -> String {
    match cwd {
        Some(dir) if !dir.is_empty() && dir != "/" => line.replace(&format!("{}/", dir), ""),
        _ => line.to_string(),
    }
}

/// The working directory as a string, for [`relative_to`].
pub fn current_dir() -> Option<String> {
    std::env::current_dir()
        .ok()
        .map(|d| d.to_string_lossy().into_owned())
}

/// The "filter" for invocations rtk does not compact (help, listings, user-chosen formats).
/// The runner ends the output with its own newline, so the tool's is dropped.
pub fn unfiltered(raw: &str) -> String {
    raw.trim_end_matches('\n').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(text: &str) -> Vec<String> {
        text.lines().map(str::to_string).collect()
    }

    #[test]
    fn test_passed_files_line() {
        assert_eq!(passed_files_line(&[]), None);
        let two = vec!["t/a.t".to_string(), "t/b.t".to_string()];
        assert_eq!(
            passed_files_line(&two).as_deref(),
            Some("Passed: t/a.t, t/b.t")
        );
        let many: Vec<String> = (0..=MAX_PASSED_NAMES).map(|i| format!("t/{i}.t")).collect();
        assert_eq!(
            passed_files_line(&many),
            Some(format!("Passed: {} files", MAX_PASSED_NAMES + 1))
        );
    }

    #[test]
    fn test_merge_joins_not_ok() {
        let input = lines(
            "not ok 2 - two plus two\n#   Failed test 'two plus two'\n#   at t/a.t line 10.\n#          got: '4'",
        );
        assert_eq!(
            merge_failure_locations(input),
            lines("not ok 2 - two plus two (t/a.t line 10)\n#          got: '4'")
        );
    }

    #[test]
    fn test_merge_without_not_ok() {
        let input =
            lines("#   Failed test 'deep'\n#   at t/a.t line 11.\n# Failed test at t/b.t line 3.");
        assert_eq!(
            merge_failure_locations(input),
            lines("# Failed test 'deep' (t/a.t line 11)\n# Failed test (t/b.t line 3)")
        );
    }

    #[test]
    fn test_merge_yath_form_and_unnamed_not_ok() {
        let input = lines(
            "not ok - big\n  Failed test 'big'\n  at t/a.t line 16.\nnot ok 3\n#   Failed test at t/a.t line 20.",
        );
        assert_eq!(
            merge_failure_locations(input),
            lines("not ok - big (t/a.t line 16)\nnot ok 3 (t/a.t line 20)")
        );
    }

    #[test]
    fn test_merge_name_ending_in_paren() {
        let input =
            lines("not ok 1 - add(1, 2)\n#   Failed test 'add(1, 2)'\n#   at t/a.t line 3.");
        assert_eq!(
            merge_failure_locations(input),
            lines("not ok 1 - add(1, 2) (t/a.t line 3)")
        );
    }

    #[test]
    fn test_merge_leaves_mismatched_names() {
        let input = lines("not ok 1 - one\n#   Failed test 'two'\n#   at t/a.t line 3.");
        assert_eq!(
            merge_failure_locations(input),
            lines("not ok 1 - one\n# Failed test 'two' (t/a.t line 3)")
        );
    }

    #[test]
    fn test_relative_to() {
        assert_eq!(
            relative_to("died at /home/u/p/lib/A.pm line 3.", Some("/home/u/p")),
            "died at lib/A.pm line 3."
        );
        assert_eq!(relative_to("at /x/y line 1", Some("/")), "at /x/y line 1");
        assert_eq!(relative_to("at /x/y line 1", None), "at /x/y line 1");
    }

    #[test]
    fn test_unfiltered_keeps_content() {
        assert_eq!(unfiltered("a\n  b  \n\n"), "a\n  b  ");
        assert_eq!(unfiltered(""), "");
    }
}
