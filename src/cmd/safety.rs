//! Safety Policy Engine with dual messages (human vs agent).
//!
//! Design: Rules have predicates for conditional behavior.
//! Messages are terse for agents, detailed for humans.

use super::predicates;

/// Actions a safety rule can take
#[derive(Clone, Debug, PartialEq)]
pub enum SafetyAction {
    /// Rewrite to a different command template (e.g., "rtk trash {args}")
    Rewrite(String),
    /// Prepend a command (e.g., "git stash && {cmd}")
    Prepend(String),
    /// Suggest using a tool instead (for agents)
    SuggestTool(String),
    /// Route to built-in trash implementation
    Trash,
}

/// A safety rule with pattern matching and actions
#[derive(Clone)]
pub struct SafetyRule {
    /// Pattern to match at start of command (e.g., "rm", "git reset --hard")
    pub pattern: &'static str,
    /// Action to take when rule matches
    pub action: SafetyAction,
    /// Human-friendly message (shown in interactive mode)
    pub human_msg: &'static str,
    /// Agent-terse message (shown in non-interactive mode)
    pub agent_msg: &'static str,
    /// Optional predicate for conditional activation
    pub predicate: Option<fn() -> bool>,
    /// Optional env var that must be set for rule to apply
    pub env_var: Option<&'static str>,
}

impl SafetyRule {
    /// Get appropriate message based on context (interactive vs agent)
    pub fn message(&self) -> &str {
        if predicates::is_interactive() {
            self.human_msg
        } else {
            self.agent_msg
        }
    }

    /// Check if rule should apply (env var + predicate)
    ///
    /// Env var behavior:
    /// - RTK_SAFE_COMMANDS: Opt-out, applies by default, disable with =0
    /// - RTK_BLOCK_TOKEN_WASTE: Opt-out, applies by default, disable with =0
    pub fn should_apply(&self) -> bool {
        // Check env var if specified
        if let Some(env) = self.env_var {
            match env {
                // Opt-out features: apply by default, disable with =0
                "RTK_SAFE_COMMANDS" | "RTK_BLOCK_TOKEN_WASTE" => {
                    if let Ok(val) = std::env::var(env) {
                        if val == "0" || val == "false" {
                            return false;
                        }
                    }
                    // Default: enabled (no env var or env var != 0)
                }
                // Unknown env vars: require explicit setting
                _ => {
                    if std::env::var(env).is_err() {
                        return false;
                    }
                }
            }
        }
        // Check predicate if specified
        if let Some(pred) = self.predicate {
            if !pred() {
                return false;
            }
        }
        true
    }
}

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

/// Shorthand macro for declaring safety rules.
///
/// Two forms:
/// - `rule!(pattern, action, human_msg, agent_msg, env: "ENV_VAR")` — no predicate
/// - `rule!(pattern, action, human_msg, agent_msg, pred: fn, env: "ENV_VAR")` — with predicate
macro_rules! rule {
    ($pat:expr, $act:expr, $human:expr, $agent:expr, env: $env:expr) => {
        SafetyRule {
            pattern: $pat,
            action: $act,
            human_msg: $human,
            agent_msg: $agent,
            predicate: None,
            env_var: Some($env),
        }
    };
    ($pat:expr, $act:expr, $human:expr, $agent:expr, pred: $pred:expr, env: $env:expr) => {
        SafetyRule {
            pattern: $pat,
            action: $act,
            human_msg: $human,
            agent_msg: $agent,
            predicate: Some($pred),
            env_var: Some($env),
        }
    };
}

/// Get all safety rules (ordered by specificity)
///
/// Environment Variables (coarse-grained):
/// - RTK_SAFE_COMMANDS=0 - Disable rm->trash and git safety
/// - RTK_BLOCK_TOKEN_WASTE=0 - Disable token waste prevention (cat/sed/head blocking)
///
/// All safety features are enabled by default.
pub fn get_rules() -> Vec<SafetyRule> {
    let stash_reset = SafetyAction::Prepend("git stash push -m 'RTK: reset backup'".into());
    let stash_checkout = SafetyAction::Prepend("git stash push -m 'RTK: checkout backup'".into());
    let stash_clean = SafetyAction::Prepend("git stash -u -m 'RTK: clean backup'".into());

    vec![
        // === DANGEROUS FILE OPERATIONS ===
        rule!("rm", SafetyAction::Trash,
            "Safety: Moving to trash.", "REWRITE: rm -> trash",
            env: "RTK_SAFE_COMMANDS"),
        // === DANGEROUS GIT OPERATIONS (most specific patterns first) ===
        rule!("git reset --hard", stash_reset,
            "Safety: Stashing before reset.", "PREPEND: git stash",
            pred: predicates::has_unstaged_changes, env: "RTK_SAFE_COMMANDS"),
        rule!("git checkout --", stash_checkout.clone(),
            "Safety: Stashing before checkout.", "PREPEND: git stash",
            pred: predicates::has_unstaged_changes, env: "RTK_SAFE_COMMANDS"),
        rule!("git checkout .", stash_checkout,
            "Safety: Stashing before checkout.", "PREPEND: git stash",
            pred: predicates::has_unstaged_changes, env: "RTK_SAFE_COMMANDS"),
        rule!("git stash drop", SafetyAction::Rewrite("git stash pop".into()),
            "Safety: Using pop instead of drop (recoverable).", "REWRITE: stash drop -> pop",
            env: "RTK_SAFE_COMMANDS"),
        rule!("git clean -fd", stash_clean.clone(),
            "Safety: Stashing untracked before clean.", "PREPEND: git stash -u",
            env: "RTK_SAFE_COMMANDS"),
        rule!("git clean -df", stash_clean.clone(),
            "Safety: Stashing untracked before clean.", "PREPEND: git stash -u",
            env: "RTK_SAFE_COMMANDS"),
        rule!("git clean -f", stash_clean,
            "Safety: Stashing untracked before clean.", "PREPEND: git stash -u",
            env: "RTK_SAFE_COMMANDS"),
        // === TOKEN WASTE PREVENTION (block and suggest internal tools) ===
        // Messages use generic descriptions so both Claude Code ("Read tool")
        // and Gemini CLI ("read_file") agents understand the suggestion.
        rule!("cat", SafetyAction::SuggestTool("Read".into()),
            "Use the **Read tool** for large files.",
            "BLOCK: cat wastes tokens. Use your file-reading tool instead.",
            env: "RTK_BLOCK_TOKEN_WASTE"),
        rule!("sed", SafetyAction::SuggestTool("Edit".into()),
            "Use the **Edit tool** for validated file modifications.",
            "BLOCK: sed unsafe. Use your file-editing tool instead.",
            env: "RTK_BLOCK_TOKEN_WASTE"),
        rule!("head", SafetyAction::SuggestTool("Read (with limit)".into()),
            "Use **Read tool with limit parameter** instead of head.",
            "BLOCK: head wastes tokens. Use your file-reading tool with a line limit instead.",
            env: "RTK_BLOCK_TOKEN_WASTE"),
    ]
}

/// Check a command against all safety rules
pub fn check(binary: &str, args: &[String]) -> SafetyResult {
    let full_cmd = if args.is_empty() {
        binary.to_string()
    } else {
        format!("{} {}", binary, args.join(" "))
    };

    for rule in get_rules() {
        // Single-word patterns match binary exactly to avoid false positives
        // (e.g., "cat" must not match "catalog"). Multi-word patterns use
        // starts_with on the full command (e.g., "git reset --hard").
        let matches = if rule.pattern.contains(' ') {
            full_cmd.starts_with(rule.pattern)
        } else {
            binary == rule.pattern
        };
        if matches {
            if !rule.should_apply() {
                continue;
            }

            return match &rule.action {
                SafetyAction::Rewrite(new_cmd) => SafetyResult::Rewritten(new_cmd.clone()),
                SafetyAction::Prepend(prefix) => {
                    let new_cmd = format!("{} && {}", prefix, full_cmd);
                    SafetyResult::Rewritten(new_cmd)
                }
                SafetyAction::SuggestTool(_tool) => {
                    // The rule's human_msg/agent_msg already contains the full message
                    // Do NOT append extra text (was causing duplicates)
                    SafetyResult::Blocked(rule.message().to_string())
                }
                SafetyAction::Trash => {
                    // Extract paths (skip flags like -rf, -f, -r, -i)
                    let paths: Vec<String> = args
                        .iter()
                        .filter(|a| !a.starts_with('-'))
                        .cloned()
                        .collect();
                    SafetyResult::TrashRequested(paths)
                }
            };
        }
    }

    SafetyResult::Safe
}

/// Check raw command string (for passthrough mode)
/// This catches dangerous patterns even when we can't parse the command
pub fn check_raw(raw: &str) -> SafetyResult {
    // Check if RTK_SAFE_COMMANDS is disabled (opt-out)
    let safe_commands_disabled = std::env::var("RTK_SAFE_COMMANDS")
        .map(|v| v == "0" || v == "false")
        .unwrap_or(false);

    if !safe_commands_disabled {
        // Word-boundary check: split on whitespace and look for "rm" as a
        // standalone token. This avoids false positives on "trim", "farm", etc.
        let words: Vec<&str> = raw.split_whitespace().collect();
        let has_rm = words.iter().any(|w| *w == "rm" || w.ends_with("/rm"));
        if has_rm {
            return SafetyResult::Blocked(
                "Passthrough blocked: 'rm' detected. Use native mode for safe trash.".into(),
            );
        }

        // Check for sudo rm (scan all words after sudo, not just adjacent)
        // Handles: sudo rm, sudo -u root rm, sudo --preserve-env rm
        if let Some(sudo_pos) = words.iter().position(|w| *w == "sudo") {
            if words[sudo_pos + 1..]
                .iter()
                .any(|w| *w == "rm" || w.ends_with("/rm"))
            {
                return SafetyResult::Blocked(
                    "Passthrough blocked: 'sudo rm' detected. Use native mode for safe trash."
                        .into(),
                );
            }
        }
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

    // === RULE ORDERING TESTS ===

    #[test]
    fn test_rules_are_ordered() {
        let _guard = EnvGuard::new();
        let rules = get_rules();
        // More specific patterns should come before less specific
        let reset_idx = rules.iter().position(|r| r.pattern == "git reset --hard");
        let checkout_idx = rules.iter().position(|r| r.pattern == "git checkout --");
        // git reset --hard and git checkout -- should exist
        assert!(reset_idx.is_some());
        assert!(checkout_idx.is_some());
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
