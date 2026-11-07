//! Utility functions for argument handling, particularly for restoring "--" escape
//! arguments that clap consumes during parsing.

/// Restores `--` tokens that clap consumed when using `trailing_var_arg = true`.
///
/// Returns `parsed_args` unchanged when `raw_args` has the same or fewer `--` tokens
/// than `parsed_args` (nothing was consumed). Otherwise restores all consumed `--` at
/// their original positions by returning the user-args suffix of `raw_args` verbatim.
pub fn restore_double_dash(parsed_args: &[String]) -> Vec<String> {
    let raw_args: Vec<String> = std::env::args().collect();
    restore_double_dash_with_raw(parsed_args, &raw_args)
}

/// Testable version that takes raw_args explicitly.
///
/// Precondition: all callers use `trailing_var_arg = true`, which guarantees that
/// `parsed_args` is the exact suffix of `raw_args` minus any `--` tokens that clap
/// stripped. This makes the user-args region length-deterministic:
///
///   user_region_len   = parsed_args.len() + missing_dashes
///   user_region_start = raw_args.len() - user_region_len
///
/// Returning `raw_args[user_region_start..]` restores all stripped `--` tokens at
/// their original positions without any value-based matching.
pub fn restore_double_dash_with_raw(parsed_args: &[String], raw_args: &[String]) -> Vec<String> {
    let raw_dash_count = raw_args.iter().filter(|a| a.as_str() == "--").count();
    let parsed_dash_count = parsed_args.iter().filter(|a| a.as_str() == "--").count();

    if raw_dash_count <= parsed_dash_count {
        return parsed_args.to_vec();
    }

    let missing_dashes = raw_dash_count - parsed_dash_count;
    let user_region_len = parsed_args.len() + missing_dashes;

    if raw_args.len() <= user_region_len {
        return parsed_args.to_vec();
    }

    let user_region_start = raw_args.len() - user_region_len;
    raw_args[user_region_start..].to_vec()
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

    // ============ Positional collision with command token ============

    #[test]
    fn test_positional_equals_subcommand() {
        // rtk git diff -- diff: filename "diff" same value as subcommand token
        let raw = vec!["rtk", "git", "diff", "--", "diff"];
        let parsed = vec!["diff"];
        assert_eq!(restore_with_raw(&parsed, &raw), vec!["--", "diff"]);
    }

    #[test]
    fn test_cargo_test_named_cargo() {
        // rtk cargo test cargo -- --nocapture: test named "cargo"
        let raw = vec!["rtk", "cargo", "test", "cargo", "--", "--nocapture"];
        let parsed = vec!["cargo", "--nocapture"];
        assert_eq!(
            restore_with_raw(&parsed, &raw),
            vec!["cargo", "--", "--nocapture"]
        );
    }

    #[test]
    fn test_consecutive_dashes_before_file() {
        // rtk git diff -- -- file: stable regardless of how many "--" clap consumed
        let raw = vec!["rtk", "git", "diff", "--", "--", "file"];
        // Case A: clap consumed one, kept second as positional
        let parsed_a = vec!["--", "file"];
        assert_eq!(restore_with_raw(&parsed_a, &raw), vec!["--", "--", "file"]);
        // Case B: clap consumed both
        let parsed_b = vec!["file"];
        assert_eq!(restore_with_raw(&parsed_b, &raw), vec!["--", "--", "file"]);
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
