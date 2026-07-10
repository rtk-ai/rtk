//! Reads Claude Code session logs from disk and streams their command history.

use crate::hooks::init::resolve_claude_dir;
use anyhow::{anyhow, Context, Result};
use rusqlite::{params, Connection, OpenFlags};
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant, SystemTime};
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
    #[allow(dead_code)]
    pub occurred_at_unix: Option<i64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProviderKind {
    Claude,
    Codex,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionSource {
    ClaudeFile(PathBuf),
    CodexThread { db_path: PathBuf, thread_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionRef {
    pub provider: ProviderKind,
    pub id: String,
    pub source: SessionSource,
}

impl SessionRef {
    pub fn display_source(&self) -> String {
        match &self.source {
            SessionSource::ClaudeFile(path) => path.display().to_string(),
            SessionSource::CodexThread { db_path, thread_id } => {
                format!("{}#{thread_id}", db_path.display())
            }
        }
    }

    pub fn claude_path(&self) -> Option<&Path> {
        match &self.source {
            SessionSource::ClaudeFile(path) => Some(path.as_path()),
            _ => None,
        }
    }
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
    ) -> Result<Vec<SessionRef>>;
    fn extract_commands(&self, session: &SessionRef) -> Result<Vec<ExtractedCommand>>;
}

pub struct ClaudeProvider;

impl ClaudeProvider {
    /// Get the base directory for Claude Code projects.
    fn projects_dir() -> Result<PathBuf> {
        let claude_dir = resolve_claude_dir().context("could not determine claude directory")?;
        Ok(claude_dir.join("projects"))
    }

    fn discover_sessions_in_projects_dir(
        projects_dir: &Path,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        if !projects_dir
            .try_exists()
            .with_context(|| format!("failed to access {}", projects_dir.display()))?
        {
            return Ok(Vec::new());
        }

        let cutoff = since_days.map(|days| {
            SystemTime::now()
                .checked_sub(Duration::from_secs(days * 86400))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        });

        let mut sessions = Vec::new();

        // List project directories
        let entries = fs::read_dir(projects_dir)
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
        const SANITIZED_CHARS: &[char] = &['/', '.', '_', '\\', ' ', '[', ']'];

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
    ) -> Result<Vec<SessionRef>> {
        let projects_dir = Self::projects_dir()?;
        let paths =
            Self::discover_sessions_in_projects_dir(&projects_dir, project_filter, since_days)?;
        Ok(paths.into_iter().map(Self::session_ref_from_path).collect())
    }

    fn extract_commands(&self, session: &SessionRef) -> Result<Vec<ExtractedCommand>> {
        let path = session
            .claude_path()
            .ok_or_else(|| anyhow!("expected Claude file session"))?;
        self.extract_commands_from_path(path)
    }
}

impl ClaudeProvider {
    fn session_ref_from_path(path: PathBuf) -> SessionRef {
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string();
        SessionRef {
            provider: ProviderKind::Claude,
            id,
            source: SessionSource::ClaudeFile(path),
        }
    }

    pub fn extract_commands_from_path(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
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
                occurred_at_unix: None,
            });
        }

        Ok(commands)
    }
}

pub struct CodexProvider {
    db_path_override: Option<PathBuf>,
}

struct CodexScanDiagnostics {
    elapsed: Duration,
    selected_rows: usize,
    extracted_commands: usize,
    db_size_bytes: u64,
}

impl CodexProvider {
    pub fn new(db_path_override: Option<PathBuf>) -> Self {
        Self { db_path_override }
    }

    pub fn candidate_db_paths(override_path: Option<PathBuf>) -> Vec<PathBuf> {
        if let Some(path) = override_path {
            return vec![path];
        }

        home_dir()
            .map(|home| vec![home.join(".codex").join("logs_2.sqlite")])
            .unwrap_or_default()
    }

    fn selected_db_path(&self) -> Option<PathBuf> {
        Self::candidate_db_paths(self.db_path_override.clone())
            .into_iter()
            .find(|path| path.is_file())
    }

    fn open_readonly(path: &Path) -> Result<Connection> {
        let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .with_context(|| {
                format!(
                    "failed to open Codex database read-only: {}",
                    path.display()
                )
            })?;
        conn.busy_timeout(Duration::from_secs(2))?;
        Ok(conn)
    }

    fn validate_schema(conn: &Connection) -> Result<()> {
        let mut stmt = conn.prepare("PRAGMA table_info(logs)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        if columns.is_empty() {
            anyhow::bail!("Codex database is missing required logs table");
        }

        for required in ["id", "ts", "ts_nanos", "thread_id", "feedback_log_body"] {
            if !columns.iter().any(|col| col == required) {
                anyhow::bail!("Codex database logs table is missing required column {required}");
            }
        }
        Ok(())
    }

    pub fn check_provider(&self) -> Result<String> {
        let candidates = Self::candidate_db_paths(self.db_path_override.clone());
        let mut out = String::new();
        out.push_str("Codex provider check\n");
        out.push_str("Candidates:\n");
        for path in &candidates {
            out.push_str(&format!("  {}\n", path.display()));
        }

        let Some(path) = self.selected_db_path() else {
            out.push_str("Selected: none\n");
            return Ok(out);
        };

        out.push_str(&format!("Selected: {}\n", path.display()));
        let conn = Self::open_readonly(&path)?;
        let mut stmt = conn.prepare("PRAGMA table_info(logs)")?;
        let columns = stmt
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        out.push_str(&format!("logs columns: {}\n", columns.join(", ")));
        Self::validate_schema(&conn)?;
        out.push_str("schema: compatible\n");
        let diagnostics = Self::scan_diagnostics_in_db(&path, None)?;
        out.push_str(&Self::format_scan_diagnostics(&diagnostics));
        Ok(out)
    }

    fn scan_diagnostics_in_db(
        path: &Path,
        since_days: Option<u64>,
    ) -> Result<CodexScanDiagnostics> {
        let started = Instant::now();
        let db_size_bytes = fs::metadata(path).map(|m| m.len()).unwrap_or(0);
        let conn = Self::open_readonly(path)?;
        Self::validate_schema(&conn)
            .with_context(|| format!("unsupported Codex database schema: {}", path.display()))?;
        let cutoff = Self::cutoff_unix(since_days);
        let mut stmt = conn
            .prepare(
                "SELECT feedback_log_body FROM logs \
             WHERE ts >= ?1 AND feedback_log_body LIKE '%ToolCall: shell_command%'",
            )
            .with_context(|| {
                format!("failed to query Codex diagnostic rows: {}", path.display())
            })?;
        let rows = stmt
            .query_map(params![cutoff], |row| row.get::<_, String>(0))
            .with_context(|| format!("failed to read Codex diagnostic rows: {}", path.display()))?;

        let mut selected_rows = 0;
        let mut extracted_commands = 0;
        for row in rows {
            let body = row.with_context(|| {
                format!("failed to read Codex diagnostic row: {}", path.display())
            })?;
            selected_rows += 1;
            if extract_codex_shell_command(&body).is_some() {
                extracted_commands += 1;
            }
        }

        Ok(CodexScanDiagnostics {
            elapsed: started.elapsed(),
            selected_rows,
            extracted_commands,
            db_size_bytes,
        })
    }

    fn format_scan_diagnostics(diagnostics: &CodexScanDiagnostics) -> String {
        let elapsed_ms = diagnostics.elapsed.as_millis();
        let mut out = String::new();
        out.push_str(&format!(
            "database size: {} bytes\n",
            diagnostics.db_size_bytes
        ));
        out.push_str(&format!("selected rows: {}\n", diagnostics.selected_rows));
        out.push_str(&format!(
            "extracted commands: {}\n",
            diagnostics.extracted_commands
        ));
        out.push_str(&format!("elapsed: {elapsed_ms} ms\n"));
        if diagnostics.elapsed > Duration::from_secs(5) {
            out.push_str("warning: Codex scan exceeded 5 seconds; results remain valid\n");
        }
        out
    }

    fn cutoff_unix(since_days: Option<u64>) -> i64 {
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        since_days
            .map(|days| now.saturating_sub((days * 86_400) as i64))
            .unwrap_or(0)
    }

    pub(crate) fn discover_sessions_in_db(
        path: &Path,
        since_days: Option<u64>,
    ) -> Result<Vec<SessionRef>> {
        let conn = Self::open_readonly(path)?;
        Self::validate_schema(&conn)
            .with_context(|| format!("unsupported Codex database schema: {}", path.display()))?;
        let cutoff = Self::cutoff_unix(since_days);
        let mut stmt = conn
            .prepare(
                "SELECT DISTINCT thread_id FROM logs \
             WHERE thread_id IS NOT NULL AND ts >= ?1 ORDER BY thread_id",
            )
            .with_context(|| format!("failed to query Codex sessions: {}", path.display()))?;
        let rows = stmt
            .query_map(params![cutoff], |row| row.get::<_, String>(0))
            .with_context(|| format!("failed to read Codex sessions: {}", path.display()))?;
        let mut sessions = Vec::new();
        for row in rows {
            let thread_id = row
                .with_context(|| format!("failed to read Codex session row: {}", path.display()))?;
            sessions.push(SessionRef {
                provider: ProviderKind::Codex,
                id: thread_id.clone(),
                source: SessionSource::CodexThread {
                    db_path: path.to_path_buf(),
                    thread_id,
                },
            });
        }
        Ok(sessions)
    }

    fn extract_commands_from_thread(
        db_path: &Path,
        thread_id: &str,
        since_days: Option<u64>,
    ) -> Result<Vec<ExtractedCommand>> {
        let conn = Self::open_readonly(db_path)?;
        Self::validate_schema(&conn).with_context(|| {
            format!(
                "unsupported Codex database schema while reading thread {thread_id}: {}",
                db_path.display()
            )
        })?;
        let cutoff = Self::cutoff_unix(since_days);
        let mut stmt = conn
            .prepare(
                "SELECT id, ts, ts_nanos, feedback_log_body FROM logs \
             WHERE thread_id = ?1 AND ts >= ?2 \
               AND feedback_log_body LIKE '%ToolCall: shell_command%' \
             ORDER BY ts, ts_nanos, id",
            )
            .with_context(|| {
                format!(
                    "failed to query Codex commands for thread {thread_id}: {}",
                    db_path.display()
                )
            })?;
        let rows = stmt
            .query_map(params![thread_id, cutoff], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, i64>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })
            .with_context(|| {
                format!(
                    "failed to read Codex commands for thread {thread_id}: {}",
                    db_path.display()
                )
            })?;

        let mut commands = Vec::new();
        for row in rows {
            let (_id, ts, _ts_nanos, body) = row.with_context(|| {
                format!(
                    "failed to read Codex command row for thread {thread_id}: {}",
                    db_path.display()
                )
            })?;
            let Some(command) = extract_codex_shell_command(&body) else {
                continue;
            };
            let sequence_index = commands.len();
            commands.push(ExtractedCommand {
                command,
                output_len: None,
                session_id: thread_id.to_string(),
                output_content: None,
                is_error: false,
                sequence_index,
                occurred_at_unix: Some(ts),
            });
        }
        Ok(commands)
    }
}

impl SessionProvider for CodexProvider {
    fn discover_sessions(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<SessionRef>> {
        if project_filter.is_some() {
            anyhow::bail!("Codex provider does not support --project yet");
        }
        let Some(path) = self.selected_db_path() else {
            return Ok(Vec::new());
        };
        Self::discover_sessions_in_db(&path, since_days)
    }

    fn extract_commands(&self, session: &SessionRef) -> Result<Vec<ExtractedCommand>> {
        let SessionSource::CodexThread { db_path, thread_id } = &session.source else {
            anyhow::bail!("expected Codex thread session");
        };
        Self::extract_commands_from_thread(db_path, thread_id, None)
    }
}

fn home_dir() -> Option<PathBuf> {
    #[cfg(windows)]
    {
        std::env::var_os("USERPROFILE").map(PathBuf::from)
    }
    #[cfg(not(windows))]
    {
        std::env::var_os("HOME").map(PathBuf::from)
    }
}

fn extract_codex_shell_command(body: &str) -> Option<String> {
    let marker = "ToolCall: shell_command";
    let after_marker = body.split_once(marker)?.1;
    let json_start = after_marker.find('{')?;
    let json_text = balanced_json_object(&after_marker[json_start..])?;
    let value: serde_json::Value = serde_json::from_str(json_text).ok()?;
    value
        .get("command")
        .and_then(|v| v.as_str())
        .or_else(|| value.pointer("/arguments/command").and_then(|v| v.as_str()))
        .or_else(|| value.pointer("/input/command").and_then(|v| v.as_str()))
        .map(str::to_string)
}

fn balanced_json_object(input: &str) -> Option<&str> {
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;

    for (idx, ch) in input.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return Some(&input[..idx + ch.len_utf8()]);
                }
            }
            _ => {}
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::io::Write;

    fn make_jsonl(lines: &[&str]) -> tempfile::NamedTempFile {
        let mut f = tempfile::NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        f.flush().unwrap();
        f
    }

    fn create_codex_db() -> tempfile::NamedTempFile {
        let db = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(db.path()).unwrap();
        conn.execute(
            "CREATE TABLE logs (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                ts INTEGER NOT NULL,
                ts_nanos INTEGER NOT NULL,
                feedback_log_body TEXT,
                thread_id TEXT,
                process_uuid TEXT
            )",
            [],
        )
        .unwrap();
        drop(conn);
        db
    }

    fn insert_codex_log(db: &Path, ts: i64, ts_nanos: i64, thread_id: &str, body: &str) {
        let conn = Connection::open(db).unwrap();
        conn.execute(
            "INSERT INTO logs (ts, ts_nanos, feedback_log_body, thread_id, process_uuid)
             VALUES (?1, ?2, ?3, ?4, 'proc')",
            params![ts, ts_nanos, body, thread_id],
        )
        .unwrap();
    }

    #[test]
    fn test_extract_assistant_bash() {
        let jsonl = make_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_abc","name":"Bash","input":{"command":"git status"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_abc","content":"On branch master\nnothing to commit"}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands_from_path(jsonl.path()).unwrap();
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
        let cmds = provider.extract_commands_from_path(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_extract_non_message_ignored() {
        let jsonl =
            make_jsonl(&[r#"{"type":"file-history-snapshot","messageId":"abc","snapshot":{}}"#]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands_from_path(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_extract_multiple_tools() {
        let jsonl = make_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_1","name":"Bash","input":{"command":"git status"}},{"type":"tool_use","id":"toolu_2","name":"Bash","input":{"command":"git diff"}}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands_from_path(jsonl.path()).unwrap();
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
        let cmds = provider.extract_commands_from_path(jsonl.path()).unwrap();
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
    fn test_encode_path_with_spaces() {
        // Even if run on Unix, encoding should replace backslashes to match Claude's behavior
        assert_eq!(
            ClaudeProvider::encode_project_path(
                r"/home/user/projects/[QZX-7K42] - Análise Genérica de Exemplo"
            ),
            "-home-user-projects--QZX-7K42----An-lise-Gen-rica-de-Exemplo"
        );
    }

    #[test]
    fn test_discover_sessions_missing_projects_dir_returns_empty() {
        let temp_home = tempfile::tempdir().unwrap();
        let missing_projects_dir = temp_home
            .path()
            .join(crate::hooks::constants::CLAUDE_DIR)
            .join("projects");

        let sessions = ClaudeProvider::discover_sessions_in_projects_dir(
            &missing_projects_dir,
            None,
            Some(30),
        )
        .unwrap();

        assert!(sessions.is_empty());
    }

    #[test]
    fn test_discover_sessions_applies_project_filter() {
        let projects_dir = tempfile::tempdir().unwrap();
        let matching_project = projects_dir.path().join("-Users-test-rtk");
        let other_project = projects_dir.path().join("-Users-test-other");
        std::fs::create_dir_all(&matching_project).unwrap();
        std::fs::create_dir_all(&other_project).unwrap();
        std::fs::write(matching_project.join("matching.jsonl"), "").unwrap();
        std::fs::write(other_project.join("other.jsonl"), "").unwrap();

        let sessions = ClaudeProvider::discover_sessions_in_projects_dir(
            projects_dir.path(),
            Some("rtk"),
            None,
        )
        .unwrap();

        assert_eq!(sessions.len(), 1);
        assert_eq!(
            sessions[0].file_name().and_then(|name| name.to_str()),
            Some("matching.jsonl")
        );
    }

    #[test]
    fn test_discover_sessions_existing_non_directory_returns_error() {
        let projects_file = tempfile::NamedTempFile::new().unwrap();

        let err =
            ClaudeProvider::discover_sessions_in_projects_dir(projects_file.path(), None, None)
                .unwrap_err();

        assert!(err.to_string().contains("failed to read"));
    }

    #[test]
    fn test_extract_output_content() {
        let jsonl = make_jsonl(&[
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"toolu_abc","name":"Bash","input":{"command":"git commit --ammend"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"toolu_abc","content":"error: unexpected argument '--ammend'","is_error":true}]}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands_from_path(jsonl.path()).unwrap();
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
        let cmds = provider.extract_commands_from_path(jsonl.path()).unwrap();
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
        let cmds = provider.extract_commands_from_path(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 3);
        assert_eq!(cmds[0].sequence_index, 0);
        assert_eq!(cmds[1].sequence_index, 1);
        assert_eq!(cmds[2].sequence_index, 2);
        assert_eq!(cmds[0].command, "first");
        assert_eq!(cmds[1].command, "second");
        assert_eq!(cmds[2].command, "third");
    }

    #[test]
    fn codex_missing_db_returns_empty_sessions() {
        let missing = tempfile::tempdir().unwrap().path().join("missing.sqlite");
        let provider = CodexProvider::new(Some(missing));
        let sessions = provider.discover_sessions(None, Some(30)).unwrap();
        assert!(sessions.is_empty());
    }

    #[test]
    fn codex_extracts_shell_command_payload() {
        let db = create_codex_db();
        insert_codex_log(
            db.path(),
            CodexProvider::cutoff_unix(None) + 1,
            0,
            "thread-a",
            r#"ToolCall: shell_command {"command":"rtk ls"}"#,
        );

        let sessions = CodexProvider::discover_sessions_in_db(db.path(), None).unwrap();
        assert_eq!(sessions.len(), 1);
        let cmds = CodexProvider::new(Some(db.path().to_path_buf()))
            .extract_commands(&sessions[0])
            .unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "rtk ls");
        assert_eq!(cmds[0].session_id, "thread-a");
    }

    #[test]
    fn codex_skips_malformed_payload() {
        let db = create_codex_db();
        insert_codex_log(
            db.path(),
            CodexProvider::cutoff_unix(None) + 1,
            0,
            "thread-a",
            r#"ToolCall: shell_command {"command":"rtk ls"}"#,
        );
        insert_codex_log(
            db.path(),
            CodexProvider::cutoff_unix(None) + 2,
            0,
            "thread-a",
            "ToolCall: shell_command {bad json",
        );

        let sessions = CodexProvider::discover_sessions_in_db(db.path(), None).unwrap();
        let cmds = CodexProvider::new(Some(db.path().to_path_buf()))
            .extract_commands(&sessions[0])
            .unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "rtk ls");
    }

    #[test]
    fn codex_since_filter_excludes_old_rows_in_recent_db() {
        let db = create_codex_db();
        let now = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        insert_codex_log(
            db.path(),
            now - 90 * 86_400,
            0,
            "old-thread",
            r#"ToolCall: shell_command {"command":"old"}"#,
        );
        insert_codex_log(
            db.path(),
            now,
            0,
            "new-thread",
            r#"ToolCall: shell_command {"command":"new"}"#,
        );

        let sessions = CodexProvider::discover_sessions_in_db(db.path(), Some(30)).unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].id, "new-thread");
    }

    #[test]
    fn codex_groups_rows_by_thread_id() {
        let db = create_codex_db();
        let now = CodexProvider::cutoff_unix(None) + 1;
        insert_codex_log(
            db.path(),
            now,
            0,
            "thread-a",
            r#"ToolCall: shell_command {"command":"a1"}"#,
        );
        insert_codex_log(
            db.path(),
            now,
            1,
            "thread-a",
            r#"ToolCall: shell_command {"command":"a2"}"#,
        );
        insert_codex_log(
            db.path(),
            now,
            0,
            "thread-b",
            r#"ToolCall: shell_command {"command":"b1"}"#,
        );

        let sessions = CodexProvider::discover_sessions_in_db(db.path(), None).unwrap();
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].id, "thread-a");
        assert_eq!(sessions[1].id, "thread-b");
    }

    #[test]
    fn codex_orders_rows_by_timestamp_nanos_and_id() {
        let db = create_codex_db();
        let now = CodexProvider::cutoff_unix(None) + 1;
        insert_codex_log(
            db.path(),
            now,
            20,
            "thread-a",
            r#"ToolCall: shell_command {"command":"third"}"#,
        );
        insert_codex_log(
            db.path(),
            now,
            10,
            "thread-a",
            r#"ToolCall: shell_command {"command":"first"}"#,
        );
        insert_codex_log(
            db.path(),
            now,
            10,
            "thread-a",
            r#"ToolCall: shell_command {"command":"second"}"#,
        );

        let sessions = CodexProvider::discover_sessions_in_db(db.path(), None).unwrap();
        let cmds = CodexProvider::new(Some(db.path().to_path_buf()))
            .extract_commands(&sessions[0])
            .unwrap();
        let commands: Vec<_> = cmds.iter().map(|cmd| cmd.command.as_str()).collect();
        assert_eq!(commands, vec!["first", "second", "third"]);
        assert_eq!(cmds[0].sequence_index, 0);
        assert_eq!(cmds[1].sequence_index, 1);
        assert_eq!(cmds[2].sequence_index, 2);
    }

    #[test]
    fn codex_unknown_schema_reports_diagnostic() {
        let db = tempfile::NamedTempFile::new().unwrap();
        let conn = Connection::open(db.path()).unwrap();
        conn.execute("CREATE TABLE unrelated (body TEXT)", [])
            .unwrap();
        drop(conn);

        let err = CodexProvider::discover_sessions_in_db(db.path(), None).unwrap_err();
        let message = format!("{err:#}");

        assert!(message.contains("unsupported Codex database schema"));
        assert!(message.contains(&db.path().display().to_string()));
        assert!(message.contains("required logs table"));
    }

    #[test]
    fn codex_check_provider_reports_schema() {
        let db = create_codex_db();
        insert_codex_log(
            db.path(),
            CodexProvider::cutoff_unix(None) + 1,
            0,
            "thread-a",
            r#"ToolCall: shell_command {"command":"rtk ls"}"#,
        );
        insert_codex_log(
            db.path(),
            CodexProvider::cutoff_unix(None) + 2,
            0,
            "thread-a",
            "ToolCall: shell_command {bad json",
        );
        let provider = CodexProvider::new(Some(db.path().to_path_buf()));

        let output = provider.check_provider().unwrap();

        assert!(output.contains("Codex provider check"));
        assert!(output.contains(&db.path().display().to_string()));
        assert!(output.contains("logs columns:"));
        assert!(output.contains("feedback_log_body"));
        assert!(output.contains("schema: compatible"));
        assert!(output.contains("database size:"));
        assert!(output.contains("selected rows: 2"));
        assert!(output.contains("extracted commands: 1"));
        assert!(output.contains("elapsed:"));
    }

    #[test]
    fn codex_scan_diagnostics_warns_after_five_seconds() {
        let output = CodexProvider::format_scan_diagnostics(&CodexScanDiagnostics {
            elapsed: Duration::from_secs(6),
            selected_rows: 3,
            extracted_commands: 2,
            db_size_bytes: 4096,
        });

        assert!(output.contains("database size: 4096 bytes"));
        assert!(output.contains("selected rows: 3"));
        assert!(output.contains("extracted commands: 2"));
        assert!(output.contains("elapsed: 6000 ms"));
        assert!(output.contains("warning: Codex scan exceeded 5 seconds"));
    }

    #[test]
    fn codex_wal_writer_and_reader_coexist() {
        let db = create_codex_db();
        {
            let conn = Connection::open(db.path()).unwrap();
            conn.pragma_update(None, "journal_mode", "WAL").unwrap();
            insert_codex_log(
                db.path(),
                CodexProvider::cutoff_unix(None) + 1,
                0,
                "thread-a",
                r#"ToolCall: shell_command {"command":"first"}"#,
            );

            let tx = conn.unchecked_transaction().unwrap();
            tx.execute(
                "INSERT INTO logs (ts, ts_nanos, feedback_log_body, thread_id, process_uuid)
                 VALUES (?1, ?2, ?3, ?4, 'proc')",
                params![
                    CodexProvider::cutoff_unix(None) + 2,
                    0,
                    r#"ToolCall: shell_command {"command":"uncommitted"}"#,
                    "thread-a"
                ],
            )
            .unwrap();

            let sessions = CodexProvider::discover_sessions_in_db(db.path(), None).unwrap();
            assert_eq!(sessions.len(), 1);
            let cmds = CodexProvider::new(Some(db.path().to_path_buf()))
                .extract_commands(&sessions[0])
                .unwrap();
            assert_eq!(cmds.len(), 1);
            assert_eq!(cmds[0].command, "first");
        }
    }

    #[test]
    fn codex_locked_database_is_not_zero_sessions() {
        let db = create_codex_db();
        let locker = Connection::open(db.path()).unwrap();
        locker.execute("BEGIN EXCLUSIVE", []).unwrap();

        let err = CodexProvider::discover_sessions_in_db(db.path(), None).unwrap_err();
        let message = format!("{err:#}");

        assert!(message.contains(&db.path().display().to_string()));
        assert!(
            message.contains("locked") || message.contains("busy"),
            "expected lock diagnostic, got: {message}"
        );
    }
}
