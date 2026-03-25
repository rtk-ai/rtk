use crate::tracking;
use anyhow::Result;
use std::fs;
use std::path::Path;

/// Ultra-condensed diff - only changed lines, no context
pub fn run(file1: &Path, file2: &Path, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Comparing: {} vs {}", file1.display(), file2.display());
    }

    let content1 = fs::read_to_string(file1)?;
    let content2 = fs::read_to_string(file2)?;
    let raw = format!("{}\n---\n{}", content1, content2);

    let lines1: Vec<&str> = content1.lines().collect();
    let lines2: Vec<&str> = content2.lines().collect();
    let diff = compute_diff(&lines1, &lines2);
    let mut rtk = String::new();

    if diff.added == 0 && diff.removed == 0 {
        rtk.push_str("[ok] Files are identical");
        println!("{}", rtk);
        timer.track(
            &format!("diff {} {}", file1.display(), file2.display()),
            "rtk diff",
            &raw,
            &rtk,
        );
        return Ok(());
    }

    rtk.push_str(&format!("{} → {}\n", file1.display(), file2.display()));
    rtk.push_str(&format!(
        "   +{} added, -{} removed, ~{} modified\n\n",
        diff.added, diff.removed, diff.modified
    ));

    // Never truncate diff content — users make decisions based on this data.
    // Only the summary header provides compression; all changes are shown in full.
    for change in &diff.changes {
        match change {
            DiffChange::Added(ln, c) => rtk.push_str(&format!("+{:4} {}\n", ln, c)),
            DiffChange::Removed(ln, c) => rtk.push_str(&format!("-{:4} {}\n", ln, c)),
            DiffChange::Modified(ln, old, new) => {
                rtk.push_str(&format!("~{:4} {} → {}\n", ln, old, new))
            }
        }
    }

    print!("{}", rtk);
    timer.track(
        &format!("diff {} {}", file1.display(), file2.display()),
        "rtk diff",
        &raw,
        &rtk,
    );
    Ok(())
}

/// Run diff from stdin (piped command output)
pub fn run_stdin(_verbose: u8) -> Result<()> {
    use std::io::{self, Read};
    let timer = tracking::TimedExecution::start();

    let mut input = String::new();
    io::stdin().read_to_string(&mut input)?;

    // Parse unified diff format
    let condensed = condense_unified_diff(&input);
    println!("{}", condensed);

    timer.track("diff (stdin)", "rtk diff (stdin)", &input, &condensed);

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
                    for c in &changes {
                        result.push(format!("  {}", c));
                    }
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
        for c in &changes {
            result.push(format!("  {}", c));
        }
    }

    result.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn test_condense_unified_diff_empty() {
        let result = condense_unified_diff("");
        assert!(result.is_empty());
    }

    #[test]
    fn test_no_truncation_large_diff() {
        // Verify all changes are shown, not truncated
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

        // Should have ~167 changes (every 3rd line), all present
        assert!(result.changes.len() > 100, "Expected 100+ changes, got {}", result.changes.len());
        // No truncation — changes count matches what we generate
        assert!(!result.changes.is_empty());
    }

    #[test]
    fn test_long_lines_not_truncated() {
        let long_line = "x".repeat(500);
        let a = vec![long_line.as_str()];
        let b = vec!["short"];
        let result = compute_diff(&a, &b);
        // The removed line should contain the full 500-char string
        match &result.changes[0] {
            DiffChange::Removed(_, content) | DiffChange::Added(_, content) => {
                assert_eq!(content.len(), 500, "Line was truncated!");
            }
            DiffChange::Modified(_, old, _) => {
                assert_eq!(old.len(), 500, "Line was truncated!");
            }
        }
    }

    #[test]
    fn test_condense_unified_no_truncation() {
        // Generate a large unified diff
        let mut lines = Vec::new();
        lines.push("diff --git a/big.yaml b/big.yaml".to_string());
        lines.push("--- a/big.yaml".to_string());
        lines.push("+++ b/big.yaml".to_string());
        for i in 0..200 {
            lines.push(format!("+added_line_{}", i));
        }
        let diff = lines.join("\n");
        let result = condense_unified_diff(&diff);

        // All 200 added lines should be present
        assert!(result.contains("added_line_0"));
        assert!(result.contains("added_line_199"));
        assert!(!result.contains("not shown"), "Should not truncate");
        assert!(!result.contains("more"), "Should not have '... more'");
    }
}
