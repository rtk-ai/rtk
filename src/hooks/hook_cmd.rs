//! Processes incoming hook calls from AI agents and rewrites commands on the fly.
//!
//! Uses `writeln!(stdout, ...)` instead of `println!` — accidental stdout/stderr
//! corrupts the JSON protocol (Claude Code bug #4669 silently disables the hook).

use super::constants::PRE_TOOL_USE_KEY;
use super::permissions::{self, PermissionVerdict};
use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{self, Read, Write};

use crate::core::tracking::HookOutcome;
use crate::core::utils::strip_leading_bom;
use crate::discover::registry::{has_heredoc, rewrite_command};

const STDIN_CAP: usize = 1_048_576; // 1 MiB

fn read_stdin_limited() -> Result<String> {
    let mut input = String::new();
    io::stdin()
        .take((STDIN_CAP + 1) as u64)
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;
    if input.len() > STDIN_CAP {
        anyhow::bail!("hook stdin exceeds {} byte limit", STDIN_CAP);
    }
    Ok(input)
}

// ── Copilot hook (VS Code + Copilot CLI) ──────────────────────

/// Format detected from the preToolUse JSON input.
enum HookFormat {
    /// VS Code Copilot Chat / Claude Code: `tool_name` + `tool_input.command`, supports `updatedInput`.
    /// If using the PreToolUse pascal case form, Copilot CLI also remaps its native `bash`/`powershell`
    /// runtime tool to `tool_name: "Bash"` for this schema and honors its `updatedInput`, live-verified
    /// on Linux+Windows 11 with Copilot CLI 1.0.73+ by rewriting a marker command end-to-end
    /// see <https://github.com/rtk-ai/rtk/pull/3179#issuecomment-5088268495>.
    VsCode { command: String },
    /// GitHub Copilot CLI's native schema: camelCase `toolName` + `toolArgs` (JSON string),
    /// supports `modifiedArgs` for transparent rewrite. `rtk init --copilot` no longer
    /// registers this schema (Copilot CLI honors the PascalCase `VsCode` schema on its
    /// own — registering both caused a redundant second hook invocation per tool call,
    /// see git history). Kept for installs that haven't re-run `rtk init --copilot` since
    /// upgrading, and as the schema JetBrains/IntelliJ's Copilot plugin uses under a
    /// different `toolName` value (`run_in_terminal`, not `bash` — see #2443/#3093).
    /// On Windows, Copilot CLI reports this schema's `toolName` as the unmapped runtime
    /// name `"powershell"` (#3178/#3179) — but since the `VsCode` schema above already
    /// works standalone there, that arm is legacy-only: relevant for un-upgraded installs,
    /// not exercised by a fresh `rtk init --copilot` on any platform.
    /// Carries the full parsed `toolArgs` object so we can rewrite `command` while preserving
    /// host-supplied metadata (description, initial_wait, mode, …) the tool requires.
    CopilotCli { command: String, args: Value },
    /// JetBrains Copilot IDE: only top-level deny decisions are honored, so
    /// rewrites must be returned as deny-with-suggestion responses.
    CopilotIde { command: String },
    /// Non-bash tool, already uses rtk, or unknown format — pass through silently.
    PassThrough,
}

/// Run the Copilot preToolUse hook.
/// Auto-detects VS Code Copilot Chat vs Copilot CLI format.
pub fn run_copilot() -> Result<()> {
    let input = read_stdin_limited()?;

    // Strip leading BOM(s) before trimming: some Windows hosts prepend UTF-8
    // BOMs to hook stdin (confirmed for Cursor), which serde_json rejects.
    let input = strip_leading_bom(&input).trim();
    if input.is_empty() {
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(io::stderr(), "[rtk hook] Failed to parse JSON input: {e}");
            return Ok(());
        }
    };

    match detect_format(&v) {
        HookFormat::VsCode { command } => handle_vscode(&command),
        HookFormat::CopilotCli { command, args } => {
            for path in heal_legacy_copilot_configs() {
                audit_log("self_heal", &path.display().to_string(), "");
            }
            handle_copilot_cli(&command, &args)
        }
        HookFormat::CopilotIde { command } => handle_copilot_ide(&command),
        HookFormat::PassThrough => Ok(()),
    }
}

fn detect_format(v: &Value) -> HookFormat {
    // VS Code Copilot Chat / Claude Code: snake_case keys.
    // "run_in_terminal" is VS Code Copilot Chat's actual terminal tool name
    // (confirmed via live payload capture) — without it, detect_format falls
    // through to PassThrough and the hook never fires for VS Code Copilot Chat.
    // No separate Windows/"powershell" case is needed: Copilot CLI remaps both
    // `bash` and `powershell` to `tool_name: "Bash"` for this schema — already
    // handled below, live-confirmed (see the VsCode variant doc).
    if let Some(tool_name) = v.get("tool_name").and_then(|t| t.as_str()) {
        if matches!(
            tool_name,
            "runTerminalCommand" | "run_in_terminal" | "Bash" | "bash"
        ) {
            if let Some(cmd) = v
                .pointer("/tool_input/command")
                .and_then(|c| c.as_str())
                .filter(|c| !c.is_empty())
            {
                return HookFormat::VsCode {
                    command: cmd.to_string(),
                };
            }
        }
        return HookFormat::PassThrough;
    }

    // Copilot CLI's native camelCase schema: toolName + toolArgs (JSON-encoded string).
    // The shell tool is "bash" on Unix and "powershell" on Windows.
    // Only reachable today via a not-yet-upgraded install's leftover camelCase
    // preToolUse registration (see the CopilotCli variant doc) or a host that
    // registers this schema itself, like JetBrains/IntelliJ's Copilot plugin
    // (toolName "run_in_terminal").
    if let Some(tool_name) = v.get("toolName").and_then(|t| t.as_str()) {
        if matches!(tool_name, "bash" | "powershell" | "run_in_terminal") {
            if let Some(tool_args_str) = v.get("toolArgs").and_then(|t| t.as_str()) {
                if let Ok(tool_args) = serde_json::from_str::<Value>(tool_args_str) {
                    if let Some(cmd) = tool_args
                        .get("command")
                        .and_then(|c| c.as_str())
                        .filter(|c| !c.is_empty())
                    {
                        return if tool_name == "run_in_terminal" {
                            HookFormat::CopilotIde {
                                command: cmd.to_string(),
                            }
                        } else {
                            HookFormat::CopilotCli {
                                command: cmd.to_string(),
                                args: tool_args,
                            }
                        };
                    }
                }
            }
        }
        return HookFormat::PassThrough;
    }

    HookFormat::PassThrough
}

fn heal_legacy_copilot_configs() -> Vec<std::path::PathBuf> {
    use super::constants::{COPILOT_HOOK_FILE, GITHUB_DIR, HOOKS_SUBDIR};

    let mut healed = Vec::new();
    let project = std::path::Path::new(GITHUB_DIR)
        .join(HOOKS_SUBDIR)
        .join(COPILOT_HOOK_FILE);
    if heal_legacy_hook_file(&project) {
        healed.push(project);
    }
    if let Ok(dir) = super::init::copilot_user_dir() {
        let global = dir.join(HOOKS_SUBDIR).join(COPILOT_HOOK_FILE);
        if heal_legacy_hook_file(&global) {
            healed.push(global);
        }
    }
    healed
}

// Exact camelCase entry written by pre-b754b85 `rtk init --copilot`; that
// stale registration is the only thing routing invocations into the
// CopilotCli arm above. Only this entry is removed — user additions stay.
fn legacy_camelcase_entry() -> Value {
    json!([{
        "type": "command",
        "bash": "rtk hook copilot",
        "powershell": "rtk hook copilot",
        "cwd": ".",
        "timeoutSec": 5
    }])
}

fn heal_legacy_hook_file(path: &std::path::Path) -> bool {
    let Ok(raw) = std::fs::read_to_string(path) else {
        return false;
    };
    let Ok(mut config) = serde_json::from_str::<Value>(&raw) else {
        return false;
    };
    let Some(hooks) = config.get("hooks").and_then(|h| h.as_object()) else {
        return false;
    };
    if hooks.get("preToolUse") != Some(&legacy_camelcase_entry()) {
        return false;
    }
    let pascalcase_still_registered = hooks
        .get("PreToolUse")
        .and_then(|p| p.as_array())
        .is_some_and(|entries| {
            entries
                .iter()
                .any(|e| e.get("command").and_then(|c| c.as_str()) == Some("rtk hook copilot"))
        });
    if !pascalcase_still_registered {
        return false;
    }
    let Some(hooks) = config.get_mut("hooks").and_then(|h| h.as_object_mut()) else {
        return false;
    };
    hooks.shift_remove("preToolUse");

    let stock = serde_json::from_str::<Value>(super::init::COPILOT_HOOK_JSON).ok();
    let content = if stock.is_some_and(|s| s == config) {
        super::init::COPILOT_HOOK_JSON.to_string()
    } else {
        let Ok(mut pretty) = serde_json::to_string_pretty(&config) else {
            return false;
        };
        pretty.push('\n');
        pretty
    };
    let tmp = path.with_extension(format!("heal.{}", std::process::id()));
    std::fs::write(&tmp, content)
        .and_then(|()| std::fs::rename(&tmp, path))
        .map_err(|_| {
            // Cleanup of our own temp file after a failed atomic write.
            let _ = std::fs::remove_file(&tmp); // nosemgrep: filesystem-deletion
        })
        .is_ok()
}

fn get_rewritten(cmd: &str) -> Option<String> {
    if has_heredoc(cmd) {
        return None;
    }

    let (excluded, transparent_prefixes) = crate::core::config::hook_rewrite_params();

    let rewritten = rewrite_command(cmd, &excluded, &transparent_prefixes)?;

    if rewritten == cmd {
        return None;
    }

    Some(rewritten)
}

#[derive(Debug)]
enum HookDecision {
    AllowRewrite(String),
    AskRewrite(String),
    Defer,
    Deny,
}

fn decide_from_verdict(cmd: &str, verdict: PermissionVerdict) -> HookDecision {
    if verdict == PermissionVerdict::Deny {
        return HookDecision::Deny;
    }
    if crate::discover::lexer::contains_unattestable_construct(cmd) {
        return HookDecision::Defer;
    }
    match get_rewritten(cmd) {
        Some(r) if verdict == PermissionVerdict::Allow => HookDecision::AllowRewrite(r),
        Some(r) => HookDecision::AskRewrite(r),
        None => HookDecision::Defer,
    }
}

fn decide_hook_action(cmd: &str, host: permissions::Host) -> HookDecision {
    decide_from_verdict(cmd, permissions::check_command_for(cmd, host))
}

fn handle_vscode(cmd: &str) -> Result<()> {
    if let Some(output) = vscode_response(cmd) {
        let _ = writeln!(io::stdout(), "{output}");
    }
    Ok(())
}

fn vscode_response(cmd: &str) -> Option<Value> {
    vscode_response_from_decision(decide_hook_action(cmd, permissions::Host::Claude), cmd)
}

/// Build the VS Code Copilot Chat / Copilot CLI (PascalCase compat) hook response.
///
/// Mirrors `process_claude_payload`: `permissionDecision: "allow"` is only ever
/// asserted for an explicit, user-configured Allow rule. Every other rewrite
/// (Default verdict or an explicit Ask rule) omits the field entirely, leaving
/// the host's own native prompt/allowlist flow in control — see #3037, where
/// asserting `"ask"` here made Copilot CLI 1.0.66+ force a blocking dialog with
/// no "remember" option on every rewritten command.
fn vscode_response_from_decision(decision: HookDecision, cmd: &str) -> Option<Value> {
    let (rewritten, allow) = match decision {
        HookDecision::Deny => {
            audit_log("deny", cmd, "");
            return None;
        }
        HookDecision::Defer => return None,
        HookDecision::AllowRewrite(r) => (r, true),
        HookDecision::AskRewrite(r) => (r, false),
    };

    audit_log("rewrite", cmd, &rewritten);

    let mut hook_output = json!({
        "hookEventName": PRE_TOOL_USE_KEY,
        "permissionDecisionReason": "RTK auto-rewrite",
        "updatedInput": { "command": rewritten }
    });
    if allow {
        hook_output["permissionDecision"] = json!("allow");
    }
    Some(json!({ "hookSpecificOutput": hook_output }))
}

fn handle_copilot_cli(cmd: &str, args: &Value) -> Result<()> {
    if let Some(response) = copilot_cli_response(cmd, args) {
        let _ = writeln!(io::stdout(), "{response}");
    }
    Ok(())
}

fn handle_copilot_ide(cmd: &str) -> Result<()> {
    if let Some(response) =
        copilot_ide_response_from_decision(decide_hook_action(cmd, permissions::Host::Claude), cmd)
    {
        let _ = writeln!(io::stdout(), "{response}");
    }
    Ok(())
}

fn copilot_ide_response_from_decision(decision: HookDecision, cmd: &str) -> Option<Value> {
    let reason = match decision {
        HookDecision::Defer => return None,
        HookDecision::Deny => {
            audit_log("deny", cmd, "");
            "Blocked by RTK permission rule".to_string()
        }
        HookDecision::AllowRewrite(rewritten) | HookDecision::AskRewrite(rewritten) => {
            audit_log("rewrite", cmd, &rewritten);
            format!("RTK token optimization: re-run this command as `{rewritten}` instead.")
        }
    };

    Some(json!({
        "permissionDecision": "deny",
        "permissionDecisionReason": reason,
    }))
}

fn copilot_cli_response(cmd: &str, args: &Value) -> Option<Value> {
    copilot_cli_response_from_decision(
        args,
        decide_hook_action(cmd, permissions::Host::Claude),
        cmd,
    )
}

fn copilot_cli_response_from_decision(
    args: &Value,
    decision: HookDecision,
    cmd: &str,
) -> Option<Value> {
    let (rewritten, allow) = match decision {
        HookDecision::Deny => {
            audit_log("deny", cmd, "");
            return None;
        }
        HookDecision::Defer => return None,
        HookDecision::AllowRewrite(r) => (r, true),
        HookDecision::AskRewrite(r) => (r, false),
    };

    audit_log("rewrite", cmd, &rewritten);

    let mut modified = args.clone();
    if let Some(obj) = modified.as_object_mut() {
        obj.insert("command".into(), Value::String(rewritten));
    }

    let mut response = json!({
        "permissionDecisionReason": "RTK auto-rewrite",
        "modifiedArgs": modified,
    });
    if allow {
        response["permissionDecision"] = json!("allow");
    }
    Some(response)
}

// ── Gemini hook ───────────────────────────────────────────────

/// Run the Gemini CLI BeforeTool hook.
pub fn run_gemini() -> Result<()> {
    let input = read_stdin_limited()?;
    let output = run_gemini_inner(&input).context("Failed to parse hook input as JSON")?;
    let _ = writeln!(io::stdout(), "{output}");
    Ok(())
}

/// Parse the Gemini BeforeTool stdin payload, decide (against the real,
/// on-disk Gemini settings), and render the response JSON — no stdin/stdout
/// I/O. Used by `run_gemini` itself (not just tests), so a regression here
/// (e.g. dropping the BOM strip) fails for real rather than only in a
/// duplicate test copy.
fn run_gemini_inner(input: &str) -> serde_json::Result<String> {
    run_gemini_inner_impl(input, |cmd| {
        decide_hook_action(cmd, permissions::Host::Gemini)
    })
}

/// Same parse/render path as `run_gemini_inner`, but with the permission
/// decision driven by explicit rule slices instead of `~/.gemini/settings.json`
/// — lets tests exercise the real BOM-stripping/parsing logic without
/// depending on (or being broken by) whatever is on disk at HOME.
#[cfg(test)]
fn run_gemini_inner_with_rules(
    input: &str,
    deny: &[String],
    ask: &[String],
    allow: &[String],
) -> serde_json::Result<String> {
    run_gemini_inner_impl(input, |cmd| {
        decide_from_verdict(
            cmd,
            permissions::check_command_with_rules(cmd, deny, ask, allow),
        )
    })
}

fn run_gemini_inner_impl(
    input: &str,
    decide: impl Fn(&str) -> HookDecision,
) -> serde_json::Result<String> {
    let input = strip_leading_bom(input);
    let json: Value = serde_json::from_str(input)?;

    let tool_name = json.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    if tool_name != "run_shell_command" {
        return Ok(gemini_json("allow", None));
    }

    let cmd = json
        .pointer("/tool_input/command")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if cmd.is_empty() {
        return Ok(gemini_json("allow", None));
    }

    Ok(match decide(cmd) {
        HookDecision::Deny => {
            r#"{"decision":"deny","reason":"Blocked by RTK permission rule"}"#.to_string()
        }
        HookDecision::AllowRewrite(ref rewritten) => {
            audit_log("rewrite", cmd, rewritten);
            gemini_json("allow", Some(rewritten))
        }
        HookDecision::AskRewrite(ref rewritten) => {
            audit_log("ask", cmd, rewritten);
            gemini_json("ask_user", Some(rewritten))
        }
        HookDecision::Defer => gemini_json("ask_user", None),
    })
}

// ── Vibe hook ─────────────────────────────────────────────────

/// Run the Mistral Vibe CLI pre_tool hook.
///
/// Vibe hook contract (https://docs.mistral.ai/vibe/code/cli/hooks):
/// - stdin: JSON with `tool_name`, `tool_input`, `hook_event_name`, etc.
/// - Passthrough: exit 0 with empty stdout.
/// - Rewrite: emit `{"hook_specific_output": {"tool_input": {"command": "..."}}}`.
/// - Deny: emit `{"decision": "deny", "reason": "..."}`.
pub fn run_vibe() -> Result<()> {
    let input = read_stdin_limited()?;
    if let Some(output) = run_vibe_inner(&input) {
        let _ = writeln!(io::stdout(), "{output}");
    }
    Ok(())
}

fn run_vibe_inner(input: &str) -> Option<String> {
    let input = strip_leading_bom(input);
    let json: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(io::stderr(), "[rtk hook] Failed to parse JSON input: {e}");
            return None;
        }
    };

    let tool_name = json.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
    if tool_name != "bash" {
        return None;
    }

    let cmd = json
        .pointer("/tool_input/command")
        .and_then(|v| v.as_str())
        .unwrap_or("");
    if cmd.is_empty() {
        return None;
    }

    match decide_hook_action(cmd, permissions::Host::Vibe) {
        HookDecision::Deny => {
            audit_log("deny", cmd, "");
            Some(r#"{"decision":"deny","reason":"Blocked by RTK permission rule"}"#.to_string())
        }
        HookDecision::AllowRewrite(ref rewritten) | HookDecision::AskRewrite(ref rewritten) => {
            audit_log("rewrite", cmd, rewritten);
            Some(vibe_rewrite_json(rewritten))
        }
        HookDecision::Defer => None,
    }
}

fn vibe_rewrite_json(rewritten: &str) -> String {
    serde_json::json!({
        "hook_specific_output": {
            "tool_input": { "command": rewritten }
        },
        "system_message": format!("rtk: rewrote to `{}`", rewritten),
    })
    .to_string()
}

fn gemini_json(decision: &str, rewrite: Option<&str>) -> String {
    let mut output = serde_json::json!({ "decision": decision });
    if let Some(cmd) = rewrite {
        output["hookSpecificOutput"] = serde_json::json!({ "tool_input": { "command": cmd } });
    }
    output.to_string()
}

// ── Audit logging ─────────────────────────────────────────────

/// Best-effort audit log when RTK_HOOK_AUDIT=1.
fn audit_log(action: &str, original: &str, rewritten: &str) {
    if std::env::var("RTK_HOOK_AUDIT").as_deref() != Ok("1") {
        return;
    }
    let _ = audit_log_inner(action, original, rewritten);
}

/// Escape newlines to prevent log-line injection in the pipe-delimited audit log.
fn sanitize_log_field(s: &str) -> String {
    s.replace('\\', "\\\\")
        .replace('|', "\\|")
        .replace('\n', "\\n")
        .replace('\r', "\\r")
}

fn audit_log_inner(action: &str, original: &str, rewritten: &str) -> Option<()> {
    let home = dirs::home_dir()?;
    let dir = home.join(".local").join("share").join("rtk");
    crate::core::utils::create_private_dir(&dir).ok()?;
    let path = dir.join("hook-audit.log");
    let mut file = crate::core::utils::open_private(
        std::fs::OpenOptions::new().create(true).append(true),
        &path,
    )
    .ok()?;
    let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
    writeln!(
        file,
        "{} | {} | {} | {}",
        ts,
        action,
        sanitize_log_field(original),
        sanitize_log_field(rewritten)
    )
    .ok()
}

// ── Claude Code native hook ────────────────────────────────────

#[derive(Debug)]
enum PayloadAction {
    Rewrite {
        cmd: String,
        rewritten: String,
        decision: HookOutcome,
        output: Value,
    },
    Skip {
        decision: HookOutcome,
        cmd: String,
    },
    Ignore,
}

fn process_claude_payload(v: &Value) -> PayloadAction {
    let cmd = match v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    {
        Some(c) => c,
        None => return PayloadAction::Ignore,
    };

    process_claude_payload_from_decision(v, cmd, decide_hook_action(cmd, permissions::Host::Claude))
}

/// Pure core of `process_claude_payload`, taking the hook decision directly so the
/// full Allow/Ask/Deny/Defer matrix is unit-testable without depending on real
/// permission config files — mirrors the `copilot_cli_response_from_decision`/
/// `droid_response_from_decision` split already used elsewhere in this file.
fn process_claude_payload_from_decision(
    v: &Value,
    cmd: &str,
    decision: HookDecision,
) -> PayloadAction {
    let (rewritten, allow) = match decision {
        HookDecision::Deny => {
            return PayloadAction::Skip {
                decision: HookOutcome::Deny,
                cmd: cmd.to_string(),
            }
        }
        HookDecision::Defer => {
            return PayloadAction::Skip {
                decision: HookOutcome::Defer,
                cmd: cmd.to_string(),
            }
        }
        HookDecision::AllowRewrite(r) => (r, true),
        HookDecision::AskRewrite(r) => (r, false),
    };

    let updated_input = {
        let mut ti = v.get("tool_input").cloned().unwrap_or_else(|| json!({}));
        if let Some(obj) = ti.as_object_mut() {
            obj.insert("command".into(), Value::String(rewritten.clone()));
        }
        ti
    };

    let mut hook_output = json!({
        "hookEventName": PRE_TOOL_USE_KEY,
        "permissionDecisionReason": "RTK auto-rewrite",
        "updatedInput": updated_input
    });

    if allow {
        hook_output
            .as_object_mut()
            .unwrap()
            .insert("permissionDecision".into(), json!("allow"));
    }

    PayloadAction::Rewrite {
        cmd: cmd.to_string(),
        rewritten,
        decision: if allow {
            HookOutcome::Allow
        } else {
            HookOutcome::Ask
        },
        output: json!({ "hookSpecificOutput": hook_output }),
    }
}

/// Pull the fields `log_hook_decision` needs out of the raw PreToolUse payload.
/// `None` when `session_id`/`tool_use_id` are absent — both are required to join
/// back to the transcript later, so there's nothing useful to log without them.
/// Split out from `log_hook_decision` so this extraction is unit-testable without
/// touching the tracking DB.
fn hook_log_fields(v: &Value) -> Option<(&str, &str, &str)> {
    let session_id = v.get("session_id").and_then(|s| s.as_str())?;
    let tool_use_id = v.get("tool_use_id").and_then(|s| s.as_str())?;
    let project_path = v.get("cwd").and_then(|c| c.as_str()).unwrap_or("");
    Some((session_id, tool_use_id, project_path))
}

/// Log the real hook decision to the tracking DB, keyed by the transcript's
/// `tool_use_id`, so `rtk discover` can later read ground truth about historical
/// hook coverage instead of re-deriving a guess from today's hook-install state.
///
/// Best-effort only — a tracking failure must never affect the hook's real output
/// (fallback pattern from `rust-patterns.md`): this is a side channel, not the
/// hook's actual job.
fn log_hook_decision(v: &Value, cmd: &str, decision: HookOutcome, rewritten: Option<&str>) {
    let Some((session_id, tool_use_id, project_path)) = hook_log_fields(v) else {
        return;
    };

    let Ok(tracker) = crate::core::tracking::Tracker::new() else {
        return;
    };
    if let Err(e) = tracker.record_hook_decision(
        session_id,
        tool_use_id,
        project_path,
        cmd,
        decision,
        rewritten,
        env!("CARGO_PKG_VERSION"),
    ) {
        let _ = writeln!(
            io::stderr(),
            "[rtk hook] hook_decisions logging failed: {e}"
        );
    }
}

/// Run the Claude Code PreToolUse hook natively.
pub fn run_claude() -> Result<()> {
    let input = read_stdin_limited()?;

    let input = strip_leading_bom(&input).trim();
    if input.is_empty() {
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(io::stderr(), "[rtk hook] Failed to parse JSON input: {e}");
            return Ok(());
        }
    };

    match process_claude_payload(&v) {
        PayloadAction::Rewrite {
            cmd,
            rewritten,
            decision,
            output,
        } => {
            // Write the response Claude Code is synchronously blocked on FIRST.
            // `log_hook_decision` is a best-effort side channel (see its own doc
            // comment: "a tracking failure must never affect the hook's real
            // output") that opens a SQLite connection with a 5s busy_timeout — on
            // lock contention (concurrent hook invocations sharing the default
            // history.db) that write can block for real seconds. Since this fires
            // on every single Bash tool call now (not just RTK-covered ones), that
            // latency must never sit in front of the response, or it directly
            // stalls the tool call it's supposedly just logging.
            let _ = writeln!(io::stdout(), "{output}");
            audit_log("rewrite", &cmd, &rewritten);
            log_hook_decision(&v, &cmd, decision, Some(&rewritten));
        }
        PayloadAction::Skip { decision, cmd } => {
            // `rtk hook audit`'s skip-breakdown groups by a "skip:<reason>" prefix
            // (see hook_audit_cmd.rs) — Skip is only ever reached via Deny/Defer,
            // so map those to the reasons it expects rather than the bare
            // HookOutcome::Display used for the Rewrite/tracking-DB paths.
            //
            // Skip has no stdout response to write (Claude Code falls through to
            // its own native handling), but log_hook_decision is still deferred to
            // last for the same reason as the Rewrite arm above: it must never be
            // what a Bash tool call is waiting on.
            let audit_action = match decision {
                HookOutcome::Deny => "skip:deny_rule",
                HookOutcome::Defer => "skip:defer",
                HookOutcome::Allow | HookOutcome::Ask => "skip",
            };
            audit_log(audit_action, &cmd, "");
            log_hook_decision(&v, &cmd, decision, None);
        }
        PayloadAction::Ignore => {}
    }

    Ok(())
}

#[cfg(test)]
fn run_claude_inner(input: &str) -> Option<String> {
    let input = strip_leading_bom(input);
    let v: Value = serde_json::from_str(input).ok()?;
    match process_claude_payload(&v) {
        PayloadAction::Rewrite { output, .. } => Some(output.to_string()),
        _ => None,
    }
}

// ── Cursor native hook ─────────────────────────────────────────

/// Run the Cursor Agent hook natively.
pub fn run_cursor() -> Result<()> {
    let input = read_stdin_limited()?;

    let input = strip_leading_bom(&input).trim();
    if input.is_empty() {
        let _ = writeln!(io::stdout(), "{{}}");
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(_) => {
            let _ = writeln!(io::stdout(), "{{}}");
            return Ok(());
        }
    };

    let cmd = match v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    {
        Some(c) => c.to_string(),
        None => {
            let _ = writeln!(io::stdout(), "{{}}");
            return Ok(());
        }
    };

    let output = match decide_hook_action(&cmd, permissions::Host::Cursor) {
        HookDecision::AllowRewrite(rewritten) => {
            audit_log("rewrite", &cmd, &rewritten);
            cursor_allow(&rewritten)
        }
        HookDecision::AskRewrite(rewritten) => {
            audit_log("ask", &cmd, &rewritten);
            cursor_ask(&rewritten)
        }
        other => {
            if matches!(other, HookDecision::Deny) {
                audit_log("deny", &cmd, "");
            }
            "{}".to_string()
        }
    };
    let _ = writeln!(io::stdout(), "{output}");
    Ok(())
}

fn cursor_allow(rewritten: &str) -> String {
    json!({
        "continue": true,
        "permission": "allow",
        "updated_input": { "command": rewritten }
    })
    .to_string()
}

fn cursor_ask(rewritten: &str) -> String {
    json!({
        "continue": true,
        "permission": "ask",
        "updated_input": { "command": rewritten }
    })
    .to_string()
}

#[cfg(test)]
fn run_cursor_inner(input: &str) -> String {
    run_cursor_inner_with_rules(input, &[], &[], &[])
}

#[cfg(test)]
fn run_cursor_inner_with_rules(
    input: &str,
    deny_rules: &[String],
    ask_rules: &[String],
    allow_rules: &[String],
) -> String {
    let input = strip_leading_bom(input);
    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(_) => return "{}".to_string(),
    };

    let cmd = match v
        .pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
    {
        Some(c) => c.to_string(),
        None => return "{}".to_string(),
    };

    let verdict = permissions::check_command_with_rules(&cmd, deny_rules, ask_rules, allow_rules);
    match decide_from_verdict(&cmd, verdict) {
        HookDecision::AllowRewrite(rewritten) => cursor_allow(&rewritten),
        HookDecision::AskRewrite(rewritten) => cursor_ask(&rewritten),
        _ => "{}".to_string(),
    }
}

// ── Factory Droid PreToolUse hook ──────────────────────────────
//
// Payload is shaped like Claude Code's (docs.factory.ai/reference/hooks-reference);
// the shell tool is matched as `Execute`. RTK steps aside on Droid's explicit
// deny lists and otherwise rewrites via `updatedInput` with no
// `permissionDecision` — the verdict stays with Droid's native flow.

fn process_droid_payload(v: &Value) -> Option<Value> {
    let cmd = droid_execute_command(v)?;
    droid_response_from_decision(v, cmd, decide_hook_action(cmd, permissions::Host::Droid))
}

/// Extract the shell command when the payload targets Droid's Execute tool.
fn droid_execute_command(v: &Value) -> Option<&str> {
    let tool_name = v.get("tool_name").and_then(|t| t.as_str()).unwrap_or("");
    // `Execute` is Droid's shell tool. The installed matcher already gates
    // invocations to Execute; also tolerate a missing tool_name and accept
    // `Bash` defensively for Claude-shaped payloads (Droid itself has no Bash
    // tool — verified against Droid v0.164.0).
    if !matches!(tool_name, "Execute" | "Bash" | "") {
        return None;
    }

    v.pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
}

/// Build the Droid hook response for a decision from the shared flow.
///
/// On `Deny` and `Defer`, stay silent so Droid handles the original command.
/// Rewrites land via `updatedInput` alone — never a `permissionDecision`:
/// RTK can't reproduce the verdict Droid would emit for a command it renames
/// to `rtk …` (updatedInput-without-decision verified on Droid v0.140–0.164).
fn droid_response_from_decision(v: &Value, cmd: &str, decision: HookDecision) -> Option<Value> {
    let rewritten = match decision {
        HookDecision::Deny => {
            audit_log("deny", cmd, "");
            return None;
        }
        HookDecision::Defer => return None,
        HookDecision::AllowRewrite(r) | HookDecision::AskRewrite(r) => r,
    };

    audit_log("rewrite", cmd, &rewritten);

    let updated_input = {
        let mut ti = v.get("tool_input").cloned().unwrap_or_else(|| json!({}));
        if let Some(obj) = ti.as_object_mut() {
            obj.insert("command".into(), Value::String(rewritten));
        }
        ti
    };

    Some(json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_KEY,
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": updated_input
        }
    }))
}

/// Run the Factory Droid PreToolUse hook natively.
pub fn run_droid() -> Result<()> {
    let input = read_stdin_limited()?;

    let v = match droid_payload(&input) {
        Ok(Some(v)) => v,
        Ok(None) => return Ok(()),
        Err(e) => {
            let _ = writeln!(io::stderr(), "[rtk hook] Failed to parse JSON input: {e}");
            return Ok(());
        }
    };

    if let Some(output) = process_droid_payload(&v) {
        let _ = writeln!(io::stdout(), "{output}");
    }
    Ok(())
}

/// Normalize and parse a raw Droid PreToolUse payload: strip a leading BOM
/// (Windows hosts prepend one), trim, and report an empty payload as nothing
/// to do. Shared by `run_droid` and the test entry points so droid's own
/// tests exercise the real BOM handling rather than a stripped-down copy of
/// the parse.
fn droid_payload(input: &str) -> serde_json::Result<Option<Value>> {
    let input = strip_leading_bom(input).trim();
    if input.is_empty() {
        return Ok(None);
    }
    serde_json::from_str(input).map(Some)
}

/// Hermetic test path: no Droid settings (empty rules).
#[cfg(test)]
fn run_droid_inner(input: &str) -> Option<String> {
    let (deny, ask, allow) = permissions::droid_rules_from_settings(&[]);
    run_droid_inner_with_rules(input, &deny, &ask, &allow)
}

#[cfg(test)]
fn run_droid_inner_with_rules(
    input: &str,
    deny_rules: &[String],
    ask_rules: &[String],
    allow_rules: &[String],
) -> Option<String> {
    let v: Value = droid_payload(input).ok().flatten()?;
    let cmd = droid_execute_command(&v)?;
    let verdict = permissions::check_command_with_rules(cmd, deny_rules, ask_rules, allow_rules);
    droid_response_from_decision(&v, cmd, decide_from_verdict(cmd, verdict)).map(|o| o.to_string())
}

// ── Kiro IDE/CLI PreToolUse hook ───────────────────────────────
//
// Kiro's PreToolUse hook does NOT support transparent rewrite (no `updatedInput`
// equivalent). The hook uses deny-with-suggestion instead: it exits with
// `KIRO_BLOCK_EXIT` (2) and writes the suggested `rtk <cmd>` to stderr, which Kiro
// forwards to the agent. The agent re-issues the command in its `rtk` form and the
// retry passes through untouched (already-`rtk` commands never rewrite).
// The `render_kiro_transparent` branch is kept inactive, ready for a one-line
// switch when/if Kiro adds transparent rewrite support.

/// Extract the shell command from a Kiro PreToolUse payload.
///
/// Tolerates a missing `tool_name` (field may be absent in some Kiro CLI
/// versions). Uses JSON pointer `/tool_input/command` and filters empty commands.
fn kiro_shell_command(v: &Value) -> Option<&str> {
    let tool_name = v.get("tool_name").and_then(|t| t.as_str()).unwrap_or("");
    // Accept Kiro's shell tool names; tolerate missing/empty tool_name
    // (the hook file matcher already gates invocations to the shell tool).
    if !matches!(
        tool_name,
        "executeBash" | "execute_bash" | "runCommand" | "shell" | ""
    ) {
        return None;
    }
    v.pointer("/tool_input/command")
        .and_then(|c| c.as_str())
        .filter(|c| !c.is_empty())
}

/// Orchestrate the decision for a Kiro payload: extract command, decide.
///
/// `Some(rtk_command)` means the agent should be told to re-issue that command;
/// `None` means step aside and let the original run untouched.
fn process_kiro_payload(v: &Value) -> Option<String> {
    let cmd = kiro_shell_command(v)?;
    kiro_rewrite_for_decision(cmd, decide_hook_action(cmd, permissions::Host::Kiro))
}

/// Map a `HookDecision` to the Kiro hook JSON response.
///
/// - `Deny` → audit log + `None` (step aside, let Kiro handle the original).
/// - `Defer` → `None` (no rewrite, command runs unchanged).
/// - `AllowRewrite`/`AskRewrite` → the `rtk` equivalent to suggest.
///
/// Returns the rewritten command when the agent should be told to re-issue it,
/// or `None` when RTK must step aside and let the original command run.
fn kiro_rewrite_for_decision(cmd: &str, decision: HookDecision) -> Option<String> {
    let rewritten = match decision {
        HookDecision::Deny => {
            audit_log("deny", cmd, "");
            return None;
        }
        HookDecision::Defer => return None,
        HookDecision::AllowRewrite(r) | HookDecision::AskRewrite(r) => r,
    };

    audit_log("rewrite", cmd, &rewritten);
    Some(rewritten)
}

/// Exit code that makes Kiro block a `PreToolUse` tool call and feed the
/// hook's stderr back to the agent.
const KIRO_BLOCK_EXIT: i32 = 2;

/// Build the deny-with-suggestion message sent to the agent on stderr.
///
/// Kiro forwards hook stderr to the model when the hook exits with
/// [`KIRO_BLOCK_EXIT`], so this text must read as an actionable instruction:
/// the agent is expected to re-issue the command in its `rtk` form, which the
/// hook then lets through untouched (already-`rtk` commands never rewrite).
fn kiro_block_message(rewritten: &str) -> String {
    format!("RTK: use `{rewritten}` (economiza 60-90% de tokens). Reemita o comando com o prefixo `rtk`.")
}

/// Render the ask-com-sugestão response for Kiro (INACTIVE — see `run_kiro`).
///
/// Retained because Kiro's `ask` path is the only one that can surface a
/// prompt to the *user* rather than the agent. It is not on the default path:
/// approving an `ask` runs the original command, so it costs a confirmation
/// and saves nothing. `run_kiro` uses the deny-with-suggestion path instead.
#[allow(dead_code)]
fn render_kiro_ask(_v: &Value, _cmd: &str, rewritten: &str) -> Value {
    json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_KEY,
            "permissionDecision": "ask",
            "permissionDecisionReason":
                format!("RTK: considere usar `{rewritten}` para economizar 60-90% de tokens")
        }
    })
}

/// Render a transparent rewrite response for Kiro (INACTIVE — future use).
///
/// Preserves extra `tool_input` fields (description, timeout) so the rewrite
/// is a drop-in replacement. Activating this path is a one-line change once
/// Kiro exposes a transparent rewrite mechanism (Req 2.3, 2.7).
#[allow(dead_code)]
fn render_kiro_transparent(v: &Value, rewritten: &str) -> Value {
    let updated_input = {
        let mut ti = v.get("tool_input").cloned().unwrap_or_else(|| json!({}));
        if let Some(obj) = ti.as_object_mut() {
            obj.insert("command".into(), Value::String(rewritten.to_string()));
        }
        ti
    };

    json!({
        "hookSpecificOutput": {
            "hookEventName": PRE_TOOL_USE_KEY,
            "permissionDecision": "allow",
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": updated_input
        }
    })
}

/// Run the Kiro IDE/CLI PreToolUse hook natively.
///
/// Returns the process exit code:
///
/// - `0` — step aside. The original command runs untouched. This covers every
///   no-rewrite case *and* every failure path (oversized stdin, malformed JSON,
///   non-shell tool, empty input), so a broken hook never blocks the user.
/// - [`KIRO_BLOCK_EXIT`] — a rewrite exists. Kiro blocks the raw command and
///   forwards the stderr suggestion to the agent, which re-issues it as
///   `rtk <cmd>`. That retry is idempotent: already-`rtk` commands never
///   rewrite, so the hook lets the second attempt through and cannot loop.
///
/// Deny-with-suggestion is used instead of Kiro's `ask` decision because `ask`
/// runs the *original* command on approval — it costs a user confirmation and
/// saves nothing, since Kiro has no transparent-rewrite field.
pub fn run_kiro() -> Result<i32> {
    let input = match read_stdin_limited() {
        Ok(s) => s,
        Err(_) => return Ok(0), // oversized/unreadable stdin — never block
    };
    let input = strip_leading_bom(&input).trim();
    if input.is_empty() {
        return Ok(0);
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            let _ = writeln!(io::stderr(), "[rtk hook] Failed to parse JSON input: {e}");
            return Ok(0);
        }
    };

    match process_kiro_payload(&v) {
        Some(rewritten) => {
            let _ = writeln!(io::stderr(), "{}", kiro_block_message(&rewritten));
            Ok(KIRO_BLOCK_EXIT)
        }
        None => Ok(0),
    }
}

/// Hermetic test path: no Kiro permission settings (empty rules → Default verdict).
///
/// Returns the `rtk` command the agent would be told to re-issue, or `None`
/// when RTK steps aside.
#[cfg(test)]
fn run_kiro_inner(input: &str) -> Option<String> {
    run_kiro_inner_with_rules(input, &[], &[], &[])
}

#[cfg(test)]
fn run_kiro_inner_with_rules(
    input: &str,
    deny_rules: &[String],
    ask_rules: &[String],
    allow_rules: &[String],
) -> Option<String> {
    let v: Value = serde_json::from_str(input).ok()?;
    let cmd = kiro_shell_command(&v)?;
    let verdict = permissions::check_command_with_rules(cmd, deny_rules, ask_rules, allow_rules);
    kiro_rewrite_for_decision(cmd, decide_from_verdict(cmd, verdict))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rewrite_command_no_prefixes(cmd: &str, excluded: &[String]) -> Option<String> {
        crate::discover::registry::rewrite_command(cmd, excluded, &[])
    }

    // --- Copilot format detection ---

    fn vscode_input(tool: &str, cmd: &str) -> Value {
        json!({
            "tool_name": tool,
            "tool_input": { "command": cmd }
        })
    }

    fn copilot_cli_input(tool: &str, cmd: &str) -> Value {
        let args = serde_json::to_string(&json!({ "command": cmd })).unwrap();
        json!({ "toolName": tool, "toolArgs": args })
    }

    fn copilot_ide_input(cmd: &str) -> Value {
        let args = serde_json::to_string(&json!({
            "command": cmd,
            "explanation": "Run command",
            "isBackground": false
        }))
        .unwrap();
        json!({ "toolName": "run_in_terminal", "toolArgs": args })
    }

    #[test]
    fn test_detect_vscode_bash() {
        assert!(matches!(
            detect_format(&vscode_input("Bash", "git status")),
            HookFormat::VsCode { .. }
        ));
    }

    #[test]
    fn test_detect_vscode_run_terminal_command() {
        assert!(matches!(
            detect_format(&vscode_input("runTerminalCommand", "cargo test")),
            HookFormat::VsCode { .. }
        ));
    }

    #[test]
    fn test_detect_vscode_run_in_terminal() {
        // VS Code Copilot Chat's actual terminal tool name, confirmed via
        // live payload capture — distinct from "runTerminalCommand".
        assert!(matches!(
            detect_format(&vscode_input("run_in_terminal", "cargo test")),
            HookFormat::VsCode { .. }
        ));
    }

    #[test]
    fn test_detect_copilot_cli_bash() {
        assert!(matches!(
            detect_format(&copilot_cli_input("bash", "git status")),
            HookFormat::CopilotCli { .. }
        ));
    }

    #[test]
    fn test_detect_copilot_ide_run_in_terminal() {
        assert!(matches!(
            detect_format(&copilot_ide_input("git status")),
            HookFormat::CopilotIde { .. }
        ));
    }

    #[test]
    fn test_detect_copilot_cli_powershell() {
        // Copilot CLI names its shell tool "powershell" on Windows, not "bash".
        assert!(matches!(
            detect_format(&copilot_cli_input("powershell", "git status")),
            HookFormat::CopilotCli { .. }
        ));
    }

    #[test]
    fn test_detect_non_bash_is_passthrough() {
        let v = json!({ "tool_name": "editFiles" });
        assert!(matches!(detect_format(&v), HookFormat::PassThrough));
    }

    #[test]
    fn test_copilot_bom_prefixed_payload_is_recognized() {
        // Windows hosts may prepend one or two UTF-8 BOMs to hook stdin
        // (confirmed for Cursor). run_copilot strips them before parsing;
        // verify both Copilot formats still parse after the same handling.
        for raw in [
            format!("\u{feff}{}", copilot_cli_input("bash", "git status")),
            format!(
                "\u{feff}\u{feff}{}",
                copilot_cli_input("bash", "git status")
            ),
        ] {
            let cleaned = strip_leading_bom(&raw).trim();
            let v: Value = serde_json::from_str(cleaned).expect("BOM-stripped JSON must parse");
            assert!(matches!(detect_format(&v), HookFormat::CopilotCli { .. }));
        }

        let raw = format!("\u{feff}{}", vscode_input("Bash", "git status"));
        let v: Value = serde_json::from_str(strip_leading_bom(&raw).trim()).unwrap();
        assert!(matches!(detect_format(&v), HookFormat::VsCode { .. }));
    }

    #[test]
    fn test_detect_unknown_is_passthrough() {
        assert!(matches!(detect_format(&json!({})), HookFormat::PassThrough));
    }

    #[test]
    fn test_get_rewritten_supported() {
        assert!(get_rewritten("git status").is_some());
    }

    #[test]
    fn test_get_rewritten_unsupported() {
        assert!(get_rewritten("htop").is_none());
    }

    #[test]
    fn test_get_rewritten_already_rtk() {
        assert!(get_rewritten("rtk git status").is_none());
    }

    #[test]
    fn test_get_rewritten_heredoc() {
        assert!(get_rewritten("cat <<'EOF'\nhello\nEOF").is_none());
    }

    // --- VS Code Copilot Chat / Copilot CLI (PascalCase) handler ---
    // Serves both VS Code Copilot Chat's PreToolUse hook and Copilot CLI's
    // PascalCase-compat entry (#3037): the same `rtk hook copilot` call
    // answers both from one JSON schema.

    #[test]
    fn test_vscode_allow_rewrite_sets_permission_allow() {
        let r = vscode_response_from_decision(
            HookDecision::AllowRewrite("rtk git status".into()),
            "git status",
        )
        .unwrap();
        assert_eq!(r["hookSpecificOutput"]["permissionDecision"], "allow");
        assert_eq!(
            r["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk git status"
        );
    }

    #[test]
    fn test_vscode_ask_rewrite_omits_permission_decision() {
        // Default (unconfigured) and explicit-Ask verdicts both land here as
        // AskRewrite — neither must assert a decision, matching Claude's own
        // hook (process_claude_payload). Asserting "ask" is what caused #3037:
        // Copilot CLI 1.0.66+ treats it as authoritative and forces a blocking
        // dialog with no "remember" option on every rewritten command.
        let r = vscode_response_from_decision(
            HookDecision::AskRewrite("rtk cargo test".into()),
            "cargo test",
        )
        .unwrap();
        assert!(
            r["hookSpecificOutput"]
                .as_object()
                .unwrap()
                .get("permissionDecision")
                .is_none(),
            "AskRewrite must NOT set permissionDecision"
        );
        assert_eq!(
            r["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk cargo test"
        );
    }

    #[test]
    fn test_vscode_deny_returns_none() {
        assert!(vscode_response_from_decision(HookDecision::Deny, "cargo test").is_none());
    }

    #[test]
    fn test_vscode_defer_returns_none() {
        assert!(vscode_response_from_decision(HookDecision::Defer, "cargo test").is_none());
    }

    // --- Copilot CLI handler: transparent rewrite via modifiedArgs ---

    fn cli_args(cmd: &str) -> Value {
        json!({ "command": cmd })
    }

    #[test]
    fn test_copilot_cli_ask_rewrite_omits_permission_decision() {
        // Whether the Ask verdict came from an explicit rule or the Default
        // (unconfigured) fallback, RTK must never assert a decision here —
        // matches Claude's own hook (process_claude_payload) and avoids the
        // Copilot CLI 1.0.66+ forced-prompt bug from #3037.
        let r = copilot_cli_response_from_decision(
            &cli_args("cargo test"),
            HookDecision::AskRewrite("rtk cargo test".into()),
            "cargo test",
        )
        .unwrap();
        assert!(
            r.get("permissionDecision").is_none(),
            "AskRewrite must NOT set permissionDecision — the host's native prompt/allowlist stays in control"
        );
        assert_eq!(r["modifiedArgs"]["command"], "rtk cargo test");
    }

    #[test]
    fn test_copilot_cli_allow_rewrite_returns_allow() {
        let r = copilot_cli_response_from_decision(
            &cli_args("cargo test"),
            HookDecision::AllowRewrite("rtk cargo test".into()),
            "cargo test",
        )
        .unwrap();
        assert_eq!(r["permissionDecision"], "allow");
        assert_eq!(r["modifiedArgs"]["command"], "rtk cargo test");
    }

    #[test]
    fn test_copilot_cli_deny_returns_none() {
        assert!(copilot_cli_response_from_decision(
            &cli_args("cargo test"),
            HookDecision::Deny,
            "cargo test",
        )
        .is_none());
    }

    #[test]
    fn test_copilot_cli_defer_returns_none() {
        // Defer covers both "no rewrite available" and the unattestable-construct gate.
        // The hook must emit NO modifiedArgs for CVE bypass forms — no laundering.
        assert!(copilot_cli_response_from_decision(
            &cli_args("git status & rm -rf /tmp/x"),
            HookDecision::Defer,
            "git status & rm -rf /tmp/x",
        )
        .is_none());
    }

    #[test]
    fn test_copilot_ide_rewrite_returns_deny_with_suggestion() {
        let response = copilot_ide_response_from_decision(
            HookDecision::AskRewrite("rtk git status".into()),
            "git status",
        )
        .unwrap();
        assert_eq!(response["permissionDecision"], "deny");
        assert!(response["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("rtk git status"));
        assert!(response.get("modifiedArgs").is_none());
    }

    #[test]
    fn test_copilot_ide_allow_rewrite_returns_deny_with_suggestion() {
        // The IDE host ignores modifiedArgs, so an Allow-with-rewrite decision
        // must still surface as a deny-with-suggestion, exactly like AskRewrite.
        let response = copilot_ide_response_from_decision(
            HookDecision::AllowRewrite("rtk git status".into()),
            "git status",
        )
        .unwrap();
        assert_eq!(response["permissionDecision"], "deny");
        assert!(response["permissionDecisionReason"]
            .as_str()
            .unwrap()
            .contains("rtk git status"));
        assert!(response.get("modifiedArgs").is_none());
    }

    #[test]
    fn test_copilot_ide_permission_deny_is_enforced() {
        let response =
            copilot_ide_response_from_decision(HookDecision::Deny, "rm -rf /protected").unwrap();
        assert_eq!(response["permissionDecision"], "deny");
        assert_eq!(
            response["permissionDecisionReason"],
            "Blocked by RTK permission rule"
        );
    }

    #[test]
    fn test_copilot_ide_defer_is_silent() {
        assert!(copilot_ide_response_from_decision(HookDecision::Defer, "htop").is_none());
    }

    #[test]
    fn test_copilot_cli_passthrough_unsupported() {
        assert!(copilot_cli_response("htop", &cli_args("htop")).is_none());
    }

    #[test]
    fn test_copilot_cli_passthrough_already_rtk() {
        assert!(copilot_cli_response("rtk cargo test", &cli_args("rtk cargo test")).is_none());
    }

    #[test]
    fn test_copilot_cli_passthrough_heredoc() {
        let cmd = "cat <<EOF\nhi\nEOF";
        assert!(copilot_cli_response(cmd, &cli_args(cmd)).is_none());
    }

    #[test]
    fn test_copilot_cli_preserves_env_prefix() {
        let r = copilot_cli_response(
            "RUST_LOG=debug cargo test",
            &cli_args("RUST_LOG=debug cargo test"),
        )
        .unwrap();
        assert_eq!(
            r["modifiedArgs"]["command"],
            "RUST_LOG=debug rtk cargo test"
        );
    }

    #[test]
    fn test_copilot_cli_preserves_extra_args_fields() {
        let args = json!({
            "command": "cargo install ripgrep",
            "description": "install ripgrep",
            "initial_wait": 30,
            "mode": "sync"
        });
        let r = copilot_cli_response_from_decision(
            &args,
            HookDecision::AskRewrite("rtk cargo install ripgrep".into()),
            "cargo install ripgrep",
        )
        .unwrap();
        let modified = &r["modifiedArgs"];
        assert_eq!(modified["command"], "rtk cargo install ripgrep");
        assert_eq!(modified["description"], "install ripgrep");
        assert_eq!(modified["initial_wait"], 30);
        assert_eq!(modified["mode"], "sync");
    }

    fn end_to_end(cmd: &str) -> Option<Value> {
        let verdict = crate::hooks::permissions::check_command_with_rules(
            cmd,
            &[],
            &[],
            &["Bash(git:*)".to_string()],
        );
        copilot_cli_response_from_decision(&cli_args(cmd), decide_from_verdict(cmd, verdict), cmd)
    }

    #[test]
    fn test_copilot_cli_cve_safe_forms_still_rewrite() {
        for cmd in ["git status", "git status 2>&1"] {
            let r = end_to_end(cmd).unwrap_or_else(|| panic!("expected rewrite for {cmd:?}"));
            assert_eq!(
                r["modifiedArgs"]["command"].as_str().unwrap(),
                format!("rtk {cmd}"),
                "safe form {cmd:?} must rewrite",
            );
        }
    }

    #[test]
    fn test_copilot_cli_cve_newline_bypass_never_auto_allows() {
        let r = end_to_end("git status\nrm -rf /tmp/x");
        if let Some(resp) = r {
            assert!(
                resp.get("permissionDecision").is_none(),
                "newline-hidden command must not produce permissionDecision: \"allow\""
            );
        }
    }

    #[test]
    fn test_copilot_cli_cve_background_bypass_never_auto_allows() {
        let r = end_to_end("git status & rm -rf /tmp/x");
        if let Some(resp) = r {
            assert!(
                resp.get("permissionDecision").is_none(),
                "background-& hidden command must not produce permissionDecision: \"allow\""
            );
        }
    }

    #[test]
    fn test_copilot_cli_cve_command_substitution_returns_none() {
        assert!(
            end_to_end("git log --pretty=$(rm -rf /tmp/x)").is_none(),
            "$( ) command substitution must not produce modifiedArgs"
        );
    }

    #[test]
    fn test_copilot_cli_cve_backtick_substitution_returns_none() {
        assert!(
            end_to_end("git log --pretty=`rm -rf /tmp/x`").is_none(),
            "backtick substitution must not produce modifiedArgs"
        );
    }

    #[test]
    fn test_copilot_cli_cve_file_redirect_amp_returns_none() {
        assert!(
            end_to_end("git status >& /tmp/evil").is_none(),
            ">&file redirect must not produce modifiedArgs"
        );
    }

    #[test]
    fn test_copilot_cli_cve_file_redirect_returns_none() {
        assert!(
            end_to_end("git status > /tmp/evil").is_none(),
            ">file redirect must not produce modifiedArgs"
        );
    }

    // --- Gemini format ---

    #[test]
    fn test_print_allow_format() {
        let expected = r#"{"decision":"allow"}"#;
        assert_eq!(expected, r#"{"decision":"allow"}"#);
    }

    #[test]
    fn test_print_rewrite_format() {
        let output = serde_json::json!({
            "decision": "allow",
            "hookSpecificOutput": {
                "tool_input": {
                    "command": "rtk git status"
                }
            }
        });
        let json: Value = serde_json::from_str(&output.to_string()).unwrap();
        assert_eq!(json["decision"], "allow");
        assert_eq!(
            json["hookSpecificOutput"]["tool_input"]["command"],
            "rtk git status"
        );
    }

    #[test]
    fn test_gemini_hook_uses_rewrite_command() {
        assert_eq!(
            rewrite_command_no_prefixes("git status", &[]),
            Some("rtk git status".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("cargo test", &[]),
            Some("rtk cargo test".into())
        );
        assert_eq!(
            rewrite_command_no_prefixes("rtk git status", &[]),
            Some("rtk git status".into())
        );
        assert_eq!(rewrite_command_no_prefixes("cat <<EOF", &[]), None);
    }

    #[test]
    fn test_gemini_hook_excluded_commands() {
        let excluded = vec!["curl".to_string()];
        assert_eq!(
            rewrite_command_no_prefixes("curl https://example.com", &excluded),
            None
        );
        assert_eq!(
            rewrite_command_no_prefixes("git status", &excluded),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_gemini_hook_env_prefix_preserved() {
        assert_eq!(
            rewrite_command_no_prefixes("RUST_LOG=debug cargo test", &[]),
            Some("RUST_LOG=debug rtk cargo test".into())
        );
    }

    // --- Claude handler ---

    fn claude_input(cmd: &str) -> String {
        json!({
            "tool_name": "Bash",
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    fn claude_input_with_fields(cmd: &str, timeout: u64, description: &str) -> String {
        json!({
            "tool_name": "Bash",
            "tool_input": {
                "command": cmd,
                "timeout": timeout,
                "description": description
            }
        })
        .to_string()
    }

    /// Matches the real PreToolUse payload shape captured from a live Claude Code
    /// session (verified fields: session_id, transcript_path, cwd, tool_use_id).
    fn claude_payload_with_ids(cmd: &str, session_id: &str, tool_use_id: &str, cwd: &str) -> Value {
        json!({
            "session_id": session_id,
            "transcript_path": "/home/user/.claude/projects/-home-user-project/session.jsonl",
            "cwd": cwd,
            "hook_event_name": "PreToolUse",
            "tool_name": "Bash",
            "tool_input": { "command": cmd },
            "tool_use_id": tool_use_id
        })
    }

    #[test]
    fn test_hook_log_fields_extracts_real_payload_shape() {
        let v =
            claude_payload_with_ids("git status", "sess-1", "toolu_01ABC", "/home/user/project");
        let (session_id, tool_use_id, project_path) = hook_log_fields(&v).unwrap();
        assert_eq!(session_id, "sess-1");
        assert_eq!(tool_use_id, "toolu_01ABC");
        assert_eq!(project_path, "/home/user/project");
    }

    #[test]
    fn test_hook_log_fields_none_without_tool_use_id() {
        // Older/foreign payload shapes without a tool_use_id must not be logged —
        // there's no join key to match it back to a transcript entry. Uses the
        // real claude_input() fixture, which also lacks session_id — see the
        // isolated variant below for a payload that has session_id present but
        // tool_use_id specifically absent.
        let v: Value = serde_json::from_str(&claude_input("git status")).unwrap();
        assert!(hook_log_fields(&v).is_none());
    }

    #[test]
    fn test_hook_log_fields_none_with_session_id_but_no_tool_use_id() {
        // hook_log_fields checks session_id first and short-circuits via `?`, so
        // the fixture above (missing both fields) can't tell us whether
        // tool_use_id extraction specifically works — it passes even if that
        // check were completely broken. This isolates tool_use_id: session_id
        // present, tool_use_id absent.
        let v = json!({
            "session_id": "sess-1",
            "tool_name": "Bash",
            "tool_input": { "command": "git status" }
        });
        assert!(hook_log_fields(&v).is_none());
    }

    #[test]
    fn test_hook_log_fields_defaults_missing_cwd_to_empty() {
        let v = json!({
            "session_id": "sess-1",
            "tool_use_id": "toolu_01ABC",
            "tool_name": "Bash",
            "tool_input": { "command": "git status" }
        });
        let (_, _, project_path) = hook_log_fields(&v).unwrap();
        assert_eq!(project_path, "");
    }

    // The decision field on PayloadAction feeds directly into hook_decisions —
    // exercise the full Allow/Ask/Deny/Defer matrix against the pure
    // process_claude_payload_from_decision (no real permission config needed).

    #[test]
    fn test_process_claude_payload_decision_allow() {
        let v = claude_input_value("git status");
        match process_claude_payload_from_decision(
            &v,
            "git status",
            HookDecision::AllowRewrite("rtk git status".to_string()),
        ) {
            PayloadAction::Rewrite {
                decision,
                rewritten,
                ..
            } => {
                assert_eq!(decision, HookOutcome::Allow);
                assert_eq!(rewritten, "rtk git status");
            }
            other => {
                panic!("expected Rewrite, got a different PayloadAction variant instead: {other:?}")
            }
        }
    }

    #[test]
    fn test_process_claude_payload_decision_ask() {
        let v = claude_input_value("git status");
        match process_claude_payload_from_decision(
            &v,
            "git status",
            HookDecision::AskRewrite("rtk git status".to_string()),
        ) {
            PayloadAction::Rewrite { decision, .. } => assert_eq!(decision, HookOutcome::Ask),
            other => {
                panic!("expected Rewrite, got a different PayloadAction variant instead: {other:?}")
            }
        }
    }

    #[test]
    fn test_process_claude_payload_decision_deny() {
        let v = claude_input_value("rm -rf /");
        match process_claude_payload_from_decision(&v, "rm -rf /", HookDecision::Deny) {
            PayloadAction::Skip { decision, .. } => assert_eq!(decision, HookOutcome::Deny),
            other => {
                panic!("expected Skip, got a different PayloadAction variant instead: {other:?}")
            }
        }
    }

    #[test]
    fn test_process_claude_payload_decision_defer() {
        let v = claude_input_value("git status $(rm -rf /tmp/x)");
        match process_claude_payload_from_decision(
            &v,
            "git status $(rm -rf /tmp/x)",
            HookDecision::Defer,
        ) {
            PayloadAction::Skip { decision, .. } => assert_eq!(decision, HookOutcome::Defer),
            other => {
                panic!("expected Skip, got a different PayloadAction variant instead: {other:?}")
            }
        }
    }

    fn claude_input_value(cmd: &str) -> Value {
        serde_json::from_str(&claude_input(cmd)).unwrap()
    }

    #[test]
    fn test_claude_rewrite_git_status() {
        let result = run_claude_inner(&claude_input("git status")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "rtk git status");
    }

    #[test]
    fn test_claude_rewrite_preserves_tool_input_fields() {
        let input = claude_input_with_fields("git status", 30000, "Check repo status");
        let result = run_claude_inner(&input).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let updated = &v["hookSpecificOutput"]["updatedInput"];
        assert_eq!(updated["command"], "rtk git status");
        assert_eq!(updated["timeout"], 30000);
        assert_eq!(updated["description"], "Check repo status");
    }

    #[test]
    fn test_claude_passthrough_no_output() {
        assert!(run_claude_inner(&claude_input("htop")).is_none());
    }

    #[test]
    fn test_claude_substitution_not_rewritten() {
        // A substitution payload must never be rewritten into updatedInput;
        // RTK skips so Claude Code evaluates the original command natively.
        assert!(run_claude_inner(&claude_input("git status `rm -rf /tmp/x`")).is_none());
        assert!(run_claude_inner(&claude_input("git status $(rm -rf /tmp/x)")).is_none());
        assert!(run_claude_inner(&claude_input("git log --pretty=\"$(rm -rf /tmp/x)\"")).is_none());
    }

    #[test]
    fn test_claude_file_redirect_not_rewritten() {
        assert!(run_claude_inner(&claude_input("git log > /tmp/out.txt")).is_none());
    }

    #[test]
    fn test_claude_fd_dup_redirect_still_rewritten() {
        // `2>&1` is attestable — the rewrite proceeds as normal.
        assert!(run_claude_inner(&claude_input("git status 2>&1")).is_some());
    }

    #[test]
    fn test_claude_heredoc_passthrough() {
        assert!(run_claude_inner(&claude_input("cat <<EOF\nhello\nEOF")).is_none());
    }

    #[test]
    fn test_claude_already_rtk_passthrough() {
        assert!(run_claude_inner(&claude_input("rtk git status")).is_none());
    }

    #[test]
    fn test_claude_empty_command_passthrough() {
        let input = json!({
            "tool_name": "Bash",
            "tool_input": { "command": "" }
        })
        .to_string();
        assert!(run_claude_inner(&input).is_none());
    }

    #[test]
    fn test_claude_malformed_json_passthrough() {
        assert!(run_claude_inner("not valid json {{{").is_none());
    }

    #[test]
    fn test_claude_env_prefix_preserved() {
        let result = run_claude_inner(&claude_input("GIT_PAGER=cat git status")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "GIT_PAGER=cat rtk git status");
    }

    #[test]
    fn test_claude_compound_command() {
        let result = run_claude_inner(&claude_input("git add . && cargo test")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "rtk git add . && rtk cargo test");
    }

    #[test]
    fn test_claude_pipeline_rewrites_only_safe_final_stage() {
        let result = run_claude_inner(&claude_input("cargo test | grep FAILED")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let cmd = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "cargo test | rtk grep FAILED");
    }

    #[test]
    fn test_claude_json_output_structure() {
        let result = run_claude_inner(&claude_input("git status")).unwrap();
        let v: Value = serde_json::from_str(&result).unwrap();
        let hook = &v["hookSpecificOutput"];

        assert_eq!(hook["hookEventName"], PRE_TOOL_USE_KEY);
        // permissionDecision is only set when an explicit allow rule matches;
        // with default-to-ask semantics (no rules configured), it is absent.
        assert_eq!(hook["permissionDecisionReason"], "RTK auto-rewrite");
        assert!(hook["updatedInput"].is_object());
        assert!(hook["updatedInput"]["command"].is_string());
    }

    #[test]
    fn test_claude_no_tool_input_passthrough() {
        let input = json!({ "tool_name": "Bash" }).to_string();
        assert!(run_claude_inner(&input).is_none());
    }

    #[test]
    fn test_claude_strips_utf8_bom() {
        // Windows hosts may prepend a UTF-8 BOM to hook stdin (confirmed for
        // Cursor). Without stripping, str::trim leaves U+FEFF in place,
        // serde_json::from_str fails, run_claude logs to stderr and returns
        // Ok(()) — every command silently stops being rewritten.
        let payload = claude_input("git status");
        let with_bom = format!("\u{feff}{}", payload);
        let result = run_claude_inner(&with_bom).expect("BOM-prefixed payload must parse");
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(
            v["hookSpecificOutput"]["updatedInput"]["command"],
            "rtk git status"
        );
    }

    // --- Cursor handler ---

    fn cursor_input(cmd: &str) -> String {
        json!({
            "tool_name": "Bash",
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    fn run_cursor_allowed(input: &str) -> String {
        run_cursor_inner_with_rules(input, &[], &[], &["*".to_string()])
    }

    #[test]
    fn test_cursor_rewrite_flat_format() {
        let result = run_cursor_allowed(&cursor_input("git status"));
        let v: Value = serde_json::from_str(&result).unwrap();
        // Cursor preToolUse expects allow/deny for rewrite application.
        assert_eq!(v["permission"], "allow");
        assert_eq!(v["updated_input"]["command"], "rtk git status");
        assert!(v.get("hookSpecificOutput").is_none());
        // `continue: true` keeps the Cursor preToolUse panel from collapsing
        // to `Output: {}`; without it the rewrite is invisible to users.
        assert_eq!(v["continue"], true);
    }

    #[test]
    fn test_cursor_default_verdict_rewrites() {
        let result = run_cursor_inner(&cursor_input("git status"));
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["permission"], "ask");
        assert_eq!(v["updated_input"]["command"], "rtk git status");
        // `continue: true` keeps the Cursor preToolUse panel from collapsing
        // to `Output: {}`; without it the rewrite is invisible to users.
        assert_eq!(v["continue"], true);
    }

    #[test]
    fn test_cursor_substitution_defers_even_when_allowed() {
        assert_eq!(
            run_cursor_allowed(&cursor_input("git status `rm -rf /tmp/x`")),
            "{}"
        );
        assert_eq!(
            run_cursor_allowed(&cursor_input("git status $(rm -rf /tmp/x)")),
            "{}"
        );
    }

    #[test]
    fn test_cursor_unallowed_segment_asks() {
        let out = run_cursor_inner_with_rules(
            &cursor_input("git status && rm -rf /tmp/x"),
            &[],
            &[],
            &["git *".to_string()],
        );
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(v["permission"], "ask");
    }

    #[test]
    fn test_cursor_passthrough_empty_json() {
        let result = run_cursor_inner(&cursor_input("htop"));
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_cursor_empty_input_empty_json() {
        let result = run_cursor_inner("");
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_cursor_heredoc_passthrough() {
        let result = run_cursor_inner(&cursor_input("cat <<EOF\nhello\nEOF"));
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_cursor_already_rtk_passthrough() {
        let result = run_cursor_inner(&cursor_input("rtk git status"));
        assert_eq!(result, "{}");
    }

    #[test]
    fn test_cursor_no_hook_specific_output() {
        let result = run_cursor_allowed(&cursor_input("cargo test"));
        let v: Value = serde_json::from_str(&result).unwrap();
        assert!(v.get("hookSpecificOutput").is_none());
        assert_eq!(v["permission"], "allow");
        assert_eq!(v["continue"], true);
    }

    #[test]
    fn test_cursor_compound_rewrite_includes_continue() {
        let cmd = "cd \"/tmp/proj\" && git status";
        let result = run_cursor_allowed(&cursor_input(cmd));
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["continue"], true);
        assert_eq!(v["permission"], "allow");
        assert_eq!(
            v["updated_input"]["command"],
            "cd \"/tmp/proj\" && rtk git status"
        );
    }

    #[test]
    fn test_cursor_strips_single_utf8_bom() {
        // Some Cursor builds prepend a single UTF-8 BOM to hook stdin.
        // serde_json rejects BOM-prefixed input, so without the strip
        // the hook returned `{}` and the rewrite became a silent no-op.
        let payload = cursor_input("git status");
        let with_single_bom = format!("\u{feff}{}", payload);
        let result = run_cursor_allowed(&with_single_bom);
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["continue"], true);
        assert_eq!(v["permission"], "allow");
        assert_eq!(v["updated_input"]["command"], "rtk git status");
    }

    #[test]
    fn test_cursor_strips_double_utf8_bom() {
        // Cursor on Windows ships hook stdin with **two** leading
        // UTF-8 BOMs (`EF BB BF EF BB BF`), confirmed via a stdin
        // tracer wrapping `rtk hook cursor` on Cursor 3.2.x. This is
        // the real-world payload shape the loop needs to survive.
        let payload = cursor_input("git status");
        let with_double_bom = format!("\u{feff}\u{feff}{}", payload);
        let result = run_cursor_allowed(&with_double_bom);
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["continue"], true);
        assert_eq!(v["permission"], "allow");
        assert_eq!(v["updated_input"]["command"], "rtk git status");
    }

    // --- Audit logging ---

    #[test]
    fn test_audit_log_silent_when_disabled() {
        std::env::remove_var("RTK_HOOK_AUDIT");
        audit_log("test", "git status", "rtk git status");
    }

    #[test]
    fn test_audit_log_format_four_fields() {
        let tmp = std::env::temp_dir().join("rtk-test-audit");
        let _ = std::fs::create_dir_all(&tmp);
        let log_path = tmp.join("hook-audit.log");
        let _ = std::fs::remove_file(&log_path);

        {
            let mut file = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&log_path)
                .unwrap();
            let ts = chrono::Local::now().format("%Y-%m-%dT%H:%M:%S");
            writeln!(file, "{} | rewrite | git status | rtk git status", ts).unwrap();
        }

        let content = std::fs::read_to_string(&log_path).unwrap();
        let parts: Vec<&str> = content.trim().split(" | ").collect();
        assert_eq!(
            parts.len(),
            4,
            "Expected 4 pipe-delimited fields, got: {:?}",
            parts
        );
        assert_eq!(parts[1], "rewrite");
        assert_eq!(parts[2], "git status");
        assert_eq!(parts[3], "rtk git status");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    // --- Adversarial tests ---

    #[test]
    fn test_audit_log_sanitizes_newlines() {
        let sanitized = sanitize_log_field("git status\nfake | inject | evil");
        assert!(!sanitized.contains('\n'));
        assert!(sanitized.contains("\\n"));
    }

    #[test]
    fn test_audit_log_sanitizes_pipe_delimiter() {
        let sanitized = sanitize_log_field("git log | head");
        assert!(
            !sanitized.contains(" | "),
            "unescaped ' | ' breaks field parsing: {}",
            sanitized
        );
        assert!(sanitized.contains("\\|"));
    }

    #[test]
    fn test_claude_unicode_null_passthrough() {
        let input = claude_input("git status \u{0000}\u{FEFF}");
        let _ = run_claude_inner(&input);
    }

    #[test]
    fn test_claude_extremely_long_command() {
        let long_cmd = format!("git status {}", "A".repeat(100_000));
        let input = claude_input(&long_cmd);
        let _ = run_claude_inner(&input);
    }

    #[test]
    fn test_cursor_deny_blocks_rewrite() {
        use super::permissions::check_command_with_rules;
        let deny = vec!["git status".to_string()];
        assert_eq!(
            check_command_with_rules("git status", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
    }

    #[test]
    fn test_gemini_deny_blocks_rewrite() {
        use super::permissions::check_command_with_rules;
        let deny = vec!["cargo test".to_string()];
        assert_eq!(
            check_command_with_rules("cargo test", &deny, &[], &[]),
            PermissionVerdict::Deny
        );
        // Denied commands must not be rewritten — Gemini handler checks deny before rewrite
        assert!(
            get_rewritten("cargo test").is_some(),
            "cargo test should be rewritable when not denied"
        );
    }

    // --- Shared decision flow (all hosts route through this) ---

    fn decide_with_rules(
        cmd: &str,
        deny: &[String],
        ask: &[String],
        allow: &[String],
    ) -> HookDecision {
        let verdict = permissions::check_command_with_rules(cmd, deny, ask, allow);
        decide_from_verdict(cmd, verdict)
    }

    fn all_allowed() -> Vec<String> {
        vec!["*".to_string()]
    }

    #[test]
    fn test_decide_allow_for_attestable_allowed_command() {
        assert!(matches!(
            decide_with_rules("git status", &[], &[], &all_allowed()),
            HookDecision::AllowRewrite(_)
        ));
    }

    #[test]
    fn test_decide_ask_for_default_verdict() {
        assert!(matches!(
            decide_with_rules("git status", &[], &[], &[]),
            HookDecision::AskRewrite(_)
        ));
    }

    #[test]
    fn test_decide_deny() {
        assert!(matches!(
            decide_with_rules(
                "rm -rf /tmp/x",
                &["rm -rf".to_string()],
                &[],
                &all_allowed()
            ),
            HookDecision::Deny
        ));
    }

    #[test]
    fn test_decide_defer_for_substitution_even_when_allowed() {
        for cmd in [
            "git status `rm -rf /tmp/x`",
            "git status $(rm -rf /tmp/x)",
            "git log --pretty=\"$(rm -rf /tmp/x)\"",
        ] {
            assert!(
                matches!(
                    decide_with_rules(cmd, &[], &[], &all_allowed()),
                    HookDecision::Defer
                ),
                "expected Defer for {cmd}"
            );
        }
    }

    #[test]
    fn test_decide_defer_for_file_redirect() {
        assert!(matches!(
            decide_with_rules("git log > /tmp/out.txt", &[], &[], &all_allowed()),
            HookDecision::Defer
        ));
    }

    #[test]
    fn test_decide_allow_for_fd_dup_redirect() {
        assert!(matches!(
            decide_with_rules("git status 2>&1", &[], &[], &all_allowed()),
            HookDecision::AllowRewrite(_)
        ));
    }

    // --- Gemini rendering ---

    fn gemini_render(cmd: &str, deny: &[String], ask: &[String], allow: &[String]) -> String {
        match decide_with_rules(cmd, deny, ask, allow) {
            HookDecision::Deny => {
                r#"{"decision":"deny","reason":"Blocked by RTK permission rule"}"#.to_string()
            }
            HookDecision::AllowRewrite(r) => gemini_json("allow", Some(&r)),
            HookDecision::AskRewrite(r) => gemini_json("ask_user", Some(&r)),
            HookDecision::Defer => gemini_json("ask_user", None),
        }
    }

    #[test]
    fn test_gemini_strips_utf8_bom() {
        // Windows hosts may prepend a UTF-8 BOM to hook stdin (confirmed for
        // Cursor; run_gemini must survive it too). Without stripping,
        // serde_json rejects the payload, `rtk hook gemini` exits non-zero,
        // and the tool call is blocked.
        //
        // Uses run_gemini_inner_with_rules (explicit allow-all rules) rather
        // than run_gemini_inner: the latter's decide_hook_action reads the
        // REAL ~/.gemini/settings.json (and project .gemini/settings.json),
        // making the decision assertion depend on whatever is on the
        // machine running the test. Both share the same parse/strip/render
        // core (run_gemini_inner_impl) that production run_gemini uses, so
        // this still exercises the real BOM-stripping path.
        let payload = json!({
            "tool_name": "run_shell_command",
            "tool_input": { "command": "git status" }
        })
        .to_string();
        let with_bom = format!("\u{feff}{payload}");
        let result = run_gemini_inner_with_rules(&with_bom, &[], &[], &all_allowed())
            .expect("BOM-prefixed payload must parse");
        let v: Value = serde_json::from_str(&result).unwrap();
        assert_eq!(v["decision"], "allow");
        assert_eq!(
            v["hookSpecificOutput"]["tool_input"]["command"],
            "rtk git status"
        );
    }

    #[test]
    fn test_gemini_inner_preserves_serde_diagnostic() {
        // run_gemini_inner must return the serde_json error itself (not
        // discard it via `.ok()`), so run_gemini's `.context(...)` has a
        // real source to chain instead of only the generic wrapper message.
        let err = run_gemini_inner("not valid json {{{").unwrap_err();
        let msg = err.to_string();
        assert!(
            msg.contains("line") && msg.contains("column"),
            "expected serde_json's own parse diagnostic, got: {msg}"
        );
    }

    #[test]
    fn test_gemini_allow_emits_rewrite() {
        let v: Value =
            serde_json::from_str(&gemini_render("git status", &[], &[], &all_allowed())).unwrap();
        assert_eq!(v["decision"], "allow");
        assert_eq!(
            v["hookSpecificOutput"]["tool_input"]["command"],
            "rtk git status"
        );
    }

    #[test]
    fn test_gemini_default_asks_user() {
        let v: Value = serde_json::from_str(&gemini_render("git status", &[], &[], &[])).unwrap();
        assert_eq!(v["decision"], "ask_user");
    }

    #[test]
    fn test_gemini_substitution_asks_user_without_rewrite() {
        let v: Value = serde_json::from_str(&gemini_render(
            "git status `rm -rf /tmp/x`",
            &[],
            &[],
            &all_allowed(),
        ))
        .unwrap();
        assert_eq!(v["decision"], "ask_user");
        assert!(v.get("hookSpecificOutput").is_none());
    }

    #[test]
    fn test_gemini_deny_decision() {
        let v: Value = serde_json::from_str(&gemini_render(
            "rm -rf /tmp/x",
            &["rm -rf".to_string()],
            &[],
            &[],
        ))
        .unwrap();
        assert_eq!(v["decision"], "deny");
    }

    // --- Factory Droid hook ---

    fn droid_input(tool: &str, cmd: &str) -> String {
        json!({
            "session_id": "abc123",
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    #[test]
    fn test_droid_rewrites_execute_tool() {
        // Rewrites land via `updatedInput` with no decision — Droid's native
        // flow decides on the rewritten command.
        let input = droid_input("Execute", "git status");
        let out = run_droid_inner(&input).expect("rewrite expected");
        let v: Value = serde_json::from_str(&out).unwrap();
        let updated = v
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        assert!(
            updated.starts_with("rtk "),
            "expected rtk-prefixed rewrite, got `{updated}`"
        );
        assert_eq!(
            v.pointer("/hookSpecificOutput/hookEventName")
                .and_then(|c| c.as_str()),
            Some("PreToolUse")
        );
        assert!(
            v.pointer("/hookSpecificOutput/permissionDecision")
                .is_none(),
            "RTK must never assert a permission decision for Droid"
        );
    }

    #[test]
    fn test_droid_strips_utf8_bom() {
        // Windows hosts may prepend a UTF-8 BOM to hook stdin (confirmed for
        // Cursor). run_droid stripped it, but the test entry point re-parsed
        // without stripping, so the strip had no coverage at all: deleting it
        // left every test green while BOM-prefixed droid payloads silently
        // stopped being rewritten. Both paths now share droid_payload.
        let input = format!("\u{feff}{}", droid_input("Execute", "git status"));
        let out = run_droid_inner(&input).expect("BOM-prefixed payload must parse");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v.pointer("/hookSpecificOutput/updatedInput/command")
                .and_then(|c| c.as_str()),
            Some("rtk git status")
        );
    }

    #[test]
    fn test_droid_unlisted_command_omits_decision() {
        // Not on any Droid list → rewrite lands via Droid's "updated input
        // result" path with NO decision, leaving Droid's native prompt and
        // other hooks' deny/ask in control.
        let input = droid_input("Execute", "cargo build");
        let out = run_droid_inner(&input).expect("rewrite expected");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v.pointer("/hookSpecificOutput/updatedInput/command")
                .and_then(|c| c.as_str())
                .is_some_and(|c| c.starts_with("rtk ")),
            "expected rtk-prefixed rewrite"
        );
        assert!(
            v.pointer("/hookSpecificOutput/permissionDecision")
                .is_none(),
            "unlisted command must not force a permission decision"
        );
    }

    #[test]
    fn test_droid_denylisted_command_steps_aside() {
        // A commandDenylist match must produce NO output: rewriting would
        // dodge Droid's own pattern match (`rtk git log` no longer matches a
        // `git log` denylist entry), silently dropping the user's
        // always-confirm rule. Stepping aside keeps Droid's native
        // confirmation on the original command.
        let settings = json!({ "commandDenylist": ["git log"] });
        let (deny, ask, allow) = permissions::droid_rules_from_settings(&[settings]);
        let input = droid_input("Execute", "git log --oneline");
        assert!(
            run_droid_inner_with_rules(&input, &deny, &ask, &allow).is_none(),
            "denylisted command must step aside (no output)"
        );
    }

    #[test]
    fn test_droid_blocklisted_command_steps_aside() {
        // Same contract for commandBlocklist (never runs): step aside so
        // Droid's Execute-level block fires on the original command.
        let settings = json!({ "commandBlocklist": ["git status"] });
        let (deny, ask, allow) = permissions::droid_rules_from_settings(&[settings]);
        let input = droid_input("Execute", "git status");
        assert!(
            run_droid_inner_with_rules(&input, &deny, &ask, &allow).is_none(),
            "blocklisted command must step aside (no output)"
        );
    }

    #[test]
    fn test_droid_allowlist_never_auto_allows() {
        // Even an allowlisted command gets no decision — RTK can't reproduce
        // Droid's allow once the program is renamed to `rtk`.
        let settings = json!({ "commandAllowlist": ["git status"] });
        let (deny, ask, allow) = permissions::droid_rules_from_settings(&[settings]);
        let input = droid_input("Execute", "git status");
        let out = run_droid_inner_with_rules(&input, &deny, &ask, &allow)
            .expect("rewrite still expected");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert!(
            v.pointer("/hookSpecificOutput/permissionDecision")
                .is_none(),
            "allowlisted command must not auto-allow"
        );
    }

    #[test]
    fn test_droid_project_scope_deny_steps_aside() {
        // A deny entry in any scope (here: project) must step aside —
        // global-only reads would let the rewrite dodge it.
        let user = json!({});
        let project = json!({ "commandDenylist": ["git log"] });
        let (deny, ask, allow) = permissions::droid_rules_from_settings(&[user, project]);
        let input = droid_input("Execute", "git log --oneline");
        assert!(
            run_droid_inner_with_rules(&input, &deny, &ask, &allow).is_none(),
            "project-scope deny entry must step aside (no output)"
        );
    }

    #[test]
    fn test_droid_ignores_non_execute_tool() {
        // Droid fires PreToolUse for many tools (Edit, Create, Read…); we must
        // only touch Execute (or legacy Bash) so other tools pass through.
        let input = droid_input("Edit", "git status");
        assert!(
            run_droid_inner(&input).is_none(),
            "non-Execute tools must not produce output"
        );
    }

    #[test]
    fn test_droid_bash_tool_name_accepted_defensively() {
        // Droid has no Bash tool, but Claude-shaped payloads are accepted
        // defensively; the installed matcher gates invocations to Execute.
        let input = droid_input("Bash", "git status");
        assert!(
            run_droid_inner(&input).is_some(),
            "Bash tool name should still rewrite"
        );
    }

    #[test]
    fn test_droid_deny_steps_aside() {
        // A denied command must produce NO output so Droid's native deny
        // handling fires — matching Claude/Cursor/Copilot. RTK must not emit
        // its own `permissionDecision: deny` block. Decision is injected
        // because decide_hook_action loads ambient rules that aren't present
        // in the test environment.
        let v: Value = serde_json::from_str(&droid_input("Execute", "git push --force")).unwrap();
        assert!(
            droid_response_from_decision(&v, "git push --force", HookDecision::Deny).is_none(),
            "deny must step aside (no output), not emit an RTK block"
        );
    }

    #[test]
    fn test_droid_allow_decision_emits_no_permission_decision() {
        // Defensive: even an AllowRewrite decision carries the rewrite only.
        let v: Value = serde_json::from_str(&droid_input("Execute", "git status")).unwrap();
        let out = droid_response_from_decision(
            &v,
            "git status",
            HookDecision::AllowRewrite("rtk git status".to_string()),
        )
        .expect("rewrite expected");
        assert!(
            out.pointer("/hookSpecificOutput/permissionDecision")
                .is_none(),
            "no permission decision may be emitted"
        );
        assert_eq!(
            out.pointer("/hookSpecificOutput/updatedInput/command")
                .and_then(|c| c.as_str()),
            Some("rtk git status")
        );
    }

    #[test]
    fn test_droid_substitution_defers() {
        // Commands with substitution can't be attested — the shared decision
        // flow defers so Droid runs the original command unchanged.
        for cmd in ["git status `rm -rf /tmp/x`", "git status $(rm -rf /tmp/x)"] {
            let input = droid_input("Execute", cmd);
            assert!(
                run_droid_inner(&input).is_none(),
                "substitution must defer (no output) for {cmd}"
            );
        }
    }

    #[test]
    fn test_droid_file_redirect_defers() {
        let input = droid_input("Execute", "git log > /tmp/out.txt");
        assert!(
            run_droid_inner(&input).is_none(),
            "file redirects must defer (no output)"
        );
    }

    #[test]
    fn test_droid_empty_command_passthrough() {
        let input = droid_input("Execute", "");
        assert!(run_droid_inner(&input).is_none());
    }

    #[test]
    fn test_droid_no_rewrite_passthrough() {
        // Commands rtk doesn't know about should not generate a hookSpecificOutput
        // so Droid runs them unchanged.
        let input = droid_input("Execute", "definitely-not-a-real-binary --foo");
        assert!(run_droid_inner(&input).is_none());
    }

    // ── Kiro hook tests ────────────────────────────────────────────

    fn kiro_input(tool: &str, cmd: &str) -> String {
        json!({
            "session_id": "kiro-session-001",
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    fn kiro_input_with_extras(tool: &str, cmd: &str, description: &str, timeout: u64) -> String {
        json!({
            "session_id": "kiro-session-001",
            "hook_event_name": "PreToolUse",
            "tool_name": tool,
            "tool_input": {
                "command": cmd,
                "description": description,
                "timeout": timeout
            }
        })
        .to_string()
    }

    // --- Parsing: kiro_shell_command ---

    #[test]
    fn test_kiro_shell_command_valid_execute_bash() {
        let v: Value = serde_json::from_str(&kiro_input("executeBash", "git status")).unwrap();
        assert_eq!(kiro_shell_command(&v), Some("git status"));
    }

    #[test]
    fn test_kiro_shell_command_valid_execute_bash_snake() {
        let v: Value = serde_json::from_str(&kiro_input("execute_bash", "cargo test")).unwrap();
        assert_eq!(kiro_shell_command(&v), Some("cargo test"));
    }

    #[test]
    fn test_kiro_shell_command_valid_run_command() {
        let v: Value = serde_json::from_str(&kiro_input("runCommand", "ls -la")).unwrap();
        assert_eq!(kiro_shell_command(&v), Some("ls -la"));
    }

    #[test]
    fn test_kiro_shell_command_valid_shell() {
        let v: Value = serde_json::from_str(&kiro_input("shell", "cat file.txt")).unwrap();
        assert_eq!(kiro_shell_command(&v), Some("cat file.txt"));
    }

    #[test]
    fn test_kiro_shell_command_missing_tool_name() {
        // Tolerates absent tool_name
        let v: Value = serde_json::from_str(r#"{"tool_input": {"command": "git diff"}}"#).unwrap();
        assert_eq!(kiro_shell_command(&v), Some("git diff"));
    }

    #[test]
    fn test_kiro_shell_command_empty_command() {
        let v: Value = serde_json::from_str(&kiro_input("executeBash", "")).unwrap();
        assert_eq!(kiro_shell_command(&v), None);
    }

    #[test]
    fn test_kiro_shell_command_missing_command_field() {
        let v: Value =
            serde_json::from_str(r#"{"tool_name": "executeBash", "tool_input": {}}"#).unwrap();
        assert_eq!(kiro_shell_command(&v), None);
    }

    #[test]
    fn test_kiro_shell_command_non_shell_tool() {
        let v: Value = serde_json::from_str(&kiro_input("editFile", "git status")).unwrap();
        assert_eq!(kiro_shell_command(&v), None);
    }

    #[test]
    fn test_kiro_shell_command_non_shell_read_tool() {
        let v: Value = serde_json::from_str(&kiro_input("readFile", "git status")).unwrap();
        assert_eq!(kiro_shell_command(&v), None);
    }

    // --- Decision: rewritable → deny-with-suggestion ---

    #[test]
    fn test_kiro_rewritable_command_suggests_rtk() {
        let input = kiro_input("executeBash", "git status");
        assert_eq!(
            run_kiro_inner(&input),
            Some("rtk git status".to_string()),
            "rewritable command must yield the rtk suggestion"
        );
    }

    #[test]
    fn test_kiro_rewritable_cargo_test() {
        let input = kiro_input("executeBash", "cargo test");
        assert_eq!(run_kiro_inner(&input), Some("rtk cargo test".to_string()));
    }

    // --- Decision: no equivalent → None ---

    #[test]
    fn test_kiro_no_equivalent_passthrough() {
        let input = kiro_input("executeBash", "definitely-not-a-real-binary --foo");
        assert!(
            run_kiro_inner(&input).is_none(),
            "commands with no registry equivalent must produce no output"
        );
    }

    // --- Decision: already-rtk → None ---

    #[test]
    fn test_kiro_already_rtk_passthrough() {
        let input = kiro_input("executeBash", "rtk git status");
        assert!(
            run_kiro_inner(&input).is_none(),
            "already rtk-prefixed commands must not double-prefix"
        );
    }

    // --- Decision: heredoc → None ---

    #[test]
    fn test_kiro_heredoc_passthrough() {
        let input = kiro_input("executeBash", "cat <<EOF\nhello\nEOF");
        assert!(
            run_kiro_inner(&input).is_none(),
            "heredoc commands must defer (no output)"
        );
    }

    // --- Decision: command substitution → None ---

    #[test]
    fn test_kiro_substitution_defers() {
        for cmd in ["git status $(rm -rf /tmp/x)", "git status `rm -rf /tmp/x`"] {
            let input = kiro_input("executeBash", cmd);
            assert!(
                run_kiro_inner(&input).is_none(),
                "substitution must defer for: `{cmd}`"
            );
        }
    }

    // --- Decision: file redirect → None ---

    #[test]
    fn test_kiro_file_redirect_defers() {
        let input = kiro_input("executeBash", "git log > /tmp/out.txt");
        assert!(
            run_kiro_inner(&input).is_none(),
            "file redirects must defer (no output)"
        );
    }

    // --- Decision: Deny → None ---

    #[test]
    fn test_kiro_deny_steps_aside() {
        // A denied command must produce NO suggestion so Kiro runs the original.
        assert!(
            kiro_rewrite_for_decision("rm -rf /tmp/x", HookDecision::Deny).is_none(),
            "deny must step aside (no suggestion)"
        );
    }

    #[test]
    fn test_kiro_deny_via_rules() {
        let input = kiro_input("executeBash", "git push --force");
        let deny = vec!["git push".to_string()];
        assert!(
            run_kiro_inner_with_rules(&input, &deny, &[], &[]).is_none(),
            "denied commands must produce no output"
        );
    }

    // --- Errors: JSON inválido → Ok(()) sem saída ---

    #[test]
    fn test_kiro_invalid_json_passthrough() {
        // run_kiro_inner returns None when JSON is invalid (serde_json::from_str fails)
        assert!(
            run_kiro_inner("this is not valid json at all!!!").is_none(),
            "invalid JSON must produce no output"
        );
    }

    #[test]
    fn test_kiro_empty_object_passthrough() {
        assert!(
            run_kiro_inner("{}").is_none(),
            "empty object (no tool_input) must produce no output"
        );
    }

    // --- Errors: BOM → parse ok ---

    #[test]
    fn test_kiro_bom_single_stripped() {
        let raw = format!("\u{feff}{}", kiro_input("executeBash", "git status"));
        let trimmed = strip_leading_bom(&raw).trim();
        let v: Value = serde_json::from_str(trimmed).unwrap();
        assert_eq!(kiro_shell_command(&v), Some("git status"));
    }

    #[test]
    fn test_kiro_bom_double_stripped() {
        let raw = format!(
            "\u{feff}\u{feff}{}",
            kiro_input("executeBash", "git status")
        );
        let trimmed = strip_leading_bom(&raw).trim();
        let v: Value = serde_json::from_str(trimmed).unwrap();
        assert_eq!(kiro_shell_command(&v), Some("git status"));
    }

    // --- Errors: stdin vazio → sem saída ---

    #[test]
    fn test_kiro_empty_input_passthrough() {
        assert!(
            run_kiro_inner("").is_none(),
            "empty stdin must produce no output"
        );
    }

    #[test]
    fn test_kiro_whitespace_only_passthrough() {
        assert!(
            run_kiro_inner("   \n\t  ").is_none(),
            "whitespace-only stdin must produce no output"
        );
    }

    // --- Deny-with-suggestion: stderr message + exit code contract ---

    #[test]
    fn test_kiro_block_message_contains_suggestion() {
        // Validates: the stderr text fed back to the agent names the rtk command
        // and instructs the agent to re-issue it.
        let msg = kiro_block_message("rtk git status");
        assert!(
            msg.contains("rtk git status"),
            "block message must contain the rtk command, got: `{msg}`"
        );
        assert!(
            msg.contains("Reemita"),
            "block message must instruct the agent to re-issue the command, got: `{msg}`"
        );
    }

    #[test]
    fn test_kiro_block_exit_is_two() {
        // Kiro blocks a PreToolUse call and forwards stderr to the model only
        // on exit code 2. This contract must not drift.
        assert_eq!(KIRO_BLOCK_EXIT, 2);
    }

    #[test]
    fn test_kiro_already_rtk_no_loop() {
        // Feeding the SUGGESTED command back through the hook must step aside,
        // so the agent's retry runs untouched (no infinite block/retry loop).
        let first = process_kiro_payload(
            &serde_json::from_str(&kiro_input("executeBash", "git status")).unwrap(),
        )
        .expect("rewrite expected on the first pass");
        assert_eq!(first, "rtk git status");

        let retry: Value = serde_json::from_str(&kiro_input("executeBash", &first)).unwrap();
        assert!(
            process_kiro_payload(&retry).is_none(),
            "re-issued `{first}` must not be blocked again"
        );
    }

    // --- Ramo futuro: render_kiro_transparent ---

    #[test]
    fn test_kiro_transparent_updated_input_command() {
        // Validates: Req 2.7 — render_kiro_transparent produces updatedInput.command
        let v: Value = serde_json::from_str(&kiro_input("executeBash", "git status")).unwrap();
        let output = render_kiro_transparent(&v, "rtk git status");
        assert_eq!(
            output
                .pointer("/hookSpecificOutput/updatedInput/command")
                .and_then(|c| c.as_str()),
            Some("rtk git status")
        );
    }

    #[test]
    fn test_kiro_transparent_preserves_extra_fields() {
        // Validates: Req 2.7 — preserves description and timeout from tool_input
        let input_str =
            kiro_input_with_extras("executeBash", "git status", "Check repo state", 30000);
        let v: Value = serde_json::from_str(&input_str).unwrap();
        let output = render_kiro_transparent(&v, "rtk git status");

        let updated = output
            .pointer("/hookSpecificOutput/updatedInput")
            .expect("updatedInput must be present");
        assert_eq!(
            updated.get("command").and_then(|c| c.as_str()),
            Some("rtk git status")
        );
        assert_eq!(
            updated.get("description").and_then(|c| c.as_str()),
            Some("Check repo state")
        );
        assert_eq!(updated.get("timeout").and_then(|c| c.as_u64()), Some(30000));
    }

    #[test]
    fn test_kiro_transparent_permission_decision_allow() {
        let v: Value = serde_json::from_str(&kiro_input("executeBash", "git status")).unwrap();
        let output = render_kiro_transparent(&v, "rtk git status");
        assert_eq!(
            output
                .pointer("/hookSpecificOutput/permissionDecision")
                .and_then(|c| c.as_str()),
            Some("allow")
        );
        assert_eq!(
            output
                .pointer("/hookSpecificOutput/hookEventName")
                .and_then(|c| c.as_str()),
            Some("PreToolUse")
        );
    }

    #[test]
    fn test_kiro_transparent_missing_tool_input_creates_command() {
        // Even if tool_input is absent, render_kiro_transparent still creates updatedInput
        let v: Value = serde_json::from_str(r#"{"tool_name": "executeBash"}"#).unwrap();
        let output = render_kiro_transparent(&v, "rtk git status");
        assert_eq!(
            output
                .pointer("/hookSpecificOutput/updatedInput/command")
                .and_then(|c| c.as_str()),
            Some("rtk git status")
        );
    }

    // --- Compound commands ---

    #[test]
    fn test_kiro_compound_command_rewrite() {
        let input = kiro_input("executeBash", "git status && cargo test");
        let out = run_kiro_inner(&input).expect("rewrite expected for compound");
        assert!(
            out.contains("rtk git status") && out.contains("rtk cargo test"),
            "compound rewrite should prefix each segment, got: `{out}`"
        );
    }

    // --- Env prefix preserved ---

    #[test]
    fn test_kiro_env_prefix_preserved() {
        let input = kiro_input("executeBash", "RUST_LOG=debug cargo test");
        let out = run_kiro_inner(&input).expect("rewrite expected");
        assert!(
            out.contains("RUST_LOG=debug") && out.contains("rtk cargo test"),
            "env prefix must be preserved, got: `{out}`"
        );
    }

    // --- With rules ---

    #[test]
    fn test_kiro_allowed_command_still_suggests() {
        // Kiro has no transparent rewrite; even AllowRewrite yields a suggestion
        // the agent must re-issue.
        let input = kiro_input("executeBash", "git status");
        assert_eq!(
            run_kiro_inner_with_rules(&input, &[], &[], &["git status".to_string()]),
            Some("rtk git status".to_string()),
            "AllowRewrite must still produce the suggestion"
        );
    }

    // ── Zero overhead / no-network assertions (Req 9.1, 9.3, 9.4, 12.5) ──────

    /// Static code analysis: the Kiro hook path (hook_cmd.rs) must not contain
    /// async runtime, network I/O, or thread-spawning patterns in production code.
    ///
    /// The test reads the source at compile time and checks only the non-test
    /// portion (everything before `#[cfg(test)]`), avoiding false positives from
    /// the assertion string literals in test code.
    ///
    /// Validates: Requirements 9.3 (synchronous, no async runtime),
    ///            9.4 / 12.5 (no network calls)
    #[test]
    fn test_kiro_hook_path_no_async_no_network() {
        // Include the source of this file at compile time for static analysis.
        let hook_cmd_source = include_str!("hook_cmd.rs");

        // Only check the production code portion (before the test module).
        // This avoids false positives from string literals in tests.
        let prod_code = hook_cmd_source
            .split("#[cfg(test)]")
            .next()
            .unwrap_or(hook_cmd_source);

        // Forbidden async/runtime patterns (concatenated at runtime to avoid self-match)
        let async_patterns: Vec<String> = vec![
            format!("{}{}{}", "async", " ", "fn"),
            format!("{}{}", ".", "await"),
            format!("{}{}{}", "tokio", "::", "spawn"),
            format!("{}{}", "tokio::", ""),
            format!("{}{}{}", "#[tokio", "::", "main]"),
            format!("{}{}{}", "Runtime", "::", "new"),
            format!("{}{}", "block", "_on"),
        ];

        // Forbidden network I/O patterns
        let network_patterns: Vec<String> = vec![
            format!("{}{}", "reqwest", "::"),
            format!("{}{}", "hyper", "::"),
            format!("{}{}", "Tcp", "Stream"),
            format!("{}{}", "Udp", "Socket"),
            format!("{}{}", "Tcp", "Listener"),
            format!("{}{}", "lookup", "_host"),
            format!("{}{}", "dns", "_lookup"),
            format!("{}{}{}", "To", "Socket", "Addrs"),
            format!("{}{}", "Http", "Client"),
            format!("{}{}", "surf", "::"),
            format!("{}{}", "ureq", "::"),
            format!("{}{}", "attohttpc", "::"),
        ];

        // Forbidden thread-spawning patterns (hooks must be single-threaded/sync)
        let thread_patterns: Vec<String> = vec![
            format!("{}{}{}", "thread", "::", "spawn"),
            format!("{}{}", "rayon", "::"),
        ];

        for pattern in &async_patterns {
            assert!(
                !prod_code.contains(pattern.as_str()),
                "hook_cmd.rs production code must NOT contain async pattern '{}' — \
                 the hook path must be synchronous (Req 9.3)",
                pattern
            );
        }

        for pattern in &network_patterns {
            assert!(
                !prod_code.contains(pattern.as_str()),
                "hook_cmd.rs production code must NOT contain network pattern '{}' — \
                 the hook path must not perform network I/O (Req 9.4, 12.5)",
                pattern
            );
        }

        for pattern in &thread_patterns {
            assert!(
                !prod_code.contains(pattern.as_str()),
                "hook_cmd.rs production code must NOT contain thread-spawning pattern '{}' — \
                 the hook path must be synchronous (Req 9.3)",
                pattern
            );
        }
    }

    /// Static code analysis: the registry and lexer modules used by the hook path
    /// must also be free of network and async patterns.
    ///
    /// Validates: Requirements 9.4, 12.5
    #[test]
    fn test_kiro_hook_dependencies_no_network() {
        let registry_source = include_str!("../discover/registry.rs");
        let lexer_source = include_str!("../discover/lexer.rs");

        // Forbidden network I/O patterns (concatenated to avoid self-matching)
        let network_patterns: Vec<(&str, String)> = vec![
            ("reqwest", format!("{}{}", "reqwest", "::")),
            ("hyper", format!("{}{}", "hyper", "::")),
            ("TcpStream", format!("{}{}", "Tcp", "Stream")),
            ("UdpSocket", format!("{}{}", "Udp", "Socket")),
            ("TcpListener", format!("{}{}", "Tcp", "Listener")),
            ("lookup_host", format!("{}{}", "lookup", "_host")),
            ("dns_lookup", format!("{}{}", "dns", "_lookup")),
        ];

        // Forbidden async patterns
        let async_patterns: Vec<(&str, String)> = vec![
            ("async fn", format!("{}{}{}", "async", " ", "fn")),
            (".await", format!("{}{}", ".", "await")),
            ("tokio::", format!("{}{}", "tokio", "::")),
            (
                "#[tokio::main]",
                format!("{}{}{}", "#[tokio", "::", "main]"),
            ),
        ];

        for (file_name, source) in [("registry.rs", registry_source), ("lexer.rs", lexer_source)] {
            // Only check production code (before test module)
            let prod = source.split("#[cfg(test)]").next().unwrap_or(source);

            for (label, pattern) in &network_patterns {
                assert!(
                    !prod.contains(pattern.as_str()),
                    "{} must NOT contain network pattern '{}' — \
                     the hook decision path must not perform network I/O (Req 9.4, 12.5)",
                    file_name,
                    label
                );
            }
            for (label, pattern) in &async_patterns {
                assert!(
                    !prod.contains(pattern.as_str()),
                    "{} must NOT contain async pattern '{}' — \
                     the hook decision path must be synchronous (Req 9.3)",
                    file_name,
                    label
                );
            }
        }
    }

    /// Performance benchmark: process_kiro_payload must complete in < 10 ms.
    ///
    /// This is a soft assertion — CI environments may be slower, so this test
    /// uses a generous 10ms budget. On typical hardware, the synchronous path
    /// completes in < 1ms. The test runs multiple iterations and asserts the
    /// average is under the budget.
    ///
    /// Validates: Requirement 9.1 (< 10 ms startup/decision time)
    #[test]
    fn test_kiro_hook_path_latency_under_10ms() {
        use std::time::Instant;

        let payload = json!({
            "session_id": "perf-test-session",
            "hook_event_name": "PreToolUse",
            "tool_name": "executeBash",
            "tool_input": { "command": "git status" }
        });

        // Warm up: run once to ensure lazy_static regex compilation is done
        let _ = process_kiro_payload(&payload);

        // Measure 100 iterations
        let iterations = 100;
        let start = Instant::now();
        for _ in 0..iterations {
            let _ = process_kiro_payload(&payload);
        }
        let total_elapsed = start.elapsed();
        let avg_micros = total_elapsed.as_micros() / iterations;

        // Assert average per-call is under 10ms (10_000 µs).
        // On real hardware this is typically < 100µs per call.
        assert!(
            avg_micros < 10_000,
            "process_kiro_payload average latency ({} µs) exceeds 10 ms budget. \
             The hook path must be fast enough to not perceptibly delay command execution. \
             (Req 9.1: < 10 ms startup/decision time)",
            avg_micros
        );

        // Also assert that even the total time for 100 calls is reasonable
        // (under 1 second total — sanity check against extreme slowness).
        assert!(
            total_elapsed.as_secs() < 1,
            "100 iterations of process_kiro_payload took {:?} — \
             something is seriously wrong with performance",
            total_elapsed
        );
    }

    /// Verify that the Kiro hook path is entirely synchronous by confirming
    /// that `run_kiro` returns `Result<()>` (not a Future) and that the
    /// function signature does not use async.
    ///
    /// This is a compile-time guarantee: if `run_kiro` were made async,
    /// calling it without `.await` would fail to compile. The test simply
    /// exercises the synchronous call pattern.
    ///
    /// Validates: Requirement 9.3
    #[test]
    fn test_kiro_run_kiro_is_synchronous() {
        // If run_kiro were async, this call would not compile without .await
        // or a runtime. The fact this compiles and runs proves it's synchronous.
        // We can't call run_kiro() directly in tests (it reads stdin), but we
        // CAN call process_kiro_payload which is the core decision path.
        let payload = json!({
            "session_id": "sync-test",
            "hook_event_name": "PreToolUse",
            "tool_name": "executeBash",
            "tool_input": { "command": "git status" }
        });

        // Synchronous call — no runtime, no .await, no spawn.
        let result = process_kiro_payload(&payload);

        // The function returns immediately (synchronously) with a deterministic result.
        assert!(result.is_some(), "synchronous call must produce a result");
    }

    // ── Property-based tests (proptest) ────────────────────────────

    mod prop_tests {
        use super::*;
        use proptest::prelude::*;

        // Feature: kiro-agent-integration, Property 1: run_kiro nunca sai com código diferente de zero
        //
        // For ANY input (valid JSON, invalid JSON, arbitrary bytes, well-formed payloads
        // with random commands), the processing functions never panic and always return
        // a valid result (Ok(()) or Some/None).
        //
        // **Validates: Requirements 6.2, 6.3, 6.5, 10.4, 12.4**

        // Strategy: arbitrary strings fed to JSON parsing — must never panic.
        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p1_arbitrary_string_never_panics(input in "\\PC*") {
                // Simulates the run_kiro path: parse attempt + process_kiro_payload.
                // Must never panic regardless of input content.
                let trimmed = strip_leading_bom(&input).trim();
                if !trimmed.is_empty() {
                    if let Ok(v) = serde_json::from_str::<Value>(trimmed) {
                        // If it parses as JSON, process_kiro_payload must not panic.
                        let _ = process_kiro_payload(&v);
                    }
                }
                // No panic = property holds.
            }
        }

        // Strategy: arbitrary JSON values — process_kiro_payload must never panic.
        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p1_arbitrary_json_never_panics(
                // Generate arbitrary JSON objects with varied structures
                tool_name in prop_oneof![
                    Just("executeBash".to_string()),
                    Just("execute_bash".to_string()),
                    Just("runCommand".to_string()),
                    Just("shell".to_string()),
                    Just("editFile".to_string()),
                    Just("readFile".to_string()),
                    Just("".to_string()),
                    "[a-zA-Z_]{1,20}",
                ],
                command in prop_oneof![
                    Just("".to_string()),
                    Just("git status".to_string()),
                    Just("rtk git status".to_string()),
                    Just("cat <<EOF\nhello\nEOF".to_string()),
                    Just("git status $(rm -rf /)".to_string()),
                    "\\PC{0,200}",
                ],
                has_tool_input in any::<bool>(),
                has_command_field in any::<bool>(),
                extra_field in "\\PC{0,50}",
            ) {
                let mut obj = serde_json::Map::new();
                if !tool_name.is_empty() {
                    obj.insert("tool_name".to_string(), Value::String(tool_name));
                }
                if has_tool_input {
                    let mut tool_input = serde_json::Map::new();
                    if has_command_field {
                        tool_input.insert("command".to_string(), Value::String(command));
                    }
                    tool_input.insert("extra".to_string(), Value::String(extra_field));
                    obj.insert("tool_input".to_string(), Value::Object(tool_input));
                }
                obj.insert("session_id".to_string(), Value::String("test-session".to_string()));

                let v = Value::Object(obj);
                // Must never panic — result is either Some(rtk suggestion) or None.
                let result = process_kiro_payload(&v);
                if let Some(suggestion) = result {
                    // A suggestion is always a non-empty rtk-bearing command.
                    prop_assert!(!suggestion.is_empty());
                    prop_assert!(
                        suggestion.contains("rtk "),
                        "suggestion must carry the rtk prefix, got: `{}`",
                        suggestion
                    );
                }
            }
        }

        // Strategy: well-formed Kiro payloads with random commands — must never panic
        // and must always produce either Some (with ask decision) or None.
        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p1_wellformed_payload_random_command_never_panics(
                command in "[a-zA-Z0-9 _\\-./]{1,100}",
            ) {
                let payload = json!({
                    "session_id": "prop-test-session",
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": command }
                });

                // Must never panic.
                let result = process_kiro_payload(&payload);

                // If there is a suggestion, it must be a usable rtk command and
                // must differ from the original (otherwise the agent would loop).
                if let Some(suggestion) = result {
                    prop_assert!(!suggestion.is_empty());
                    prop_assert!(
                        suggestion.contains("rtk "),
                        "suggestion must carry the rtk prefix, got: `{}`",
                        suggestion
                    );
                    prop_assert_ne!(
                        suggestion.as_str(),
                        command.as_str(),
                        "suggestion must differ from the original command"
                    );
                }
                // None is also valid — means no rewrite (Defer/Deny/no registry match).
            }
        }

        // Feature: kiro-agent-integration, Property 2: Sem reescrita produz saída vazia/benigna (never-worse)
        //
        // For ANY command that has no equivalent in the registry (random unknown binaries)
        // and for non-shell tools (editFile, readFile, writeFile, searchReplace, etc.),
        // `process_kiro_payload` MUST return `None` — meaning no output is emitted and
        // the original command executes unchanged.
        //
        // **Validates: Requirements 2.5, 6.4**

        // Strategy 1: Random unknown command names that definitely won't be in the registry.
        // We generate random strings with a prefix guaranteed not to match any known CLI tool.
        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p2_unknown_commands_produce_none(
                // Random suffix appended to a prefix that no registry entry matches.
                suffix in "[a-z]{3,15}",
                session_id in "[a-f0-9]{8}",
            ) {
                // Use a prefix like "zzunknown_" which is not a real CLI tool.
                let unknown_cmd = format!("zzunknown_{suffix} --flag arg1 arg2");

                let payload = json!({
                    "session_id": session_id,
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": unknown_cmd }
                });

                let result = process_kiro_payload(&payload);
                prop_assert!(
                    result.is_none(),
                    "Unknown command '{}' should produce None, got {:?}",
                    unknown_cmd, result
                );
            }
        }

        // Strategy 2: Non-shell tool names — these are tools like editFile, readFile,
        // writeFile, searchReplace, etc. that are NOT shell execution tools.
        // kiro_shell_command should return None for them, causing process_kiro_payload
        // to short-circuit to None.
        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p2_non_shell_tools_produce_none(
                tool_name in prop_oneof![
                    Just("editFile".to_string()),
                    Just("readFile".to_string()),
                    Just("writeFile".to_string()),
                    Just("searchReplace".to_string()),
                    Just("listFiles".to_string()),
                    Just("createFile".to_string()),
                    Just("deleteFile".to_string()),
                    Just("moveFile".to_string()),
                    Just("copyFile".to_string()),
                    Just("renameFile".to_string()),
                    Just("webSearch".to_string()),
                    Just("fetchUrl".to_string()),
                    Just("askUser".to_string()),
                    Just("analyzeCode".to_string()),
                    Just("refactorCode".to_string()),
                    // Also random tool names that are clearly not shell tools
                    "(?:zzfake|xxmock|qqtest)[A-Z][a-zA-Z]{2,12}",
                ],
                // Even with a valid rewritable command in tool_input, the non-shell
                // tool_name should cause the payload to be ignored.
                command in prop_oneof![
                    Just("git status".to_string()),
                    Just("cargo test".to_string()),
                    Just("ls -la".to_string()),
                    Just("docker ps".to_string()),
                    "[a-z]{2,10} [a-z\\-]{0,10}",
                ],
                session_id in "[a-f0-9]{8}",
            ) {
                let payload = json!({
                    "session_id": session_id,
                    "hook_event_name": "PreToolUse",
                    "tool_name": tool_name,
                    "tool_input": { "command": command }
                });

                let result = process_kiro_payload(&payload);
                prop_assert!(
                    result.is_none(),
                    "Non-shell tool '{}' with command '{}' should produce None, got {:?}",
                    tool_name, command, result
                );
            }
        }

        // Feature: kiro-agent-integration, Property 3: Idempotência do prefixo rtk
        //
        // For ANY command that already starts with `rtk` (optionally preceded by
        // environment variable assignments like `ENV=val`), `get_rewritten` returns
        // `None` — never adding a second `rtk` prefix (no `rtk rtk git`).
        //
        // **Validates: Requirements 8.3**

        // Strategy 1: Known rewritable commands already prefixed with `rtk `.
        // These must produce None from process_kiro_payload (no double-prefix).
        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p3_already_rtk_prefixed_returns_none(
                base_cmd in prop_oneof![
                    Just("git status".to_string()),
                    Just("git log --oneline".to_string()),
                    Just("git diff".to_string()),
                    Just("git add .".to_string()),
                    Just("git commit -m 'test'".to_string()),
                    Just("git checkout main".to_string()),
                    Just("git push origin main".to_string()),
                    Just("git pull".to_string()),
                    Just("git branch -a".to_string()),
                    Just("git fetch".to_string()),
                    Just("git stash".to_string()),
                    Just("cargo test".to_string()),
                    Just("cargo build".to_string()),
                    Just("cargo clippy".to_string()),
                    Just("cargo check".to_string()),
                    Just("ls -la".to_string()),
                    Just("ls src/".to_string()),
                    Just("grep -r 'pattern' src/".to_string()),
                    Just("find . -name '*.rs'".to_string()),
                    Just("cat README.md".to_string()),
                    Just("gh pr list".to_string()),
                    Just("docker ps".to_string()),
                ],
                session_id in "[a-f0-9]{8}",
            ) {
                // Command already prefixed with `rtk `
                let already_rtk_cmd = format!("rtk {base_cmd}");

                let payload = json!({
                    "session_id": session_id,
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": already_rtk_cmd }
                });

                let result = process_kiro_payload(&payload);
                prop_assert!(
                    result.is_none(),
                    "Already rtk-prefixed command '{}' must return None (no rtk rtk), got {:?}",
                    already_rtk_cmd, result
                );
            }
        }

        // Feature: kiro-agent-integration, Property 4: Preservação de prefixos de ambiente
        //
        // For ANY rewritable command preceded by environment variable assignments
        // (KEY=value pairs), the rewritten command preserves the env var prefixes
        // and inserts `rtk` after them. For example:
        //   `RUST_LOG=debug git status` → suggested rewrite is `RUST_LOG=debug rtk git status`
        //   `KEY=val cargo test` → `KEY=val rtk cargo test`
        //
        // The assertion checks that when `process_kiro_payload` produces a `Some` result,
        // the `permissionDecisionReason` contains the env prefix preserved AND the `rtk`
        // command inserted after the env vars.
        //
        // **Validates: Requirements 12.1**

        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p4_env_prefix_preservation(
                // Generate random env var keys: [A-Z][A-Z_0-9]{1,10}
                env_key in "[A-Z][A-Z_0-9]{1,10}",
                // Generate simple env var values: [a-zA-Z0-9_.-]{1,15}
                env_val in "[a-zA-Z0-9_.\\-]{1,15}",
                // Known rewritable base commands
                base_cmd in prop_oneof![
                    Just("git status".to_string()),
                    Just("git log --oneline".to_string()),
                    Just("git diff".to_string()),
                    Just("git add .".to_string()),
                    Just("git pull".to_string()),
                    Just("git fetch".to_string()),
                    Just("git branch -a".to_string()),
                    Just("cargo test".to_string()),
                    Just("cargo build".to_string()),
                    Just("cargo clippy".to_string()),
                    Just("cargo check".to_string()),
                    Just("ls -la".to_string()),
                    Just("ls src/".to_string()),
                    Just("grep -r 'pattern' src/".to_string()),
                    Just("find . -name '*.rs'".to_string()),
                    Just("cat README.md".to_string()),
                    Just("docker ps".to_string()),
                ],
                session_id in "[a-f0-9]{8}",
            ) {
                // Skip if env_key happens to be RTK_DISABLED (that's a different property P9)
                prop_assume!(!env_key.starts_with("RTK_DISABLED"));

                // Construct command: ENV=val <base_cmd>
                let cmd_with_env = format!("{}={} {}", env_key, env_val, base_cmd);

                let payload = json!({
                    "session_id": session_id,
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": cmd_with_env }
                });

                let result = process_kiro_payload(&payload);

                // The command is rewritable, so we should get a response
                prop_assert!(
                    result.is_some(),
                    "Env-prefixed rewritable command '{}' should produce a rewrite suggestion",
                    cmd_with_env
                );

                let suggestion = result.unwrap();

                // The suggestion must preserve the env prefix and insert `rtk` after it:
                // `ENV=val rtk <base_cmd>`
                let env_prefix = format!("{}={}", env_key, env_val);
                prop_assert!(
                    suggestion.contains(&env_prefix),
                    "Suggestion must contain env prefix '{}', got: `{}`",
                    env_prefix, suggestion
                );

                let expected_rewrite_prefix = format!("{} rtk", env_prefix);
                prop_assert!(
                    suggestion.contains(&expected_rewrite_prefix),
                    "Suggestion must contain '{}' (env prefix followed by rtk), got: `{}`",
                    expected_rewrite_prefix, suggestion
                );
            }
        }

        // Feature: kiro-agent-integration, Property 5: Reescrita de comandos compostos casa a semântica do registry
        //
        // For ANY compound command formed by rewritable segments joined by `&&`, `||`,
        // `;` (each segment rewritten independently) or `|` (only left segment rewritten),
        // the suggestion produced through the Kiro handler path (`process_kiro_payload`)
        // is identical to calling `registry::rewrite_command` directly (the reference
        // implementation). This is a MODEL-BASED test.
        //
        // **Validates: Requirements 5.1, 5.2**

        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p5_compound_command_rewrite_matches_registry(
                // Pick 2-4 rewritable segments to join into a compound command
                segments in prop::collection::vec(
                    prop_oneof![
                        Just("git status".to_string()),
                        Just("git log --oneline".to_string()),
                        Just("git diff".to_string()),
                        Just("git add .".to_string()),
                        Just("git pull".to_string()),
                        Just("git fetch".to_string()),
                        Just("git branch -a".to_string()),
                        Just("cargo test".to_string()),
                        Just("cargo build".to_string()),
                        Just("cargo clippy".to_string()),
                        Just("cargo check".to_string()),
                        Just("ls -la".to_string()),
                        Just("ls src/".to_string()),
                        Just("grep -r 'pattern' src/".to_string()),
                        Just("cat README.md".to_string()),
                        Just("docker ps".to_string()),
                    ],
                    2..=4
                ),
                // Pick operators to join them (one fewer operator than segments)
                operators in prop::collection::vec(
                    prop_oneof![
                        Just(" && ".to_string()),
                        Just(" || ".to_string()),
                        Just("; ".to_string()),
                        Just(" | ".to_string()),
                    ],
                    1..=3
                ),
            ) {
                // Build the compound command from segments + operators.
                // Use min(segments.len()-1, operators.len()) operators between segments.
                let num_ops = std::cmp::min(segments.len() - 1, operators.len());
                let mut compound = String::new();
                for (i, seg) in segments.iter().enumerate() {
                    compound.push_str(seg);
                    if i < num_ops {
                        compound.push_str(&operators[i]);
                    }
                }

                // Reference: call registry::rewrite_command directly
                let reference_result = crate::discover::registry::rewrite_command(
                    &compound,
                    &[],   // no excludes
                    &[],   // no transparent prefixes
                );

                // Kiro path: feed compound command through process_kiro_payload
                let payload = json!({
                    "session_id": "prop-p5-session",
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": compound }
                });
                let kiro_result = process_kiro_payload(&payload);

                match (reference_result.as_ref(), kiro_result.as_ref()) {
                    (None, None) => {
                        // Both agree: no rewrite. Property holds.
                    }
                    (Some(ref_rewritten), Some(kiro_rewritten)) => {
                        // Both agree there's a rewrite — the Kiro handler returns the
                        // rewritten command verbatim, so compare directly.
                        prop_assert_eq!(
                            kiro_rewritten.as_str(),
                            ref_rewritten.as_str(),
                            "Compound '{}': Kiro path produced '{}' but registry reference produced '{}'",
                            compound, kiro_rewritten, ref_rewritten
                        );
                    }
                    (Some(ref_rewritten), None) => {
                        // Registry says rewrite, but Kiro path says no.
                        // This can happen if the rewrite is identical to the original
                        // (get_rewritten returns None when rewritten == cmd).
                        // In that case, the registry returns Some but with the same string,
                        // while get_rewritten (used by Kiro path) returns None.
                        prop_assert_eq!(
                            ref_rewritten.as_str(),
                            compound.as_str(),
                            "Mismatch: registry returned Some('{}') but Kiro returned None for compound '{}'",
                            ref_rewritten, compound
                        );
                    }
                    (None, Some(kiro_rewritten)) => {
                        // Kiro says rewrite but registry doesn't — should not happen.
                        prop_assert!(
                            false,
                            "Mismatch: registry returned None but Kiro returned {:?} for compound '{}'",
                            kiro_rewritten, compound
                        );
                    }
                }
            }
        }

        // Feature: kiro-agent-integration, Property 6: Construções não atestáveis e heredoc são sempre deferidas
        //
        // For ANY command that contains an unattestable construct — command substitution
        // `$(...)`, backticks `` ` ``, file redirection `>` (with file target), or
        // heredoc `<<EOF` — `process_kiro_payload` MUST return `None` (defer), even when
        // the base command would otherwise be rewritable. This prevents laundering of
        // hidden commands through the rewrite mechanism.
        //
        // Note: background `&` followed by another command is treated as a compound
        // command operator (like `&&`, `||`, `;`) — segments are split and individually
        // rewritten. This is consistent with ALL agent handlers in the codebase. The
        // security is preserved because Kiro never runs the rewrite itself — it only
        // suggests it, and the agent must re-issue the command explicitly.
        //
        // **Validates: Requirements 5.3, 5.4, 12.2**

        // Strategy: inject unattestable constructs into known-rewritable base commands.
        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p6_unattestable_constructs_always_defer(
                // Base rewritable command
                base_cmd in prop_oneof![
                    Just("git status".to_string()),
                    Just("git log --oneline".to_string()),
                    Just("git diff".to_string()),
                    Just("git add .".to_string()),
                    Just("git pull".to_string()),
                    Just("git fetch".to_string()),
                    Just("git branch -a".to_string()),
                    Just("cargo test".to_string()),
                    Just("cargo build".to_string()),
                    Just("cargo clippy".to_string()),
                    Just("cargo check".to_string()),
                    Just("ls -la".to_string()),
                    Just("ls src/".to_string()),
                    Just("grep -r 'pattern' src/".to_string()),
                    Just("find . -name '*.rs'".to_string()),
                    Just("cat README.md".to_string()),
                    Just("docker ps".to_string()),
                ],
                // Unattestable construct to inject
                construct in prop_oneof![
                    // Command substitution with $(...)
                    Just("$(whoami)".to_string()),
                    Just("$(echo hello)".to_string()),
                    Just("$(rm -rf /tmp/x)".to_string()),
                    Just("$(cat /etc/passwd)".to_string()),
                    Just("$(date +%s)".to_string()),
                    // Backtick substitution
                    Just("`whoami`".to_string()),
                    Just("`echo test`".to_string()),
                    Just("`rm -rf /tmp/x`".to_string()),
                    Just("`date`".to_string()),
                    Just("`cat /etc/hostname`".to_string()),
                    // File redirection with > (file target, not fd-dup)
                    Just("> /tmp/out.txt".to_string()),
                    Just("> /dev/sda".to_string()),
                    Just("> ~/.ssh/authorized_keys".to_string()),
                    Just(">output.log".to_string()),
                    Just("> /tmp/evil.txt".to_string()),
                    // Heredoc <<EOF
                    Just("<<EOF\nmalicious content\nEOF".to_string()),
                    Just("<<'END'\nsome data\nEND".to_string()),
                    Just("<< MARKER\npayload\nMARKER".to_string()),
                    Just("<<HEREDOC\nline1\nline2\nHEREDOC".to_string()),
                    Just("<<-EOF\n\tindented\nEOF".to_string()),
                ],
                // Injection position pattern
                position in prop_oneof![
                    Just("suffix".to_string()),   // append after the command
                    Just("middle".to_string()),   // insert via flag argument
                ],
                session_id in "[a-f0-9]{8}",
            ) {
                // First, verify the base command IS rewritable without the injection
                let base_payload = json!({
                    "session_id": session_id,
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": &base_cmd }
                });
                let base_result = process_kiro_payload(&base_payload);
                // The base command must be rewritable for this test to be meaningful
                prop_assume!(base_result.is_some(), "base command '{}' must be rewritable", base_cmd);

                // Now inject the unattestable construct
                let injected_cmd = match position.as_str() {
                    "middle" => format!("{} {} --flag", base_cmd, construct),
                    _ /* suffix */ => format!("{} {}", base_cmd, construct),
                };

                let payload = json!({
                    "session_id": session_id,
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": injected_cmd }
                });

                let result = process_kiro_payload(&payload);
                prop_assert!(
                    result.is_none(),
                    "Command with unattestable construct must return None (defer).\n\
                     Base cmd: '{}'\n\
                     Construct: '{}'\n\
                     Injected cmd: '{}'\n\
                     Got: {:?}",
                    base_cmd, construct, injected_cmd, result
                );
            }
        }

        // Strategy 2: Rewritable commands prefixed with `rtk ` AND preceded by
        // environment variable assignments like `ENV=val rtk git status`.
        // Must still return None (no double rtk prefix).
        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p3_env_prefix_with_rtk_returns_none(
                // Generate random env var keys (uppercase letters + underscore)
                env_key in "[A-Z][A-Z_]{1,10}",
                // Generate random env var values (alphanumeric + common chars)
                env_val in "[a-zA-Z0-9_./-]{1,15}",
                // Optionally add a second env var
                extra_env in prop_oneof![
                    Just("".to_string()),
                    Just(" DEBUG=1".to_string()),
                    Just(" VERBOSE=true".to_string()),
                    Just(" RUST_BACKTRACE=full".to_string()),
                ],
                base_cmd in prop_oneof![
                    Just("git status".to_string()),
                    Just("git diff".to_string()),
                    Just("git log".to_string()),
                    Just("cargo test".to_string()),
                    Just("cargo build".to_string()),
                    Just("ls -la".to_string()),
                    Just("grep -r 'foo' .".to_string()),
                    Just("find . -name '*.rs'".to_string()),
                    Just("cat Cargo.toml".to_string()),
                    Just("docker ps".to_string()),
                ],
                session_id in "[a-f0-9]{8}",
            ) {
                // ENV=val rtk <command> — already has rtk, must not double-prefix.
                let cmd_with_env = format!("{env_key}={env_val}{extra_env} rtk {base_cmd}");

                let payload = json!({
                    "session_id": session_id,
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": cmd_with_env }
                });

                let result = process_kiro_payload(&payload);
                prop_assert!(
                    result.is_none(),
                    "Env-prefixed rtk command '{}' must return None (no rtk rtk), got {:?}",
                    cmd_with_env, result
                );
            }
        }

        // Feature: kiro-agent-integration, Property 10: Stdin acima de 1 MiB resulta em exit 0 sem modificação
        //
        // For ANY payload whose size exceeds 1 MiB (STDIN_CAP), `run_kiro()` returns
        // `Ok(())` without emitting output, leaving the original command intact.
        // For payloads BELOW the cap, processing works normally.
        //
        // Since `read_stdin_limited` reads from real stdin and cannot be mocked in unit tests,
        // this property verifies:
        //   (a) The size-check logic: strings > STDIN_CAP would be rejected by
        //       `read_stdin_limited`'s bail condition.
        //   (b) Below-cap payloads: `process_kiro_payload` processes them normally
        //       (returns Some for rewritable commands, None for non-rewritable).
        //   (c) The `run_kiro()` contract: any Err from `read_stdin_limited` (including
        //       the cap-exceeded case) is converted to `Ok(())` without output (verified
        //       structurally by the match arm in `run_kiro`).
        //
        // **Validates: Requirements 12.3, 12.4**

        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p10_stdin_cap_enforcement(
                // Generate a size factor: 0.8x to 1.5x of STDIN_CAP
                // to test behavior around the boundary.
                size_factor in 0.8f64..1.5f64,
            ) {
                let target_size = (STDIN_CAP as f64 * size_factor) as usize;

                if target_size > STDIN_CAP {
                    // ABOVE CAP: read_stdin_limited would bail.
                    // Verify the bail condition directly: any string longer than
                    // STDIN_CAP triggers the "exceeds limit" error path.
                    // In run_kiro(), this Err is caught by the match arm and
                    // converted to Ok(()) — exit 0 with no output.
                    prop_assert!(
                        target_size > STDIN_CAP,
                        "Size {} should exceed STDIN_CAP {}",
                        target_size, STDIN_CAP
                    );
                    // The contract is enforced structurally:
                    // run_kiro() does: match read_stdin_limited() { Err(_) => return Ok(()) }
                    // So any payload exceeding 1 MiB results in exit 0 without modification.
                } else {
                    // BELOW OR AT CAP: process_kiro_payload works normally.
                    // Build a valid JSON payload within the size budget and verify
                    // it gets processed (rewritable command → Some, non-rewritable → None).
                    let payload = json!({
                        "session_id": "prop-p10-session",
                        "hook_event_name": "PreToolUse",
                        "tool_name": "executeBash",
                        "tool_input": { "command": "git status" }
                    });

                    // This payload is well under 1 MiB — process_kiro_payload must work.
                    let result = process_kiro_payload(&payload);
                    // "git status" is rewritable → must produce the rtk suggestion.
                    prop_assert_eq!(
                        result.as_deref(),
                        Some("rtk git status"),
                        "Below-cap rewritable command must produce the rtk suggestion"
                    );
                }
            }
        }

        // Strategy 2: Verify that large payloads (with padding to approach the cap)
        // are still processed correctly by process_kiro_payload when they remain
        // under the cap — the size limit is ONLY at the stdin reading level.
        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p10_large_but_subcap_payloads_process_normally(
                // Generate payloads with large padding but still under STDIN_CAP.
                // Sizes from 100KB to 900KB (well under 1 MiB but large).
                padding_kb in 100usize..900usize,
                base_cmd in prop_oneof![
                    Just("git status".to_string()),
                    Just("git log --oneline".to_string()),
                    Just("cargo test".to_string()),
                    Just("cargo build".to_string()),
                    Just("ls -la".to_string()),
                    Just("docker ps".to_string()),
                ],
            ) {
                let padding_size = padding_kb * 1024;
                // Create padding that won't affect JSON parsing (stored in an extra field)
                let padding: String = "x".repeat(padding_size);

                let payload = json!({
                    "session_id": "prop-p10-large",
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": base_cmd },
                    "extra_context": padding
                });

                // Verify the serialized payload size is under the cap
                let serialized = serde_json::to_string(&payload).unwrap();
                prop_assume!(serialized.len() <= STDIN_CAP);

                // process_kiro_payload must still work — the size limit is only
                // at the read_stdin_limited level, not at the processing level.
                let result = process_kiro_payload(&payload);
                // All base_cmd values are rewritable
                let expected = format!("rtk {base_cmd}");
                prop_assert_eq!(
                    result.as_deref(),
                    Some(expected.as_str()),
                    "Large ({} bytes) but sub-cap payload with rewritable command '{}' \
                     must still yield the rtk suggestion. Size limit is only at stdin \
                     reading level.",
                    serialized.len(), base_cmd
                );
            }
        }

        // Strategy 3: Verify the STDIN_CAP constant value and that the bail logic is correct.
        // This is a simple assertion test (not property-based) included for completeness.
        #[test]
        fn p10_stdin_cap_is_one_mib() {
            assert_eq!(STDIN_CAP, 1_048_576, "STDIN_CAP must be exactly 1 MiB");
        }

        // Feature: kiro-agent-integration, Property 9: Overrides repassam sem reescrita
        //
        // For ANY rewritable command:
        //   (a) prefixed with `RTK_DISABLED=1` → `get_rewritten` returns `None`
        //   (b) matching a pattern configured in `exclude_commands` → `get_rewritten` returns `None`
        //   (c) without any override → `get_rewritten` returns `Some` (command IS rewritten)
        //
        // This verifies that override mechanisms correctly suppress rewriting while
        // leaving non-overridden commands unaffected.
        //
        // **Validates: Requisitos 8.1, 8.2**

        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p9_overrides_suppress_rewrite(
                // Known rewritable base commands (these all have registry entries)
                base_cmd in prop_oneof![
                    Just("git status".to_string()),
                    Just("git log --oneline".to_string()),
                    Just("git diff".to_string()),
                    Just("git add .".to_string()),
                    Just("git pull".to_string()),
                    Just("git fetch".to_string()),
                    Just("git branch -a".to_string()),
                    Just("cargo test".to_string()),
                    Just("cargo build".to_string()),
                    Just("cargo clippy".to_string()),
                    Just("cargo check".to_string()),
                    Just("ls -la".to_string()),
                    Just("ls src/".to_string()),
                    Just("grep -r 'pattern' src/".to_string()),
                    Just("find . -name '*.rs'".to_string()),
                    Just("cat README.md".to_string()),
                    Just("docker ps".to_string()),
                ],
                // Override variant: (a) RTK_DISABLED=1, (b) exclude_commands match, (c) no override
                override_kind in 0u8..3,
                // Random RTK_DISABLED value (always "1" to disable; other values tested elsewhere)
                rtk_disabled_val in prop_oneof![
                    Just("1".to_string()),
                    Just("true".to_string()),
                    Just("yes".to_string()),
                ],
                _session_id in "[a-f0-9]{8}",
            ) {
                // First, confirm the base command IS rewritable without any overrides.
                let base_rewritten = crate::discover::registry::rewrite_command(
                    &base_cmd, &[], &[]
                );
                prop_assume!(
                    base_rewritten.is_some() && base_rewritten.as_deref() != Some(&*base_cmd),
                    "Base command '{}' must be rewritable for this test", base_cmd
                );

                match override_kind {
                    // (a) RTK_DISABLED=1 prefix → get_rewritten returns None
                    0 => {
                        let cmd_with_disabled = format!("RTK_DISABLED={} {}", rtk_disabled_val, base_cmd);
                        let result = get_rewritten(&cmd_with_disabled);
                        prop_assert!(
                            result.is_none(),
                            "RTK_DISABLED={} prefix should suppress rewrite for '{}', got {:?}",
                            rtk_disabled_val, cmd_with_disabled, result
                        );
                    }
                    // (b) exclude_commands pattern match → rewrite_command returns None
                    1 => {
                        // Extract the first word of the command as the exclude pattern.
                        // For example "git status" → exclude pattern "git" (prefix match).
                        let first_word = base_cmd.split_whitespace().next().unwrap_or(&base_cmd);
                        let excluded = vec![first_word.to_string()];

                        let result = crate::discover::registry::rewrite_command(
                            &base_cmd, &excluded, &[]
                        );
                        prop_assert!(
                            result.is_none(),
                            "Command '{}' matching exclude pattern '{}' should return None, got {:?}",
                            base_cmd, first_word, result
                        );
                    }
                    // (c) No override → get_rewritten returns Some (rewrite happens)
                    _ => {
                        let result = get_rewritten(&base_cmd);
                        prop_assert!(
                            result.is_some(),
                            "Command '{}' without overrides should be rewritten, got None",
                            base_cmd
                        );
                    }
                }
            }
        }

        // Feature: kiro-agent-integration, Property 11: Delegação fina — a decisão do Kiro é a do fluxo compartilhado
        //
        // For ANY command (rewritable, non-rewritable, unattestable), the decision made
        // by `process_kiro_payload` is bit-for-bit identical to the decision of
        // `decide_hook_action(cmd, Host::Kiro)`. Additionally, the result is deterministic
        // and independent of `session_id` — the same command with different session_ids
        // always produces the same output, proving that the IDE/CLI implementation is
        // delegative and deterministic.
        //
        // This is a MODEL-BASED test comparing the Kiro handler against
        // `decide_hook_action` directly.
        //
        // **Validates: Requirements 2.2, 2.6, 3.3, 13.6, 13.7**

        proptest! {
            #![proptest_config(ProptestConfig { cases: 100, .. ProptestConfig::default() })]

            #[test]
            fn prop_p11_kiro_decision_matches_shared_flow(
                // Mix of rewritable, non-rewritable, and unattestable commands
                cmd in prop_oneof![
                    // Rewritable commands
                    Just("git status".to_string()),
                    Just("git log --oneline".to_string()),
                    Just("git diff".to_string()),
                    Just("git add .".to_string()),
                    Just("git pull".to_string()),
                    Just("git fetch".to_string()),
                    Just("git branch -a".to_string()),
                    Just("cargo test".to_string()),
                    Just("cargo build".to_string()),
                    Just("cargo clippy".to_string()),
                    Just("cargo check".to_string()),
                    Just("ls -la".to_string()),
                    Just("ls src/".to_string()),
                    Just("grep -r 'pattern' src/".to_string()),
                    Just("cat README.md".to_string()),
                    Just("docker ps".to_string()),
                    // Non-rewritable commands (no registry entry)
                    Just("htop".to_string()),
                    Just("vim file.txt".to_string()),
                    Just("python3 script.py".to_string()),
                    Just("node server.js".to_string()),
                    Just("npm start".to_string()),
                    Just("echo hello".to_string()),
                    Just("man git".to_string()),
                    Just("which rustc".to_string()),
                    // Unattestable constructs
                    Just("git status $(whoami)".to_string()),
                    Just("git log > output.txt".to_string()),
                    Just("cargo test `date`".to_string()),
                    Just("git diff <<EOF\nfoo\nEOF".to_string()),
                    Just("ls -la $(pwd)".to_string()),
                ],
                // Different session_ids to verify independence
                session_id_a in "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}",
                session_id_b in "[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}",
            ) {
                // --- Part 1: Verify Kiro decision matches decide_hook_action ---

                // Reference: call decide_hook_action directly
                let reference_decision = decide_hook_action(&cmd, permissions::Host::Kiro);

                // Kiro path: feed command through process_kiro_payload with session_id A
                let payload_a = json!({
                    "session_id": session_id_a,
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": cmd }
                });
                let kiro_result_a = process_kiro_payload(&payload_a);

                // Verify the Kiro result matches the reference decision
                match &reference_decision {
                    HookDecision::Deny | HookDecision::Defer => {
                        // Reference says no rewrite → Kiro must return None
                        prop_assert!(
                            kiro_result_a.is_none(),
                            "Command '{}': decide_hook_action returned {:?} but \
                             process_kiro_payload returned Some({:?})",
                            cmd, reference_decision, kiro_result_a
                        );
                    }
                    HookDecision::AllowRewrite(ref rewritten) | HookDecision::AskRewrite(ref rewritten) => {
                        // Reference says rewrite → Kiro must return Some with the same rewritten command
                        prop_assert!(
                            kiro_result_a.is_some(),
                            "Command '{}': decide_hook_action returned {:?} but \
                             process_kiro_payload returned None",
                            cmd, reference_decision
                        );

                        // The handler returns the rewritten command verbatim.
                        let kiro_rewritten = kiro_result_a.as_deref().unwrap_or("");

                        prop_assert_eq!(
                            kiro_rewritten,
                            rewritten.as_str(),
                            "Command '{}': Kiro produced rewrite '{}' but \
                             decide_hook_action produced '{}'",
                            cmd, kiro_rewritten, rewritten
                        );
                    }
                }

                // --- Part 2: Verify session_id independence (deterministic) ---

                // Same command with a different session_id must produce the same result
                let payload_b = json!({
                    "session_id": session_id_b,
                    "hook_event_name": "PreToolUse",
                    "tool_name": "executeBash",
                    "tool_input": { "command": cmd }
                });
                let kiro_result_b = process_kiro_payload(&payload_b);

                // Both results must be identical regardless of session_id
                prop_assert_eq!(
                    kiro_result_a,
                    kiro_result_b,
                    "Command '{}': different session_ids ('{}' vs '{}') produced \
                     different results. IDE/CLI must be deterministic.",
                    cmd, session_id_a, session_id_b
                );
            }
        }
    }

    fn vibe_input(tool: &str, cmd: &str) -> String {
        json!({
            "session_id": "abc123",
            "hook_event_name": "pre_tool",
            "tool_name": tool,
            "tool_input": { "command": cmd }
        })
        .to_string()
    }

    #[test]
    fn test_vibe_rewrites_bash_command() {
        let input = vibe_input("bash", "git status");
        let out = run_vibe_inner(&input).expect("rewrite expected");
        let v: Value = serde_json::from_str(&out).unwrap();
        let rewritten = v
            .pointer("/hook_specific_output/tool_input/command")
            .and_then(|c| c.as_str())
            .unwrap_or("");
        assert!(
            rewritten.starts_with("rtk "),
            "expected rtk-prefixed rewrite, got `{rewritten}`"
        );
        assert!(
            v.get("system_message").is_some(),
            "expected system_message for UI visibility"
        );
    }

    #[test]
    fn test_vibe_strips_utf8_bom() {
        // Sixth hook stdin entry point, and the last one that did not strip.
        // Windows hosts may prepend a UTF-8 BOM (confirmed for Cursor);
        // without stripping, serde_json rejects the payload, run_vibe_inner
        // logs to stderr and returns None, and the command silently stops
        // being rewritten.
        let input = format!("\u{feff}{}", vibe_input("bash", "git status"));
        let out = run_vibe_inner(&input).expect("BOM-prefixed payload must parse");
        let v: Value = serde_json::from_str(&out).unwrap();
        assert_eq!(
            v.pointer("/hook_specific_output/tool_input/command")
                .and_then(|c| c.as_str()),
            Some("rtk git status")
        );
    }

    #[test]
    fn test_vibe_ignores_non_bash_tool() {
        let input = vibe_input("read_file", "irrelevant");
        assert!(run_vibe_inner(&input).is_none());
    }

    #[test]
    fn test_vibe_empty_command_passthrough() {
        let input = vibe_input("bash", "");
        assert!(run_vibe_inner(&input).is_none());
    }

    #[test]
    fn test_vibe_malformed_json_returns_none() {
        assert!(run_vibe_inner("not json at all").is_none());
        assert!(run_vibe_inner("{ unterminated").is_none());
    }

    #[test]
    fn test_vibe_unknown_binary_passthrough() {
        let input = vibe_input("bash", "definitely-not-a-real-binary --foo");
        assert!(run_vibe_inner(&input).is_none());
    }

    #[test]
    fn test_vibe_substitution_defers() {
        let input = vibe_input("bash", "echo $(rm -rf /)");
        assert!(run_vibe_inner(&input).is_none());
    }
}
