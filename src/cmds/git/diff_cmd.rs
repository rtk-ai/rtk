//! Compares two files and shows only the changed lines.

use crate::core::guard::never_worse;
use crate::core::tracking;
use crate::core::utils::read_path_operands;
use anyhow::Result;
use std::path::Path;

const IDENTICAL_FILES_MESSAGE: &str = "[ok] Files are identical\n";
const WHITESPACE_ONLY_DIFF_DETAIL: &str =
    "   files differ only in whitespace or line endings (no line-content change)\n";

/// Ultra-condensed diff - only changed lines, no context.
/// Returns the diff-convention exit code: 0 if identical, 1 if files differ.
pub fn run(file1: &Path, file2: &Path, verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Comparing: {} vs {}", file1.display(), file2.display());
    }

    let (content1, content2) = read_path_operands(file1, file2)?;
    let lines1: Vec<&str> = content1.lines().collect();
    let lines2: Vec<&str> = content2.lines().collect();
    let diff = compute_diff(&lines1, &lines2);
    let fallback = format_classic_diff(&diff);
    let both_files = format!("{}\n---\n{}", content1, content2);

    let (rtk, exit_code) = render_diff(file1, file2, &diff, content1 == content2);

    let shown = select_file_diff_output(&diff, &fallback, &rtk);
    print!("{}", shown);
    timer.track(
        &format!("diff {} {}", file1.display(), file2.display()),
        "rtk diff",
        tracking_baseline(&diff, &fallback, &both_files, shown),
        shown,
    );
    Ok(exit_code)
}

fn render_file_header(file1: &Path, file2: &Path) -> String {
    format!("{} → {}\n", file1.display(), file2.display())
}

fn render_diff(file1: &Path, file2: &Path, diff: &DiffResult, bytes_equal: bool) -> (String, i32) {
    if diff.changes.is_empty() {
        if bytes_equal {
            return (IDENTICAL_FILES_MESSAGE.to_string(), 0);
        }
        // `str::lines()` strips `\r` and drops a trailing newline, so these
        // byte-level differences can leave no line changes to render.
        return (
            format!(
                "{}{}",
                render_file_header(file1, file2),
                WHITESPACE_ONLY_DIFF_DETAIL
            ),
            1,
        );
    }

    let mut rtk = String::new();
    rtk.push_str(&render_file_header(file1, file2));
    rtk.push_str(&format!(
        "   +{} added, -{} removed, ~{} modified\n\n",
        diff.added, diff.removed, diff.modified
    ));
    rtk.push_str(&format_diff_changes(diff));
    (rtk, 1)
}

/// Run diff from stdin (piped command output)
pub fn run_stdin(_verbose: u8) -> Result<()> {
    use std::io::{self, Read};
    let timer = tracking::TimedExecution::start();

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    // Parse unified diff format
    let condensed = condense_unified_diff(&input);
    let shown = never_worse(&input, &condensed);
    println!("{}", shown);

    timer.track("diff (stdin)", "rtk diff (stdin)", &input, shown);

    Ok(())
}

#[derive(Debug)]
enum DiffChange {
    Added(usize, String),
    Removed(usize, String),
    Modified(usize, String, String),
}

struct DiffResult {
    added: usize,
    removed: usize,
    modified: usize,
    changes: Vec<DiffChange>,
}

fn format_diff_changes(diff: &DiffResult) -> String {
    let mut out = String::new();
    for change in &diff.changes {
        match change {
            DiffChange::Added(ln, c) => out.push_str(&format!("+{:4} {}\n", ln, c)),
            DiffChange::Removed(ln, c) => out.push_str(&format!("-{:4} {}\n", ln, c)),
            DiffChange::Modified(ln, old, new) => {
                out.push_str(&format!("~{:4} {} → {}\n", ln, old, new))
            }
        }
    }
    out
}

fn format_classic_diff(diff: &DiffResult) -> String {
    let mut out = String::new();
    let mut index = 0;

    while index < diff.changes.len() {
        match &diff.changes[index] {
            DiffChange::Modified(start, _, _) => {
                let start = *start;
                let mut end = start;
                let mut old_lines = Vec::new();
                let mut new_lines = Vec::new();

                while let Some(DiffChange::Modified(line, old, new)) = diff.changes.get(index) {
                    if *line != end {
                        break;
                    }
                    old_lines.push(old);
                    new_lines.push(new);
                    end += 1;
                    index += 1;
                }

                out.push_str(&format!(
                    "{}c{}\n",
                    format_line_range(start, end - 1),
                    format_line_range(start, end - 1)
                ));
                for line in old_lines {
                    out.push_str(&format!("< {}\n", line));
                }
                out.push_str("---\n");
                for line in new_lines {
                    out.push_str(&format!("> {}\n", line));
                }
            }
            DiffChange::Removed(start, _) if matches!(
                diff.changes.get(index + 1),
                Some(DiffChange::Added(line, _)) if line == start
            ) => {
                let start = *start;
                let mut end = start;
                let mut old_lines = Vec::new();
                let mut new_lines = Vec::new();

                while let (
                    Some(DiffChange::Removed(old_line, old)),
                    Some(DiffChange::Added(new_line, new)),
                ) = (diff.changes.get(index), diff.changes.get(index + 1))
                {
                    if *old_line != end || *new_line != end {
                        break;
                    }
                    old_lines.push(old);
                    new_lines.push(new);
                    end += 1;
                    index += 2;
                }

                out.push_str(&format!(
                    "{}c{}\n",
                    format_line_range(start, end - 1),
                    format_line_range(start, end - 1)
                ));
                for line in old_lines {
                    out.push_str(&format!("< {}\n", line));
                }
                out.push_str("---\n");
                for line in new_lines {
                    out.push_str(&format!("> {}\n", line));
                }
            }
            DiffChange::Added(start, _) => {
                let start = *start;
                let mut end = start;
                let mut new_lines = Vec::new();

                while let Some(DiffChange::Added(line, new)) = diff.changes.get(index) {
                    if *line != end {
                        break;
                    }
                    new_lines.push(new);
                    end += 1;
                    index += 1;
                }

                out.push_str(&format!(
                    "{}a{}\n",
                    start - 1,
                    format_line_range(start, end - 1)
                ));
                for line in new_lines {
                    out.push_str(&format!("> {}\n", line));
                }
            }
            DiffChange::Removed(start, _) => {
                let start = *start;
                let mut end = start;
                let mut old_lines = Vec::new();

                while let Some(DiffChange::Removed(line, old)) = diff.changes.get(index) {
                    if *line != end {
                        break;
                    }
                    old_lines.push(old);
                    end += 1;
                    index += 1;
                }

                out.push_str(&format!(
                    "{}d{}\n",
                    format_line_range(start, end - 1),
                    start - 1
                ));
                for line in old_lines {
                    out.push_str(&format!("< {}\n", line));
                }
            }
        }
    }
    out
}

fn format_line_range(start: usize, end: usize) -> String {
    if start == end {
        start.to_string()
    } else {
        format!("{start},{end}")
    }
}

/// Baseline the savings are measured against: what `diff` itself would have
/// printed, so the recorded ratio compares like with like and can never go
/// negative -- the guard already caps the shown output at the fallback.
fn tracking_baseline<'a>(
    diff: &DiffResult,
    fallback: &'a str,
    both_files: &'a str,
    shown: &'a str,
) -> &'a str {
    if !diff.changes.is_empty() {
        return fallback;
    }

    // Identical files: `diff` prints nothing, so the dump of both files
    // stands in as the output that would otherwise have to be read. Two
    // near-empty files can make that dump cheaper than the verdict line,
    // which would book a loss against the cheapest possible answer.
    if tracking::estimate_tokens(both_files) >= tracking::estimate_tokens(shown) {
        both_files
    } else {
        shown
    }
}

fn select_file_diff_output<'a>(diff: &DiffResult, raw: &'a str, rendered: &'a str) -> &'a str {
    if diff.changes.is_empty() {
        rendered
    } else {
        never_worse(raw, rendered)
    }
}

fn compute_diff(lines1: &[&str], lines2: &[&str]) -> DiffResult {
    let mut changes = Vec::new();
    let mut added = 0;
    let mut removed = 0;
    let mut modified = 0;

    // Simple line-by-line comparison (not optimal but fast)
    let max_len = lines1.len().max(lines2.len());

    for i in 0..max_len {
        let l1 = lines1.get(i).copied();
        let l2 = lines2.get(i).copied();

        match (l1, l2) {
            (Some(a), Some(b)) if a != b => {
                // Check if it's similar (modification) or completely different
                if similarity(a, b) > 0.5 {
                    changes.push(DiffChange::Modified(i + 1, a.to_string(), b.to_string()));
                    modified += 1;
                } else {
                    changes.push(DiffChange::Removed(i + 1, a.to_string()));
                    changes.push(DiffChange::Added(i + 1, b.to_string()));
                    removed += 1;
                    added += 1;
                }
            }
            (Some(a), None) => {
                changes.push(DiffChange::Removed(i + 1, a.to_string()));
                removed += 1;
            }
            (None, Some(b)) => {
                changes.push(DiffChange::Added(i + 1, b.to_string()));
                added += 1;
            }
            _ => {}
        }
    }

    DiffResult {
        added,
        removed,
        modified,
        changes,
    }
}

fn similarity(a: &str, b: &str) -> f64 {
    let a_chars: std::collections::HashSet<char> = a.chars().collect();
    let b_chars: std::collections::HashSet<char> = b.chars().collect();

    let intersection = a_chars.intersection(&b_chars).count();
    let union = a_chars.union(&b_chars).count();

    if union == 0 {
        1.0
    } else {
        intersection as f64 / union as f64
    }
}

fn condense_unified_diff(diff: &str) -> String {
    let mut result = Vec::new();
    let mut current_file = String::new();
    let mut added = 0;
    let mut removed = 0;
    let mut changes = Vec::new();

    // Never truncate diff content — users make decisions based on this data.
    // Only strip diff metadata (headers, @@ hunks); all +/- lines shown in full.
    for line in diff.lines() {
        if line.starts_with("diff --git") || line.starts_with("--- ") || line.starts_with("+++ ") {
            if line.starts_with("+++ ") {
                if !current_file.is_empty() && (added > 0 || removed > 0) {
                    result.push(format!("[file] {} (+{} -{})", current_file, added, removed));
                    // Column 0: anchored greps (`^[+-]`) must match these.
                    result.append(&mut changes);
                }
                current_file = line
                    .trim_start_matches("+++ ")
                    .trim_start_matches("b/")
                    .to_string();
                added = 0;
                removed = 0;
                changes.clear();
            }
        } else if line.starts_with('+') && !line.starts_with("+++") {
            added += 1;
            changes.push(line.to_string());
        } else if line.starts_with('-') && !line.starts_with("---") {
            removed += 1;
            changes.push(line.to_string());
        }
    }

    // Last file
    if !current_file.is_empty() && (added > 0 || removed > 0) {
        result.push(format!("[file] {} (+{} -{})", current_file, added, removed));
        // Column 0: anchored greps (`^[+-]`) must match these.
        result.append(&mut changes);
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn render_test_diff(file1: &str, file2: &str, content1: &str, content2: &str) -> (String, i32) {
        let lines1: Vec<&str> = content1.lines().collect();
        let lines2: Vec<&str> = content2.lines().collect();
        let diff = compute_diff(&lines1, &lines2);
        render_diff(
            Path::new(file1),
            Path::new(file2),
            &diff,
            content1 == content2,
        )
    }

    // --- similarity ---

    #[test]
    fn test_similarity_identical() {
        assert_eq!(similarity("hello", "hello"), 1.0);
    }

    #[test]
    fn test_similarity_completely_different() {
        assert_eq!(similarity("abc", "xyz"), 0.0);
    }

    #[test]
    fn test_similarity_empty_strings() {
        // Both empty: union is 0, returns 1.0 by convention
        assert_eq!(similarity("", ""), 1.0);
    }

    #[test]
    fn test_similarity_partial_overlap() {
        let s = similarity("abcd", "abef");
        // Shared: a, b. Union: a, b, c, d, e, f = 6. Jaccard = 2/6
        assert!((s - 2.0 / 6.0).abs() < f64::EPSILON);
    }

    #[test]
    fn test_similarity_threshold_for_modified() {
        // "let x = 1;" vs "let x = 2;" should be > 0.5 (treated as modification)
        assert!(similarity("let x = 1;", "let x = 2;") > 0.5);
    }

    // --- compute_diff ---

    #[test]
    fn test_compute_diff_identical() {
        let a = vec!["line1", "line2", "line3"];
        let b = vec!["line1", "line2", "line3"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.added, 0);
        assert_eq!(result.removed, 0);
        assert_eq!(result.modified, 0);
        assert!(result.changes.is_empty());
    }

    #[test]
    fn test_compute_diff_added_lines() {
        let a = vec!["line1"];
        let b = vec!["line1", "line2", "line3"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.added, 2);
        assert_eq!(result.removed, 0);
    }

    #[test]
    fn test_compute_diff_removed_lines() {
        let a = vec!["line1", "line2", "line3"];
        let b = vec!["line1"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.removed, 2);
        assert_eq!(result.added, 0);
    }

    #[test]
    fn test_compute_diff_modified_line() {
        // Similar lines (>0.5 similarity) are classified as modified
        let a = vec!["let x = 1;"];
        let b = vec!["let x = 2;"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.modified, 1);
        assert_eq!(result.added, 0);
        assert_eq!(result.removed, 0);
    }

    #[test]
    fn test_compute_diff_completely_different_line() {
        // Dissimilar lines (<= 0.5 similarity) are added+removed, not modified
        let a = vec!["aaaa"];
        let b = vec!["zzzz"];
        let result = compute_diff(&a, &b);
        assert_eq!(result.modified, 0);
        assert_eq!(result.added, 1);
        assert_eq!(result.removed, 1);
    }

    #[test]
    fn test_compute_diff_empty_inputs() {
        let result = compute_diff(&[], &[]);
        assert_eq!(result.added, 0);
        assert_eq!(result.removed, 0);
        assert!(result.changes.is_empty());
    }

    // --- render_diff (issue #2364 regression) ---

    #[test]
    fn test_render_modified_only_yaml_not_identical() {
        // "a: 1" vs "a: 2" is classified as modified (similarity > 0.5);
        // the identical check must not ignore modified-only diffs.
        let (out, code) = render_test_diff("one.yaml", "two.yaml", "a: 1\n", "a: 2\n");
        assert!(
            !out.contains("identical"),
            "modified-only diff reported as identical:\n{}",
            out
        );
        assert!(out.contains("~1 modified"));
        assert!(out.contains("a: 1"));
        assert!(out.contains("a: 2"));
        assert_eq!(code, 1, "differing files must exit 1 (diff convention)");
    }

    #[test]
    fn test_render_modified_only_json_not_identical() {
        let (out, code) = render_test_diff("j1.json", "j2.json", "{\"a\": 1}\n", "{\"a\": 2}\n");
        assert!(
            !out.contains("identical"),
            "modified-only diff reported as identical:\n{}",
            out
        );
        assert_eq!(code, 1);
    }

    #[test]
    fn test_render_identical_files_exit_zero() {
        let (out, code) =
            render_test_diff("a.yaml", "b.yaml", "a: 1\nb: 2\n", "a: 1\nb: 2\n");
        assert!(out.contains("[ok] Files are identical"));
        assert_eq!(code, 0);
    }

    #[test]
    fn test_render_added_removed_exit_one() {
        let (out, code) = render_test_diff("t1.txt", "t2.txt", "x\n", "y\n");
        assert!(out.contains("+1 added, -1 removed"));
        assert_eq!(code, 1);
    }

    // --- byte-different but line-equal files must not be "identical" (issue #3469) ---

    #[test]
    fn test_render_crlf_vs_lf_not_identical() {
        let (out, code) = render_test_diff(
            "a.txt",
            "b.txt",
            "alpha\nbeta\n",
            "alpha\r\nbeta\r\n",
        );
        assert!(
            !out.contains("identical"),
            "CRLF-vs-LF difference reported as identical:\n{}",
            out
        );
        assert!(
            out.contains("whitespace or line endings"),
            "expected the whitespace/line-ending message, got:\n{}",
            out
        );
        assert_eq!(code, 1, "byte-different files must exit 1 (diff convention)");
    }

    #[test]
    fn test_render_trailing_newline_not_identical() {
        let (out, code) = render_test_diff("a.txt", "b.txt", "abc", "abc\n");
        assert!(
            !out.contains("identical"),
            "trailing-newline difference reported as identical:\n{}",
            out
        );
        assert_eq!(code, 1);
    }

    #[test]
    fn test_render_byte_identical_exit_zero_with_crlf() {
        let (out, code) = render_test_diff("a.txt", "b.txt", "a\r\nb\r\n", "a\r\nb\r\n");
        assert!(out.contains("[ok] Files are identical"));
        assert_eq!(code, 0);
    }

    #[test]
    fn test_never_worse_fallback_is_a_classic_diff() {
        let diff = compute_diff(&["alpha beta"], &["alpha zzzz"]);
        let fallback = format_classic_diff(&diff);
        let (rendered, code) =
            render_diff(Path::new("before"), Path::new("after"), &diff, false);
        let shown = select_file_diff_output(&diff, &fallback, &rendered);

        assert_eq!(code, 1);
        assert!(shown.contains("1c1"));
        assert!(shown.contains("< alpha beta"));
        assert!(shown.contains("\n---\n"));
        assert!(shown.contains("> alpha zzzz"));
    }

    #[test]
    fn test_tracking_baseline_never_books_a_loss() {
        // Two unrelated files: the classic diff carries both of them plus the
        // "< " / "> " markers, so it is bigger than a plain dump. Measuring
        // against the dump used to record negative savings.
        let old: Vec<String> = (0..40).map(|i| format!("old line {i}")).collect();
        let new: Vec<String> = (0..40).map(|i| format!("brand new content {i}")).collect();
        let r1: Vec<&str> = old.iter().map(|s| s.as_str()).collect();
        let r2: Vec<&str> = new.iter().map(|s| s.as_str()).collect();

        let diff = compute_diff(&r1, &r2);
        let fallback = format_classic_diff(&diff);
        let old_content = old.join("\n");
        let new_content = new.join("\n");
        let both_files = format!("{}\n---\n{}", old_content, new_content);
        let (rendered, _) = render_diff(
            Path::new("a"),
            Path::new("b"),
            &diff,
            old_content == new_content,
        );
        let shown = select_file_diff_output(&diff, &fallback, &rendered);
        let baseline = tracking_baseline(&diff, &fallback, &both_files, shown);

        assert!(
            tracking::estimate_tokens(baseline) >= tracking::estimate_tokens(shown),
            "baseline {} < shown {} would record negative savings",
            tracking::estimate_tokens(baseline),
            tracking::estimate_tokens(shown)
        );
    }

    #[test]
    fn test_tracking_baseline_identical_files_use_both_files() {
        let diff = compute_diff(&["a: 1", "b: 2"], &["a: 1", "b: 2"]);
        let both_files = "a: 1\nb: 2\n\n---\na: 1\nb: 2\n";
        let shown = "[ok] Files are identical\n";

        assert_eq!(
            tracking_baseline(&diff, "", both_files, shown),
            both_files,
            "identical files should still measure against the dump"
        );
    }

    #[test]
    fn test_tracking_baseline_empty_files_do_not_book_a_loss() {
        // Both files empty: the dump is shorter than the verdict line.
        let diff = compute_diff(&[], &[]);
        let shown = "[ok] Files are identical\n";

        assert_eq!(tracking_baseline(&diff, "", "\n---\n", shown), shown);
    }

    #[test]
    fn test_identical_files_keep_the_success_message() {
        let diff = compute_diff(&["same"], &["same"]);
        let rendered = "[ok] Files are identical\n";

        assert_eq!(select_file_diff_output(&diff, "", rendered), rendered);
    }

    #[test]
    fn test_classic_diff_covers_modified_line_boundary_cases() {
        for (old, new) in [
            ("alpha beta gamma delta", "alpha beta XXXXX delta"),
            ("alpha beta gamma", "alpha beta"),
            ("alpha beta gamma delta", "XXXXX beta gamma delta"),
        ] {
            let diff = compute_diff(&[old], &[new]);
            let fallback = format_classic_diff(&diff);

            assert!(fallback.contains(&format!("< {old}")));
            assert!(fallback.contains(&format!("> {new}")));
        }
    }

    // --- condense_unified_diff ---

    #[test]
    fn test_condense_unified_diff_single_file() {
        let diff = r#"diff --git a/src/main.rs b/src/main.rs
--- a/src/main.rs
+++ b/src/main.rs
@@ -1,3 +1,4 @@
 fn main() {
+    println!("hello");
     println!("world");
 }
"#;
        let result = condense_unified_diff(diff);
        assert!(result.contains("src/main.rs"));
        assert!(result.contains("+1"));
        assert!(result.contains("println"));
    }

    #[test]
    fn test_condense_unified_diff_multiple_files() {
        let diff = r#"diff --git a/a.rs b/a.rs
--- a/a.rs
+++ b/a.rs
+added line
diff --git a/b.rs b/b.rs
--- a/b.rs
+++ b/b.rs
-removed line
"#;
        let result = condense_unified_diff(diff);
        assert!(result.contains("a.rs"));
        assert!(result.contains("b.rs"));
    }

    #[test]
    fn test_condense_unified_diff_markers_at_column_0() {
        // Indented markers make anchored greps (`^[+-]`) match nothing, so a
        // "was anything removed?" audit answers no while the content is there.
        //
        // Two files on purpose. A file's changes are flushed at two separate
        // sites: once per `+++` for the preceding file, once after the loop for
        // the last one. A single-file fixture only ever reaches the second, so
        // the first could be reverted with the whole suite still green.
        let diff = "diff --git a/a.rs b/a.rs\n--- a/a.rs\n+++ b/a.rs\n@@ -1 +1 @@\n-fn old() {}\n+fn new() {}\ndiff --git a/b.rs b/b.rs\n--- a/b.rs\n+++ b/b.rs\n@@ -1 +1 @@\n-let x = 1;\n+let x = 2;\n";
        let result = condense_unified_diff(diff);
        for want in ["-fn old() {}", "+fn new() {}", "-let x = 1;", "+let x = 2;"] {
            assert!(
                result.lines().any(|l| l == want),
                "missing {want:?} at column 0 in:\n{}",
                result
            );
        }
        // Match on leading whitespace rather than a single space: the indent
        // this guards against is two spaces, so `" +"` / `" -"` would never
        // fire and the assertion would pass on the very code it rejects.
        assert!(
            !result.lines().any(|l| {
                let trimmed = l.trim_start();
                trimmed.len() != l.len()
                    && (trimmed.starts_with('+') || trimmed.starts_with('-'))
            }),
            "change lines must not be indented:\n{}",
            result
        );
    }

    #[test]
    fn test_condense_unified_diff_empty() {
        let result = condense_unified_diff("");
        assert!(result.is_empty());
    }

    // --- overflow indicator ---

    fn make_large_unified_diff(added: usize, removed: usize) -> String {
        let mut lines = vec![
            "diff --git a/config.yaml b/config.yaml".to_string(),
            "--- a/config.yaml".to_string(),
            "+++ b/config.yaml".to_string(),
            "@@ -1,200 +1,200 @@".to_string(),
        ];
        for i in 0..removed {
            lines.push(format!("-old_value_{}", i));
        }
        for i in 0..added {
            lines.push(format!("+new_value_{}", i));
        }
        lines.join("\n")
    }

    #[test]
    fn test_condense_unified_diff_large_no_false_overflow_indicator() {
        // All 200 changes are shown in full (never truncate diff content).
        // No misleading "... +N more" should appear.
        let diff = make_large_unified_diff(100, 100);
        let result = condense_unified_diff(&diff);
        assert!(
            !result.contains("more"),
            "No overflow indicator expected when all lines are shown, got:\n{}",
            result
        );
        assert!(
            result.contains("+new_value_99"),
            "Last added line must be present (no truncation)"
        );
        assert!(
            result.contains("-old_value_99"),
            "Last removed line must be present (no truncation)"
        );
    }

    #[test]
    fn test_condense_unified_diff_no_false_overflow() {
        // Counter-case to the 200-change test above: no indicator at small sizes either.
        let diff = make_large_unified_diff(4, 4);
        let result = condense_unified_diff(&diff);
        assert!(
            !result.contains("more"),
            "No overflow message expected for 8 changes, got:\n{}",
            result
        );
    }

    #[test]
    fn test_no_truncation_large_diff() {
        // Verify compute_diff returns all changes without truncation
        let mut a = Vec::new();
        let mut b = Vec::new();
        for i in 0..500 {
            a.push(format!("line_{}", i));
            if i % 3 == 0 {
                b.push(format!("CHANGED_{}", i));
            } else {
                b.push(format!("line_{}", i));
            }
        }
        let a_refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        let b_refs: Vec<&str> = b.iter().map(|s| s.as_str()).collect();
        let result = compute_diff(&a_refs, &b_refs);

        assert!(
            result.changes.len() > 100,
            "Expected 100+ changes, got {}",
            result.changes.len()
        );
        assert!(!result.changes.is_empty());
    }

    #[test]
    fn test_format_diff_shows_all_changes() {
        let mut a = Vec::new();
        let mut b = Vec::new();
        for i in 0..100 {
            a.push(format!("old_line_{}", i));
            b.push(format!("new_line_{}", i));
        }
        let a_refs: Vec<&str> = a.iter().map(|s| s.as_str()).collect();
        let b_refs: Vec<&str> = b.iter().map(|s| s.as_str()).collect();
        let diff = compute_diff(&a_refs, &b_refs);
        let output = format_diff_changes(&diff);

        assert!(output.contains("old_line_0"), "should contain first change");
        assert!(output.contains("new_line_99"), "should contain last change");
    }

    #[test]
    fn test_long_lines_not_truncated() {
        let long_line = "x".repeat(500);
        let a = vec![long_line.as_str()];
        let b = vec!["short"];
        let result = compute_diff(&a, &b);
        match &result.changes[0] {
            DiffChange::Removed(_, content) | DiffChange::Added(_, content) => {
                assert_eq!(content.len(), 500, "Line was truncated!");
            }
            DiffChange::Modified(_, old, _) => {
                assert_eq!(old.len(), 500, "Line was truncated!");
            }
        }
    }
}
