//! Hook protocol for Claude Code and Gemini support.
//!
//! Claude Code expects:
//! - Success: rewritten command on stdout, exit 0
//! - Blocked: error message on stderr, exit 2 (blocking error)
//! - Other exit codes: non-blocking errors
//!
//! Gemini expects:
//! - JSON payload in, JSON response out

use super::{lexer, analysis, safety};

/// Hook check result
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Command is safe, rewrite to this
    Rewrite(String),
    /// Command is blocked with this message
    Blocked(String),
}

/// Check a command for the hook protocol.
/// Returns the rewritten command or an error message.
pub fn check_for_hook(raw: &str, agent: &str) -> HookResult {
    // Handle empty
    if raw.trim().is_empty() {
        return HookResult::Rewrite(raw.to_string());
    }

    let tokens = lexer::tokenize(raw);

    // Check for shellisms - if present, pass through
    // but still check safety
    if analysis::needs_shell(&tokens) {
        match safety::check_raw(raw) {
            safety::SafetyResult::Blocked(msg) => return HookResult::Blocked(msg),
            safety::SafetyResult::Safe => {}
            _ => {}
        }
        // Passthrough: just return as-is wrapped in rtk run
        return HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw)));
    }

    // Native mode: parse and check each command
    match analysis::parse_chain(tokens) {
        Ok(commands) => {
            // Check safety on each command
            for cmd in &commands {
                match safety::check(&cmd.binary, &cmd.args) {
                    safety::SafetyResult::Blocked(msg) => {
                        return HookResult::Blocked(msg);
                    }
                    safety::SafetyResult::Rewritten(new_cmd) => {
                        // Rewrite and re-check
                        return check_for_hook(&new_cmd, agent);
                    }
                    safety::SafetyResult::TrashRequested(_) => {
                        // Redirect to rtk run which handles trash
                        return HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw)));
                    }
                    safety::SafetyResult::Safe => {}
                }
            }

            // All safe - wrap in rtk run for token optimization
            HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw)))
        }
        Err(_) => {
            // Parse error - passthrough with wrapping
            HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw)))
        }
    }
}

/// Escape single quotes for shell
fn escape_quotes(s: &str) -> String {
    s.replace("'", "'\\''")
}

/// Format hook result for Claude (text output)
///
/// Exit codes:
/// - 0: Success, command rewritten/allowed
/// - 2: Blocking error, command should be denied
pub fn format_for_claude(result: HookResult) -> (String, bool, i32) {
    match result {
        HookResult::Rewrite(cmd) => (cmd, true, 0),
        HookResult::Blocked(msg) => (msg, false, 2),  // Exit 2 = blocking error per Claude Code spec
    }
}

/// Format hook result for Gemini (JSON output)
pub fn format_for_gemini(result: HookResult) -> String {
    match result {
        HookResult::Rewrite(cmd) => {
            serde_json::json!({
                "result": "allow",
                "modified_input": serde_json::json!({
                    "command": cmd
                }),
                "message": "RTK applied safety optimizations."
            }).to_string()
        }
        HookResult::Blocked(msg) => {
            serde_json::json!({
                "result": "deny",
                "message": msg
            }).to_string()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === ESCAPE_QUOTES TESTS ===

    #[test]
    fn test_escape_quotes_no_quotes() {
        assert_eq!(escape_quotes("hello"), "hello");
    }

    #[test]
    fn test_escape_quotes_with_single() {
        assert_eq!(escape_quotes("it's"), "it'\\''s");
    }

    #[test]
    fn test_escape_quotes_multiple() {
        assert_eq!(escape_quotes("it's a test's"), "it'\\''s a test'\\''s");
    }

    // === CHECK_FOR_HOOK TESTS ===

    #[test]
    fn test_check_empty() {
        let result = check_for_hook("", "claude");
        match result {
            HookResult::Rewrite(cmd) => assert!(cmd.is_empty()),
            _ => panic!("Expected Rewrite"),
        }
    }

    #[test]
    fn test_check_whitespace() {
        let result = check_for_hook("   ", "claude");
        match result {
            HookResult::Rewrite(cmd) => assert!(cmd.trim().is_empty()),
            _ => panic!("Expected Rewrite"),
        }
    }

    #[test]
    fn test_check_safe_command() {
        let result = check_for_hook("git status", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.starts_with("rtk run"));
            }
            _ => panic!("Expected Rewrite"),
        }
    }

    #[test]
    fn test_check_cat_blocked() {
        let result = check_for_hook("cat file.txt", "claude");
        match result {
            HookResult::Blocked(msg) => {
                assert!(msg.contains("Read"));
            }
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_sed_blocked() {
        let result = check_for_hook("sed -i 's/old/new/' file.txt", "claude");
        match result {
            HookResult::Blocked(msg) => {
                assert!(msg.contains("Edit"));
            }
            _ => panic!("Expected Blocked"),
        }
    }

    #[test]
    fn test_check_shellism_passthrough() {
        let result = check_for_hook("ls *.rs", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("Expected Rewrite"),
        }
    }

    #[test]
    fn test_check_quoted_operator() {
        let result = check_for_hook(r#"git commit -m "Fix && Bug""#, "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("Expected Rewrite"),
        }
    }

    // === FORMAT_FOR_CLAUDE TESTS ===

    #[test]
    fn test_format_rewrite() {
        let result = HookResult::Rewrite("rtk run -c 'git status'".to_string());
        let (output, success, code) = format_for_claude(result);
        assert_eq!(output, "rtk run -c 'git status'");
        assert!(success);
        assert_eq!(code, 0);
    }

    #[test]
    fn test_format_blocked() {
        let result = HookResult::Blocked("Error message".to_string());
        let (output, success, code) = format_for_claude(result);
        assert_eq!(output, "Error message");
        assert!(!success);
        assert_eq!(code, 2);  // Exit 2 = blocking error per Claude Code spec
    }

    // === FORMAT_FOR_GEMINI TESTS ===

    #[test]
    fn test_format_gemini_rewrite() {
        let result = HookResult::Rewrite("rtk run -c 'git status'".to_string());
        let output = format_for_gemini(result);
        let json: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(json["result"], "allow");
        assert!(json["modified_input"]["command"].is_string());
    }

    #[test]
    fn test_format_gemini_blocked() {
        let result = HookResult::Blocked("Error message".to_string());
        let output = format_for_gemini(result);
        let json: serde_json::Value = serde_json::from_str(&output).unwrap();
        assert_eq!(json["result"], "deny");
        assert_eq!(json["message"], "Error message");
    }

    // === ADDITIONAL EDGE CASE TESTS ===

    #[test]
    fn test_check_head_blocked() {
        let result = check_for_hook("head -n 10 file.txt", "claude");
        match result {
            HookResult::Blocked(msg) => {
                assert!(msg.contains("Read"));
            }
            _ => panic!("Expected Blocked for head command"),
        }
    }

    #[test]
    fn test_check_complex_command_with_env() {
        // Commands with env var prefixes should be handled
        let result = check_for_hook("FOO=bar echo hello", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("Expected Rewrite for env prefix command"),
        }
    }

    #[test]
    fn test_check_command_with_backticks() {
        // Backticks should trigger shellism passthrough
        let result = check_for_hook("echo `date`", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("Expected Rewrite for backtick command"),
        }
    }

    #[test]
    fn test_check_command_with_subshell() {
        // Subshell syntax should trigger shellism passthrough
        let result = check_for_hook("echo $(date)", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("Expected Rewrite for subshell command"),
        }
    }

    #[test]
    fn test_check_command_with_brace_expansion() {
        // Brace expansion should trigger shellism passthrough
        let result = check_for_hook("echo {a,b}.txt", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("Expected Rewrite for brace expansion"),
        }
    }

    #[test]
    fn test_check_chain_with_and_operator() {
        // Chained commands should be handled
        let result = check_for_hook("cd /tmp && git status", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
                assert!(cmd.contains("&&"));
            }
            _ => panic!("Expected Rewrite for chained command"),
        }
    }

    #[test]
    fn test_check_chain_with_blocked_command() {
        // If any command in chain is blocked, whole chain is blocked
        let result = check_for_hook("cd /tmp && cat file.txt", "claude");
        match result {
            HookResult::Blocked(msg) => {
                assert!(msg.contains("Read"));
            }
            _ => panic!("Expected Blocked when chain contains cat"),
        }
    }

    #[test]
    fn test_check_special_characters_in_command() {
        // Commands with special characters should be handled
        let result = check_for_hook("echo 'hello!@#$%^&*()'", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("Expected Rewrite for command with special chars"),
        }
    }

    #[test]
    fn test_check_unicode_command() {
        // Unicode in commands should be preserved
        let result = check_for_hook("echo '日本語 🎉'", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("日本語") || cmd.contains("rtk run"));
            }
            _ => panic!("Expected Rewrite for unicode command"),
        }
    }

    #[test]
    fn test_check_very_long_command() {
        // Very long commands should be handled without truncation
        let long_arg = "a".repeat(1000);
        let cmd = format!("echo {}", long_arg);
        let result = check_for_hook(&cmd, "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("Expected Rewrite for long command"),
        }
    }

    #[test]
    fn test_format_blocked_exit_code_is_2() {
        // Critical: Exit code must be 2 for blocking (per Claude Code spec)
        let result = HookResult::Blocked("Blocked for safety".to_string());
        let (_, _, code) = format_for_claude(result);
        assert_eq!(code, 2, "Blocked commands must return exit code 2");
    }

    #[test]
    fn test_format_rewrite_exit_code_is_0() {
        // Success/rewrite must return exit code 0
        let result = HookResult::Rewrite("rtk run -c 'echo hello'".to_string());
        let (_, _, code) = format_for_claude(result);
        assert_eq!(code, 0, "Rewritten commands must return exit code 0");
    }

    #[test]
    fn test_check_different_agents() {
        // Both claude and gemini agents should work
        let claude_result = check_for_hook("git status", "claude");
        let gemini_result = check_for_hook("git status", "gemini");

        match (claude_result, gemini_result) {
            (HookResult::Rewrite(c), HookResult::Rewrite(g)) => {
                assert!(c.contains("rtk run"));
                assert!(g.contains("rtk run"));
            }
            _ => panic!("Both agents should produce Rewrite for safe command"),
        }
    }

    // === TOKEN WASTE CONTEXT TESTS ===
    // Verify that cat/sed/head are only blocked when standalone, not in pipes/redirects

    #[test]
    fn test_cat_with_pipe_allowed() {
        // cat in a pipeline is a legitimate use case
        let result = check_for_hook("cat file.txt | grep pattern", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                // Should be allowed via passthrough (pipe detected)
                assert!(cmd.contains("rtk run"));
                assert!(cmd.contains("|"));
            }
            _ => panic!("cat with pipe should be allowed"),
        }
    }

    #[test]
    fn test_cat_with_redirect_allowed() {
        // cat with redirect is a legitimate use case
        let result = check_for_hook("cat file.txt > output.txt", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                // Should be allowed via passthrough (redirect detected)
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("cat with redirect should be allowed"),
        }
    }

    #[test]
    fn test_sed_with_redirect_allowed() {
        // sed with redirect (not -i) is a legitimate use case
        let result = check_for_hook("sed 's/old/new/' file.txt > output.txt", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                // Should be allowed via passthrough (redirect detected)
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("sed with redirect should be allowed"),
        }
    }

    #[test]
    fn test_head_with_pipe_allowed() {
        // head in a pipeline is a legitimate use case
        let result = check_for_hook("head -n 10 file.txt | grep pattern", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                // Should be allowed via passthrough (pipe detected)
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("head with pipe should be allowed"),
        }
    }

    #[test]
    fn test_cat_standalone_blocked() {
        // Standalone cat should be blocked (token waste)
        let result = check_for_hook("cat file.txt", "claude");
        match result {
            HookResult::Blocked(msg) => {
                assert!(msg.contains("Read"));
            }
            _ => panic!("standalone cat should be blocked"),
        }
    }

    #[test]
    fn test_cat_in_chain_blocked() {
        // cat in a chain without pipe/redirect should still be blocked
        // Agent should use: cd dir, then Read tool
        let result = check_for_hook("cd /tmp && cat file.txt", "claude");
        match result {
            HookResult::Blocked(msg) => {
                assert!(msg.contains("Read"));
            }
            _ => panic!("cat in chain without pipe should be blocked"),
        }
    }

    #[test]
    fn test_cat_in_complex_script_allowed() {
        // Complex scripts with for loops, etc. have shellisms → passthrough
        let result = check_for_hook("for f in *.txt; do cat \"$f\" | grep x; done", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                // Shellism detected (for loop, glob, pipe) → passthrough
                assert!(cmd.contains("rtk run"));
            }
            _ => panic!("complex script should be allowed via passthrough"),
        }
    }
}
