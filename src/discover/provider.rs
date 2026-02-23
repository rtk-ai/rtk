use anyhow::{Context, Result};
use rusqlite::Connection;
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
    pub sequence_index: usize,
}

/// Trait for session providers (Claude Code, future: Cursor, Windsurf).
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
        let dir = home.join(".claude").join("projects");
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

pub struct OpenCodeProvider;

impl OpenCodeProvider {
    /// Get the path to the OpenCode SQLite database.
    fn db_path() -> Result<PathBuf> {
        let home = dirs::home_dir().context("could not determine home directory")?;
        let db = home
            .join(".local")
            .join("share")
            .join("opencode")
            .join("opencode.db");
        if !db.exists() {
            anyhow::bail!(
                "OpenCode database not found: {}\nMake sure OpenCode has been used at least once.",
                db.display()
            );
        }
        Ok(db)
    }

    /// Open a read-only connection to an SQLite database at the given path.
    fn open_db(path: &Path) -> Result<Connection> {
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| format!("failed to open database: {}", path.display()))?;
        Ok(conn)
    }

    /// Extract bash commands from a single session in the database.
    fn extract_commands_from_session(
        conn: &Connection,
        session_id: &str,
    ) -> Result<Vec<ExtractedCommand>> {
        let mut stmt = conn
            .prepare(
                "SELECT data FROM part \
                 WHERE session_id = ?1 \
                   AND json_extract(data, '$.tool') = 'bash' \
                   AND json_extract(data, '$.type') = 'tool' \
                 ORDER BY time_created ASC",
            )
            .context("failed to prepare part query")?;

        let mut commands = Vec::new();
        let mut sequence_counter = 0usize;

        let rows = stmt
            .query_map([session_id], |row| {
                let data: String = row.get(0)?;
                Ok(data)
            })
            .context("failed to query parts")?;

        for row in rows {
            let data = match row {
                Ok(d) => d,
                Err(_) => continue,
            };

            let entry: serde_json::Value = match serde_json::from_str(&data) {
                Ok(v) => v,
                Err(_) => continue,
            };

            // Extract command from state.input.command
            let command = match entry
                .pointer("/state/input/command")
                .and_then(|c| c.as_str())
            {
                Some(cmd) => cmd.to_string(),
                None => continue,
            };

            // Extract output content and length
            let output = entry
                .pointer("/state/output")
                .and_then(|o| o.as_str())
                .unwrap_or("");
            let output_len = Some(output.len());

            // First ~1000 chars for error detection
            let output_content: String = output.chars().take(1000).collect();
            let output_content = if output_content.is_empty() {
                None
            } else {
                Some(output_content)
            };

            // Extract exit code: non-zero = error
            let exit_code = entry
                .pointer("/state/metadata/exit")
                .and_then(|e| e.as_i64())
                .unwrap_or(0);
            let is_error = exit_code != 0;

            commands.push(ExtractedCommand {
                command,
                output_len,
                session_id: session_id.to_string(),
                output_content,
                is_error,
                sequence_index: sequence_counter,
            });
            sequence_counter += 1;
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
        let db_path = Self::db_path()?;
        let conn = Self::open_db(&db_path)?;

        let cutoff_ms = since_days.map(|days| {
            let now_ms = SystemTime::now()
                .duration_since(SystemTime::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64;
            now_ms - (days as i64 * 86400 * 1000)
        });

        // Query sessions, optionally filtering by project directory and time
        // We return PathBuf-encoded session IDs as "virtual paths" since
        // the SessionProvider trait uses PathBuf to identify sessions.
        // Format: db_path + "#" + session_id (parsed back in extract_commands)
        let mut query =
            String::from("SELECT s.id, s.directory, s.time_created FROM session s WHERE 1=1");
        let mut params: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();

        if let Some(filter) = project_filter {
            query.push_str(" AND s.directory LIKE ?");
            params.push(Box::new(format!("%{}%", filter)));
        }

        if let Some(cutoff) = cutoff_ms {
            query.push_str(" AND s.time_created >= ?");
            params.push(Box::new(cutoff));
        }

        let mut stmt = conn
            .prepare(&query)
            .context("failed to prepare session query")?;

        let param_refs: Vec<&dyn rusqlite::types::ToSql> =
            params.iter().map(|p| p.as_ref()).collect();

        let rows = stmt
            .query_map(param_refs.as_slice(), |row| {
                let id: String = row.get(0)?;
                Ok(id)
            })
            .context("failed to query sessions")?;

        let mut sessions = Vec::new();
        for session_id in rows.flatten() {
            // Encode as virtual path: "db_path#session_id"
            let virtual_path = format!("{}#{}", db_path.display(), session_id);
            sessions.push(PathBuf::from(virtual_path));
        }

        Ok(sessions)
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        // Parse virtual path format: "db_path#session_id"
        let path_str = path.to_string_lossy();
        let (db_path_str, session_id) = path_str
            .rsplit_once('#')
            .context("invalid OpenCode session path (expected db_path#session_id)")?;

        let db_path = Path::new(db_path_str);
        let conn = Self::open_db(db_path)?;

        Self::extract_commands_from_session(&conn, session_id)
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

    // --- OpenCodeProvider tests ---

    /// Create a temporary SQLite database mimicking the OpenCode schema.
    fn make_opencode_db() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("opencode.db");
        let conn = Connection::open(&db_path).unwrap();

        conn.execute_batch(
            "CREATE TABLE project (
                id TEXT PRIMARY KEY,
                worktree TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                sandboxes TEXT NOT NULL DEFAULT '[]'
            );
            CREATE TABLE session (
                id TEXT PRIMARY KEY,
                project_id TEXT NOT NULL,
                directory TEXT NOT NULL,
                title TEXT NOT NULL DEFAULT '',
                slug TEXT NOT NULL DEFAULT '',
                version TEXT NOT NULL DEFAULT '',
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                FOREIGN KEY (project_id) REFERENCES project(id)
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL,
                FOREIGN KEY (session_id) REFERENCES session(id)
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL,
                FOREIGN KEY (message_id) REFERENCES message(id)
            );",
        )
        .unwrap();

        (dir, db_path)
    }

    fn insert_project(conn: &Connection, id: &str, worktree: &str) {
        conn.execute(
            "INSERT INTO project (id, worktree, time_created, time_updated) VALUES (?1, ?2, ?3, ?3)",
            rusqlite::params![id, worktree, 1000000],
        )
        .unwrap();
    }

    fn insert_session(
        conn: &Connection,
        id: &str,
        project_id: &str,
        directory: &str,
        time_created: i64,
    ) {
        conn.execute(
            "INSERT INTO session (id, project_id, directory, time_created, time_updated) VALUES (?1, ?2, ?3, ?4, ?4)",
            rusqlite::params![id, project_id, directory, time_created],
        )
        .unwrap();
    }

    fn insert_part(
        conn: &Connection,
        id: &str,
        message_id: &str,
        session_id: &str,
        time_created: i64,
        data: &str,
    ) {
        // Ensure a message row exists
        let _ = conn.execute(
            "INSERT OR IGNORE INTO message (id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?3, '{}')",
            rusqlite::params![message_id, session_id, time_created],
        );
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, time_updated, data) VALUES (?1, ?2, ?3, ?4, ?4, ?5)",
            rusqlite::params![id, message_id, session_id, time_created, data],
        )
        .unwrap();
    }

    #[test]
    fn test_opencode_extract_bash_command() {
        let (_dir, db_path) = make_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        insert_project(&conn, "proj1", "/Users/foo/myproject");
        insert_session(&conn, "ses1", "proj1", "/Users/foo/myproject", 1000000);

        let data = r#"{"type":"tool","tool":"bash","callID":"call1","state":{"status":"completed","input":{"command":"git status"},"output":"On branch main\nnothing to commit","metadata":{"exit":0}}}"#;
        insert_part(&conn, "part1", "msg1", "ses1", 1000001, data);

        let cmds = OpenCodeProvider::extract_commands_from_session(&conn, "ses1").unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "git status");
        assert_eq!(
            cmds[0].output_len,
            Some("On branch main\nnothing to commit".len())
        );
        assert!(!cmds[0].is_error);
        assert_eq!(cmds[0].session_id, "ses1");
        assert_eq!(cmds[0].sequence_index, 0);
    }

    #[test]
    fn test_opencode_extract_error_command() {
        let (_dir, db_path) = make_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        insert_project(&conn, "proj1", "/Users/foo/myproject");
        insert_session(&conn, "ses1", "proj1", "/Users/foo/myproject", 1000000);

        let data = r#"{"type":"tool","tool":"bash","callID":"call1","state":{"status":"completed","input":{"command":"invalid_cmd"},"output":"command not found: invalid_cmd","metadata":{"exit":127}}}"#;
        insert_part(&conn, "part1", "msg1", "ses1", 1000001, data);

        let cmds = OpenCodeProvider::extract_commands_from_session(&conn, "ses1").unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "invalid_cmd");
        assert!(cmds[0].is_error);
        assert!(cmds[0]
            .output_content
            .as_ref()
            .unwrap()
            .contains("command not found"));
    }

    #[test]
    fn test_opencode_extract_multiple_commands() {
        let (_dir, db_path) = make_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        insert_project(&conn, "proj1", "/Users/foo/myproject");
        insert_session(&conn, "ses1", "proj1", "/Users/foo/myproject", 1000000);

        let data1 = r#"{"type":"tool","tool":"bash","callID":"call1","state":{"status":"completed","input":{"command":"ls"},"output":"file1.txt","metadata":{"exit":0}}}"#;
        let data2 = r#"{"type":"tool","tool":"bash","callID":"call2","state":{"status":"completed","input":{"command":"git diff"},"output":"diff output","metadata":{"exit":0}}}"#;
        insert_part(&conn, "part1", "msg1", "ses1", 1000001, data1);
        insert_part(&conn, "part2", "msg2", "ses1", 1000002, data2);

        let cmds = OpenCodeProvider::extract_commands_from_session(&conn, "ses1").unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "ls");
        assert_eq!(cmds[0].sequence_index, 0);
        assert_eq!(cmds[1].command, "git diff");
        assert_eq!(cmds[1].sequence_index, 1);
    }

    #[test]
    fn test_opencode_ignores_non_bash_tools() {
        let (_dir, db_path) = make_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        insert_project(&conn, "proj1", "/Users/foo/myproject");
        insert_session(&conn, "ses1", "proj1", "/Users/foo/myproject", 1000000);

        // A non-bash tool (e.g., "read") should be ignored
        let data = r#"{"type":"tool","tool":"read","callID":"call1","state":{"status":"completed","input":{"filePath":"/tmp/foo"},"output":"file contents"}}"#;
        insert_part(&conn, "part1", "msg1", "ses1", 1000001, data);

        let cmds = OpenCodeProvider::extract_commands_from_session(&conn, "ses1").unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_opencode_ignores_non_tool_parts() {
        let (_dir, db_path) = make_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        insert_project(&conn, "proj1", "/Users/foo/myproject");
        insert_session(&conn, "ses1", "proj1", "/Users/foo/myproject", 1000000);

        // text-type part should be ignored
        let data = r#"{"type":"text","text":"Let me check something..."}"#;
        insert_part(&conn, "part1", "msg1", "ses1", 1000001, data);

        let cmds = OpenCodeProvider::extract_commands_from_session(&conn, "ses1").unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_opencode_empty_output() {
        let (_dir, db_path) = make_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        insert_project(&conn, "proj1", "/Users/foo/myproject");
        insert_session(&conn, "ses1", "proj1", "/Users/foo/myproject", 1000000);

        let data = r#"{"type":"tool","tool":"bash","callID":"call1","state":{"status":"completed","input":{"command":"true"},"output":"","metadata":{"exit":0}}}"#;
        insert_part(&conn, "part1", "msg1", "ses1", 1000001, data);

        let cmds = OpenCodeProvider::extract_commands_from_session(&conn, "ses1").unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].output_len, Some(0));
        assert!(cmds[0].output_content.is_none()); // Empty string becomes None
    }

    #[test]
    fn test_opencode_discover_sessions_all() {
        let (_dir, db_path) = make_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        insert_project(&conn, "proj1", "/Users/foo/project-a");
        insert_project(&conn, "proj2", "/Users/foo/project-b");
        insert_session(&conn, "ses1", "proj1", "/Users/foo/project-a", 1000000);
        insert_session(&conn, "ses2", "proj2", "/Users/foo/project-b", 2000000);
        drop(conn);

        // We can't use OpenCodeProvider directly (it uses hardcoded db_path()),
        // but we can test the virtual path format via extract_commands
        let virtual_path = format!("{}#ses1", db_path.display());
        let provider = OpenCodeProvider;
        let cmds = provider.extract_commands(Path::new(&virtual_path)).unwrap();
        assert_eq!(cmds.len(), 0); // No parts inserted
    }

    #[test]
    fn test_opencode_virtual_path_roundtrip() {
        let (_dir, db_path) = make_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        insert_project(&conn, "proj1", "/Users/foo/myproject");
        insert_session(&conn, "ses1", "proj1", "/Users/foo/myproject", 1000000);

        let data = r#"{"type":"tool","tool":"bash","callID":"call1","state":{"status":"completed","input":{"command":"echo hello"},"output":"hello","metadata":{"exit":0}}}"#;
        insert_part(&conn, "part1", "msg1", "ses1", 1000001, data);
        drop(conn);

        // Simulate what discover_sessions would produce
        let virtual_path = format!("{}#ses1", db_path.display());

        let provider = OpenCodeProvider;
        let cmds = provider.extract_commands(Path::new(&virtual_path)).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "echo hello");
    }

    #[test]
    fn test_opencode_extract_output_content_truncated() {
        let (_dir, db_path) = make_opencode_db();
        let conn = Connection::open(&db_path).unwrap();

        insert_project(&conn, "proj1", "/Users/foo/myproject");
        insert_session(&conn, "ses1", "proj1", "/Users/foo/myproject", 1000000);

        // Create output longer than 1000 chars
        let long_output = "x".repeat(2000);
        let data = format!(
            r#"{{"type":"tool","tool":"bash","callID":"call1","state":{{"status":"completed","input":{{"command":"cat bigfile"}},"output":"{}","metadata":{{"exit":0}}}}}}"#,
            long_output
        );
        insert_part(&conn, "part1", "msg1", "ses1", 1000001, &data);

        let cmds = OpenCodeProvider::extract_commands_from_session(&conn, "ses1").unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].output_len, Some(2000));
        // output_content should be truncated to 1000 chars
        assert_eq!(cmds[0].output_content.as_ref().unwrap().len(), 1000);
    }
}
