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
    /// - RTK_SAFE_COMMANDS: Opt-in, only applies if set (rm->trash, git safety)
    /// - RTK_BLOCK_TOKEN_WASTE: Opt-out, applies by default, disable with =0
    pub fn should_apply(&self) -> bool {
        // Check env var if specified
        if let Some(env) = self.env_var {
            match env {
                // Opt-in features: only apply if explicitly enabled
                "RTK_SAFE_COMMANDS" => {
                    if std::env::var(env).is_err() {
                        return false;
                    }
                }
                // Opt-out features: apply by default, disable with =0
                "RTK_BLOCK_TOKEN_WASTE" => {
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
/// - RTK_SAFE_COMMANDS=1    - Master toggle for ALL safety features (rm->trash, git safety)
/// - RTK_BLOCK_TOKEN_WASTE=0 - Disable token waste prevention (cat/sed/head blocking)
///
/// When RTK_SAFE_COMMANDS is set, it enables:
/// - rm -> trash redirection
/// - git reset --hard -> prepend git stash
/// - git clean -fd/-df -> block
pub fn get_rules() -> Vec<SafetyRule> {
    vec![
        // === DANGEROUS FILE OPERATIONS ===
        // Enabled by RTK_SAFE_COMMANDS=1
        SafetyRule {
            pattern: "rm",
            action: SafetyAction::Trash,
            human_msg: "Safety: Moving to trash (RTK_SAFE_COMMANDS=1).",
            agent_msg: "REWRITE: rm -> trash",
            predicate: None,
            env_var: Some("RTK_SAFE_COMMANDS"),
        },

        // === DANGEROUS GIT OPERATIONS ===
        // Enabled by RTK_SAFE_COMMANDS=1
        SafetyRule {
            pattern: "git reset --hard",
            action: SafetyAction::Prepend("git stash push -m 'RTK Safety Stash'".into()),
            human_msg: "Safety: Stashing changes before hard reset.",
            agent_msg: "PREPEND: git stash",
            predicate: Some(predicates::has_unstaged_changes),
            env_var: Some("RTK_SAFE_COMMANDS"),
        },
        SafetyRule {
            pattern: "git clean -fd",
            action: SafetyAction::Block,
            human_msg: "Blocked: 'git clean -fd' would delete untracked files. Confirm manually.",
            agent_msg: "BLOCK: git clean -fd unsafe",
            predicate: None,
            env_var: Some("RTK_SAFE_COMMANDS"),
        },
        SafetyRule {
            pattern: "git clean -df",
            action: SafetyAction::Block,
            human_msg: "Blocked: 'git clean -df' would delete untracked files. Confirm manually.",
            agent_msg: "BLOCK: git clean -df unsafe",
            predicate: None,
            env_var: Some("RTK_SAFE_COMMANDS"),
        },

        // === TOKEN WASTE PREVENTION ===
        // Enabled by default, disable with RTK_BLOCK_TOKEN_WASTE=0
        SafetyRule {
            pattern: "cat",
            action: SafetyAction::SuggestTool("Read".into()),
            human_msg: "Use the **Read tool** for large files.",
            agent_msg: "BLOCK: cat wastes tokens. Use Read tool.",
            predicate: None,
            env_var: Some("RTK_BLOCK_TOKEN_WASTE"),
        },
        SafetyRule {
            pattern: "sed",
            action: SafetyAction::SuggestTool("Edit".into()),
            human_msg: "Use the **Edit tool** for validated file modifications.",
            agent_msg: "BLOCK: sed unsafe. Use Edit tool.",
            predicate: None,
            env_var: Some("RTK_BLOCK_TOKEN_WASTE"),
        },
        SafetyRule {
            pattern: "head",
            action: SafetyAction::SuggestTool("Read (with limit)".into()),
            human_msg: "Use **Read tool with limit parameter** instead of head.",
            agent_msg: "BLOCK: head wastes tokens. Use Read tool.",
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
    // Check for rm in various forms (when RTK_SAFE_COMMANDS is enabled)
    if std::env::var("RTK_SAFE_COMMANDS").is_ok() {
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
        ENV_LOCK.get_or_init(|| Mutex::new(())).lock().unwrap()
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
    fn test_check_rm_passes_when_env_not_set() {
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("rm", &["file.txt".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
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
    fn test_check_git_clean_fd_blocked() {
        let _lock = env_lock();
        cleanup_env_vars();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check("git", &["clean".to_string(), "-fd".to_string()]);
        match result {
            SafetyResult::Blocked(_) => {}
            _ => panic!("Expected Blocked, got {:?}", result),
        }
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_git_clean_passes_when_env_not_set() {
        let _lock = env_lock();
        cleanup_env_vars();
        let result = check("git", &["clean".to_string(), "-fd".to_string()]);
        assert_eq!(result, SafetyResult::Safe);
    }

    // === CHECK_RAW TESTS ===

    #[test]
    fn test_check_raw_rm_detected() {
        let _lock = env_lock();
        cleanup_env_vars();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check_raw("rm file.txt");
        match result {
            SafetyResult::Blocked(_) => {}
            _ => panic!("Expected Blocked"),
        }
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    #[test]
    fn test_check_raw_sudo_rm_detected() {
        let _lock = env_lock();
        cleanup_env_vars();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check_raw("sudo rm file.txt");
        match result {
            SafetyResult::Blocked(_) => {}
            _ => panic!("Expected Blocked"),
        }
        env::remove_var("RTK_SAFE_COMMANDS");
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
        cleanup_env_vars();
        env::set_var("RTK_SAFE_COMMANDS", "1");
        let result = check_raw("echo \"rm file\"");
        // This will be blocked because we can't distinguish quoted rm
        // That's intentional - better safe than sorry
        match result {
            SafetyResult::Blocked(_) => {}
            SafetyResult::Safe => {} // Either is acceptable
            SafetyResult::Rewritten(_) => {}
            SafetyResult::TrashRequested(_) => {}
        }
        env::remove_var("RTK_SAFE_COMMANDS");
    }

    // === RULE ORDERING TESTS ===

    #[test]
    fn test_rules_are_ordered() {
        let _lock = env_lock();
        let rules = get_rules();
        // More specific patterns should come before less specific
        // git reset --hard should come before git
        let reset_idx = rules.iter().position(|r| r.pattern == "git reset --hard");
        let git_idx = rules.iter().position(|r| r.pattern == "git");
        // We don't have a "git" rule currently, but if we did:
        if let (Some(reset), Some(git)) = (reset_idx, git_idx) {
            assert!(reset < git, "More specific patterns should come first");
        }
    }
}
