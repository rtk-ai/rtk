//! Reads agent session logs and streams their command history.

use crate::hooks::constants::CLAUDE_DIR;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

const OPENCODE_DB_RELATIVE_PATH: &str = ".local/share/opencode/opencode.db";
const OPENCODE_SESSION_PREFIX: &str = "opencode-session:";

/// A command extracted from a session file.
#[derive(Debug)]
pub struct ExtractedCommand {
    pub command: String,
    pub output_len: Option<usize>,
    #[allow(dead_code)]
    pub session_id: String,
    /// Actual output content (first ~1000 chars for error detection)
    pub output_content: Option<String>,
    /// Whether the tool_result indicated an error
    pub is_error: bool,
    /// Chronological sequence index within the session
    #[allow(dead_code)]
    pub sequence_index: usize,
}

/// Trait for session providers (Claude Code, OpenCode, etc.).
///
/// Note: Cursor Agent transcripts use a text-only format without structured
/// tool_use/tool_result blocks, so command extraction is not possible.
/// Use `rtk gain` to track savings for Cursor sessions instead.
pub trait SessionProvider {
    fn discover_sessions(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>>;
    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>>;
}

pub struct ClaudeProvider;

impl ClaudeProvider {
    /// Get the base directory for Claude Code projects.
    fn projects_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("could not determine home directory")?;
        let dir = home.join(CLAUDE_DIR).join("projects");
        if !dir.exists() {
            anyhow::bail!(
                "Claude Code projects directory not found: {}\nMake sure Claude Code has been used at least once.",
                dir.display()
            );
        }
        Ok(dir)
    }

    /// Encode a filesystem path to Claude Code's directory name format.
    /// `/Users/foo/bar` → `-Users-foo-bar`
    pub fn encode_project_path(path: &str) -> String {
        path.replace('/', "-")
    }
}

#[derive(Default)]
pub struct OpenCodeProvider {
    db_path: Option<PathBuf>,
}

#[derive(Debug)]
struct OpenCodePartRow {
    key: String,
    time_updated: i64,
    command: String,
    output: Option<String>,
    error_output: Option<String>,
    status: Option<String>,
}

impl OpenCodeProvider {
    fn db_path(&self) -> Result<PathBuf> {
        if let Some(path) = &self.db_path {
            return Ok(path.clone());
        }

        let home = dirs::home_dir().context("could not determine home directory")?;
        let path = home.join(OPENCODE_DB_RELATIVE_PATH);

        if !path.exists() {
            anyhow::bail!(
                "OpenCode database not found: {}\nMake sure OpenCode has been used at least once.",
                path.display()
            );
        }

        Ok(path)
    }

    fn session_path_from_id(id: &str) -> PathBuf {
        PathBuf::from(format!("{}{}", OPENCODE_SESSION_PREFIX, id))
    }

    fn session_id_from_path(path: &Path) -> Option<String> {
        let raw = path.to_string_lossy();
        raw.strip_prefix(OPENCODE_SESSION_PREFIX)
            .map(str::to_string)
    }

    fn to_unix_seconds(ts: i64) -> i64 {
        if ts > 1_000_000_000_000 {
            ts / 1000
        } else {
            ts
        }
    }
}

impl SessionProvider for ClaudeProvider {
    fn discover_sessions(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        let projects_dir = Self::projects_dir()?;
        let cutoff = since_days.map(|days| {
            SystemTime::now()
                .checked_sub(Duration::from_secs(days * 86400))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });

        let mut sessions = Vec::new();

        // List project directories
        let entries = fs::read_dir(&projects_dir)
            .with_context(|| format!("failed to read {}", projects_dir.display()))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }

            // Apply project filter: substring match on directory name
            if let Some(filter) = project_filter {
                let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let encoded_filter = ClaudeProvider::encode_project_path(filter);
                if !dir_name.contains(filter) && !dir_name.contains(&encoded_filter) {
                    continue;
                }
            }

            // Walk the project directory recursively (catches subagents/)
            for walk_entry in WalkDir::new(&path)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let file_path = walk_entry.path();
                if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }

                // Apply mtime filter
                if let Some(cutoff_time) = cutoff {
                    if let Ok(meta) = fs::metadata(file_path) {
                        if let Ok(mtime) = meta.modified() {
                            if mtime < cutoff_time {
                                continue;
                            }
                        }
                    }
                }

                sessions.push(file_path.to_path_buf());
            }
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

        // First pass: collect all tool_use Bash commands with their IDs and sequence
        // Second pass (same loop): collect tool_result output lengths, content, and error status
        let mut pending_tool_uses: Vec<(String, String, usize)> = Vec::new(); // (tool_use_id, command, sequence)
        let mut tool_results: HashMap<String, (usize, String, bool)> = HashMap::new(); // (len, content, is_error)
        let mut commands = Vec::new();
        let mut sequence_counter = 0;

        for line in reader.lines() {
            let line = match line {
                Ok(l) => l,
                Err(_) => continue,
            };

            // Pre-filter: skip lines that can't contain Bash tool_use or tool_result
            if !line.contains("\"Bash\"") && !line.contains("\"tool_result\"") {
                continue;
            }

            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match entry_type {
                "assistant" => {
                    // Look for tool_use Bash blocks in message.content
                    if let Some(content) =
                        entry.pointer("/message/content").and_then(|c| c.as_array())
                    {
                        for block in content {
                            if block.get("type").and_then(|t| t.as_str()) == Some("tool_use")
                                && block.get("name").and_then(|n| n.as_str()) == Some("Bash")
                            {
                                if let (Some(id), Some(cmd)) = (
                                    block.get("id").and_then(|i| i.as_str()),
                                    block.pointer("/input/command").and_then(|c| c.as_str()),
                                ) {
                                    pending_tool_uses.push((
                                        id.to_string(),
                                        cmd.to_string(),
                                        sequence_counter,
                                    ));
                                    sequence_counter += 1;
                                }
                            }
                        }
                    }
                }
                "user" => {
                    // Look for tool_result blocks
                    if let Some(content) =
                        entry.pointer("/message/content").and_then(|c| c.as_array())
                    {
                        for block in content {
                            if block.get("type").and_then(|t| t.as_str()) == Some("tool_result") {
                                if let Some(id) = block.get("tool_use_id").and_then(|i| i.as_str())
                                {
                                    // Get content, length, and error status
                                    let content =
                                        block.get("content").and_then(|c| c.as_str()).unwrap_or("");

                                    let output_len = content.len();
                                    let is_error = block
                                        .get("is_error")
                                        .and_then(|e| e.as_bool())
                                        .unwrap_or(false);

                                    // Store first ~1000 chars of content for error detection
                                    let content_preview: String =
                                        content.chars().take(1000).collect();

                                    tool_results.insert(
                                        id.to_string(),
                                        (output_len, content_preview, is_error),
                                    );
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Match tool_uses with their results
        for (tool_id, command, sequence_index) in pending_tool_uses {
            let (output_len, output_content, is_error) = tool_results
                .get(&tool_id)
                .map(|(len, content, err)| (Some(*len), Some(content.clone()), *err))
                .unwrap_or((None, None, false));

            commands.push(ExtractedCommand {
                command,
                output_len,
                session_id: session_id.clone(),
                output_content,
                is_error,
                sequence_index,
            });
        }

        Ok(commands)
    }
}

impl SessionProvider for OpenCodeProvider {
    fn discover_sessions(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        let db_path = self.db_path()?;
        let conn = rusqlite::Connection::open(&db_path)
            .with_context(|| format!("failed to open {}", db_path.display()))?;

        let cutoff_unix = since_days.map(|days| {
            let cutoff = SystemTime::now()
                .checked_sub(Duration::from_secs(days * 86400))
                .unwrap_or(SystemTime::UNIX_EPOCH);
            cutoff
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs() as i64
        });

        let mut stmt = conn.prepare("SELECT id, directory, time_updated FROM session")?;
        let rows = stmt.query_map([], |row| {
            let id: String = row.get(0)?;
            let directory: String = row.get(1)?;
            let time_updated: i64 = row.get(2)?;
            Ok((id, directory, time_updated))
        })?;

        let mut sessions = Vec::new();
        for row in rows {
            let (id, directory, time_updated) = row?;

            if let Some(filter) = project_filter {
                if !directory.contains(filter) {
                    continue;
                }
            }

            if let Some(cutoff) = cutoff_unix {
                if OpenCodeProvider::to_unix_seconds(time_updated) < cutoff {
                    continue;
                }
            }

            sessions.push(OpenCodeProvider::session_path_from_id(&id));
        }

        Ok(sessions)
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        let session_id = OpenCodeProvider::session_id_from_path(path)
            .with_context(|| format!("invalid OpenCode session path: {}", path.display()))?;

        let db_path = self.db_path()?;
        let conn = rusqlite::Connection::open(&db_path)
            .with_context(|| format!("failed to open {}", db_path.display()))?;

        let mut stmt = conn.prepare(
            "SELECT id,
                    time_updated,
                    json_extract(data, '$.callID') AS call_id,
                    json_extract(data, '$.state.input.command') AS command,
                    json_extract(data, '$.state.output') AS output,
                    json_extract(data, '$.state.error') AS error_output,
                    json_extract(data, '$.state.status') AS status
             FROM part
             WHERE session_id = ?1
               AND json_extract(data, '$.type') = 'tool'
               AND lower(json_extract(data, '$.tool')) IN ('bash', 'shell')
             ORDER BY time_updated ASC",
        )?;

        let rows = stmt.query_map([session_id.as_str()], |row| {
            let id: String = row.get(0)?;
            let time_updated: i64 = row.get(1)?;
            let call_id: Option<String> = row.get(2)?;
            let command: Option<String> = row.get(3)?;
            let output: Option<String> = row.get(4)?;
            let error_output: Option<String> = row.get(5)?;
            let status: Option<String> = row.get(6)?;
            Ok(OpenCodePartRow {
                key: call_id.unwrap_or(id),
                time_updated,
                command: command.unwrap_or_default(),
                output,
                error_output,
                status,
            })
        })?;

        // Keep the latest state per callID (running -> completed/error)
        let mut by_call: HashMap<String, OpenCodePartRow> = HashMap::new();
        for row in rows {
            let parsed = row?;
            by_call.insert(parsed.key.clone(), parsed);
        }

        let mut latest_rows: Vec<OpenCodePartRow> = by_call.into_values().collect();
        latest_rows.sort_by_key(|r| r.time_updated);

        let mut commands = Vec::new();
        for (sequence_index, row) in latest_rows.into_iter().enumerate() {
            if row.command.trim().is_empty() {
                continue;
            }

            let output = row.output.or(row.error_output);
            let output_len = output.as_ref().map(|content| content.len());
            let output_content = output.map(|content| content.chars().take(1000).collect());
            let is_error = row.status.as_deref() == Some("error");

            commands.push(ExtractedCommand {
                command: row.command,
                output_len,
                session_id: session_id.clone(),
                output_content,
                is_error,
                sequence_index,
            });
        }

        Ok(commands)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{params, Connection};
    use std::io::Write;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn make_jsonl(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        f.flush().unwrap();
        f
    }

    #[test]
    fn test_extract_assistant_bash() {
        let jsonl = make_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_abc","name":"Bash","input":{"command":"git status"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_abc","content":"On branch master\nnothing to commit"}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "git status");
        assert!(cmds[0].output_len.is_some());
        assert_eq!(
            cmds[0].output_len.unwrap(),
            "On branch master\nnothing to commit".len()
        );
    }

    #[test]
    fn test_extract_non_bash_ignored() {
        let jsonl = make_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_abc","name":"Read","input":{"file_path":"/tmp/foo"}}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_extract_non_message_ignored() {
        let jsonl =
            make_jsonl(&[r#"{"type":"file-history-snapshot","messageId":"abc","snapshot":{}}"#]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_extract_multiple_tools() {
        let jsonl = make_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"git status"}},{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"git diff"}}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "git status");
        assert_eq!(cmds[1].command, "git diff");
    }

    #[test]
    fn test_extract_malformed_line() {
        let jsonl = make_jsonl(&[
            "this is not json at all",
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_ok","name":"Bash","input":{"command":"ls"}}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "ls");
    }

    #[test]
    fn test_encode_project_path() {
        assert_eq!(
            ClaudeProvider::encode_project_path("/Users/foo/bar"),
            "-Users-foo-bar"
        );
    }

    #[test]
    fn test_encode_project_path_trailing_slash() {
        assert_eq!(
            ClaudeProvider::encode_project_path("/Users/foo/bar/"),
            "-Users-foo-bar-"
        );
    }

    #[test]
    fn test_match_project_filter() {
        let encoded = ClaudeProvider::encode_project_path("/Users/foo/Sites/rtk");
        assert!(encoded.contains("rtk"));
        assert!(encoded.contains("Sites"));
    }

    #[test]
    fn test_extract_output_content() {
        let jsonl = make_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_abc","name":"Bash","input":{"command":"git commit --ammend"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_abc","content":"error: unexpected argument '--ammend'","is_error":true}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "git commit --ammend");
        assert!(cmds[0].is_error);
        assert!(cmds[0].output_content.is_some());
        assert_eq!(
            cmds[0].output_content.as_ref().unwrap(),
            "error: unexpected argument '--ammend'"
        );
    }

    #[test]
    fn test_extract_is_error_flag() {
        let jsonl = make_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"ls"}},{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"invalid_cmd"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_1","content":"file1.txt","is_error":false},{"type":"tool_result","tool_use_id":"toolu_2","content":"command not found","is_error":true}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 2);
        assert!(!cmds[0].is_error);
        assert!(cmds[1].is_error);
    }

    #[test]
    fn test_extract_sequence_ordering() {
        let jsonl = make_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"first"}},{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"second"}},{"type":"tool_use","id":"toolu_3","name":"Bash","input":{"command":"third"}}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0].sequence_index, 0);
        assert_eq!(cmds[1].sequence_index, 1);
        assert_eq!(cmds[2].sequence_index, 2);
        assert_eq!(cmds[0].command, "first");
        assert_eq!(cmds[1].command, "second");
        assert_eq!(cmds[2].command, "third");
    }

    fn setup_opencode_db() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let db_path = temp.path().join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();

        conn.execute(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                directory TEXT NOT NULL,
                time_updated INTEGER NOT NULL
            )",
            [],
        )
        .unwrap();

        conn.execute(
            "CREATE TABLE part (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            )",
            [],
        )
        .unwrap();

        (temp, db_path)
    }

    fn now_ms() -> i64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    #[test]
    fn test_opencode_discover_sessions_filters_project_and_since() {
        let (_temp, db_path) = setup_opencode_db();
        let conn = Connection::open(&db_path).unwrap();
        let now = now_ms();
        let old = now - (40 * 86_400 * 1000);

        conn.execute(
            "INSERT INTO session (id, directory, time_updated) VALUES (?1, ?2, ?3)",
            params!["ses_new_match", "/home/user/code/rtk", now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, directory, time_updated) VALUES (?1, ?2, ?3)",
            params!["ses_new_other", "/home/user/code/other", now],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO session (id, directory, time_updated) VALUES (?1, ?2, ?3)",
            params!["ses_old_match", "/home/user/code/rtk", old],
        )
        .unwrap();

        let provider = OpenCodeProvider {
            db_path: Some(db_path),
        };
        let sessions = provider.discover_sessions(Some("rtk"), Some(30)).unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].to_string_lossy(),
            "opencode-session:ses_new_match"
        );
    }

    #[test]
    fn test_opencode_extract_commands_reads_bash_tools() {
        let (_temp, db_path) = setup_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        conn.execute(
            "INSERT INTO session (id, directory, time_updated) VALUES (?1, ?2, ?3)",
            params!["ses_1", "/home/user/code/rtk", now_ms()],
        )
        .unwrap();

        let bash_data = r#"{"type":"tool","tool":"bash","callID":"call_1","state":{"status":"completed","input":{"command":"git status"},"output":"clean"}}"#;
        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["prt_1", "ses_1", now_ms(), bash_data],
        )
        .unwrap();

        let read_data = r#"{"type":"tool","tool":"read","callID":"call_2","state":{"status":"completed","input":{"filePath":"/tmp/x"},"output":"abc"}}"#;
        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["prt_2", "ses_1", now_ms() + 1, read_data],
        )
        .unwrap();

        let provider = OpenCodeProvider {
            db_path: Some(db_path),
        };
        let cmds = provider
            .extract_commands(Path::new("opencode-session:ses_1"))
            .unwrap();

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "git status");
        assert_eq!(cmds[0].output_len, Some("clean".len()));
        assert!(!cmds[0].is_error);
    }

    #[test]
    fn test_opencode_extract_commands_uses_latest_call_state() {
        let (_temp, db_path) = setup_opencode_db();
        let conn = Connection::open(&db_path).unwrap();
        let t = now_ms();

        conn.execute(
            "INSERT INTO session (id, directory, time_updated) VALUES (?1, ?2, ?3)",
            params!["ses_2", "/home/user/code/rtk", t],
        )
        .unwrap();

        let running = r#"{"type":"tool","tool":"bash","callID":"call_same","state":{"status":"running","input":{"command":"cargo test"}}}"#;
        let completed = r#"{"type":"tool","tool":"bash","callID":"call_same","state":{"status":"completed","input":{"command":"cargo test"},"output":"ok"}}"#;
        let errored = r#"{"type":"tool","tool":"bash","callID":"call_err","state":{"status":"error","input":{"command":"git bad"},"error":"failed"}}"#;

        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["prt_r", "ses_2", t, running],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["prt_c", "ses_2", t + 1, completed],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["prt_e", "ses_2", t + 2, errored],
        )
        .unwrap();

        let provider = OpenCodeProvider {
            db_path: Some(db_path),
        };
        let cmds = provider
            .extract_commands(Path::new("opencode-session:ses_2"))
            .unwrap();

        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "cargo test");
        assert_eq!(cmds[0].output_len, Some(2));
        assert!(!cmds[0].is_error);

        assert_eq!(cmds[1].command, "git bad");
        assert_eq!(cmds[1].output_len, Some("failed".len()));
        assert_eq!(cmds[1].output_content.as_deref(), Some("failed"));
        assert!(cmds[1].is_error);
    }
}
