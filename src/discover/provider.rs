//! Reads Claude Code session logs from disk and streams their command history.

use crate::hooks::constants::CLAUDE_DIR;
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
    ///
    /// Claude Code replaces `/`, `.`, `_`, `\`, and any non-ASCII character
    /// with `-` when computing the project directory slug under `~/.claude/projects/`.
    ///
    /// `/Users/foo/bar`          → `-Users-foo-bar`
    /// `/Users/first.last/bar`   → `-Users-first-last-bar`
    /// `/home/chris/2_project`   → `-home-chris-2-project`
    /// `C:\Users\foo\bar`        → `C:-Users-foo-bar`
    pub fn encode_project_path(path: &str) -> String {
        const SANITIZED_CHARS: &[char] = &['/', '.', '_', '\\'];

        path.chars()
            .map(|c| {
                if !c.is_ascii() || SANITIZED_CHARS.contains(&c) {
                    '-'
                } else {
                    c
                }
            })
            .collect()
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

// ─── Crush Provider ──────────────────────────────────────────

/// Prefix used to encode Crush session IDs as pseudo file paths.
const CRUSH_PATH_PREFIX: &str = "crush:";

/// Provider that reads Charmbracelet Crush session data from its SQLite database.
///
/// Crush stores conversations in `~/.local/share/crush/crush.db` (Linux) or the
/// platform-appropriate XDG data directory. Each message's `parts` column holds a
/// JSON array of Google genai `Part` objects where Bash tool invocations appear as
/// `functionCall` entries and their results as `functionResponse` entries.
pub struct CrushProvider {
    db_path: PathBuf,
}

impl CrushProvider {
    /// Create a new CrushProvider, resolving the Crush database path.
    pub fn new() -> Result<Self> {
        let data_dir = dirs::data_dir().context("could not determine XDG data directory")?;
        let db_path = data_dir.join("crush").join("crush.db");
        if !db_path.exists() {
            anyhow::bail!(
                "Crush database not found at {}\nMake sure Charmbracelet Crush has been used at least once.",
                db_path.display()
            );
        }
        Ok(Self { db_path })
    }

    /// Open a read-only connection to the Crush SQLite database.
    fn open_db(&self) -> Result<rusqlite::Connection> {
        let conn = rusqlite::Connection::open_with_flags(
            &self.db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
        )
        .with_context(|| {
            format!(
                "failed to open Crush database at {}",
                self.db_path.display()
            )
        })?;
        Ok(conn)
    }
}

#[cfg(test)]
impl CrushProvider {
    /// Test-only constructor that accepts a custom database path.
    fn with_db(db_path: PathBuf) -> Self {
        Self { db_path }
    }
}

impl SessionProvider for CrushProvider {
    fn discover_sessions(
        &self,
        _project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        let conn = self.open_db()?;

        let mut query = String::from("SELECT id FROM sessions WHERE 1=1");

        // Apply time filter (created_at is Unix milliseconds)
        if let Some(days) = since_days {
            let cutoff_ms = (std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis() as i64)
                .saturating_sub(days as i64 * 86_400_000);
            query.push_str(&format!(" AND created_at >= {}", cutoff_ms));
        }

        query.push_str(" ORDER BY created_at DESC");

        let mut stmt = conn
            .prepare(&query)
            .context("failed to query Crush sessions")?;

        let sessions: Vec<PathBuf> = stmt
            .query_map([], |row| {
                let id: String = row.get(0)?;
                Ok(PathBuf::from(format!("{}{}", CRUSH_PATH_PREFIX, id)))
            })
            .context("failed to map Crush session rows")?
            .filter_map(|r| r.ok())
            .collect();

        Ok(sessions)
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        let path_str = path.to_string_lossy();
        let session_id = path_str
            .strip_prefix(CRUSH_PATH_PREFIX)
            .context("invalid crush session path")?;

        let conn = self.open_db()?;

        // Query messages for this session, ordered by creation time
        let mut stmt = conn
            .prepare(
                "SELECT id, role, parts FROM messages WHERE session_id = ?1 ORDER BY created_at ASC",
            )
            .context("failed to query Crush messages")?;

        let rows: Vec<(String, String, String)> = stmt
            .query_map([session_id], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                ))
            })
            .context("failed to map Crush message rows")?
            .filter_map(|r| r.ok())
            .collect();

        // Collect function calls (bash commands) and their responses
        let mut pending_calls: Vec<(String, String, usize)> = Vec::new(); // (msg_id, command, seq)
        let mut responses: std::collections::HashMap<String, (usize, String, bool)> =
            std::collections::HashMap::new(); // (msg_id, output_len, output_content, is_error)
        let mut commands: Vec<ExtractedCommand> = Vec::new();
        let mut sequence_counter: usize = 0;

        for (msg_id, role, parts_json) in &rows {
            let parts: serde_json::Value = match serde_json::from_str(parts_json) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let parts_array = match parts.as_array() {
                Some(a) => a,
                None => continue,
            };

            match role.as_str() {
                "user" | "model" => {
                    for part in parts_array {
                        // Look for functionResponse (bash output)
                        if let Some(fr) = part.get("functionResponse") {
                            if fr.get("name").and_then(|n| n.as_str()) == Some("bash") {
                                let output = fr
                                    .pointer("/response/output")
                                    .and_then(|o| o.as_str())
                                    .unwrap_or("");

                                let exit_code = fr
                                    .pointer("/response/exitCode")
                                    .and_then(|c| c.as_i64())
                                    .unwrap_or(0);

                                let output_len = output.len();
                                let is_error = exit_code != 0;
                                let content_preview: String = output.chars().take(1000).collect();

                                // Match to the most recent unmatched bash call from this message
                                // (Crush typically pairs them by position within the same message)
                                // For simplicity, just store by message ID and match later
                                if let Some(last) = pending_calls.last() {
                                    responses.insert(
                                        last.0.clone(),
                                        (output_len, content_preview, is_error),
                                    );
                                }
                            }
                        }

                        // Look for functionCall (bash invocation)
                        if let Some(fc) = part.get("functionCall") {
                            if fc.get("name").and_then(|n| n.as_str()) == Some("bash") {
                                if let Some(command) =
                                    fc.pointer("/args/command").and_then(|c| c.as_str())
                                {
                                    pending_calls.push((
                                        msg_id.to_string(),
                                        command.to_string(),
                                        sequence_counter,
                                    ));
                                    sequence_counter += 1;
                                }
                            }
                        }
                    }
                }
                _ => {}
            }
        }

        // Match pending calls with responses
        for (msg_id, command, sequence_index) in pending_calls {
            let (output_len, output_content, is_error) = responses
                .get(&msg_id)
                .map(|(len, content, err)| (Some(*len), Some(content.clone()), *err))
                .unwrap_or((None, None, false));

            commands.push(ExtractedCommand {
                command,
                output_len,
                session_id: session_id.to_string(),
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
    fn test_encode_project_path_dot_in_username() {
        // Claude Code replaces both '/' and '.' with '-'.
        // A cwd like /Users/first.last must produce the same slug as
        // Claude's projects directory (-Users-first-last), otherwise
        // `rtk discover` finds zero sessions for that project.
        assert_eq!(
            ClaudeProvider::encode_project_path("/Users/first.last/my-project"),
            "-Users-first-last-my-project"
        );
    }

    #[test]
    fn test_encode_project_path_multiple_dots() {
        assert_eq!(
            ClaudeProvider::encode_project_path("/Users/a.b.c/proj"),
            "-Users-a-b-c-proj"
        );
    }

    #[test]
    fn test_encode_project_path_underscore() {
        // Claude Code also replaces '_' with '-' (https://github.com/anthropics/claude-code/issues/24067)
        assert_eq!(
            ClaudeProvider::encode_project_path("/home/chris/2_project-files/proj"),
            "-home-chris-2-project-files-proj"
        );
    }

    #[test]
    fn test_encode_project_path_non_ascii() {
        // Non-ASCII characters are each replaced with '-' (https://github.com/anthropics/claude-code/issues/40946)
        // '/home/user/' + '外' + '主' + '/app' -> '-home-user' + '-' + '-' + '-' + '-' + 'app'
        assert_eq!(
            ClaudeProvider::encode_project_path("/home/user/\u{5916}\u{4e3b}/app"),
            "-home-user----app"
        );
    }

    #[test]
    fn test_encode_project_path_windows() {
        // Windows backslashes are also replaced with '-'
        assert_eq!(
            ClaudeProvider::encode_project_path(r"C:\Users\foo\bar"),
            "C:-Users-foo-bar"
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

    // ─── CrushProvider tests ───────────────────────────────────────

    use rusqlite::params;

    /// Create an in-memory SQLite DB with the Crush schema, populate it, and
    /// return a `CrushProvider` backed by a temporary file. The `NamedTempFile`
    /// must be kept alive for the provider to access the DB.
    fn setup_crush_db(
        sessions: &[(&str, &str, i64, i64)],
        messages: &[(&str, &str, &str, &str)],
    ) -> (CrushProvider, tempfile::NamedTempFile) {
        let tf = tempfile::NamedTempFile::new().unwrap();
        let conn = rusqlite::Connection::open(tf.path()).expect("failed to open temp sqlite db");

        conn.execute_batch(
            "CREATE TABLE sessions (
                id TEXT PRIMARY KEY,
                parent_session_id TEXT,
                title TEXT NOT NULL,
                message_count INTEGER NOT NULL DEFAULT 0,
                prompt_tokens INTEGER NOT NULL DEFAULT 0,
                completion_tokens INTEGER NOT NULL DEFAULT 0,
                cost REAL NOT NULL DEFAULT 0.0,
                updated_at INTEGER NOT NULL,
                created_at INTEGER NOT NULL
            );
            CREATE TABLE messages (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                role TEXT NOT NULL,
                parts TEXT NOT NULL DEFAULT '[]',
                model TEXT,
                created_at INTEGER NOT NULL,
                updated_at INTEGER NOT NULL,
                finished_at INTEGER,
                FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
            );",
        )
        .expect("failed to create schema");

        for (id, title, created_at, updated_at) in sessions {
            conn.execute(
                "INSERT INTO sessions (id, title, message_count, created_at, updated_at)
                 VALUES (?1, ?2, 0, ?3, ?4)",
                params![id, title, created_at, updated_at],
            )
            .unwrap();
        }

        for (id, session_id, role, parts) in messages {
            conn.execute(
                "INSERT INTO messages (id, session_id, role, parts, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, 0, 0)",
                params![id, session_id, role, parts],
            )
            .unwrap();
        }

        let provider = CrushProvider::with_db(tf.path().to_path_buf());
        (provider, tf)
    }

    /// Unix timestamp in milliseconds (now).
    fn now_ms() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_millis() as i64
    }

    #[test]
    fn test_crush_discover_sessions() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(
            &[
                ("sess-001", "Fixing the login bug", now, now),
                ("sess-002", "Refactoring the API", now - 86_400_000, now),
                ("sess-003", "Planning sprint", now - 172_800_000, now),
            ],
            &[],
        );

        let sessions = provider.discover_sessions(None, None).unwrap();
        assert_eq!(sessions.len(), 3);
        // Should all use the crush: prefix
        for s in &sessions {
            assert!(s.to_string_lossy().starts_with("crush:"));
        }
    }

    #[test]
    fn test_crush_discover_sessions_since_days() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(
            &[
                ("sess-001", "Recent", now, now),
                ("sess-002", "3 days ago", now - (3 * 86_400_000), now),
                ("sess-003", "5 days ago", now - (5 * 86_400_000), now),
            ],
            &[],
        );

        // Filter to last 2 days: should only include sess-001
        let sessions = provider.discover_sessions(None, Some(2)).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].to_string_lossy(), "crush:sess-001");
    }

    #[test]
    fn test_crush_extract_commands_bash_with_output() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(
            &[("sess-001", "Test session", now, now)],
            &[
                (
                    "msg-001",
                    "sess-001",
                    "user",
                    r#"[{"functionCall":{"name":"bash","args":{"command":"git status"}}}]"#,
                ),
                (
                    "msg-002",
                    "sess-001",
                    "user",
                    r#"[{"functionResponse":{"name":"bash","response":{"output":"On branch master\nnothing to commit","exitCode":0}}}]"#,
                ),
            ],
        );

        let session_path = PathBuf::from("crush:sess-001");
        let cmds = provider.extract_commands(&session_path).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "git status");
        assert_eq!(cmds[0].session_id, "sess-001");
        assert!(!cmds[0].is_error);
        assert_eq!(cmds[0].sequence_index, 0);
        assert!(cmds[0].output_len.is_some());
    }

    #[test]
    fn test_crush_extract_commands_error_exit_code() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(
            &[("sess-001", "Test session", now, now)],
            &[
                (
                    "msg-001",
                    "sess-001",
                    "user",
                    r#"[{"functionCall":{"name":"bash","args":{"command":"cargo build --release"}}}]"#,
                ),
                (
                    "msg-002",
                    "sess-001",
                    "user",
                    r#"[{"functionResponse":{"name":"bash","response":{"output":"error: could not compile","exitCode":101}}}]"#,
                ),
            ],
        );

        let session_path = PathBuf::from("crush:sess-001");
        let cmds = provider.extract_commands(&session_path).unwrap();
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].is_error);
        assert_eq!(
            cmds[0].output_content.as_deref(),
            Some("error: could not compile")
        );
    }

    #[test]
    fn test_crush_extract_commands_multiple() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(
            &[("sess-001", "Test session", now, now)],
            &[
                (
                    "msg-001",
                    "sess-001",
                    "user",
                    r#"[
                        {"functionCall":{"name":"bash","args":{"command":"git status"}}},
                        {"functionCall":{"name":"bash","args":{"command":"git diff"}}}
                    ]"#,
                ),
                (
                    "msg-002",
                    "sess-001",
                    "user",
                    r#"[
                        {"functionResponse":{"name":"bash","response":{"output":"clean","exitCode":0}}}
                    ]"#,
                ),
            ],
        );

        let session_path = PathBuf::from("crush:sess-001");
        let cmds = provider.extract_commands(&session_path).unwrap();
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "git status");
        assert_eq!(cmds[0].sequence_index, 0);
        assert_eq!(cmds[1].command, "git diff");
        assert_eq!(cmds[1].sequence_index, 1);
    }

    #[test]
    fn test_crush_extract_commands_non_bash_ignored() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(
            &[("sess-001", "Test session", now, now)],
            &[(
                "msg-001",
                "sess-001",
                "user",
                r#"[{"functionCall":{"name":"Read","args":{"file_path":"/tmp/foo"}}}]"#,
            )],
        );

        let session_path = PathBuf::from("crush:sess-001");
        let cmds = provider.extract_commands(&session_path).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_crush_extract_commands_malformed_parts() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(
            &[("sess-001", "Test session", now, now)],
            &[
                ("msg-001", "sess-001", "user", "this is not json"),
                (
                    "msg-002",
                    "sess-001",
                    "user",
                    r#"[{"functionCall":{"name":"bash","args":{"command":"ls"}}}]"#,
                ),
            ],
        );

        let session_path = PathBuf::from("crush:sess-001");
        let cmds = provider.extract_commands(&session_path).unwrap();
        // msg-001 is skipped (malformed JSON), msg-002 yields one command
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "ls");
    }

    #[test]
    fn test_crush_extract_commands_invalid_session_path() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(&[("sess-001", "Test session", now, now)], &[]);

        // Path without crush: prefix
        let session_path = PathBuf::from("sess-001");
        let result = provider.extract_commands(&session_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_crush_extract_commands_empty_session() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(&[("sess-001", "Test session", now, now)], &[]);

        let session_path = PathBuf::from("crush:sess-001");
        let cmds = provider.extract_commands(&session_path).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_crush_project_filter_ignored() {
        // Crush sessions table has no project_path column, so project_filter is
        // always ignored (all sessions returned).
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(
            &[
                ("sess-001", "Project A", now, now),
                ("sess-002", "Project B", now, now),
            ],
            &[],
        );

        let sessions = provider
            .discover_sessions(Some("nonexistent"), None)
            .unwrap();
        // project_filter is ignored; all sessions should be returned
        assert_eq!(sessions.len(), 2);
    }

    #[test]
    fn test_crush_session_id_encoding() {
        let now = now_ms();
        let (provider, _tmp) = setup_crush_db(&[("abc-123-def", "Test session", now, now)], &[]);

        let sessions = provider.discover_sessions(None, None).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0], PathBuf::from("crush:abc-123-def"));
    }
}
