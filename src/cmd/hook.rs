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

    let tokens = lexer::tokenize(raw);

    if analysis::needs_shell(&tokens) {
        return HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw)));
    }

    match analysis::parse_chain(tokens) {
        Ok(commands) => {
            // Single command: route to optimized RTK subcommand.
            // Chained commands (&&, ||, ;): wrap entire chain in rtk run -c.
            if commands.len() == 1 {
                HookResult::Rewrite(route_native_command(&commands[0], raw))
            } else {
                HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw)))
            }
        }
        Err(_) => HookResult::Rewrite(format!("rtk run -c '{}'", escape_quotes(raw))),
    }
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

/// Replace the first occurrence of `old_prefix` in `raw` with `new_prefix`.
///
/// Preserves everything after the prefix (including original quoting).
/// Falls back to `rtk run -c '<raw>'` if prefix not found (safe degradation).
///
/// # Examples
/// - `replace_first_word("grep -r p src/", "grep", "rtk grep")` → `"rtk grep -r p src/"`
/// - `replace_first_word("rg pattern", "rg", "rtk grep")` → `"rtk grep pattern"`
fn replace_first_word(raw: &str, old_prefix: &str, new_prefix: &str) -> String {
    raw.strip_prefix(old_prefix)
        .map(|rest| format!("{new_prefix}{rest}"))
        .unwrap_or_else(|| format!("rtk run -c '{}'", escape_quotes(raw)))
}

/// Route pnpm subcommands to RTK equivalents.
///
/// Uses `cmd.args` (parsed, quote-stripped) for routing decisions.
/// Uses `raw` or reconstructed args for output to preserve original quoting.
fn route_pnpm(cmd: &analysis::NativeCommand, raw: &str) -> String {
    let sub = cmd.args.first().map(String::as_str).unwrap_or("");
    match sub {
        "list" | "ls" | "outdated" | "install" => format!("rtk {raw}"),

        // pnpm vitest [run] [flags] → rtk vitest run [flags]
        // Shell script sed bug: 's/^(pnpm )?vitest/rtk vitest run/' on
        // "pnpm vitest run --coverage" produces "rtk vitest run run --coverage".
        // Binary hook corrects this by stripping the leading "run" from parsed args.
        "vitest" => {
            let after_vitest: Vec<&str> = cmd.args[1..]
                .iter()
                .map(String::as_str)
                .skip_while(|&a| a == "run")
                .collect();
            if after_vitest.is_empty() {
                "rtk vitest run".to_string()
            } else {
                format!("rtk vitest run {}", after_vitest.join(" "))
            }
        }

        // pnpm test [flags] → rtk vitest run [flags]
        "test" => {
            let after_test: Vec<&str> = cmd.args[1..].iter().map(String::as_str).collect();
            if after_test.is_empty() {
                "rtk vitest run".to_string()
            } else {
                format!("rtk vitest run {}", after_test.join(" "))
            }
        }

        "tsc" => replace_first_word(raw, "pnpm tsc", "rtk tsc"),
        "lint" => replace_first_word(raw, "pnpm lint", "rtk lint"),
        "playwright" => replace_first_word(raw, "pnpm playwright", "rtk playwright"),

        _ => format!("rtk run -c '{}'", escape_quotes(raw)),
    }
}

/// Route npx subcommands to RTK equivalents.
fn route_npx(cmd: &analysis::NativeCommand, raw: &str) -> String {
    let sub = cmd.args.first().map(String::as_str).unwrap_or("");
    match sub {
        "tsc" | "typescript" => replace_first_word(raw, &format!("npx {sub}"), "rtk tsc"),
        "eslint" => replace_first_word(raw, "npx eslint", "rtk lint"),
        "prettier" => replace_first_word(raw, "npx prettier", "rtk prettier"),
        "playwright" => replace_first_word(raw, "npx playwright", "rtk playwright"),
        "prisma" => replace_first_word(raw, "npx prisma", "rtk prisma"),
        _ => format!("rtk run -c '{}'", escape_quotes(raw)),
    }
}

/// Route a single parsed native command to its optimized RTK subcommand.
///
/// ## Design
/// - Uses `cmd.binary`/`cmd.args` (lexer→parse_chain output) for routing DECISIONS.
/// - Uses `raw: &str` with `replace_first_word` for string REPLACEMENT (preserves quoting).
/// - `format!("rtk {raw}")` works when the binary name equals the RTK subcommand.
/// - `replace_first_word` handles renames: `rg → rtk grep`, `cat → rtk read`.
///
/// ## Fallback
/// Unknown binaries or unrecognized subcommands → `rtk run -c '<raw>'` (safe passthrough).
///
/// ## Mirrors
/// `~/.claude/hooks/rtk-rewrite.sh` routing table. Corrects the shell script's
/// `vitest run` double-"run" bug by using parsed args rather than regex substitution.
///
/// ## Safety interaction
/// PR 2 adds safety::check before this function. The `cat` arm is defensive for
/// when `RTK_BLOCK_TOKEN_WASTE=0`.
fn route_native_command(cmd: &analysis::NativeCommand, raw: &str) -> String {
    let sub = cmd.args.first().map(String::as_str).unwrap_or("");
    let sub2 = cmd.args.get(1).map(String::as_str).unwrap_or("");

    match cmd.binary.as_str() {
        // Git: known subcommands (global options like --no-pager fall through to fallback)
        "git"
            if matches!(
                sub,
                "status"
                    | "diff"
                    | "log"
                    | "add"
                    | "commit"
                    | "push"
                    | "pull"
                    | "branch"
                    | "fetch"
                    | "stash"
                    | "show"
            ) =>
        {
            format!("rtk {raw}")
        }

        // GitHub CLI
        "gh" if matches!(sub, "pr" | "issue" | "run") => format!("rtk {raw}"),

        // Cargo: test/build/clippy/check have rtk equivalents
        "cargo" if matches!(sub, "test" | "build" | "clippy" | "check") => format!("rtk {raw}"),

        // File ops — renames (rg/grep → rtk grep, cat → rtk read)
        // NOTE: PR 2 adds safety rules that block cat/head/sed before reaching here.
        // These arms are defensive for if RTK_BLOCK_TOKEN_WASTE=0.
        "cat" => replace_first_word(raw, "cat", "rtk read"),
        "grep" | "rg" => replace_first_word(raw, cmd.binary.as_str(), "rtk grep"),
        "eslint" => replace_first_word(raw, "eslint", "rtk lint"),

        // Direct prepend: rtk subcommand name = binary name
        "ls" | "tsc" | "prettier" | "playwright" | "prisma" | "curl" | "pytest"
        | "golangci-lint" => format!("rtk {raw}"),

        // tail: may be blocked by safety (PR 2); defensive routing if allowed
        "tail" => format!("rtk {raw}"),

        // vitest: bare vitest → rtk vitest run (not rtk vitest)
        "vitest" if sub.is_empty() => "rtk vitest run".to_string(),
        "vitest" => format!("rtk {raw}"),

        // Containers: info-read subcommands only
        "docker" if matches!(sub, "ps" | "images" | "logs") => format!("rtk {raw}"),
        "kubectl" if matches!(sub, "get" | "logs") => format!("rtk {raw}"),

        // Go
        "go" if matches!(sub, "test" | "build" | "vet") => format!("rtk {raw}"),

        // Ruff: check/format only
        "ruff" if matches!(sub, "check" | "format") => format!("rtk {raw}"),

        // pip/uv: list/outdated/install/show only
        "pip" if matches!(sub, "list" | "outdated" | "install" | "show") => format!("rtk {raw}"),
        "uv" if sub == "pip" && matches!(sub2, "list" | "outdated" | "install" | "show") => {
            replace_first_word(raw, "uv pip", "rtk pip")
        }

        // python/python3 -m pytest
        "python" | "python3" if sub == "-m" && sub2 == "pytest" => {
            let prefix = format!("{} -m pytest", cmd.binary);
            replace_first_word(raw, &prefix, "rtk pytest")
        }

        // pnpm / npx: delegated to helpers (complex sub-routing)
        "pnpm" => route_pnpm(cmd, raw),
        "npx" => route_npx(cmd, raw),

        // Fallback: unknown binary or unrecognized subcommand
        _ => format!("rtk run -c '{}'", escape_quotes(raw)),
    }
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
            ("git status", "rtk git status"), // now routes to optimized subcommand
            ("ls *.rs", "rtk run"),           // shellism passthrough (glob)
            (r#"git commit -m "Fix && Bug""#, "rtk git commit"), // quoted &&: single cmd, routes
            ("FOO=bar echo hello", "rtk run"), // env prefix → shellism
            ("echo `date`", "rtk run"),       // backticks
            ("echo $(date)", "rtk run"),      // subshell
            ("echo {a,b}.txt", "rtk run"),    // brace expansion
            ("echo 'hello!@#$%^&*()'", "rtk run"), // special chars
            ("echo '日本語 🎉'", "rtk run"),  // unicode
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
            // Test name intent: commands must Rewrite (not Blocked), regardless of routing target.
            // Specific routing targets are verified in test_routing_native_commands.
            assert!(
                matches!(check_for_hook(input, "claude"), HookResult::Rewrite(_)),
                "'{}' should Rewrite (not Blocked)",
                input
            );
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
        // && inside quotes must NOT split the command into a chain.
        // parse_chain sees one command: git commit with args ["-m", "Fix && Bug"].
        // That single command routes to rtk git commit (not rtk run -c).
        let input = r#"git commit -m "Fix && Bug""#;
        match check_for_hook(input, "claude") {
            HookResult::Rewrite(cmd) => {
                assert!(
                    cmd.contains("rtk git commit"),
                    "Quoted && must not split; should route to rtk git commit, got '{cmd}'"
                );
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
        // Both agents must Rewrite (not Block) safe commands.
        // Specific routing targets verified in test_cross_agent_routing_identical.
        for agent in ["claude", "gemini"] {
            match check_for_hook("git status", agent) {
                HookResult::Rewrite(_) => {}
                other => panic!("Expected Rewrite for agent '{}', got {:?}", agent, other),
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

    // =====================================================================
    // ROUTING TESTS — verify route_native_command dispatch
    // =====================================================================

    #[test]
    fn test_routing_native_commands() {
        // Table-driven: commands that route to optimized rtk subcommands.
        // Each (input, expected_substr) must appear in the rewritten output.
        let cases = [
            // Git: known subcommands
            ("git status", "rtk git status"),
            ("git log --oneline -10", "rtk git log --oneline -10"),
            ("git diff HEAD", "rtk git diff HEAD"),
            ("git add .", "rtk git add ."),
            ("git commit -m msg", "rtk git commit"),
            // GitHub CLI
            ("gh pr view 156", "rtk gh pr view 156"),
            // Cargo
            ("cargo test", "rtk cargo test"),
            (
                "cargo clippy --all-targets",
                "rtk cargo clippy --all-targets",
            ),
            // File ops (rg → rtk grep rename)
            // NOTE: PR 2 adds safety that blocks cat before reaching router; arm is defensive.
            ("grep -r pattern src/", "rtk grep -r pattern src/"),
            ("rg pattern src/", "rtk grep pattern src/"),
            ("ls -la", "rtk ls -la"),
            ("tail -n 20 file.txt", "rtk tail -n 20 file.txt"),
            // JS/TS tooling
            ("vitest", "rtk vitest run"),     // bare → rtk vitest run
            ("vitest run", "rtk vitest run"), // explicit run preserved
            ("vitest run --coverage", "rtk vitest run --coverage"),
            ("pnpm test", "rtk vitest run"),
            ("pnpm vitest", "rtk vitest run"),
            ("pnpm lint", "rtk lint"),
            ("npx tsc --noEmit", "rtk tsc --noEmit"),
            // Python
            ("python -m pytest tests/", "rtk pytest tests/"),
            ("uv pip list", "rtk pip list"),
            // Go
            ("go test ./...", "rtk go test ./..."),
        ];
        for (input, expected) in cases {
            assert_rewrite(input, expected);
        }
    }

    #[test]
    fn test_routing_vitest_no_double_run() {
        // Shell script sed bug: 's/^(pnpm )?vitest/rtk vitest run/' on
        // "pnpm vitest run --coverage" produces "rtk vitest run run --coverage".
        // Binary hook corrects this by using parsed args instead of regex substitution.
        let result = match check_for_hook("pnpm vitest run --coverage", "claude") {
            HookResult::Rewrite(cmd) => cmd,
            other => panic!("Expected Rewrite, got {:?}", other),
        };
        assert_rewrite("pnpm vitest run --coverage", "rtk vitest run --coverage");
        assert!(
            !result.contains("run run"),
            "Must not double 'run' in output: '{}'",
            result
        );
    }

    #[test]
    fn test_routing_fallbacks_to_rtk_run() {
        // Unknown subcommand, chains (2+ cmds), and pipes fall back to rtk run -c.
        let cases = [
            "git checkout main",              // unknown git subcommand
            "git add . && git commit -m msg", // chain → 2 commands → rtk run -c
            "git log | grep fix",             // pipe → needs_shell → rtk run -c
        ];
        for input in cases {
            assert_rewrite(input, "rtk run -c");
        }
    }

    #[test]
    fn test_cross_agent_routing_identical() {
        // Both claude and gemini must route the same commands to the same output.
        for cmd in ["git status", "cargo test", "ls -la"] {
            let claude_result = check_for_hook(cmd, "claude");
            let gemini_result = check_for_hook(cmd, "gemini");
            match (&claude_result, &gemini_result) {
                (HookResult::Rewrite(c), HookResult::Rewrite(g)) => {
                    assert_eq!(c, g, "claude and gemini must route '{}' identically", cmd);
                    assert!(
                        !c.contains("rtk run -c"),
                        "'{}' should not fall back to rtk run -c",
                        cmd
                    );
                }
                _ => panic!(
                    "'{}' should Rewrite for both agents: claude={:?} gemini={:?}",
                    cmd, claude_result, gemini_result
                ),
            }
        }
    }
}
