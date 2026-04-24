//! Delta encoder — SimHash-gated LCS diff for near-duplicate file reads.
//! When a file is re-read with minor edits, sends only changed lines
//! instead of recompressing the full content.

use crate::core::simhash::simhash;

const NEAR_DUPLICATE_MAX_DISTANCE: u32 = 20;

/// Return true if two texts are close enough to attempt delta encoding.
pub fn is_near_duplicate(a: &str, b: &str) -> bool {
    simhash(a).is_near_duplicate(&simhash(b), NEAR_DUPLICATE_MAX_DISTANCE)
}

/// Compute a compact line-level delta between `old` and `new`.
/// Format: `§delta:HASH§\n-removed\n+added\n...`
/// Falls back to a "too large" notice for files > 5000 lines.
pub fn compute_delta(old_hash: &str, old: &str, new: &str) -> String {
    let old_lines: Vec<&str> = old.lines().collect();
    let new_lines: Vec<&str> = new.lines().collect();

    if old_lines.len() > 5000 || new_lines.len() > 5000 {
        return format!(
            "§delta:{}§ [too large for delta, {} lines]",
            &old_hash[..old_hash.len().min(8)],
            new_lines.len()
        );
    }

    let lcs = lcs_indices(&old_lines, &new_lines);
    let old_in_lcs: std::collections::HashSet<usize> = lcs.iter().map(|(i, _)| *i).collect();
    let new_in_lcs: std::collections::HashSet<usize> = lcs.iter().map(|(_, j)| *j).collect();

    let mut parts: Vec<String> = Vec::new();
    for (i, line) in old_lines.iter().enumerate() {
        if !old_in_lcs.contains(&i) {
            parts.push(format!("-{}", line));
        }
    }
    for (j, line) in new_lines.iter().enumerate() {
        if !new_in_lcs.contains(&j) {
            parts.push(format!("+{}", line));
        }
    }

    if parts.is_empty() {
        return format!("§delta:{}§ (unchanged)", &old_hash[..old_hash.len().min(8)]);
    }

    format!(
        "§delta:{}§\n{}",
        &old_hash[..old_hash.len().min(8)],
        parts.join("\n")
    )
}

fn lcs_indices(a: &[&str], b: &[&str]) -> Vec<(usize, usize)> {
    let m = a.len();
    let n = b.len();
    let mut dp = vec![vec![0usize; n + 1]; m + 1];
    for i in 1..=m {
        for j in 1..=n {
            if a[i - 1] == b[j - 1] {
                dp[i][j] = dp[i - 1][j - 1] + 1;
            } else {
                dp[i][j] = dp[i - 1][j].max(dp[i][j - 1]);
            }
        }
    }
    let mut result = Vec::new();
    let (mut i, mut j) = (m, n);
    while i > 0 && j > 0 {
        if a[i - 1] == b[j - 1] {
            result.push((i - 1, j - 1));
            i -= 1;
            j -= 1;
        } else if dp[i - 1][j] > dp[i][j - 1] {
            i -= 1;
        } else {
            j -= 1;
        }
    }
    result.reverse();
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_identical_texts_zero_delta() {
        let text = "line1\nline2\nline3\nline4\nline5";
        let delta = compute_delta("abc123", text, text);
        assert!(delta.starts_with("§delta:"));
        assert!(!delta.contains('+') || delta.contains("(unchanged)"));
    }

    #[test]
    fn test_detects_added_line() {
        let old =
            "use anyhow::Result;\nuse std::path::Path;\n\npub fn foo() -> Result<()> { Ok(()) }";
        let new = "use anyhow::Result;\nuse std::path::Path;\nuse std::fs;\n\npub fn foo() -> Result<()> { Ok(()) }";
        assert!(is_near_duplicate(old, new), "should be near-duplicate");
        let delta = compute_delta("hash12", old, new);
        assert!(delta.contains("+use std::fs;"), "delta: {delta}");
    }

    #[test]
    fn test_detects_modified_line() {
        let old = include_str!("../../tests/fixtures/near_duplicate_a.txt");
        let new = include_str!("../../tests/fixtures/near_duplicate_b.txt");
        assert!(is_near_duplicate(old, new));
        let delta = compute_delta("hashXY", old, new);
        assert!(delta.starts_with("§delta:hashXY§"), "delta: {delta}");
        assert!(
            delta.contains("+pub fn load_config") || delta.contains("-pub fn read_config"),
            "delta should mention the renamed function: {delta}"
        );
    }

    #[test]
    fn test_delta_shorter_than_full_content() {
        let old = include_str!("../../tests/fixtures/near_duplicate_a.txt");
        let new = include_str!("../../tests/fixtures/near_duplicate_b.txt");
        let delta = compute_delta("hashZ", old, new);
        assert!(
            delta.len() < new.len(),
            "delta ({}) should be shorter than full content ({})",
            delta.len(),
            new.len()
        );
    }

    #[test]
    fn test_guard_on_large_files() {
        let big = "line\n".repeat(6000);
        let result = compute_delta("bigfile", &big, &big);
        assert!(!result.is_empty());
    }
}
