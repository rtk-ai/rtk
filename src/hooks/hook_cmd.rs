//! Processes incoming hook calls from AI agents and rewrites commands on the fly.
//!
//! Uses `writeln!(stdout, ...)` instead of `println!` — accidental stdout/stderr
//! corrupts the JSON protocol (Claude Code bug #4669 silently disables the hook).

use super::constants::PRE_TOOL_USE_KEY;
use super::permissions::{self, PermissionVerdict};
use anyhow::{Context, Result};
use serde_json::{Value, json};
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
        ) && let Some(cmd) = v
            .pointer("/tool_input/command")
            .and_then(|c| c.as_str())
            .filter(|c| !c.is_empty())
        {
            return HookFormat::VsCode {
                command: cmd.to_string(),
            };
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
        if matches!(tool_name, "bash" | "powershell" | "run_in_terminal")
            && let Some(tool_args_str) = v.get("toolArgs").and_then(|t| t.as_str())
            && let Ok(tool_args) = serde_json::from_str::<Value>(tool_args_str)
            && let Some(cmd) = tool_args
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
            };
        }
        HookDecision::Defer => {
            return PayloadAction::Skip {
                decision: HookOutcome::Defer,
                cmd: cmd.to_string(),
            };
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
        assert!(
            copilot_cli_response_from_decision(
                &cli_args("cargo test"),
                HookDecision::Deny,
                "cargo test",
            )
            .is_none()
        );
    }

    #[test]
    fn test_copilot_cli_defer_returns_none() {
        // Defer covers both "no rewrite available" and the unattestable-construct gate.
        // The hook must emit NO modifiedArgs for CVE bypass forms — no laundering.
        assert!(
            copilot_cli_response_from_decision(
                &cli_args("git status & rm -rf /tmp/x"),
                HookDecision::Defer,
                "git status & rm -rf /tmp/x",
            )
            .is_none()
        );
    }

    #[test]
    fn test_copilot_ide_rewrite_returns_deny_with_suggestion() {
        let response = copilot_ide_response_from_decision(
            HookDecision::AskRewrite("rtk git status".into()),
            "git status",
        )
        .unwrap();
        assert_eq!(response["permissionDecision"], "deny");
        assert!(
            response["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("rtk git status")
        );
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
        assert!(
            response["permissionDecisionReason"]
                .as_str()
                .unwrap()
                .contains("rtk git status")
        );
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
        temp_env::with_var_unset("RTK_HOOK_AUDIT", || {
            audit_log("test", "git status", "rtk git status");
        });
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
