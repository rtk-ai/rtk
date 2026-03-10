use anyhow::{Context, Result};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

use super::provider::{
    cutoff_from_days, is_recent, ExtractedCommand, SessionProvider, ToolSource,
    OUTPUT_PREVIEW_CHARS,
};

pub struct CodexProvider;

impl CodexProvider {
    fn base_dir() -> Option<PathBuf> {
        let user_home = dirs::home_dir();

        // $CODEX_HOME overrides default location, but must exist and sit under $HOME
        if let Ok(val) = std::env::var("CODEX_HOME") {
            let p = PathBuf::from(val);
            if p.exists() {
                if let Some(ref home) = user_home {
                    if p.starts_with(home) {
                        return Some(p);
                    }
                }
            }
            return None;
        }

        let home = user_home?;
        let dir = home.join(".codex");
        if dir.exists() {
            Some(dir)
        } else {
            None
        }
    }
}

impl SessionProvider for CodexProvider {
    fn tool_source(&self) -> ToolSource {
        ToolSource::CodexCli
    }

    fn discover_sessions(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        let base = match Self::base_dir() {
            Some(d) => d,
            None => return Ok(vec![]),
        };

        let sessions_dir = base.join("sessions");
        if !sessions_dir.exists() {
            return Ok(vec![]);
        }

        let cutoff = cutoff_from_days(since_days);
        let mut sessions = Vec::new();

        for entry in WalkDir::new(&sessions_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                continue;
            }

            if !is_recent(path, cutoff) {
                continue;
            }

            // Project filter: peek at first few lines for SessionMeta with cwd
            if let Some(filter) = project_filter {
                if !session_matches_project(path, filter) {
                    continue;
                }
            }

            sessions.push(path.to_path_buf());
        }

        Ok(sessions)
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        let file =
            fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let reader = BufReader::new(file);

        let session_id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mut commands = Vec::new();
        let mut sequence_counter = 0;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };

            // Pre-filter: look for exec-related events
            if !line.contains("exec") && !line.contains("Exec") {
                continue;
            }

            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Codex events: { type: "ExecCommandEnd", payload: { command, output, exitCode } }
            // Also handle: { event: "exec_command_end", ... } variant
            let event_type = entry
                .get("type")
                .or_else(|| entry.get("event"))
                .and_then(|t| t.as_str())
                .unwrap_or("");

            let is_exec_end = event_type == "ExecCommandEnd"
                || event_type == "exec_command_end"
                || event_type == "ExecCommand";

            if !is_exec_end {
                continue;
            }

            let payload = entry.get("payload").unwrap_or(&entry);

            let command = payload
                .get("command")
                .or_else(|| payload.get("cmd"))
                .and_then(|c| c.as_str());

            let Some(command) = command else {
                continue;
            };

            let output = payload
                .get("output")
                .or_else(|| payload.get("stdout"))
                .and_then(|o| o.as_str())
                .unwrap_or("");

            let exit_code = payload
                .get("exitCode")
                .or_else(|| payload.get("exit_code"))
                .and_then(|e| e.as_i64())
                .unwrap_or(0);

            let output_preview: String = output.chars().take(OUTPUT_PREVIEW_CHARS).collect();

            commands.push(ExtractedCommand {
                command: command.to_string(),
                output_len: Some(output.len()),
                session_id: session_id.clone(),
                output_content: if output_preview.is_empty() {
                    None
                } else {
                    Some(output_preview)
                },
                is_error: exit_code != 0,
                sequence_index: sequence_counter,
            });
            sequence_counter += 1;
        }

        Ok(commands)
    }
}

/// Check if a Codex session file's SessionMeta event mentions the project path.
fn session_matches_project(path: &Path, filter: &str) -> bool {
    let file = match fs::File::open(path) {
        Ok(f) => f,
        Err(_) => return false,
    };
    let reader = BufReader::new(file);

    // Only check first 20 lines for SessionMeta
    for line in reader.lines().take(20) {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };

        if !line.contains("cwd") && !line.contains("SessionMeta") {
            continue;
        }

        let entry: serde_json::Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Look for cwd field in payload or top-level
        let cwd = entry
            .pointer("/payload/cwd")
            .or_else(|| entry.get("cwd"))
            .and_then(|c| c.as_str());

        if let Some(cwd) = cwd {
            return cwd.contains(filter);
        }
    }

    // No project info found: include by default
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn make_jsonl(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn test_extract_exec_command_end() {
        let jsonl = make_jsonl(&[
            r#"{"type":"ExecCommandEnd","payload":{"command":"git status","output":"On branch main\nnothing to commit","exitCode":0}}"#,
        ]);

        let provider = CodexProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "git status");
        assert!(!cmds[0].is_error);
        assert_eq!(
            cmds[0].output_len.unwrap(),
            "On branch main\nnothing to commit".len()
        );
    }

    #[test]
    fn test_extract_exec_command_error() {
        let jsonl = make_jsonl(&[
            r#"{"type":"ExecCommandEnd","payload":{"command":"npm test","output":"FAIL tests/app.test.js","exitCode":1}}"#,
        ]);

        let provider = CodexProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].is_error);
        assert!(cmds[0].output_content.is_some());
    }

    #[test]
    fn test_extract_snake_case_variant() {
        let jsonl = make_jsonl(&[
            r#"{"event":"exec_command_end","payload":{"cmd":"ls -la","stdout":"total 42","exit_code":0}}"#,
        ]);

        let provider = CodexProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "ls -la");
    }

    #[test]
    fn test_non_exec_events_ignored() {
        let jsonl = make_jsonl(&[
            r#"{"type":"SessionMeta","payload":{"cwd":"/home/user/project"}}"#,
            r#"{"type":"ChatMessage","payload":{"text":"hello"}}"#,
        ]);

        let provider = CodexProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_malformed_lines_skipped() {
        let jsonl = make_jsonl(&[
            "not valid json with exec",
            r#"{"type":"ExecCommandEnd","payload":{"command":"echo hello","output":"hello","exitCode":0}}"#,
        ]);

        let provider = CodexProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "echo hello");
    }

    #[test]
    fn test_session_matches_project_filter() {
        let jsonl = make_jsonl(&[
            r#"{"type":"SessionMeta","payload":{"cwd":"/Users/dev/myproject"}}"#,
            r#"{"type":"ExecCommandEnd","payload":{"command":"git status","output":"ok","exitCode":0}}"#,
        ]);

        assert!(session_matches_project(jsonl.path(), "myproject"));
        assert!(!session_matches_project(jsonl.path(), "other"));
    }

    #[test]
    fn test_session_no_meta_includes_by_default() {
        let jsonl = make_jsonl(&[
            r#"{"type":"ExecCommandEnd","payload":{"command":"ls","output":"ok","exitCode":0}}"#,
        ]);

        assert!(session_matches_project(jsonl.path(), "anything"));
    }

    #[test]
    fn test_tool_source() {
        let provider = CodexProvider;
        assert_eq!(provider.tool_source(), ToolSource::CodexCli);
    }

    #[test]
    fn test_sequence_ordering() {
        let jsonl = make_jsonl(&[
            r#"{"type":"ExecCommandEnd","payload":{"command":"first","output":"a","exitCode":0}}"#,
            r#"{"type":"ExecCommandEnd","payload":{"command":"second","output":"b","exitCode":0}}"#,
        ]);

        let provider = CodexProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].sequence_index, 0);
        assert_eq!(cmds[1].sequence_index, 1);
    }
}
