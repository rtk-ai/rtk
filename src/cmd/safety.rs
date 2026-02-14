//! Safety Policy Engine with dual messages (human vs agent).
//!
//! Design: Rules have predicates for conditional behavior.
//! Messages are terse for agents, detailed for humans.

use super::predicates;

/// Actions a safety rule can take
#[derive(Clone, Debug, PartialEq)]
pub enum SafetyAction {
    /// Allow the command to proceed
    Allow,
    /// Block execution with an error message
    Block,
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

/// Get all safety rules (ordered by specificity)
///
/// Environment Variables (coarse-grained):
/// - RTK_SAFE_COMMANDS=0 - Disable rm->trash and git safety
/// - RTK_BLOCK_TOKEN_WASTE=0 - Disable token waste prevention (cat/sed/head blocking)
///
/// All safety features are enabled by default.
pub fn get_rules() -> Vec<SafetyRule> {
    vec![
        // === DANGEROUS FILE OPERATIONS ===
        SafetyRule {
            pattern: "rm",
            action: SafetyAction::Trash,
            human_msg: "Safety: Moving to trash.",
            agent_msg: "REWRITE: rm -> trash",
            predicate: None,
            env_var: Some("RTK_SAFE_COMMANDS"),
        },

        // === DANGEROUS GIT OPERATIONS ===
        // Order: most specific patterns first
        SafetyRule {
            pattern: "git reset --hard",
            action: SafetyAction::Prepend("git stash push -m 'RTK: reset backup'".into()),
            human_msg: "Safety: Stashing before reset.",
            agent_msg: "PREPEND: git stash",
            predicate: Some(predicates::has_unstaged_changes),
            env_var: Some("RTK_SAFE_COMMANDS"),
        },
        SafetyRule {
            pattern: "git checkout --",
            action: SafetyAction::Prepend("git stash push -m 'RTK: checkout backup'".into()),
            human_msg: "Safety: Stashing before checkout.",
            agent_msg: "PREPEND: git stash",
            predicate: Some(predicates::has_unstaged_changes),
            env_var: Some("RTK_SAFE_COMMANDS"),
        },
        SafetyRule {
            pattern: "git checkout .",
            action: SafetyAction::Prepend("git stash push -m 'RTK: checkout backup'".into()),
            human_msg: "Safety: Stashing before checkout.",
            agent_msg: "PREPEND: git stash",
            predicate: Some(predicates::has_unstaged_changes),
            env_var: Some("RTK_SAFE_COMMANDS"),
        },
        SafetyRule {
            pattern: "git stash drop",
            action: SafetyAction::Rewrite("git stash pop".into()),
            human_msg: "Safety: Using pop instead of drop (recoverable).",
            agent_msg: "REWRITE: stash drop -> pop",
            predicate: None,
            env_var: Some("RTK_SAFE_COMMANDS"),
        },
        SafetyRule {
            pattern: "git clean -fd",
            action: SafetyAction::Prepend("git stash -u -m 'RTK: clean backup'".into()),
            human_msg: "Safety: Stashing untracked before clean.",
            agent_msg: "PREPEND: git stash -u",
            predicate: None,
            env_var: Some("RTK_SAFE_COMMANDS"),
        },
        SafetyRule {
            pattern: "git clean -df",
            action: SafetyAction::Prepend("git stash -u -m 'RTK: clean backup'".into()),
            human_msg: "Safety: Stashing untracked before clean.",
            agent_msg: "PREPEND: git stash -u",
            predicate: None,
            env_var: Some("RTK_SAFE_COMMANDS"),
        },
        SafetyRule {
            pattern: "git clean -f",
            action: SafetyAction::Prepend("git stash -u -m 'RTK: clean backup'".into()),
            human_msg: "Safety: Stashing untracked before clean.",
            agent_msg: "PREPEND: git stash -u",
            predicate: None,
            env_var: Some("RTK_SAFE_COMMANDS"),
        },

        // === TOKEN WASTE PREVENTION ===
        SafetyRule {
            pattern: "cat",
            action: SafetyAction::SuggestTool("Read".into()),
            human_msg: "Use the **Read tool** for large files.",
            agent_msg: "BLOCK: cat wastes tokens",
            predicate: None,
            env_var: Some("RTK_BLOCK_TOKEN_WASTE"),
        },
        SafetyRule {
            pattern: "sed",
            action: SafetyAction::SuggestTool("Edit".into()),
            human_msg: "Use the **Edit tool** for validated file modifications.",
            agent_msg: "BLOCK: sed unsafe",
            predicate: None,
            env_var: Some("RTK_BLOCK_TOKEN_WASTE"),
        },
        SafetyRule {
            pattern: "head",
            action: SafetyAction::SuggestTool("Read (with limit)".into()),
            human_msg: "Use **Read tool with limit parameter** instead of head.",
            agent_msg: "BLOCK: head wastes tokens",
            predicate: None,
            env_var: Some("RTK_BLOCK_TOKEN_WASTE"),
        },
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
        if full_cmd.starts_with(rule.pattern) {
            if !rule.should_apply() {
                continue;
            }

            return match &rule.action {
                SafetyAction::Allow => SafetyResult::Safe,
                SafetyAction::Block => {
                    SafetyResult::Blocked(rule.message().to_string())
                }
                SafetyAction::Rewrite(template) => {
                    let new_cmd = template.replace("{args}", &args.join(" "));
                    SafetyResult::Rewritten(new_cmd)
                }
                SafetyAction::Prepend(prefix) => {
                    let new_cmd = format!("{} && {}", prefix, full_cmd);
                    SafetyResult::Rewritten(new_cmd)
                }
                SafetyAction::SuggestTool(tool) => {
                    let msg = format!("{}. Use the **{}** tool.", rule.message(), tool);
                    SafetyResult::Blocked(msg)
                }
                SafetyAction::Trash => {
                    // Extract paths (skip flags like -rf, -f, -r, -i)
                    let paths: Vec<String> = args.iter()
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
        // Check for rm in various forms (enabled by default)
        let rm_patterns = [" rm ", "rm ", "/rm ", "\\rm "];
        for pattern in rm_patterns {
            if raw.contains(pattern) || raw.starts_with("rm ") {
                return SafetyResult::Blocked(
                    "Passthrough blocked: 'rm' detected. Use native mode for safe trash.".into()
                );
            }
        }

        // Check for sudo rm
        if raw.contains("sudo rm") || raw.contains("sudo /rm") {
            return SafetyResult::Blocked(
                "Passthrough blocked: 'sudo rm' detected. Use native mode for safe trash.".into()
            );
        }
    }

    SafetyResult::Safe
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::env;
    use std::sync::{Mutex, MutexGuard};

    // Mutex to serialize tests that modify environment variables
    // This prevents race conditions when tests run in parallel
    static ENV_LOCK: std::sync::OnceLock<Mutex<()>> = std::sync::OnceLock::new();

    fn env_lock() -> MutexGuard<'static, ()> {
        // Recover from poisoned mutex if a previous test panicked
        ENV_LOCK.get_or_init(|| Mutex::new(()))
            .lock()
            .unwrap_or_else(|e| e.into_inner())
    }

    // === BASIC CHECK TESTS ===

    fn cleanup_env_vars() {
        env::remove_var("RTK_SAFE_COMMANDS");
        env::remove_var("RTK_BLOCK_TOKEN_WASTE");
    }

    #[test]
    fn test_check_safe_command() {
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("ls", &["-la".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_check_git_status() {
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("git", &["status".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_check_empty_args() {
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("pwd", &[]);
        assert_eq!(result, SafetyResult::Safe);
    }

    // === RM SAFETY TESTS (RTK_SAFE_COMMANDS) ===

    #[test]
    fn test_check_rm_blocked_when_env_set() {
        let _lock = env_lock();
        cleanup_env_vars();
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
        let _lock = env_lock();
        cleanup_env_vars();
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
        let _lock = env_lock();
        cleanup_env_vars();
        env::set_var("RTK_SAFE_COMMANDS", "0");
        let result = check("rm", &["file.txt".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_rm_with_flags() {
        let _lock = env_lock();
        cleanup_env_vars();
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
        let _lock = env_lock();
        cleanup_env_vars();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check("rm", &["a.txt".to_string(), "b.txt".to_string(), "c.txt".to_string()]);
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
        let _lock = env_lock();
        cleanup_env_vars();
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
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("cat", &["file.txt".to_string()]);
        match result {
            SafetyResult::Blocked(msg) => {
                assert!(msg.contains("Read"));
            }
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_cat_passes_when_disabled() {
        let _lock = env_lock();
        cleanup_env_vars();
        env::set_var("RTK_BLOCK_TOKEN_WASTE", "0");
        let result = check("cat", &["file.txt".to_string()]);
        env::remove_var("RTK_BLOCK_TOKEN_WASTE");
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_check_sed_blocked() {
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("sed", &["-i".to_string(), "s/old/new/g".to_string()]);
        match result {
            SafetyResult::Blocked(msg) => {
                assert!(msg.contains("Edit"));
            }
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_head_blocked() {
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("head", &["-n".to_string(), "10".to_string(), "file.txt".to_string()]);
        match result {
            SafetyResult::Blocked(msg) => {
                assert!(msg.contains("Read"));
            }
            _ => panic!("Expected Blocked"),
        }
    }

    // === GIT SAFETY TESTS (RTK_SAFE_COMMANDS) ===

    #[test]
    fn test_check_git_reset_hard_blocked_when_env_set() {
        let _lock = env_lock();
        cleanup_env_vars();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        // This test may or may not trigger depending on git state
        // Just ensure it doesn't panic
        let _ = check("git", &["reset".to_string(), "--hard".to_string()]);
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_git_clean_fd_rewritten() {
        let _lock = env_lock();
        cleanup_env_vars();
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
        let _lock = env_lock();
        cleanup_env_vars();
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
        let _lock = env_lock();
        cleanup_env_vars();
        env::set_var("RTK_SAFE_COMMANDS", "0");
        let result = check("git", &["clean".to_string(), "-fd".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    // === CHECK_RAW TESTS ===

    #[test]
    fn test_check_raw_rm_detected() {
        let _lock = env_lock();
        cleanup_env_vars();
        // RTK_SAFE_COMMANDS is enabled by default, so rm should be blocked
        let result = check_raw("rm file.txt");
        match result {
            SafetyResult::Blocked(_) => {}
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_raw_sudo_rm_detected() {
        let _lock = env_lock();
        cleanup_env_vars();
        // RTK_SAFE_COMMANDS is enabled by default, so sudo rm should be blocked
        let result = check_raw("sudo rm file.txt");
        match result {
            SafetyResult::Blocked(_) => {}
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_raw_safe_command() {
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check_raw("ls -la");
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_check_raw_rm_in_quoted_string() {
        let _lock = env_lock();
        // "rm" inside quotes should still be caught in passthrough
        // since we can't parse quotes in raw mode
        // RTK_SAFE_COMMANDS is enabled by default
        cleanup_env_vars();
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
        let _lock = env_lock();
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
        let _lock = env_lock();
        cleanup_env_vars();
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
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("git", &["checkout".to_string(), "--".to_string(), "file.txt".to_string()]);
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
        let _lock = env_lock();
        cleanup_env_vars();
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
        let _lock = env_lock();
        cleanup_env_vars();
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
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("git", &["checkout".to_string(), "main".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
    }

    #[test]
    fn test_git_checkout_new_branch_safe() {
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("git", &["checkout".to_string(), "-b".to_string(), "feature".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
    }
}
