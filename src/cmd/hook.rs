//! Hook protocol for Claude Code and Gemini support.
//!
//! This module provides **shared decision logic** for both Claude Code and Gemini CLI hooks.
//! Protocol-specific I/O handling lives in `claude_hook.rs` and `gemini_hook.rs`.
//!
//! ## Architecture: Separation of Concerns
//!
//! ```text
//! main.rs (CAN use println! - normal RTK behavior)
//!    ↓
//! Commands::Hook match
//!    ├─→ HookCommands::Check → hook::check_for_hook() (THIS MODULE - CAN use println!)
//!    ├─→ HookCommands::Claude → claude_hook::run() [DENY ENFORCED - see claude_hook.rs:52]
//!    └─→ HookCommands::Gemini → gemini_hook::run() [DENY ENFORCED - see gemini_hook.rs:42]
//! ```
//!
//! **I/O Policy Scope:**
//! - **This module (hook.rs)**: CAN use `println!`/`eprintln!` (used by `rtk hook check` text protocol)
//! - **main.rs and all command modules**: CAN use `println!`/`eprintln!` (normal RTK behavior)
//! - **claude_hook.rs, gemini_hook.rs ONLY**: CANNOT use `println!`/`eprintln!` (JSON protocols)
//!
//! The `#![deny(clippy::print_stdout, clippy::print_stderr)]` attribute is applied
//! at the **module boundary** (earliest possible stage) — when control enters
//! `claude_hook::run()` or `gemini_hook::run()`, the deny is enforced.
//!
//! ## Protocol Differences
//!
//! **Claude Code** (`rtk hook check` text protocol):
//! - Success: rewritten command on stdout, exit 0
//! - Blocked: error message on stderr, exit 2 (blocking error)
//! - Other exit codes: non-blocking errors
//!
//! **Claude Code** (JSON protocol via `claude_hook.rs`):
//! - See `claude_hook.rs` module documentation
//!
//! **Gemini CLI** (JSON protocol via `gemini_hook.rs`):
//! - See `gemini_hook.rs` module documentation

use super::{analysis, lexer};
// PR 2 adds: use super::safety;

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
        return HookResult::Blocked("Rewrite loop detected (max depth exceeded)".to_string());
    }
    if raw.trim().is_empty() {
        return HookResult::Rewrite(raw.to_string());
    }
    // PR 2 adds: crate::config::rules::try_remap() alias expansion
    // PR 2 adds: safety::check_raw() and safety::check() dispatch
    HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw)))
}

// --- Shared guard logic (used by both claude_hook.rs and gemini_hook.rs) ---

/// Check if hook processing is disabled by environment.
///
/// Returns true if:
/// - `RTK_HOOK_ENABLED=0` (master toggle off)
/// - `RTK_ACTIVE` is set (recursion prevention — rtk sets this when running commands)
pub fn is_hook_disabled() -> bool {
    std::env::var("RTK_HOOK_ENABLED").as_deref() == Ok("0") || std::env::var("RTK_ACTIVE").is_ok()
}

/// Check if this command should bypass hook processing entirely.
///
/// Returns true for commands that should not be rewritten:
/// - Already routed through rtk (`rtk ...` or `/path/to/rtk ...`)
/// - Contains heredoc (`<<`) which needs raw shell processing
pub fn should_passthrough(cmd: &str) -> bool {
    cmd.starts_with("rtk ") || cmd.contains("/rtk ") || cmd.contains("<<")
}

/// Replace the command field in a tool_input object, preserving other fields.
///
/// Used by both claude_hook.rs and gemini_hook.rs when rewriting commands.
/// If tool_input is None or not an object, creates a new object with just the command.
///
/// # Arguments
/// * `tool_input` - The original tool_input from the hook payload (may be None)
/// * `new_cmd` - The rewritten command string to replace with
///
/// # Returns
/// A Value with the command field updated, all other fields preserved.
pub fn update_command_in_tool_input(
    tool_input: Option<serde_json::Value>,
    new_cmd: String,
) -> serde_json::Value {
    use serde_json::Value;
    let mut updated = tool_input.unwrap_or_else(|| Value::Object(Default::default()));
    if let Some(obj) = updated.as_object_mut() {
        obj.insert("command".into(), Value::String(new_cmd));
    }
    updated
}

/// Hook output for protocol handlers (claude_hook.rs, gemini_hook.rs).
///
/// This enum separates decision logic from I/O: `run_inner()` returns a
/// `HookResponse`, and `run()` is the single place that writes to stdout/stderr.
/// Combined with `#[deny(clippy::print_stdout, clippy::print_stderr)]` on the
/// hook modules, this prevents any stray output from corrupting the JSON protocol.
#[derive(Debug, Clone, PartialEq)]
pub enum HookResponse {
    /// No opinion — exit 0, no output. Host proceeds normally.
    NoOpinion,
    /// Allow/rewrite — exit 0, JSON to stdout.
    Allow(String),
    /// Deny — exit 2, JSON to stdout + reason to stderr.
    /// Fields: (stdout_json, stderr_reason)
    Deny(String, String),
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
        HookResult::Blocked(msg) => (msg, false, 2), // Exit 2 = blocking error per Claude Code spec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // === TEST HELPERS ===

    fn assert_rewrite(input: &str, contains: &str) {
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => assert!(
                cmd.contains(contains),
                "'{}' rewrite should contain '{}', got '{}'",
                input,
                contains,
                cmd
            ),
            other => panic!("Expected Rewrite for '{}', got {:?}", input, other),
        }
    }

    fn assert_blocked(input: &str, contains: &str) {
        match check_for_hook(input, "claude") {
            HookResult::Blocked(msg) => assert!(
                msg.contains(contains),
                "'{}' block msg should contain '{}', got '{}'",
                input,
                contains,
                msg
            ),
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
            ("ls *.rs", "rtk run"), // shellism passthrough
            (r#"git commit -m "Fix && Bug""#, "rtk run"), // quoted operator
            ("FOO=bar echo hello", "rtk run"), // env prefix
            ("echo `date`", "rtk run"), // backticks
            ("echo $(date)", "rtk run"), // subshell
            ("echo {a,b}.txt", "rtk run"), // brace expansion
            ("echo 'hello!@#$%^&*()'", "rtk run"), // special chars
            ("echo '日本語 🎉'", "rtk run"), // unicode
            ("cd /tmp && git status", "rtk run"), // chain rewrite
        ];
        for (input, expected) in cases {
            assert_rewrite(input, expected);
        }
        // Chain rewrite preserves operator structure
        match check_for_hook("cd /tmp && git status", "claude") {
            HookResult::Rewrite(cmd) => assert!(
                cmd.contains("&&"),
                "Chain rewrite must preserve '&&', got '{}'",
                cmd
            ),
            other => panic!("Expected Rewrite for chain, got {:?}", other),
        }
        // Very long command
        assert_rewrite(&format!("echo {}", "a".repeat(1000)), "rtk run");
    }

    // === ENV VAR PREFIX PRESERVATION ===
    // Ported from old hooks/test-rtk-rewrite.sh Section 2.
    // Commands prefixed with KEY=VALUE env vars must not be blocked.

    #[test]
    fn test_env_var_prefix_preserved() {
        let cases = [
            "GIT_PAGER=cat git status",
            "GIT_PAGER=cat git log --oneline -10",
            "NODE_ENV=test CI=1 npx vitest run",
            "LANG=C ls -la",
            "NODE_ENV=test npm run test:e2e",
            "COMPOSE_PROJECT_NAME=test docker compose up -d",
            "TEST_SESSION_ID=2 npx playwright test --config=foo",
        ];
        for input in cases {
            assert_rewrite(input, "rtk run");
        }
    }

    // === GLOBAL OPTIONS (PR #99 parity) ===
    // Commands with global options before subcommands must not be blocked.
    // Ported from upstream hooks/rtk-rewrite.sh global option stripping.

    #[test]
    fn test_global_options_not_blocked() {
        let cases = [
            // Git global options
            "git --no-pager status",
            "git -C /path/to/project status",
            "git -C /path --no-pager log --oneline",
            "git --no-optional-locks diff HEAD",
            "git --bare log",
            // Cargo toolchain prefix
            "cargo +nightly test",
            "cargo +stable build --release",
            // Docker global options
            "docker --context prod ps",
            "docker -H tcp://host:2375 images",
            // Kubectl global options
            "kubectl -n kube-system get pods",
            "kubectl --context prod describe pod foo",
        ];
        for input in cases {
            assert_rewrite(input, "rtk run");
        }
    }

    // === SPECIFIC COMMANDS NOT BLOCKED ===
    // Ported from old hooks/test-rtk-rewrite.sh Sections 1 & 3.
    // These commands must pass through (not be blocked by safety rules).

    #[test]
    fn test_specific_commands_not_blocked() {
        let cases = [
            // Git variants
            "git log --oneline -10",
            "git diff HEAD",
            "git show abc123",
            "git add .",
            // GitHub CLI
            "gh pr list",
            "gh api repos/owner/repo",
            "gh release list",
            // Package managers
            "npm run test:e2e",
            "npm run build",
            "npm test",
            // Docker
            "docker compose up -d",
            "docker compose logs postgrest",
            "docker compose down",
            "docker run --rm postgres",
            "docker exec -it db psql",
            // Kubernetes
            "kubectl describe pod foo",
            "kubectl apply -f deploy.yaml",
            // Test runners
            "npx playwright test",
            "npx prisma migrate",
            "cargo test",
            // Vitest variants (dedup is internal to rtk run, not hook level)
            "vitest",
            "vitest run",
            "vitest run --reporter=verbose",
            "npx vitest run",
            "pnpm vitest run --coverage",
            // TypeScript
            "vue-tsc -b",
            "npx vue-tsc --noEmit",
            // Utilities
            "curl -s https://example.com",
            "ls -la",
            "grep -rn pattern src/",
            "rg pattern src/",
        ];
        for input in cases {
            assert_rewrite(input, "rtk run");
        }
    }

    // === COMMANDS THAT PASS THROUGH (builtins/unknown) ===
    // Ported from old hooks/test-rtk-rewrite.sh Section 5.
    // These are not blocked — they get wrapped in rtk run -c.

    #[test]
    fn test_builtins_not_blocked() {
        let cases = [
            "echo hello world",
            "cd /tmp",
            "mkdir -p foo/bar",
            "python3 script.py",
            "node -e 'console.log(1)'",
            "find . -name '*.ts'",
            "tree src/",
            "wget https://example.com/file",
        ];
        for input in cases {
            assert_rewrite(input, "rtk run");
        }
    }

    // === COMPOUND COMMANDS (chained with &&, ||, ;) ===
    // Shell script only matched FIRST command in a chain.
    // Rust hook parses each command independently (#112).

    #[test]
    fn test_compound_commands_rewrite() {
        let cases = [
            // Basic chains — each command rewritten independently
            ("cd /tmp && git status", "&&"),
            ("cd dir && git status && git diff", "&&"),
            ("git add . && git commit -m msg", "&&"),
            // Semicolon chains
            ("echo start ; git status ; echo done", ";"),
            // Or-chains
            ("git pull || echo failed", "||"),
        ];
        for (input, operator) in cases {
            match check_for_hook(input, "claude") {
                HookResult::Rewrite(cmd) => {
                    assert!(cmd.contains("rtk run"), "'{input}' should rewrite");
                    assert!(
                        cmd.contains(operator),
                        "'{input}' must preserve '{operator}', got '{cmd}'"
                    );
                }
                other => panic!("Expected Rewrite for '{input}', got {other:?}"),
            }
        }
    }

    // PR 2 adds: test_compound_blocked_in_chain (safety-dependent test)

    #[test]
    fn test_compound_quoted_operators_not_split() {
        // && inside quotes must NOT split the command
        let input = r#"git commit -m "Fix && Bug""#;
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(cmd.contains("rtk run"), "Should rewrite, got '{cmd}'");
            }
            other => panic!("Expected Rewrite for quoted &&, got {other:?}"),
        }
    }

    // PR 2 adds: test_blocked_commands (safety-dependent test)

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
        let (output, success, code) =
            format_for_claude(HookResult::Rewrite("rtk run -c 'git status'".to_string()));
        assert_eq!(output, "rtk run -c 'git status'");
        assert!(success);
        assert_eq!(code, 0);

        let (output, success, code) =
            format_for_claude(HookResult::Blocked("Error message".to_string()));
        assert_eq!(output, "Error message");
        assert!(!success);
        assert_eq!(code, 2); // Exit 2 = blocking error per Claude Code spec
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
        assert_eq!(
            code, 2,
            "Block must exit 2 (blocking error per Claude Code spec)"
        );
    }

    #[test]
    fn test_claude_rewrite_output_is_command_text() {
        // Claude Code reads stdout as the rewritten command — must be plain text, not JSON
        let (output, success, _) =
            format_for_claude(HookResult::Rewrite("rtk run -c 'git status'".into()));
        assert_eq!(output, "rtk run -c 'git status'");
        assert!(success);
        // Must NOT be JSON
        assert!(
            !output.starts_with('{'),
            "Rewrite output must be plain text, not JSON"
        );
    }

    #[test]
    fn test_claude_block_output_is_human_message() {
        // Claude Code reads stderr for the block reason
        let (output, success, _) =
            format_for_claude(HookResult::Blocked("Use Read tool instead".into()));
        assert_eq!(output, "Use Read tool instead");
        assert!(!success);
        // Must NOT be JSON
        assert!(
            !output.starts_with('{'),
            "Block output must be plain text, not JSON"
        );
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
        assert_ne!(
            rewrite_code, 1,
            "Exit code 1 is non-blocking error, not valid for rewrite"
        );
        assert_ne!(
            block_code, 1,
            "Exit code 1 is non-blocking error, not valid for block"
        );
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
                _ => panic!(
                    "'{}': Claude={:?}, Gemini={:?} — both should Rewrite",
                    cmd, claude, gemini
                ),
            }
        }
    }

    // PR 2 adds: test_cross_protocol_blocked_command_denied_by_both (safety-dependent test)
}
