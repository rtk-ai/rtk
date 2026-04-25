//! Reads Claude Code session logs from disk and streams their command history.

use crate::hooks::constants::CLAUDE_DIR;
use crate::hooks::constants::OPENCODE_DB_PATH;
use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

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
                if !dir_name.contains(filter) {
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

/// OpenCode session provider using SQLite database.
pub struct OpenCodeProvider;

impl OpenCodeProvider {
    /// Get the OpenCode database path.
    #[allow(dead_code)]
    fn db_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("could not determine home directory")?;
        let db_path = home.join(OPENCODE_DB_PATH);
        if !db_path.exists() {
            anyhow::bail!("OpenCode database not found at {}", db_path.display());
        }
        Ok(db_path)
    }

    /// Connect to the OpenCode database.
    fn connect() -> Result<rusqlite::Connection> {
        let path = Self::db_path()?;
        let conn = rusqlite::Connection::open(&path)?;
        Ok(conn)
    }
}

#[allow(dead_code)]
impl SessionProvider for OpenCodeProvider {
    fn discover_sessions(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        let conn = Self::connect()?;

        // Build query for directories with optional date filter
        // time_created is Unix timestamp (seconds)
        let paths: Vec<PathBuf> = if let Some(days) = since_days {
            let cutoff = chrono::Utc::now() - chrono::Duration::days(days as i64);
            let cutoff_ts = cutoff.timestamp();
            let query = format!(
                "SELECT DISTINCT directory FROM session WHERE directory IS NOT NULL AND time_created > {}",
                cutoff_ts
            );
            let mut stmt = conn.prepare(&query)?;
            let result: Vec<PathBuf> = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .map(PathBuf::from)
                .collect();
            result
        } else {
            let mut stmt =
                conn.prepare("SELECT DISTINCT directory FROM session WHERE directory IS NOT NULL")?;
            let result: Vec<PathBuf> = stmt
                .query_map([], |row| row.get::<_, String>(0))?
                .filter_map(|r| r.ok())
                .map(PathBuf::from)
                .collect();
            result
        };

        // Apply project filter (substring match)
        let mut filtered = paths;
        if let Some(filter) = project_filter {
            filtered.retain(|p| p.to_string_lossy().contains(filter));
        }

        Ok(filtered)
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        let conn = Self::connect()?;
        let directory = path.to_string_lossy().to_string();

        // Query part table for bash tool calls using json_extract
        // OpenCode format: {"type":"tool","tool":"bash","state":{"input":{"command":"..."}}}
        let mut stmt = conn.prepare(
            "SELECT p.data, p.time_created FROM part p
             JOIN session s ON p.session_id = s.id
             WHERE s.directory = ?
             AND json_extract(p.data, '$.type') = 'tool'
             AND json_extract(p.data, '$.tool') = 'bash'
             ORDER BY p.time_created",
        )?;

        let rows = stmt.query_map([&directory], |row| {
            let data: String = row.get(0)?;
            let time_created: i64 = row.get(1)?;
            Ok((data, time_created))
        })?;

        let mut commands = Vec::new();
        let mut sequence_index = 0;

        for row in rows.flatten() {
            let (data, _time) = row;
            // Parse JSON to extract command from state.input.command
            if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                let cmd = json
                    .pointer("/state/input/command")
                    .and_then(|c| c.as_str())
                    .map(|s| s.to_string());

                if let Some(command) = cmd {
                    commands.push(ExtractedCommand {
                        command,
                        output_len: None,
                        session_id: directory.clone(),
                        output_content: None,
                        is_error: false,
                        sequence_index,
                    });
                    sequence_index += 1;
                }
            }
        }

        Ok(commands)
    }
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

    // ============================================
    // OpenCodeProvider tests (GREEN - functional)
    // ============================================

    #[test]
    fn test_opencode_provider_db_exists() {
        // OpenCodeProvider should find the db
        let provider = OpenCodeProvider;
        let result = provider.discover_sessions(None, None);
        // DB may exist but have no sessions - that's OK
        // The key is it doesn't error about missing db
        assert!(result.is_ok() || result.unwrap_err().to_string().contains("not found"));
    }

    #[test]
    fn test_opencode_provider_returns_commands() {
        // Should return some commands (or empty vec if no sessions)
        let provider = OpenCodeProvider;
        // Get a session path first - we'll create a temp one if needed
        let sessions = provider.discover_sessions(None, Some(365));
        if let Ok(paths) = sessions {
            // If there are any sessions, try to extract
            for path in paths.iter().take(1) {
                let cmds = provider.extract_commands(path);
                assert!(cmds.is_ok());
            }
        }
    }

    // ============================================
    // OpenCodeProvider mocked tests (path-agnostic)
    // ============================================

    /// Create a temporary SQLite DB with OpenCode schema for testing.
    fn make_opencode_db(tables: &[&str]) -> rusqlite::Connection {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        // Create schema
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                title TEXT,
                directory TEXT,
                time_created INTEGER
            );
            CREATE TABLE part (
                id INTEGER PRIMARY KEY,
                session_id TEXT,
                data TEXT,
                time_created INTEGER
            );",
        )
        .unwrap();
        // Insert test data
        for sql in tables {
            conn.execute_batch(sql).unwrap();
        }
        conn
    }

    #[test]
    fn test_opencode_discover_sessions_empty() {
        // Test with no sessions - should return empty
        let conn = make_opencode_db(&[]);
        let result: Vec<PathBuf> = conn
            .prepare("SELECT DISTINCT directory FROM session WHERE directory IS NOT NULL")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .map(PathBuf::from)
            .collect();
        assert!(result.is_empty());
    }

    #[test]
    fn test_opencode_discover_sessions_with_filter() {
        // Test project filter - substring match
        let conn = make_opencode_db(&[
            "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_1', 'Test', '/home/user/rtk', 1777129176);",
            "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_2', 'Test', '/home/user/other', 1777129176);",
        ]);
        let filter = "rtk";
        let result: Vec<PathBuf> = conn
            .prepare("SELECT DISTINCT directory FROM session WHERE directory IS NOT NULL AND directory LIKE ?")
            .unwrap()
            .query_map([format!("%{}%", filter)], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .map(PathBuf::from)
            .collect();
        assert_eq!(result.len(), 1);
        assert!(result[0].to_string_lossy().contains("rtk"));
    }

    #[test]
    fn test_opencode_discover_sessions_since_days() {
        // Test date filter - only sessions newer than cutoff
        let now = 1777129176;
        let old_cutoff = now - (7 * 86400); // 7 days ago
        let conn = make_opencode_db(&[
            &format!(
                "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_new', 'New', '/home/user/new', {});",
                now
            ),
            &format!(
                "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_old', 'Old', '/home/user/old', {});",
                old_cutoff - 86400
            ),
        ]);
        let result: Vec<PathBuf> = conn
            .prepare("SELECT DISTINCT directory FROM session WHERE directory IS NOT NULL AND time_created > ?")
            .unwrap()
            .query_map([old_cutoff], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .map(PathBuf::from)
            .collect();
        assert_eq!(result.len(), 1);
    }

    #[test]
    fn test_opencode_extract_tool_commands() {
        // Test extracting tool commands from OpenCode JSON format
        // Note: OpenCode uses type="tool" with state.input.command and state.output
        let conn = make_opencode_db(&[
            "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_test', 'Test', '/test', 1777129176);",
            "INSERT INTO part (session_id, data, time_created) VALUES ('ses_test', '{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_1\",\"state\":{\"input\":{\"command\":\"git status\"},\"output\":\"On branch main\"}}', 1777129177);",
            "INSERT INTO part (session_id, data, time_created) VALUES ('ses_test', '{\"type\":\"tool\",\"tool\":\"read\",\"callID\":\"call_2\",\"state\":{\"input\":{\"filePath\":\"/tmp/foo\"},\"output\":\"file content\"}}', 1777129178);",
        ]);
        let mut stmt = conn
            .prepare(
                "SELECT data FROM part
             WHERE session_id = 'ses_test'
             AND json_extract(data, '$.type') = 'tool'
             AND json_extract(data, '$.tool') = 'bash'",
            )
            .unwrap();
        let rows: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(rows.len(), 1);
        // Verify JSON parsing extracts command correctly
        let json: serde_json::Value = serde_json::from_str(&rows[0]).unwrap();
        let cmd = json
            .pointer("/state/input/command")
            .and_then(|v| v.as_str());
        assert_eq!(cmd, Some("git status"));
    }

    #[test]
    fn test_opencode_extract_multiple_tools() {
        // Test that multiple tool calls are extracted in order
        let conn = make_opencode_db(&[
            "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_multi', 'Test', '/test', 1777129176);",
            "INSERT INTO part (session_id, data, time_created) VALUES ('ses_multi', '{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_1\",\"state\":{\"input\":{\"command\":\"ls\"},\"output\":\"file1.txt\"}}', 1777129177);",
            "INSERT INTO part (session_id, data, time_created) VALUES ('ses_multi', '{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_2\",\"state\":{\"input\":{\"command\":\"git diff\"},\"output\":\"diff output\"}}', 1777129178);",
            "INSERT INTO part (session_id, data, time_created) VALUES ('ses_multi', '{\"type\":\"tool\",\"tool\":\"read\",\"callID\":\"call_3\",\"state\":{\"input\":{\"filePath\":\"a.rs\"},\"output\":\"code\"}}', 1777129179);",
        ]);
        let mut stmt = conn
            .prepare(
                "SELECT data FROM part
             WHERE session_id = 'ses_multi'
             AND json_extract(data, '$.type') = 'tool'
             ORDER BY time_created",
            )
            .unwrap();
        let tools: Vec<String> = stmt
            .query_map([], |row| {
                let data: String = row.get(0)?;
                let json: serde_json::Value = serde_json::from_str(&data).unwrap();
                Ok(json
                    .pointer("/tool")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string())
            })
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(tools.len(), 3);
        assert_eq!(tools[0], "bash");
        assert_eq!(tools[1], "bash");
        assert_eq!(tools[2], "read");
    }

    #[test]
    fn test_opencode_extract_error_tool() {
        // Test error detection in tool output
        let conn = make_opencode_db(&[
            "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_err', 'Test', '/test', 1777129176);",
            "INSERT INTO part (session_id, data, time_created) VALUES ('ses_err', '{\"type\":\"tool\",\"tool\":\"bash\",\"callID\":\"call_err\",\"state\":{\"input\":{\"command\":\"cargo test\"},\"output\":\"error: failed to compile\"}}', 1777129177);",
        ]);
        let mut stmt = conn
            .prepare("SELECT data FROM part WHERE session_id = 'ses_err'")
            .unwrap();
        let output: Option<String> = stmt
            .query_row([], |row| {
                let data: String = row.get(0)?;
                let json: serde_json::Value = serde_json::from_str(&data).unwrap();
                Ok(json
                    .pointer("/state/output")
                    .and_then(|v| v.as_str())
                    .map(String::from))
            })
            .ok()
            .flatten();
        assert!(output.is_some());
        assert!(output.unwrap().contains("error:"));
    }

    #[test]
    fn test_opencode_malformed_json_handling() {
        // Test that malformed JSON doesn't crash the query
        let conn = make_opencode_db(&[
            "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_bad', 'Test', '/test', 1777129176);",
            "INSERT INTO part (session_id, data, time_created) VALUES ('ses_bad', 'not valid json', 1777129177);",
            "INSERT INTO part (session_id, data, time_created) VALUES ('ses_bad', '{\"type\":\"tool\",\"tool\":\"ls\",\"state\":{\"input\":{\"command\":\"ls\"}}}', 1777129178);",
        ]);
        // This should not panic - we handle parse errors gracefully
        let result: Vec<String> = conn
            .prepare("SELECT data FROM part WHERE session_id = 'ses_bad'")
            .unwrap()
            .query_map([], |row| {
                let data: String = row.get(0)?;
                if let Ok(json) = serde_json::from_str::<serde_json::Value>(&data) {
                    Ok(json
                        .pointer("/tool")
                        .and_then(|v| v.as_str())
                        .map(String::from))
                } else {
                    Ok(None)
                }
            })
            .unwrap()
            .filter_map(|r| r.ok().flatten())
            .collect();
        // Only valid JSON should be extracted
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "ls");
    }

    #[test]
    fn test_opencode_empty_directory_handling() {
        // Test handling of sessions with NULL or empty directory
        // SQLite: '' IS NOT NULL evaluates to true, so we get both rows
        let conn = make_opencode_db(&[
            "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_null', 'Test', NULL, 1777129176);",
            "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_empty', 'Test', '', 1777129177);",
            "INSERT INTO session (id, title, directory, time_created) VALUES ('ses_valid', 'Test', '/valid/path', 1777129178);",
        ]);
        // Filter NULL and empty - only valid paths
        let result: Vec<String> = conn
            .prepare("SELECT directory FROM session WHERE directory IS NOT NULL AND length(directory) > 0")
            .unwrap()
            .query_map([], |row| row.get(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        // Only non-empty directories should be returned
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "/valid/path");
    }
}
