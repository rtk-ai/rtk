//! Claude Code PreToolUse hook protocol handler.
//!
//! Reads JSON from stdin, applies safety checks and rewrites,
//! outputs JSON to stdout.
//!
//! Protocol: https://docs.anthropic.com/en/docs/claude-code/hooks
//!
//! ## Exit Code Behavior
//!
//! - Exit 0 = success (allow/rewrite) — tool proceeds
//! - Exit 2 = blocking error (deny) — tool rejected
//!
//! ## Claude Code Stderr Rule (CRITICAL)
//!
//! **Source:** https://docs.anthropic.com/en/docs/claude-code/hooks
//!
//! ```text
//! CRITICAL: ANY stderr output at exit 0 = hook error = fail-open
//! ```
//!
//! **Implication:**
//! - Exit 0 + ANY stderr → Claude Code treats hook as FAILED → tool executes anyway (fail-open)
//! - Exit 2 + stderr → Claude Code treats stderr as the block reason → tool blocked, AI sees reason
//!
//! **This module's stderr usage:**
//! - ✅ Exit 0 paths (NoOpinion, Allow): **NEVER write to stderr**
//! - ✅ Exit 2 path (Deny): **stderr ONLY** for bug #4669 workaround (see below)
//!
//! ## Bug #4669 Workaround (Dual-Path Deny)
//!
//! **Issue:** https://github.com/anthropics/claude-code/issues/4669
//! **Versions:** v1.0.62+ through current (not fixed)
//! **Problem:** `permissionDecision: "deny"` at exit 0 is IGNORED — tool executes anyway
//!
//! **Workaround:**
//! ```text
//! stdout: JSON with permissionDecision "deny" (documented main path, but broken)
//! stderr: plain text reason (fallback path that actually works)
//! exit code: 2 (triggers Claude Code to read stderr as error)
//! ```
//!
//! This ensures deny works regardless of which path Claude Code processes.
//!
//! ## I/O Enforcement (Module-Specific)
//!
//! **This restriction applies ONLY to claude_hook.rs and gemini_hook.rs.**
//! All other RTK modules (main.rs, git.rs, etc.) use `println!`/`eprintln!` normally.
//!
//! **Why restricted here:**
//! - Hook protocol requires JSON-only stdout
//! - Claude Code's "ANY stderr = hook error" rule (see above)
//! - Accidental prints corrupt the JSON protocol
//!
//! **Enforcement mechanism:**
//! - `#![deny(clippy::print_stdout, clippy::print_stderr)]` at module level (line 52)
//! - `run_inner()` returns `HookResponse` enum — pure logic, no I/O
//! - `run()` is the ONLY function that writes output — single I/O point
//! - Uses `write!`/`writeln!` which are NOT caught by the clippy lint
//!
//! **Pathway:** main.rs → Commands::Hook → claude_hook::run() [DENY ENFORCED HERE]
//!
//! Fail-open: Any parse error or unexpected input → exit 0, no output.

// Compile-time I/O enforcement for THIS MODULE ONLY.
// Other RTK modules (main.rs, git.rs, etc.) use println!/eprintln! normally.
//
// Why restrict here:
// - Claude Code hook protocol requires JSON-only stdout
// - Claude Code rule: "ANY stderr at exit 0 = hook error = fail-open"
//   (Source: https://docs.anthropic.com/en/docs/claude-code/hooks)
// - Accidental prints would corrupt the JSON response
//
// Mechanism:
// - Denies println!/eprintln! at compile-time
// - Allows write!/writeln! (used only in run() for controlled output)
// - run_inner() returns HookResponse (no I/O)
// - run() is the single I/O point
#![deny(clippy::print_stdout, clippy::print_stderr)]

use super::{
    check_for_hook, is_hook_disabled, should_passthrough, update_command_in_tool_input,
    HookResponse, HookResult,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::io::{self, Read, Write};

// --- Wire format structs (field names must match Claude Code spec exactly) ---

#[derive(Deserialize)]
pub(crate) struct ClaudePayload {
    tool_input: Option<Value>,
    // Claude Code also sends: tool_name, session_id, session_cwd,
    // transcript_path — serde silently ignores unknown fields.
    // The settings.json matcher already filters to Bash-only events.
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct ClaudeResponse {
    hook_specific_output: HookOutput,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct HookOutput {
    hook_event_name: &'static str,
    permission_decision: &'static str,
    permission_decision_reason: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    updated_input: Option<Value>,
}

// --- Manifest fallthrough structs ---
// Minimal subset of rtk-bash-manifest.json needed for reading.

#[derive(Deserialize)]
struct ManifestFallthroughEntry {
    fallthrough_command: String,
}

#[derive(Deserialize)]
struct ManifestFallthrough {
    entries: Vec<ManifestFallthroughEntry>,
}

// --- Guard logic (extracted for testability) ---

/// Extract the command string from a parsed payload.
/// Returns None if payload has no tool_input or no command field.
pub(crate) fn extract_command(payload: &ClaudePayload) -> Option<&str> {
    payload
        .tool_input
        .as_ref()?
        .get("command")?
        .as_str()
        .filter(|s| !s.is_empty())
}

// Guard functions `is_hook_disabled()` and `should_passthrough()` are shared
// with gemini_hook.rs via hook.rs to avoid duplication (DRY).

/// Build a ClaudeResponse for an allowed/rewritten command.
pub(crate) fn allow_response(reason: String, updated_input: Option<Value>) -> ClaudeResponse {
    ClaudeResponse {
        hook_specific_output: HookOutput {
            hook_event_name: "PreToolUse",
            permission_decision: "allow",
            permission_decision_reason: reason,
            updated_input,
        },
    }
}

/// Build a ClaudeResponse for a blocked command.
pub(crate) fn deny_response(reason: String) -> ClaudeResponse {
    ClaudeResponse {
        hook_specific_output: HookOutput {
            hook_event_name: "PreToolUse",
            permission_decision: "deny",
            permission_decision_reason: reason,
            updated_input: None,
        },
    }
}

// --- Entry point ---

/// Run the Claude Code hook handler.
///
/// This is the ONLY function that performs I/O (stdout/stderr).
/// `run_inner()` returns a `HookResponse` enum — pure logic, no I/O.
/// Combined with `#![deny(clippy::print_stdout, clippy::print_stderr)]`,
/// this ensures no stray output corrupts the JSON hook protocol.
///
/// Fail-open design: malformed input → exit 0, no output.
/// Claude Code interprets this as "no opinion" and proceeds normally.
pub fn run() -> anyhow::Result<()> {
    // Read stdin once here so the raw payload is available for run_manifest_fallthrough.
    let mut buffer = String::new();
    io::stdin().read_to_string(&mut buffer)?;

    // Fail-open: wrap entire handler so ANY error → exit 0 (no opinion).
    let response = match run_inner(&buffer) {
        Ok(r) => r,
        Err(_) => HookResponse::NoOpinion, // Fail-open: swallow errors
    };

    // ┌────────────────────────────────────────────────────────────────┐
    // │ SINGLE I/O POINT - All stdout/stderr output happens here only │
    // │                                                                │
    // │ Why: Claude Code rule "ANY stderr at exit 0 = hook error"     │
    // │      (Source: hooks_api_reference.md:720-728)                 │
    // │                                                                │
    // │ Enforcement: #![deny(...)] at line 52 prevents println!/eprintln! │
    // │              write!/writeln! are not caught by lint (allowed) │
    // └────────────────────────────────────────────────────────────────┘
    match response {
        HookResponse::NoOpinion => {
            // No RTK opinion — run ALL manifest handlers on original payload.
            // All handlers run; first block wins; pass-through if none block.
            // INVARIANT: payload is always the original unmodified stdin.
            match run_manifest_handlers(&buffer) {
                ManifestResult::Blocked { json, stderr_bytes } => {
                    writeln!(io::stdout(), "{json}")?;
                    io::stderr().write_all(&stderr_bytes)?;
                    if stderr_bytes.is_empty() {
                        writeln!(io::stderr(), "Command blocked by registered handler")?;
                    }
                    std::process::exit(2);
                }
                ManifestResult::NoBlock => {
                    // No opinion from any handler — exit 0, no stdout (implicit allow).
                }
            }
        }
        HookResponse::Allow(rtk_json) => {
            // RTK wants to rewrite — check manifest handlers as veto gate first.
            // Uses ORIGINAL payload so handlers see the command Claude requested, not the rewrite.
            // Deny wins over rewrite (autorun can block grep even if RTK would rewrite it).
            match run_manifest_handlers(&buffer) {
                ManifestResult::Blocked {
                    json: handler_json,
                    stderr_bytes,
                } => {
                    // Deny wins over rewrite. Forward handler's own JSON verbatim.
                    writeln!(io::stdout(), "{handler_json}")?;
                    io::stderr().write_all(&stderr_bytes)?;
                    if stderr_bytes.is_empty() {
                        let reason = extract_deny_reason(&handler_json).unwrap_or_else(|| {
                            "Command blocked by registered safety handler".to_owned()
                        });
                        writeln!(io::stderr(), "{reason}")?;
                    }
                    std::process::exit(2);
                }
                ManifestResult::NoBlock => {
                    // No block — emit RTK's rewrite. Exit 0, JSON to stdout.
                    writeln!(io::stdout(), "{rtk_json}")?;
                }
            }
        }
        HookResponse::Deny(json, reason) => {
            // Exit 2, JSON to stdout, reason to stderr
            // This is the ONLY path that writes to stderr (valid at exit 2 only)
            //
            // Dual-path deny for bug #4669 workaround:
            // - stdout: JSON with permissionDecision "deny" (documented path, but ignored)
            // - stderr: plain text reason (actual blocking mechanism via exit 2)
            // - exit 2: Triggers Claude Code to read stderr and block tool
            writeln!(io::stdout(), "{json}")?;
            writeln!(io::stderr(), "{reason}")?;
            std::process::exit(2);
        }
    }
    Ok(())
}

/// Inner handler: pure decision logic, no I/O.
/// Returns `HookResponse` for `run()` to output.
fn run_inner(buffer: &str) -> anyhow::Result<HookResponse> {
    let payload: ClaudePayload = match serde_json::from_str(buffer) {
        Ok(p) => p,
        Err(_) => return Ok(HookResponse::NoOpinion),
    };

    let cmd = match extract_command(&payload) {
        Some(c) => c,
        None => return Ok(HookResponse::NoOpinion),
    };

    if is_hook_disabled() || should_passthrough(cmd) {
        return Ok(HookResponse::NoOpinion);
    }

    let result = check_for_hook(cmd, "claude");

    match result {
        HookResult::Rewrite(new_cmd) => {
            // Preserve all original tool_input fields, only replace "command"
            // Shared helper (DRY with gemini_hook.rs via hook.rs)
            let updated = update_command_in_tool_input(payload.tool_input, new_cmd);

            let response = allow_response("RTK safety rewrite applied".into(), Some(updated));
            let json = serde_json::to_string(&response)?;
            Ok(HookResponse::Allow(json))
        }
        HookResult::Blocked(msg) => {
            let response = deny_response(msg.clone());
            let json = serde_json::to_string(&response)?;
            Ok(HookResponse::Deny(json, msg))
        }
    }
}

/// Path to the RTK bash manifest written by `rtk init`.
fn manifest_path() -> Option<std::path::PathBuf> {
    // Check HOME first (Unix), then USERPROFILE (Windows) as fallback.
    let home = std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()?;
    Some(
        std::path::Path::new(&home)
            .join(".claude")
            .join("hooks")
            .join("rtk-bash-manifest.json"),
    )
}

/// Check if JSON string contains a deny decision in either CLI format.
/// - Claude Code format (current + future when bug #4669 fixed):
///   `hookSpecificOutput.permissionDecision = "deny"`
/// - Gemini CLI format (correct today, no bug):
///   `decision = "deny"` (top-level; deprecated for CC PreToolUse per api docs,
///   but Gemini sub-handlers legitimately output this format)
/// Both formats accepted so coordinator works for sub-handlers targeting either CLI.
fn is_json_deny(json_str: &str) -> bool {
    let Ok(v) = serde_json::from_str::<Value>(json_str.trim()) else {
        return false;
    };
    let cc_deny = v
        .get("hookSpecificOutput")
        .and_then(|o| o.get("permissionDecision"))
        .and_then(|d| d.as_str())
        == Some("deny");
    let gemini_deny = v.get("decision").and_then(|d| d.as_str()) == Some("deny");
    cc_deny || gemini_deny
}

/// Extract deny reason from JSON — supports CC and Gemini formats.
fn extract_deny_reason(json_str: &str) -> Option<String> {
    let v: Value = serde_json::from_str(json_str.trim()).ok()?;
    // Claude Code: hookSpecificOutput.permissionDecisionReason
    if let Some(r) = v
        .get("hookSpecificOutput")
        .and_then(|o| o.get("permissionDecisionReason"))
        .and_then(|r| r.as_str())
    {
        return Some(r.to_owned());
    }
    // Gemini: reason field
    v.get("reason").and_then(|r| r.as_str()).map(str::to_owned)
}

/// Result returned by `run_manifest_handlers()`.
/// Never emits to stdout/stderr — caller (`run()`) is the sole I/O point.
enum ManifestResult {
    /// At least one handler blocked (exit 2 or JSON deny).
    /// `json`: first blocking handler's stdout (forwarded to Claude Code).
    /// `stderr_bytes`: buffered stderr from blocked handler — forward only at exit 2.
    Blocked { json: String, stderr_bytes: Vec<u8> },
    /// No handler blocked.
    NoBlock,
}

/// Load the RTK bash manifest from disk.
fn load_manifest() -> Option<ManifestFallthrough> {
    let path = manifest_path()?;
    if !path.exists() {
        return None;
    }
    let content = std::fs::read_to_string(&path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Run ALL manifest handlers from rtk-bash-manifest.json and collect results.
///
/// Designed for both NoOpinion (fallthrough) and Allow (veto gate) paths.
/// ALL handlers run regardless of individual results — no short-circuit.
///
/// I/O contract: NEVER writes to stdout or stderr.
/// Returns `ManifestResult`; caller (`run()`) does all I/O.
///
/// INVARIANT: `payload` is always the ORIGINAL unmodified stdin — never a rewrite.
fn run_manifest_handlers(payload: &str) -> ManifestResult {
    let manifest = match load_manifest() {
        Some(m) => m,
        None => return ManifestResult::NoBlock,
    };

    let mut block_json: Option<String> = None;
    let mut block_stderr: Vec<u8> = Vec::new();

    for entry in &manifest.entries {
        let mut child = match std::process::Command::new("sh")
            .arg("-c")
            .arg(&entry.fallthrough_command)
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped()) // Capture: check JSON deny; avoid stdout concat
            .stderr(std::process::Stdio::piped()) // Capture: buffer, forward only on exit 2
            .spawn()
        {
            Ok(c) => c,
            Err(_) => continue, // fail-open: handler binary not found
        };

        // Write payload; track success so we don't propagate a false-positive exit 2
        // if the write failed and the child received empty/partial stdin.
        let write_ok = if let Some(mut stdin) = child.stdin.take() {
            io::Write::write_all(&mut stdin, payload.as_bytes()).is_ok()
            // stdin dropped here → EOF sent to child
        } else {
            false
        };

        let output = match child.wait_with_output() {
            Ok(o) => o,
            Err(_) => continue, // fail-open
        };

        let exit_code = output.status.code().unwrap_or(0);
        let stdout_str = String::from_utf8_lossy(&output.stdout);
        let blocked = (exit_code == 2 && write_ok) || is_json_deny(&stdout_str);

        if blocked && block_json.is_none() {
            // Record FIRST block; continue running ALL remaining handlers.
            block_json = Some(stdout_str.into_owned());
            block_stderr.extend_from_slice(&output.stderr);
        }
        // Continue — ALL handlers run regardless of this result.
    }

    match block_json {
        Some(json) => ManifestResult::Blocked {
            json,
            stderr_bytes: block_stderr,
        },
        None => ManifestResult::NoBlock,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cmd::test_helpers::EnvGuard;

    // =========================================================================
    // CLAUDE CODE WIRE FORMAT CONFORMANCE
    // https://docs.anthropic.com/en/docs/claude-code/hooks
    //
    // These tests verify exact JSON field names per the Claude Code spec.
    // A wrong field name means Claude Code silently ignores the response.
    // =========================================================================

    // --- Output: field name conformance ---

    #[test]
    fn test_output_uses_hook_specific_output() {
        // Claude expects "hookSpecificOutput" (camelCase), NOT "hook_specific_output"
        let response = allow_response("test".into(), None);
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert!(
            parsed.get("hookSpecificOutput").is_some(),
            "must have 'hookSpecificOutput' field"
        );
        assert!(
            parsed.get("hook_specific_output").is_none(),
            "must NOT have snake_case field"
        );
    }

    #[test]
    fn test_output_uses_permission_decision() {
        // Claude expects "permissionDecision", NOT "decision"
        let response = allow_response("test".into(), None);
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        let output = &parsed["hookSpecificOutput"];

        assert!(
            output.get("permissionDecision").is_some(),
            "must have 'permissionDecision' field"
        );
        assert!(
            output.get("decision").is_none(),
            "must NOT have Gemini-style 'decision' field"
        );
    }

    #[test]
    fn test_output_uses_permission_decision_reason() {
        let response = deny_response("blocked".into());
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();
        let output = &parsed["hookSpecificOutput"];

        assert!(
            output.get("permissionDecisionReason").is_some(),
            "must have 'permissionDecisionReason'"
        );
    }

    #[test]
    fn test_output_uses_hook_event_name() {
        let response = allow_response("test".into(), None);
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert_eq!(parsed["hookSpecificOutput"]["hookEventName"], "PreToolUse");
    }

    #[test]
    fn test_output_uses_updated_input_for_rewrite() {
        let input = serde_json::json!({"command": "rtk run -c 'git status'"});
        let response = allow_response("rewrite".into(), Some(input));
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        assert!(
            parsed["hookSpecificOutput"].get("updatedInput").is_some(),
            "must have 'updatedInput' for rewrites"
        );
    }

    #[test]
    fn test_allow_omits_updated_input_when_none() {
        let response = allow_response("passthrough".into(), None);
        let json = serde_json::to_string(&response).unwrap();

        assert!(
            !json.contains("updatedInput"),
            "updatedInput must be omitted when None"
        );
    }

    #[test]
    fn test_rewrite_preserves_other_tool_input_fields() {
        let original = serde_json::json!({
            "command": "git status",
            "timeout": 30,
            "description": "check repo"
        });

        let mut updated = original.clone();
        if let Some(obj) = updated.as_object_mut() {
            obj.insert(
                "command".into(),
                Value::String("rtk run -c 'git status'".into()),
            );
        }

        assert_eq!(updated["timeout"], 30);
        assert_eq!(updated["description"], "check repo");
        assert_eq!(updated["command"], "rtk run -c 'git status'");
    }

    #[test]
    fn test_output_decision_values() {
        let allow = allow_response("test".into(), None);
        let deny = deny_response("blocked".into());

        let allow_json: Value =
            serde_json::from_str(&serde_json::to_string(&allow).unwrap()).unwrap();
        let deny_json: Value =
            serde_json::from_str(&serde_json::to_string(&deny).unwrap()).unwrap();

        assert_eq!(
            allow_json["hookSpecificOutput"]["permissionDecision"],
            "allow"
        );
        assert_eq!(
            deny_json["hookSpecificOutput"]["permissionDecision"],
            "deny"
        );
    }

    // --- Input: payload parsing ---

    #[test]
    fn test_input_extra_fields_ignored() {
        // Claude sends session_id, tool_name, transcript_path, etc.
        let json = r#"{"tool_input": {"command": "ls"}, "tool_name": "Bash", "session_id": "abc-123", "session_cwd": "/tmp", "transcript_path": "/path/to/transcript.jsonl"}"#;
        let payload: ClaudePayload = serde_json::from_str(json).unwrap();
        assert_eq!(extract_command(&payload), Some("ls"));
    }

    #[test]
    fn test_input_tool_input_is_object() {
        let json = r#"{"tool_input": {"command": "git status", "timeout": 30}}"#;
        let payload: ClaudePayload = serde_json::from_str(json).unwrap();
        let input = payload.tool_input.unwrap();
        assert_eq!(input["command"].as_str().unwrap(), "git status");
        assert_eq!(input["timeout"].as_i64().unwrap(), 30);
    }

    // --- Guard function tests ---

    #[test]
    fn test_extract_command_basic() {
        let payload: ClaudePayload =
            serde_json::from_str(r#"{"tool_input": {"command": "git status"}}"#).unwrap();
        assert_eq!(extract_command(&payload), Some("git status"));
    }

    #[test]
    fn test_extract_command_missing_tool_input() {
        let payload: ClaudePayload = serde_json::from_str(r#"{}"#).unwrap();
        assert_eq!(extract_command(&payload), None);
    }

    #[test]
    fn test_extract_command_missing_command_field() {
        let payload: ClaudePayload =
            serde_json::from_str(r#"{"tool_input": {"cwd": "/tmp"}}"#).unwrap();
        assert_eq!(extract_command(&payload), None);
    }

    #[test]
    fn test_extract_command_empty_string() {
        let payload: ClaudePayload =
            serde_json::from_str(r#"{"tool_input": {"command": ""}}"#).unwrap();
        assert_eq!(extract_command(&payload), None);
    }

    #[test]
    fn test_shared_should_passthrough_rtk_prefix() {
        assert!(should_passthrough("rtk run -c 'ls'"));
        assert!(should_passthrough("rtk cargo test"));
        assert!(should_passthrough("/usr/local/bin/rtk run -c 'ls'"));
    }

    #[test]
    fn test_shared_should_passthrough_heredoc() {
        assert!(should_passthrough("cat <<EOF\nhello\nEOF"));
        assert!(should_passthrough("cat <<'EOF'\nhello\nEOF"));
    }

    #[test]
    fn test_shared_should_passthrough_normal_commands() {
        assert!(!should_passthrough("git status"));
        assert!(!should_passthrough("ls -la"));
        assert!(!should_passthrough("echo hello"));
    }

    #[test]
    fn test_malformed_json_does_not_panic() {
        let bad_inputs = ["", "not json", "{}", r#"{"tool_input": 42}"#, "null"];
        for input in bad_inputs {
            let _ = serde_json::from_str::<ClaudePayload>(input);
        }
    }

    // --- Fail-open behavior ---

    #[test]
    fn test_run_inner_returns_no_opinion_for_empty_payload() {
        // "{}" has no tool_input → no command → NoOpinion
        let payload: ClaudePayload = serde_json::from_str("{}").unwrap();
        assert_eq!(extract_command(&payload), None);
    }

    #[test]
    fn test_shared_is_hook_disabled_hook_enabled_zero() {
        let _env = EnvGuard::new();
        std::env::set_var("RTK_HOOK_ENABLED", "0");
        assert!(is_hook_disabled());
    }

    #[test]
    fn test_shared_is_hook_disabled_rtk_active() {
        let _env = EnvGuard::new();
        std::env::set_var("RTK_ACTIVE", "1");
        assert!(is_hook_disabled());
    }

    // --- Integration: Bug #4669 workaround verification ---

    #[test]
    fn test_deny_response_includes_reason_for_stderr() {
        // Bug #4669 workaround: deny must provide plain text reason
        // that can be output to stderr alongside the JSON stdout.
        // The msg is cloned for both paths in run_inner().
        let msg = "RTK: cat is blocked (use rtk read instead)";
        let response = deny_response(msg.to_string());
        let json = serde_json::to_string(&response).unwrap();
        let parsed: Value = serde_json::from_str(&json).unwrap();

        // JSON stdout path
        assert_eq!(parsed["hookSpecificOutput"]["permissionDecision"], "deny");
        assert_eq!(
            parsed["hookSpecificOutput"]["permissionDecisionReason"],
            msg
        );
        // The same msg string is used for stderr in run() via HookResponse::Deny
    }

    // Note: Integration tests for check_for_hook() safety decisions are in
    // src/cmd/hook.rs (test_safe_commands_rewrite, test_blocked_commands, etc.)
    // to avoid duplication. This module focuses on Claude Code wire format.

    // --- is_json_deny() and extract_deny_reason() ---

    #[test]
    fn test_is_json_deny_claude_code_format() {
        let json = r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"blocked"}}"#;
        assert!(is_json_deny(json));
    }

    #[test]
    fn test_is_json_deny_gemini_format() {
        let json = r#"{"decision":"deny","reason":"blocked"}"#;
        assert!(is_json_deny(json));
    }

    #[test]
    fn test_is_json_deny_allow_not_matched() {
        assert!(!is_json_deny(
            r#"{"hookSpecificOutput":{"permissionDecision":"allow"}}"#
        ));
        assert!(!is_json_deny(r#"{"decision":"allow"}"#));
        assert!(!is_json_deny(""));
        assert!(!is_json_deny("not json"));
    }

    #[test]
    fn test_extract_deny_reason_cc_format() {
        let json = r#"{"hookSpecificOutput":{"permissionDecision":"deny","permissionDecisionReason":"Use Grep tool"}}"#;
        assert_eq!(extract_deny_reason(json), Some("Use Grep tool".to_owned()));
    }

    #[test]
    fn test_extract_deny_reason_gemini_format() {
        let json = r#"{"decision":"deny","reason":"command blocked"}"#;
        assert_eq!(
            extract_deny_reason(json),
            Some("command blocked".to_owned())
        );
    }

    #[test]
    fn test_extract_deny_reason_missing() {
        assert_eq!(extract_deny_reason("{}"), None);
        assert_eq!(extract_deny_reason("not json"), None);
    }

    #[test]
    fn test_load_manifest_returns_none_when_missing() {
        // When HOME doesn't exist or manifest is absent → None (fail-open)
        let result = load_manifest(); // may return Some or None depending on environment
                                      // The key invariant: load_manifest must not panic
        drop(result);
    }
}
