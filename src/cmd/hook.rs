//! Hook protocol for Claude Code and Gemini support.
//!
//! Claude Code expects:
//! - Success: rewritten command on stdout, exit 0
//! - Blocked: error message on stderr, exit 2 (blocking error)
//! - Other exit codes: non-blocking errors
//!
//! Gemini expects:
//! - JSON payload in, JSON response out (see gemini_hook module)

use super::{lexer, analysis, safety};

/// Hook check result
#[derive(Debug, Clone)]
pub enum HookResult {
    /// Command is safe, rewrite to this
    Rewrite(String),
    /// Command is blocked with this message
    Blocked(String),
}

/// Maximum rewrite depth to prevent infinite recursion from cyclic safety rules.
const MAX_REWRITE_DEPTH: usize = 3;

/// Check a command for the hook protocol.
/// Returns the rewritten command or an error message.
///
/// The `_agent` parameter is reserved for future per-agent behavior.
pub fn check_for_hook(raw: &str, _agent: &str) -> HookResult {
    check_for_hook_inner(raw, 0)
}

fn check_for_hook_inner(raw: &str, depth: usize) -> HookResult {
    if depth >= MAX_REWRITE_DEPTH {
        return HookResult::Blocked(
            "Safety rewrite loop detected (max depth exceeded)".to_string()
        );
    }

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
                        return check_for_hook_inner(&new_cmd, depth + 1);
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

#[cfg(test)]
mod tests {
    use super::*;

    // === TEST HELPERS ===

    fn assert_rewrite(input: &str, contains: &str) {
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => assert!(cmd.contains(contains),
                "'{}' rewrite should contain '{}', got '{}'", input, contains, cmd),
            other => panic!("Expected Rewrite for '{}', got {:?}", input, other),
        }
    }

    fn assert_blocked(input: &str, contains: &str) {
        match check_for_hook(input, "claude") {
            HookResult::Blocked(msg) => assert!(msg.contains(contains),
                "'{}' block msg should contain '{}', got '{}'", input, contains, msg),
            other => panic!("Expected Blocked for '{}', got {:?}", input, other),
        }
    }

    // === ESCAPE_QUOTES ===

    #[test]
    fn test_escape_quotes() {
        assert_eq!(escape_quotes("hello"), "hello");
        assert_eq!(escape_quotes("it's"), "it'\\''s");
        assert_eq!(escape_quotes("it's a test's"), "it'\\''s a test'\\''s");
    }

    // === EMPTY / WHITESPACE ===

    #[test]
    fn test_check_empty_and_whitespace() {
        match check_for_hook("", "claude") {
            HookResult::Rewrite(cmd) => assert!(cmd.is_empty()),
            _ => panic!("Expected Rewrite for empty"),
        }
        match check_for_hook("   ", "claude") {
            HookResult::Rewrite(cmd) => assert!(cmd.trim().is_empty()),
            _ => panic!("Expected Rewrite for whitespace"),
        }
    }

    // === COMMANDS THAT SHOULD REWRITE (table-driven) ===

    #[test]
    fn test_safe_commands_rewrite() {
        let cases = [
            ("git status", "rtk run"),
            ("ls *.rs", "rtk run"),                          // shellism passthrough
            (r#"git commit -m "Fix && Bug""#, "rtk run"),    // quoted operator
            ("FOO=bar echo hello", "rtk run"),               // env prefix
            ("echo `date`", "rtk run"),                      // backticks
            ("echo $(date)", "rtk run"),                     // subshell
            ("echo {a,b}.txt", "rtk run"),                   // brace expansion
            ("echo 'hello!@#$%^&*()'", "rtk run"),          // special chars
            ("echo '日本語 🎉'", "rtk run"),                   // unicode
        ];
        for (input, expected) in cases {
            assert_rewrite(input, expected);
        }
    }

    #[test]
    fn test_chain_rewrite() {
        let result = check_for_hook("cd /tmp && git status", "claude");
        match result {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"));
                assert!(cmd.contains("&&"));
            }
            _ => panic!("Expected Rewrite for chain"),
        }
    }

    #[test]
    fn test_very_long_command() {
        let long_arg = "a".repeat(1000);
        assert_rewrite(&format!("echo {}", long_arg), "rtk run");
    }

    // === COMMANDS THAT SHOULD BLOCK (table-driven) ===

    #[test]
    fn test_blocked_commands() {
        let cases = [
            ("cat file.txt", "Read"),
            ("sed -i 's/old/new/' file.txt", "Edit"),
            ("head -n 10 file.txt", "Read"),
            ("cd /tmp && cat file.txt", "Read"),             // cat in chain
        ];
        for (input, expected_msg) in cases {
            assert_blocked(input, expected_msg);
        }
    }

    // === SHELLISM PASSTHROUGH: cat/sed/head allowed with pipe/redirect ===

    #[test]
    fn test_token_waste_allowed_in_pipelines() {
        let cases = [
            "cat file.txt | grep pattern",
            "cat file.txt > output.txt",
            "sed 's/old/new/' file.txt > output.txt",
            "head -n 10 file.txt | grep pattern",
            "for f in *.txt; do cat \"$f\" | grep x; done",
        ];
        for input in cases {
            assert_rewrite(input, "rtk run");
        }
    }

    // === MULTI-AGENT ===

    #[test]
    fn test_different_agents_same_result() {
        for agent in ["claude", "gemini"] {
            match check_for_hook("git status", agent) {
                HookResult::Rewrite(cmd) => assert!(cmd.contains("rtk run")),
                _ => panic!("Expected Rewrite for agent '{}'", agent),
            }
        }
    }

    // === FORMAT_FOR_CLAUDE ===

    #[test]
    fn test_format_for_claude() {
        let (output, success, code) = format_for_claude(
            HookResult::Rewrite("rtk run -c 'git status'".to_string()));
        assert_eq!(output, "rtk run -c 'git status'");
        assert!(success);
        assert_eq!(code, 0);

        let (output, success, code) = format_for_claude(
            HookResult::Blocked("Error message".to_string()));
        assert_eq!(output, "Error message");
        assert!(!success);
        assert_eq!(code, 2);  // Exit 2 = blocking error per Claude Code spec
    }

    // === RECURSION DEPTH LIMIT ===

    #[test]
    fn test_rewrite_depth_limit() {
        // At max depth → blocked
        match check_for_hook_inner("echo hello", MAX_REWRITE_DEPTH) {
            HookResult::Blocked(msg) => assert!(msg.contains("loop"), "msg: {}", msg),
            _ => panic!("Expected Blocked at max depth"),
        }
        // At depth 0 → normal rewrite
        match check_for_hook_inner("echo hello", 0) {
            HookResult::Rewrite(cmd) => assert!(cmd.contains("rtk run")),
            _ => panic!("Expected Rewrite at depth 0"),
        }
    }

    // =========================================================================
    // CLAUDE CODE WIRE FORMAT CONFORMANCE
    // https://docs.anthropic.com/en/docs/claude-code/hooks
    //
    // Claude Code hook protocol:
    // - Rewrite: command on stdout, exit code 0
    // - Block: message on stderr, exit code 2
    // - Other exit codes are non-blocking errors
    //
    // format_for_claude() is the boundary between HookResult and the wire.
    // These tests verify it produces the exact contract Claude Code expects.
    // =========================================================================

    #[test]
    fn test_claude_rewrite_exit_code_is_zero() {
        let (_, _, code) = format_for_claude(HookResult::Rewrite("rtk run -c 'ls'".into()));
        assert_eq!(code, 0, "Rewrite must exit 0 (success)");
    }

    #[test]
    fn test_claude_block_exit_code_is_two() {
        let (_, _, code) = format_for_claude(HookResult::Blocked("denied".into()));
        assert_eq!(code, 2, "Block must exit 2 (blocking error per Claude Code spec)");
    }

    #[test]
    fn test_claude_rewrite_output_is_command_text() {
        // Claude Code reads stdout as the rewritten command — must be plain text, not JSON
        let (output, success, _) = format_for_claude(
            HookResult::Rewrite("rtk run -c 'git status'".into()));
        assert_eq!(output, "rtk run -c 'git status'");
        assert!(success);
        // Must NOT be JSON
        assert!(!output.starts_with('{'), "Rewrite output must be plain text, not JSON");
    }

    #[test]
    fn test_claude_block_output_is_human_message() {
        // Claude Code reads stderr for the block reason
        let (output, success, _) = format_for_claude(
            HookResult::Blocked("Use Read tool instead".into()));
        assert_eq!(output, "Use Read tool instead");
        assert!(!success);
        // Must NOT be JSON
        assert!(!output.starts_with('{'), "Block output must be plain text, not JSON");
    }

    #[test]
    fn test_claude_rewrite_success_flag_true() {
        let (_, success, _) = format_for_claude(HookResult::Rewrite("cmd".into()));
        assert!(success, "Rewrite must set success=true");
    }

    #[test]
    fn test_claude_block_success_flag_false() {
        let (_, success, _) = format_for_claude(HookResult::Blocked("msg".into()));
        assert!(!success, "Block must set success=false");
    }

    #[test]
    fn test_claude_exit_codes_not_one() {
        // Exit code 1 means non-blocking error in Claude Code — we must never use it
        let (_, _, rewrite_code) = format_for_claude(HookResult::Rewrite("cmd".into()));
        let (_, _, block_code) = format_for_claude(HookResult::Blocked("msg".into()));
        assert_ne!(rewrite_code, 1, "Exit code 1 is non-blocking error, not valid for rewrite");
        assert_ne!(block_code, 1, "Exit code 1 is non-blocking error, not valid for block");
    }

    // === CROSS-PROTOCOL: Same decision for both agents ===

    #[test]
    fn test_cross_protocol_safe_command_allowed_by_both() {
        // Both Claude and Gemini must allow the same safe commands
        for cmd in ["git status", "cargo test", "ls -la", "echo hello"] {
            let claude = check_for_hook(cmd, "claude");
            let gemini = check_for_hook(cmd, "gemini");
            match (&claude, &gemini) {
                (HookResult::Rewrite(_), HookResult::Rewrite(_)) => {}
                _ => panic!("'{}': Claude={:?}, Gemini={:?} — both should Rewrite", cmd, claude, gemini),
            }
        }
    }

    #[test]
    fn test_cross_protocol_blocked_command_denied_by_both() {
        // Both Claude and Gemini must block the same unsafe commands
        for cmd in ["cat file.txt", "head -n 10 file.txt"] {
            let claude = check_for_hook(cmd, "claude");
            let gemini = check_for_hook(cmd, "gemini");
            match (&claude, &gemini) {
                (HookResult::Blocked(_), HookResult::Blocked(_)) => {}
                _ => panic!("'{}': Claude={:?}, Gemini={:?} — both should Block", cmd, claude, gemini),
            }
        }
    }
}
