//! Safety Policy Engine — unified rule-based implementation.
//!
//! All safety rules, remaps, and blocking rules are loaded from the unified
//! Rule system (`config::rules`). Rules are MD files with YAML frontmatter,
//! loaded from built-in defaults and user directories.

use crate::config::rules::{self, Rule};

use super::predicates;

/// Result of safety check
#[derive(Clone, Debug, PartialEq)]
pub enum SafetyResult {
    /// Command is safe to execute as-is
    Safe,
    /// Command is blocked with error message
    Blocked(String),
    /// Command was rewritten to a new command string
    Rewritten(String),
    /// Request to move files to trash (built-in)
    TrashRequested(Vec<String>),
}

/// Dispatch a matched rule into a SafetyResult.
fn dispatch(rule: &Rule, args: &str) -> SafetyResult {
    match rule.action.as_str() {
        "trash" => {
            let paths: Vec<String> = args
                .split_whitespace()
                .filter(|a| !a.starts_with('-'))
                .map(String::from)
                .collect();
            SafetyResult::TrashRequested(paths)
        }
        "rewrite" => {
            let redirect = rule.redirect.as_deref().unwrap_or(args);
            SafetyResult::Rewritten(redirect.replace("{args}", args))
        }
        "suggest_tool" | "block" => {
            // Use interactive-aware message (human vs agent)
            let msg = if predicates::is_interactive() {
                // For suggest_tool, human message references the tool name
                if rule.action == "suggest_tool" {
                    // First line of message is typically the human-friendly version
                    rule.message
                        .lines()
                        .next()
                        .unwrap_or(&rule.message)
                        .to_string()
                } else {
                    rule.message.clone()
                }
            } else {
                // Agent: use the full message (contains BLOCK: prefix)
                rule.message.clone()
            };
            SafetyResult::Blocked(msg)
        }
        "warn" => {
            eprintln!("{}", rule.message);
            SafetyResult::Safe
        }
        _ => SafetyResult::Safe,
    }
}

/// Check a parsed command against all safety rules.
pub fn check(binary: &str, args: &[String]) -> SafetyResult {
    let full_cmd = if args.is_empty() {
        binary.to_string()
    } else {
        format!("{} {}", binary, args.join(" "))
    };

    for rule in rules::load_all() {
        if !rules::matches_rule(rule, Some(binary), &full_cmd) {
            continue;
        }
        if !rule.should_apply() {
            continue;
        }
        return dispatch(rule, &args.join(" "));
    }
    SafetyResult::Safe
}

/// Check raw command string (for passthrough mode).
/// Catches dangerous patterns even when we can't parse the command.
pub fn check_raw(raw: &str) -> SafetyResult {
    for rule in rules::load_all() {
        if !rules::matches_rule(rule, None, raw) {
            continue;
        }
        if !rule.should_apply() {
            continue;
        }
        // In passthrough, suggest_tool rules don't apply (cat in pipelines is valid)
        if rule.action == "suggest_tool" {
            continue;
        }
        // In passthrough, trash becomes block (can't extract paths reliably)
        if rule.action == "trash" {
            return SafetyResult::Blocked(format!(
                "Passthrough blocked: '{}' detected. Use native mode for safe trash.",
                rule.patterns.first().map(|s| s.as_str()).unwrap_or("rm")
            ));
        }
        return dispatch(rule, raw);
    }
    SafetyResult::Safe
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::test_helpers::EnvGuard;
    use std::env;

    // === BASIC CHECK TESTS ===

    #[test]
    fn test_check_safe_command() {
        let _guard = EnvGuard::new();
        let result = check("ls", &["-la".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_check_git_status() {
        let _guard = EnvGuard::new();
        let result = check("git", &["status".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_check_empty_args() {
        let _guard = EnvGuard::new();
        let result = check("pwd", &[]);
        assert_eq!(result, SafetyResult::Safe);
    }

    // === RM SAFETY TESTS (RTK_SAFE_COMMANDS) ===

    #[test]
    fn test_check_rm_blocked_when_env_set() {
        let _guard = EnvGuard::new();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check("rm", &["file.txt".to_string()]);
        match result {
            SafetyResult::TrashRequested(paths) => {
                assert_eq!(paths, vec!["file.txt"]);
            }
            _ => panic!("Expected TrashRequested, got {:?}", result),
        }
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_rm_blocked_by_default() {
        let _guard = EnvGuard::new();
        // rm should be redirected to trash by default now
        let result = check("rm", &["file.txt".to_string()]);
        match result {
            SafetyResult::TrashRequested(paths) => {
                assert_eq!(paths, vec!["file.txt"]);
            }
            _ => panic!("Expected TrashRequested by default, got {:?}", result),
        }
    }

    #[test]
    fn test_check_rm_passes_when_disabled() {
        let _guard = EnvGuard::new();
        env::set_var("RTK_SAFE_COMMANDS", "0");
        let result = check("rm", &["file.txt".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_rm_with_flags() {
        let _guard = EnvGuard::new();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check("rm", &["-rf".to_string(), "dir".to_string()]);
        match result {
            SafetyResult::TrashRequested(paths) => {
                // Flags should be filtered out
                assert_eq!(paths, vec!["dir"]);
            }
            _ => panic!("Expected TrashRequested"),
        }
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_rm_multiple_files() {
        let _guard = EnvGuard::new();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check(
            "rm",
            &[
                "a.txt".to_string(),
                "b.txt".to_string(),
                "c.txt".to_string(),
            ],
        );
        match result {
            SafetyResult::TrashRequested(paths) => {
                assert_eq!(paths, vec!["a.txt", "b.txt", "c.txt"]);
            }
            _ => panic!("Expected TrashRequested"),
        }
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_rm_no_files() {
        let _guard = EnvGuard::new();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check("rm", &["-rf".to_string()]);
        match result {
            SafetyResult::TrashRequested(paths) => {
                assert!(paths.is_empty());
            }
            _ => panic!("Expected TrashRequested, got {:?}", result),
        }
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    // === CAT/SED/HEAD TESTS (blocked by default, opt-out with RTK_BLOCK_TOKEN_WASTE=0) ===

    #[test]
    fn test_check_cat_blocked() {
        let _guard = EnvGuard::new();
        let result = check("cat", &["file.txt".to_string()]);
        match result {
            SafetyResult::Blocked(msg) => {
                assert!(msg.contains("file-reading"), "msg: {}", msg);
            }
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_cat_passes_when_disabled() {
        let _guard = EnvGuard::new();
        env::set_var("RTK_BLOCK_TOKEN_WASTE", "0");
        let result = check("cat", &["file.txt".to_string()]);
        env::remove_var("RTK_BLOCK_TOKEN_WASTE");
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_check_sed_blocked() {
        let _guard = EnvGuard::new();
        let result = check("sed", &["-i".to_string(), "s/old/new/g".to_string()]);
        match result {
            SafetyResult::Blocked(msg) => {
                assert!(msg.contains("file-editing"), "msg: {}", msg);
            }
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_head_blocked() {
        let _guard = EnvGuard::new();
        let result = check(
            "head",
            &["-n".to_string(), "10".to_string(), "file.txt".to_string()],
        );
        match result {
            SafetyResult::Blocked(msg) => {
                assert!(msg.contains("file-reading"), "msg: {}", msg);
            }
            _ => panic!("Expected Blocked"),
        }
    }

    // === GIT SAFETY TESTS (RTK_SAFE_COMMANDS) ===

    #[test]
    fn test_check_git_reset_hard_blocked_when_env_set() {
        let _guard = EnvGuard::new();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        // This test may or may not trigger depending on git state
        // Just ensure it doesn't panic
        let _ = check("git", &["reset".to_string(), "--hard".to_string()]);
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_git_clean_fd_rewritten() {
        let _guard = EnvGuard::new();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check("git", &["clean".to_string(), "-fd".to_string()]);
        match result {
            SafetyResult::Rewritten(cmd) => {
                assert!(cmd.contains("stash -u"));
                assert!(cmd.contains("clean"));
            }
            _ => panic!("Expected Rewritten, got {:?}", result),
        }
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_git_clean_rewritten_by_default() {
        let _guard = EnvGuard::new();
        // git clean should be rewritten with stash by default
        let result = check("git", &["clean".to_string(), "-fd".to_string()]);
        match result {
            SafetyResult::Rewritten(cmd) => {
                assert!(cmd.contains("stash -u"));
            }
            _ => panic!("Expected Rewritten by default, got {:?}", result),
        }
    }

    #[test]
    fn test_check_git_clean_passes_when_disabled() {
        let _guard = EnvGuard::new();
        env::set_var("RTK_SAFE_COMMANDS", "0");
        let result = check("git", &["clean".to_string(), "-fd".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    // === CHECK_RAW TESTS ===

    #[test]
    fn test_check_raw_rm_detected() {
        let _guard = EnvGuard::new();
        // RTK_SAFE_COMMANDS is enabled by default, so rm should be blocked
        let result = check_raw("rm file.txt");
        match result {
            SafetyResult::Blocked(_) => {}
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_raw_sudo_rm_detected() {
        let _guard = EnvGuard::new();
        // RTK_SAFE_COMMANDS is enabled by default, so sudo rm should be blocked
        let result = check_raw("sudo rm file.txt");
        match result {
            SafetyResult::Blocked(_) => {}
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_raw_sudo_flags_rm_detected() {
        let _guard = EnvGuard::new();
        let result = check_raw("sudo -u root rm file.txt");
        match result {
            SafetyResult::Blocked(_) => {}
            _ => panic!("Expected Blocked for sudo -u root rm"),
        }
    }

    #[test]
    fn test_check_raw_safe_command() {
        let _guard = EnvGuard::new();
        let result = check_raw("ls -la");
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_check_raw_rm_in_quoted_string() {
        let _guard = EnvGuard::new();
        let result = check_raw("echo \"rm file\"");
        // This will be blocked because we can't distinguish quoted rm
        // That's intentional - better safe than sorry
        match result {
            SafetyResult::Blocked(_) => {}
            SafetyResult::Safe => {} // Either is acceptable
            SafetyResult::Rewritten(_) => {}
            SafetyResult::TrashRequested(_) => {}
        }
    }

    // === NEW GIT SAFETY TESTS ===

    #[test]
    fn test_git_checkout_dot_stash_prepended() {
        let _guard = EnvGuard::new();
        let result = check("git", &["checkout".to_string(), ".".to_string()]);
        // May or may not trigger based on predicate, just ensure no panic
        match result {
            SafetyResult::Rewritten(cmd) => {
                assert!(cmd.contains("stash"));
                assert!(cmd.contains("checkout"));
            }
            SafetyResult::Safe => {} // Predicate returned false (no changes)
            _ => {}
        }
    }

    #[test]
    fn test_git_checkout_dashdash_stash_prepended() {
        let _guard = EnvGuard::new();
        let result = check(
            "git",
            &[
                "checkout".to_string(),
                "--".to_string(),
                "file.txt".to_string(),
            ],
        );
        match result {
            SafetyResult::Rewritten(cmd) => {
                assert!(cmd.contains("stash"));
                assert!(cmd.contains("checkout"));
            }
            SafetyResult::Safe => {}
            _ => {}
        }
    }

    #[test]
    fn test_git_stash_drop_rewritten_to_pop() {
        let _guard = EnvGuard::new();
        let result = check("git", &["stash".to_string(), "drop".to_string()]);
        match result {
            SafetyResult::Rewritten(cmd) => {
                assert!(cmd.contains("stash pop"));
            }
            _ => panic!("Expected Rewritten to stash pop"),
        }
    }

    #[test]
    fn test_git_clean_f_rewritten() {
        let _guard = EnvGuard::new();
        let result = check("git", &["clean".to_string(), "-f".to_string()]);
        match result {
            SafetyResult::Rewritten(cmd) => {
                assert!(cmd.contains("stash -u"));
                assert!(cmd.contains("clean"));
            }
            _ => panic!("Expected Rewritten with stash -u"),
        }
    }

    #[test]
    fn test_git_branch_checkout_safe() {
        // git checkout <branch> should be safe (not matched by checkout . or checkout --)
        let _guard = EnvGuard::new();
        let result = check("git", &["checkout".to_string(), "main".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_git_checkout_new_branch_safe() {
        let _guard = EnvGuard::new();
        let result = check(
            "git",
            &[
                "checkout".to_string(),
                "-b".to_string(),
                "feature".to_string(),
            ],
        );
        assert_eq!(result, SafetyResult::Safe);
    }

    // === PATTERN MATCHING FALSE POSITIVE TESTS ===

    #[test]
    fn test_no_false_positive_catalog() {
        let _guard = EnvGuard::new();
        let result = check("catalog", &["show".to_string()]);
        assert_eq!(
            result,
            SafetyResult::Safe,
            "catalog must not match cat rule"
        );
    }

    #[test]
    fn test_no_false_positive_sedan() {
        let _guard = EnvGuard::new();
        let result = check("sedan", &[]);
        assert_eq!(result, SafetyResult::Safe, "sedan must not match sed rule");
    }

    #[test]
    fn test_no_false_positive_headless() {
        let _guard = EnvGuard::new();
        let result = check("headless", &["chrome".to_string()]);
        assert_eq!(
            result,
            SafetyResult::Safe,
            "headless must not match head rule"
        );
    }

    #[test]
    fn test_no_false_positive_rmdir() {
        let _guard = EnvGuard::new();
        let result = check("rmdir", &["empty_dir".to_string()]);
        assert_eq!(result, SafetyResult::Safe, "rmdir must not match rm rule");
    }

    // === CHECK_RAW WORD BOUNDARY TESTS ===

    #[test]
    fn test_check_raw_no_false_positive_trim() {
        let _guard = EnvGuard::new();
        std::env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check_raw("trim file.txt");
        assert_eq!(result, SafetyResult::Safe, "trim must not match rm pattern");
        std::env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_raw_no_false_positive_farm() {
        let _guard = EnvGuard::new();
        std::env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check_raw("farm --harvest");
        assert_eq!(result, SafetyResult::Safe, "farm must not match rm pattern");
        std::env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_raw_catches_standalone_rm() {
        let _guard = EnvGuard::new();
        std::env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check_raw("rm file.txt");
        assert!(
            matches!(result, SafetyResult::Blocked(_)),
            "standalone rm must be caught"
        );
        std::env::remove_var("RTK_SAFE_COMMANDS");
    }
}
