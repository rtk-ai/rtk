//! Hook protocol for Claude Code and Gemini support.
//!
//! Claude Code expects:
//! - Success: rewritten command on stdout, exit 0
//! - Blocked: error message on stderr, exit 1
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
pub fn format_for_claude(result: HookResult) -> (String, bool, i32) {
    match result {
        HookResult::Rewrite(cmd) => (cmd, true, 0),
        HookResult::Blocked(msg) => (msg, false, 1),
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
        assert_eq!(code, 1);
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
}
