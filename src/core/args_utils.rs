//! Utility functions for argument handling, particularly for restoring "--" escape
//! arguments that clap consumes during parsing.

/// Restores "--" escape arguments that clap consumed.
/// Handles:
/// - Single/multiple "--" swallowed by clap (restores each at its original position)
/// - "--" already present in parsed (no change)
/// - No "--" in original command (no injection)
/// - Args appearing before/after "--" in original (preserves order)
/// - Interleaved "--" and args (preserves relative positions, e.g., "-- arg1 -- arg2")
/// - Duplicate args on both sides of "--"
///
/// Returns parsed_args unchanged if raw has same or fewer "--" than parsed
/// (meaning clap didn't consume any, or preserved them).
pub fn restore_double_dash(parsed_args: &[String]) -> Vec<String> {
    let raw_args: Vec<String> = std::env::args().collect();
    restore_double_dash_with_raw(parsed_args, &raw_args)
}

/// Testable version that takes raw_args explicitly.
pub fn restore_double_dash_with_raw(parsed_args: &[String], raw_args: &[String]) -> Vec<String> {
    let raw_dash_count = raw_args.iter().filter(|a| a.as_str() == "--").count();
    let parsed_dash_count = parsed_args.iter().filter(|a| a.as_str() == "--").count();

    if raw_dash_count <= parsed_dash_count {
        return parsed_args.to_vec();
    }

    // Find all positions of "--" in raw_args (skip index 0 = "rtk")
    let mut dash_positions: Vec<usize> = Vec::new();
    for (i, arg) in raw_args.iter().enumerate().skip(1) {
        if arg == "--" {
            dash_positions.push(i);
        }
    }

    if dash_positions.is_empty() {
        return parsed_args.to_vec();
    }

    // Build result by inserting "--" at correct positions relative to parsed args
    let mut result = Vec::new();
    let mut raw_idx = 1; // start after "rtk"
    let mut dash_idx = 0;

    while raw_idx < raw_args.len() {
        // Check if current position is a "--" that was swallowed
        if dash_idx < dash_positions.len() && raw_idx == dash_positions[dash_idx] {
            result.push("--".to_string());
            dash_idx += 1;
        } else if parsed_args.contains(&raw_args[raw_idx]) {
            // This arg is in parsed_args, add it
            result.push(raw_args[raw_idx].clone());
        }
        raw_idx += 1;
    }

    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn restore_with_raw(parsed: &[&str], raw: &[&str]) -> Vec<String> {
        let parsed: Vec<String> = parsed.iter().map(|s| s.to_string()).collect();
        let raw: Vec<String> = raw.iter().map(|s| s.to_string()).collect();
        restore_double_dash_with_raw(parsed.as_slice(), raw.as_slice())
    }

    // ============ Single "--" swallowed ============

    #[test]
    fn test_single_dash_swallowed() {
        // rtk git diff -- file → clap gave ["file"], restore "--"
        let raw = vec!["rtk", "git", "diff", "--", "file"];
        let parsed = vec!["file"];
        assert_eq!(restore_with_raw(&parsed, &raw), vec!["--", "file"]);
    }

    #[test]
    fn test_args_before_dash() {
        // rtk cargo test name -- --nocapture → args before "--" stay before
        let raw = vec!["rtk", "cargo", "test", "name", "--", "--nocapture"];
        let parsed = vec!["name", "--nocapture"];
        assert_eq!(
            restore_with_raw(&parsed, &raw),
            vec!["name", "--", "--nocapture"]
        );
    }

    // ============ Multiple "--" swallowed ============

    #[test]
    fn test_multiple_dashes_all_swallowed() {
        // rtk git diff -- -- -- → all 3 "--" swallowed, consecutive in output
        let raw = vec!["rtk", "git", "diff", "--", "--", "--"];
        let parsed: Vec<&str> = vec![];
        assert_eq!(restore_with_raw(&parsed, &raw), vec!["--", "--", "--"]);
    }

    #[test]
    fn test_dashes_with_args_between() {
        // rtk git diff -- arg1 -- arg2 → both "--" consumed, preserve positions
        let raw = vec!["rtk", "git", "diff", "--", "arg1", "--", "arg2"];
        let parsed = vec!["arg1", "arg2"];
        // Result: each "--" inserted at its original position relative to args
        assert_eq!(
            restore_with_raw(&parsed, &raw),
            vec!["--", "arg1", "--", "arg2"]
        );
    }

    #[test]
    fn test_multiple_dashes_some_preserved() {
        // rtk git diff -- -- → 2 in raw, 1 preserved in parsed
        let raw = vec!["rtk", "git", "diff", "--", "--"];
        let parsed = vec!["--"];
        assert_eq!(restore_with_raw(&parsed, &raw), vec!["--", "--"]);
    }

    #[test]
    fn test_compound_command_with_dashes() {
        // Multiple segments with "--" → restore all
        let raw = vec!["rtk", "cmd1", "--", "arg1", "&&", "cmd2", "--", "file"];
        let parsed = vec!["file"];
        assert_eq!(restore_with_raw(&parsed, &raw), vec!["--", "--", "file"]);
    }

    // ============ "--" already present (no change needed) ============

    #[test]
    fn test_dash_already_preserved() {
        // rtk cargo clippy -p pkg -- -D warnings → clap kept "--"
        let raw = vec![
            "rtk", "cargo", "clippy", "-p", "pkg", "--", "-D", "warnings",
        ];
        let parsed = vec!["-p", "pkg", "--", "-D", "warnings"];
        assert_eq!(
            restore_with_raw(&parsed, &raw),
            vec!["-p", "pkg", "--", "-D", "warnings"]
        );
    }

    #[test]
    fn test_trailing_dash_preserved() {
        // rtk git diff file -- → trailing "--" preserved
        let raw = vec!["rtk", "git", "diff", "file", "--"];
        let parsed = vec!["file", "--"];
        assert_eq!(restore_with_raw(&parsed, &raw), vec!["file", "--"]);
    }

    // ============ No "--" in original (no injection) ============

    #[test]
    fn test_no_dash_in_original() {
        // Various cases: branch with /, range, bare word, flags only
        // All should return args unchanged (no injection)
        let cases = vec![
            (
                vec!["rtk", "git", "diff", "feature/auth"],
                vec!["feature/auth"],
            ),
            (
                vec!["rtk", "git", "diff", "main...feature"],
                vec!["main...feature"],
            ),
            (vec!["rtk", "git", "diff", "main"], vec!["main"]),
            (
                vec!["rtk", "git", "diff", "--stat", "--cached"],
                vec!["--stat", "--cached"],
            ),
        ];
        for (raw, parsed) in cases {
            assert_eq!(restore_with_raw(&parsed, &raw), parsed);
        }
    }

    // ============ Edge cases ============

    #[test]
    fn test_duplicate_args_both_sides() {
        // -p pkg1 -p pkg2 -- -p pkg3 → restore after last -p
        let raw = vec![
            "rtk", "cargo", "clippy", "-p", "p1", "-p", "p2", "--", "-p", "p3",
        ];
        let parsed = vec!["-p", "p1", "-p", "p2", "-p", "p3"];
        assert_eq!(
            restore_with_raw(&parsed, &raw),
            vec!["-p", "p1", "-p", "p2", "--", "-p", "p3"]
        );
    }

    #[test]
    fn test_empty_args() {
        let raw = vec!["rtk", "cargo", "test"];
        let parsed: Vec<&str> = vec![];
        assert_eq!(restore_with_raw(&parsed, &raw), Vec::<String>::new());
    }

    #[test]
    fn test_cargo_clippy_missing_dash() {
        // No "--" in original → no injection
        let raw = vec!["rtk", "cargo", "clippy", "-D", "warnings"];
        let parsed = vec!["-D", "warnings"];
        assert_eq!(restore_with_raw(&parsed, &raw), vec!["-D", "warnings"]);
    }

    // ============ Git diff specific cases ============

    #[test]
    fn test_git_diff_ref_before_path() {
        // rtk git diff HEAD -- file
        let raw = vec!["rtk", "git", "diff", "HEAD", "--", "file"];
        let parsed = vec!["HEAD", "file"];
        assert_eq!(restore_with_raw(&parsed, &raw), vec!["HEAD", "--", "file"]);
    }

    #[test]
    fn test_git_diff_flags_before_path() {
        // rtk git diff --cached -- file
        let raw = vec!["rtk", "git", "diff", "--cached", "--", "file"];
        let parsed = vec!["--cached", "file"];
        assert_eq!(
            restore_with_raw(&parsed, &raw),
            vec!["--cached", "--", "file"]
        );
    }

    #[test]
    fn test_git_diff_multiple_files() {
        // Original issue: multiple files caused "fatal: bad revision"
        let raw = vec!["rtk", "git", "diff", "--", "file1", "file2", "file3"];
        let parsed = vec!["file1", "file2", "file3"];
        assert_eq!(
            restore_with_raw(&parsed, &raw),
            vec!["--", "file1", "file2", "file3"]
        );
    }
}
