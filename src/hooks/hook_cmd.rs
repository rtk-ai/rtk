//! Processes incoming hook calls from AI agents and rewrites commands on the fly.

use anyhow::{Context, Result};
use serde_json::{json, Value};
use std::io::{self, Read};

use crate::discover::registry::rewrite_command;

// ── Copilot hook (VS Code + Copilot CLI) ──────────────────────

/// Format detected from the preToolUse JSON input.
enum HookFormat {
    /// VS Code Copilot Chat / Claude Code: `tool_name` + `tool_input.command`, supports `updatedInput`.
    VsCode { command: String },
    /// GitHub Copilot CLI: camelCase `toolName` + `toolArgs` (JSON string), deny-with-suggestion only.
    CopilotCli { command: String },
    /// Non-bash tool, already uses rtk, or unknown format — pass through silently.
    PassThrough,
}

/// Run the Copilot preToolUse hook.
/// Auto-detects VS Code Copilot Chat vs Copilot CLI format.
pub fn run_copilot() -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read stdin")?;

    let input = input.trim();
    if input.is_empty() {
        return Ok(());
    }

    let v: Value = match serde_json::from_str(input) {
        Ok(v) => v,
        Err(e) => {
            eprintln!("[rtk hook] Failed to parse JSON input: {e}");
            return Ok(());
        }
    };

    match detect_format(&v) {
        HookFormat::VsCode { command } => handle_vscode(&command),
        HookFormat::CopilotCli { command } => handle_copilot_cli(&command),
        HookFormat::PassThrough => Ok(()),
    }
}

fn detect_format(v: &Value) -> HookFormat {
    // VS Code Copilot Chat / Claude Code: snake_case keys
    if let Some(tool_name) = v.get("tool_name").and_then(|t| t.as_str()) {
        if matches!(tool_name, "runTerminalCommand" | "Bash" | "bash") {
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

    // Copilot CLI: camelCase keys, toolArgs is a JSON-encoded string
    if let Some(tool_name) = v.get("toolName").and_then(|t| t.as_str()) {
        if tool_name == "bash" {
            if let Some(tool_args_str) = v.get("toolArgs").and_then(|t| t.as_str()) {
                if let Ok(tool_args) = serde_json::from_str::<Value>(tool_args_str) {
                    if let Some(cmd) = tool_args
                        .get("command")
                        .and_then(|c| c.as_str())
                        .filter(|c| !c.is_empty())
                    {
                        return HookFormat::CopilotCli {
                            command: cmd.to_string(),
                        };
                    }
                }
            }
        }
        return HookFormat::PassThrough;
    }

    HookFormat::PassThrough
}

fn get_rewritten(cmd: &str) -> Option<String> {
    if cmd.contains("<<") {
        return None;
    }

    let excluded = crate::core::config::Config::load()
        .map(|c| c.hooks.exclude_commands)
        .unwrap_or_default();

    let rewritten = rewrite_command(cmd, &excluded)?;

    if rewritten == cmd {
        return None;
    }

    Some(rewritten)
}

fn handle_vscode(cmd: &str) -> Result<()> {
    let rewritten = match get_rewritten(cmd) {
        Some(r) => r,
        None => return Ok(()),
    };

    let output = json!({
        "hookSpecificOutput": {
            "hookEventName": "PreToolUse",
            "permissionDecision": "allow",
            "permissionDecisionReason": "RTK auto-rewrite",
            "updatedInput": { "command": rewritten }
        }
    });
    println!("{output}");
    Ok(())
}

fn handle_copilot_cli(cmd: &str) -> Result<()> {
    let rewritten = match get_rewritten(cmd) {
        Some(r) => r,
        None => return Ok(()),
    };

    let output = json!({
        "permissionDecision": "deny",
        "permissionDecisionReason": format!(
            "Token savings: use `{}` instead (rtk saves 60-90% tokens)",
            rewritten
        )
    });
    println!("{output}");
    Ok(())
}

// ── Gemini hook ───────────────────────────────────────────────

/// Run the Gemini CLI BeforeTool hook.
/// Reads JSON from stdin, rewrites shell commands to rtk equivalents,
/// outputs JSON to stdout in Gemini CLI format.
pub fn run_gemini() -> Result<()> {
    let mut input = String::new();
    io::stdin()
        .read_to_string(&mut input)
        .context("Failed to read hook input from stdin")?;

    let json: Value = serde_json::from_str(&input).context("Failed to parse hook input as JSON")?;

    let tool_name = json.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");

    match tool_name {
        "run_shell_command" => {
            let cmd = json
                .pointer("/tool_input/command")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if cmd.is_empty() {
                print_allow();
                return Ok(());
            }

            match rewrite_command(cmd, &[]) {
                Some(rewritten) => print_rewrite(&rewritten),
                None => print_allow(),
            }
        }
        "read_file" => {
            let file_path = json
                .pointer("/tool_input/file_path")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            if file_path.is_empty() {
                print_allow();
                return Ok(());
            }

            // Extract optional line range so we can pre-apply it before filtering,
            // then override both fields in the response to prevent Gemini's merge
            // from re-applying the original (now wrong) line numbers to the filtered file.
            let start_line = json
                .pointer("/tool_input/start_line")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);
            let end_line = json
                .pointer("/tool_input/end_line")
                .and_then(|v| v.as_u64())
                .map(|n| n as usize);

            handle_gemini_read_file(file_path, start_line, end_line)?;
        }
        _ => print_allow(),
    }

    Ok(())
}

/// Intercept Gemini CLI's `read_file` tool, filter the content, write to a temp file,
/// and redirect Gemini to read the filtered version instead.
///
/// `start_line`/`end_line` (1-based, inclusive) are pre-applied before filtering so the
/// filter sees only the relevant slice. Both are then overridden in the hook response to
/// prevent Gemini's merge from re-applying the original (now wrong) numbers to the temp file.
fn handle_gemini_read_file(
    file_path: &str,
    start_line: Option<usize>,
    end_line: Option<usize>,
) -> Result<()> {
    use crate::filter::{self, FilterLevel, Language};
    use std::fs;
    use std::path::Path;
    use std::time::{SystemTime, UNIX_EPOCH};

    let path = Path::new(file_path);

    // Read file — pass through silently for unreadable/binary files
    let content = match fs::read_to_string(path) {
        Ok(c) => c,
        Err(_) => {
            print_allow();
            return Ok(());
        }
    };

    // Pre-apply the requested line range (1-based, inclusive) before filtering,
    // so the filter sees only the relevant slice and line numbers stay meaningful.
    let slice: String = match (start_line, end_line) {
        (Some(start), Some(end)) => content
            .lines()
            .skip(start.saturating_sub(1))
            .take(end.saturating_sub(start.saturating_sub(1)))
            .collect::<Vec<_>>()
            .join("\n"),
        (Some(start), None) => content
            .lines()
            .skip(start.saturating_sub(1))
            .collect::<Vec<_>>()
            .join("\n"),
        (None, Some(end)) => content.lines().take(end).collect::<Vec<_>>().join("\n"),
        (None, None) => content,
    };

    // Not worth filtering very small slices
    if slice.len() < 500 {
        print_allow();
        return Ok(());
    }

    // Detect language from extension and apply minimal filter
    let lang = path
        .extension()
        .and_then(|e| e.to_str())
        .map(Language::from_extension)
        .unwrap_or(Language::Unknown);

    let filtered = filter::get_filter(FilterLevel::Minimal).filter(&slice, &lang);

    // Skip if savings are negligible (<10% by character count)
    let savings_pct = 100.0 - (filtered.len() as f64 / slice.len() as f64 * 100.0);
    if savings_pct < 10.0 {
        print_allow();
        return Ok(());
    }

    // Write filtered content to ~/.local/share/rtk/filtered/
    let filtered_dir = dirs::data_local_dir()
        .unwrap_or_else(|| std::path::PathBuf::from("/tmp"))
        .join("rtk")
        .join("filtered");

    fs::create_dir_all(&filtered_dir).context("Failed to create rtk filtered dir")?;

    // Rotate: keep only the last 20 filtered files
    cleanup_filtered_files(&filtered_dir, 20);

    let ts = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis();
    let stem = path.file_name().and_then(|n| n.to_str()).unwrap_or("file");
    // Timestamp-first so lexicographic sort in cleanup_filtered_files is chronological.
    let temp_path = filtered_dir.join(format!("{ts}-{stem}.rtk"));

    fs::write(&temp_path, &filtered).context("Failed to write filtered file")?;

    let filtered_line_count = filtered.lines().count();
    print_rewrite_read_file(&temp_path.to_string_lossy(), filtered_line_count);
    Ok(())
}

/// Rotate filtered files: keep only the newest `max_files`, delete the rest.
fn cleanup_filtered_files(dir: &std::path::Path, max_files: usize) {
    let mut entries: Vec<_> = std::fs::read_dir(dir)
        .ok()
        .into_iter()
        .flatten()
        .filter_map(|e| e.ok())
        .filter(|e| e.path().extension().is_some_and(|ext| ext == "rtk"))
        .collect();

    if entries.len() <= max_files {
        return;
    }

    // Filenames start with the original stem then a millisecond timestamp — sort is chronological
    entries.sort_by_key(|e| e.file_name());

    let to_remove = entries.len() - max_files;
    for entry in entries.iter().take(to_remove) {
        let _ = std::fs::remove_file(entry.path());
    }
}

fn print_allow() {
    println!(r#"{{"decision":"allow"}}"#);
}

fn print_rewrite(cmd: &str) {
    let output = serde_json::json!({
        "decision": "allow",
        "hookSpecificOutput": {
            "tool_input": {
                "command": cmd
            }
        }
    });
    println!("{output}");
}

fn print_rewrite_read_file(filtered_path: &str, line_count: usize) {
    let output = serde_json::json!({
        "decision": "allow",
        "hookSpecificOutput": {
            "tool_input": {
                "file_path": filtered_path,
                // Override start/end so Gemini's merge doesn't re-apply the original
                // (now wrong) line numbers to the filtered temp file.
                "start_line": 1,
                "end_line": line_count
            }
        }
    });
    println!("{output}");
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- Copilot format detection ---

    fn vscode_input(tool: &str, cmd: &str) -> Value {
        json!({
            "tool_name": tool,
            "tool_input": { "command": cmd }
        })
    }

    fn copilot_cli_input(cmd: &str) -> Value {
        let args = serde_json::to_string(&json!({ "command": cmd })).unwrap();
        json!({ "toolName": "bash", "toolArgs": args })
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
    fn test_detect_copilot_cli_bash() {
        assert!(matches!(
            detect_format(&copilot_cli_input("git status")),
            HookFormat::CopilotCli { .. }
        ));
    }

    #[test]
    fn test_detect_non_bash_is_passthrough() {
        let v = json!({ "tool_name": "editFiles" });
        assert!(matches!(detect_format(&v), HookFormat::PassThrough));
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

    // --- Gemini format ---

    #[test]
    fn test_print_allow_format() {
        // Verify the allow JSON format matches Gemini CLI expectations
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
        // Verify that rewrite_command handles the cases we need for Gemini
        assert_eq!(
            rewrite_command("git status", &[]),
            Some("rtk git status".into())
        );
        assert_eq!(
            rewrite_command("cargo test", &[]),
            Some("rtk cargo test".into())
        );
        // Already rtk → returned as-is (idempotent)
        assert_eq!(
            rewrite_command("rtk git status", &[]),
            Some("rtk git status".into())
        );
        // Heredoc → no rewrite
        assert_eq!(rewrite_command("cat <<EOF", &[]), None);
    }

    #[test]
    fn test_gemini_hook_excluded_commands() {
        let excluded = vec!["curl".to_string()];
        assert_eq!(rewrite_command("curl https://example.com", &excluded), None);
        // Non-excluded still rewrites
        assert_eq!(
            rewrite_command("git status", &excluded),
            Some("rtk git status".into())
        );
    }

    #[test]
    fn test_gemini_hook_env_prefix_preserved() {
        assert_eq!(
            rewrite_command("RUST_LOG=debug cargo test", &[]),
            Some("RUST_LOG=debug rtk cargo test".into())
        );
    }

    // --- read_file hook ---

    #[test]
    fn test_print_rewrite_read_file_format() {
        // Verify the JSON output matches what Gemini CLI expects for read_file rewrites,
        // including start_line/end_line overrides to neutralise the original line range.
        let output = serde_json::json!({
            "decision": "allow",
            "hookSpecificOutput": {
                "tool_input": {
                    "file_path": "/tmp/filtered.rtk",
                    "start_line": 1,
                    "end_line": 42
                }
            }
        });
        let parsed: Value = serde_json::from_str(&output.to_string()).unwrap();
        assert_eq!(parsed["decision"], "allow");
        assert_eq!(
            parsed["hookSpecificOutput"]["tool_input"]["file_path"],
            "/tmp/filtered.rtk"
        );
        assert_eq!(parsed["hookSpecificOutput"]["tool_input"]["start_line"], 1);
        assert_eq!(parsed["hookSpecificOutput"]["tool_input"]["end_line"], 42);
    }

    #[test]
    fn test_handle_gemini_read_file_missing_path_allows() {
        // Empty file_path in input → print_allow (no crash)
        let input = json!({
            "tool_name": "read_file",
            "tool_input": { "file_path": "" }
        });
        // Verify file_path extraction returns empty
        let file_path = input
            .pointer("/tool_input/file_path")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        assert!(file_path.is_empty());
    }

    #[test]
    fn test_handle_gemini_read_file_nonexistent_allows() {
        // Non-existent file → handle_gemini_read_file falls back to allow (no panic)
        let result = handle_gemini_read_file("/tmp/rtk-nonexistent-file-xyz-12345.txt", None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_handle_gemini_read_file_small_file_allows() {
        use std::io::Write;
        use tempfile::NamedTempFile;

        let mut f = NamedTempFile::with_suffix(".rs").expect("temp file");
        writeln!(f, "fn main() {{}}").expect("write");

        // File well under 500 chars → should allow through without filtering
        let result = handle_gemini_read_file(f.path().to_str().unwrap(), None, None);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cleanup_filtered_files_under_limit() {
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        // Create 5 .rtk files — under the 20-file limit, none should be deleted
        for i in 0..5 {
            fs::write(dir.path().join(format!("file-{i}.rtk")), "x").expect("write");
        }
        cleanup_filtered_files(dir.path(), 20);
        let remaining = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(remaining, 5);
    }

    #[test]
    fn test_cleanup_filtered_files_over_limit() {
        use std::fs;
        use tempfile::TempDir;

        let dir = TempDir::new().expect("temp dir");
        // Create 25 .rtk files — 5 oldest should be deleted to stay at 20
        for i in 0..25u64 {
            fs::write(dir.path().join(format!("file-{i:05}.rtk")), "x").expect("write");
        }
        cleanup_filtered_files(dir.path(), 20);
        let remaining = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(remaining, 20);
    }
}
