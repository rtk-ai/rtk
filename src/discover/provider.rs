//! Reads agent session logs and streams their command history.

use crate::hooks::constants::CLAUDE_DIR;
use anyhow::{Context, Result};
use chrono::DateTime;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

const OPENCODE_DB_RELATIVE_PATH: &str = ".local/share/opencode/opencode.db";
const OPENCODE_SESSION_PREFIX: &str = "opencode-session:";
const CODEX_SESSIONS_RELATIVE_PATH: &str = ".codex/sessions";
const COPILOT_SESSIONS_RELATIVE_PATH: &str = ".copilot/session-state";
const CODEX_ROLLOUT_PREFIX: &str = "rollout-";
const COPILOT_EVENTS_FILE: &str = "events.jsonl";

const CODEX_TIMESTAMP_POINTERS: [&str; 3] =
    ["/timestamp", "/payload/timestamp", "/payload/created_at"];
const COPILOT_START_TIMESTAMP_POINTERS: [&str; 3] = ["/startTime", "/data/startTime", "/timestamp"];
const COPILOT_OUTPUT_POINTERS: [&str; 8] = [
    "/data/output",
    "/output",
    "/data/result/content",
    "/result/content",
    "/data/result/output",
    "/result/output",
    "/data/result",
    "/result",
];

const SUPPORTED_PROVIDERS: [ProviderId; 4] = [
    ProviderId::ClaudeCode,
    ProviderId::OpenCode,
    ProviderId::CodexCli,
    ProviderId::CopilotCli,
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderId {
    ClaudeCode,
    OpenCode,
    CodexCli,
    CopilotCli,
}

impl ProviderId {
    pub fn display_name(&self) -> &'static str {
        match self {
            ProviderId::ClaudeCode => "Claude Code",
            ProviderId::OpenCode => "OpenCode",
            ProviderId::CodexCli => "Codex CLI",
            ProviderId::CopilotCli => "Copilot CLI",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiscoveredSession {
    pub provider: ProviderId,
    pub session_id: String,
    pub path: PathBuf,
    pub updated_unix: i64,
}

#[derive(Debug, Default)]
pub struct ProviderDiscovery {
    pub sessions: Vec<DiscoveredSession>,
    pub available_sources: usize,
    pub unavailable_sources: Vec<(ProviderId, String)>,
}

pub fn all_provider_ids() -> &'static [ProviderId] {
    &SUPPORTED_PROVIDERS
}

pub fn supported_providers_display() -> &'static str {
    "Claude Code, OpenCode, Codex CLI, Copilot CLI"
}

pub fn discover_provider_sessions(
    project_filter: Option<&str>,
    since_days: Option<u64>,
) -> ProviderDiscovery {
    let mut discovery = ProviderDiscovery::default();

    for provider_id in all_provider_ids() {
        let result = match *provider_id {
            ProviderId::ClaudeCode => {
                ClaudeProvider.discover_with_metadata(project_filter, since_days)
            }
            ProviderId::OpenCode => {
                OpenCodeProvider::default().discover_with_metadata(project_filter, since_days)
            }
            ProviderId::CodexCli => {
                CodexProvider.discover_with_metadata(project_filter, since_days)
            }
            ProviderId::CopilotCli => {
                CopilotCliProvider.discover_with_metadata(project_filter, since_days)
            }
        };

        match result {
            Ok(mut sessions) => {
                discovery.available_sources += 1;
                discovery.sessions.append(&mut sessions);
            }
            Err(error) => {
                discovery
                    .unavailable_sources
                    .push((*provider_id, error.to_string()));
            }
        }
    }

    discovery
}

pub fn extract_commands_for_session(session: &DiscoveredSession) -> Result<Vec<ExtractedCommand>> {
    match session.provider {
        ProviderId::ClaudeCode => ClaudeProvider.extract_commands(&session.path),
        ProviderId::OpenCode => OpenCodeProvider::default().extract_commands(&session.path),
        ProviderId::CodexCli => CodexProvider.extract_commands(&session.path),
        ProviderId::CopilotCli => CopilotCliProvider.extract_commands(&session.path),
    }
}

fn cutoff_unix_seconds(since_days: Option<u64>) -> Option<i64> {
    since_days.map(|days| {
        let cutoff = SystemTime::now()
            .checked_sub(Duration::from_secs(days * 86400))
            .unwrap_or(SystemTime::UNIX_EPOCH);
        cutoff
            .duration_since(SystemTime::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64
    })
}

fn system_time_to_unix_seconds(time: SystemTime) -> i64 {
    time.duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn file_mtime_unix_seconds(path: &Path) -> i64 {
    fs::metadata(path)
        .and_then(|metadata| metadata.modified())
        .map(system_time_to_unix_seconds)
        .unwrap_or(0)
}

fn json_string(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value
        .pointer(pointer)
        .and_then(|v| v.as_str())
        .map(str::to_string)
}

fn json_unix_timestamp(value: &serde_json::Value, pointer: &str) -> Option<i64> {
    value.pointer(pointer).and_then(value_to_unix_seconds)
}

fn json_text(value: &serde_json::Value, pointer: &str) -> Option<String> {
    value.pointer(pointer).and_then(|v| {
        if v.is_null() {
            None
        } else if let Some(text) = v.as_str() {
            Some(text.to_string())
        } else {
            Some(v.to_string())
        }
    })
}

fn first_json_text(value: &serde_json::Value, pointers: &[&str]) -> Option<String> {
    pointers
        .iter()
        .find_map(|pointer| json_text(value, pointer))
}

fn first_json_unix_timestamp(value: &serde_json::Value, pointers: &[&str]) -> Option<i64> {
    pointers
        .iter()
        .find_map(|pointer| json_unix_timestamp(value, pointer))
}

fn value_to_i64(value: &serde_json::Value) -> Option<i64> {
    if let Some(v) = value.as_i64() {
        return Some(v);
    }
    if let Some(v) = value.as_u64() {
        return Some(v as i64);
    }
    value.as_str().and_then(|s| s.parse::<i64>().ok())
}

fn value_to_unix_seconds(value: &serde_json::Value) -> Option<i64> {
    value_to_i64(value)
        .map(OpenCodeProvider::to_unix_seconds)
        .or_else(|| {
            value
                .as_str()
                .and_then(|text| DateTime::parse_from_rfc3339(text).ok())
                .map(|parsed| parsed.timestamp())
        })
}

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

pub struct CodexProvider;

pub struct CopilotCliProvider;

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

    fn discover_with_metadata(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<DiscoveredSession>> {
        let paths = self.discover_sessions(project_filter, since_days)?;
        let mut sessions = Vec::with_capacity(paths.len());

        for path in paths {
            let session_id = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("unknown")
                .to_string();

            sessions.push(DiscoveredSession {
                provider: ProviderId::ClaudeCode,
                session_id,
                updated_unix: file_mtime_unix_seconds(&path),
                path,
            });
        }

        Ok(sessions)
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

    fn discover_with_metadata(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<DiscoveredSession>> {
        let db_path = self.db_path()?;
        let conn = rusqlite::Connection::open(&db_path)
            .with_context(|| format!("failed to open {}", db_path.display()))?;

        let cutoff_unix = cutoff_unix_seconds(since_days);

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
            let updated_unix = OpenCodeProvider::to_unix_seconds(time_updated);

            if let Some(filter) = project_filter {
                if !directory.contains(filter) {
                    continue;
                }
            }

            if let Some(cutoff) = cutoff_unix {
                if updated_unix < cutoff {
                    continue;
                }
            }

            sessions.push(DiscoveredSession {
                provider: ProviderId::OpenCode,
                session_id: id.clone(),
                path: OpenCodeProvider::session_path_from_id(&id),
                updated_unix,
            });
        }

        Ok(sessions)
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

impl CodexProvider {
    fn sessions_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("could not determine home directory")?;
        let dir = home.join(CODEX_SESSIONS_RELATIVE_PATH);
        if !dir.exists() {
            anyhow::bail!(
                "Codex CLI sessions directory not found: {}\nMake sure Codex CLI has been used at least once.",
                dir.display()
            );
        }
        Ok(dir)
    }

    fn parse_discovery_metadata(path: &Path) -> (Option<String>, Option<i64>) {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return (None, None),
        };

        let reader = BufReader::new(file);
        let mut cwd = None;
        let mut latest_unix: Option<i64> = None;

        for line in reader.lines().map_while(|line| line.ok()) {
            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };

            if entry.get("type").and_then(|t| t.as_str()) == Some("session_meta") && cwd.is_none() {
                cwd = json_string(&entry, "/payload/cwd");
            }

            if let Some(unix_ts) = first_json_unix_timestamp(&entry, &CODEX_TIMESTAMP_POINTERS) {
                latest_unix = Some(latest_unix.map_or(unix_ts, |current| current.max(unix_ts)));
            }
        }

        (cwd, latest_unix)
    }

    fn discover_with_metadata(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<DiscoveredSession>> {
        let sessions_dir = Self::sessions_dir()?;
        self.discover_in_dir(&sessions_dir, project_filter, since_days)
    }

    fn discover_in_dir(
        &self,
        sessions_dir: &Path,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<DiscoveredSession>> {
        let cutoff_unix = cutoff_unix_seconds(since_days);
        let mut sessions = Vec::new();

        for entry in WalkDir::new(sessions_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
                continue;
            }

            let file_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if !file_name.starts_with(CODEX_ROLLOUT_PREFIX) {
                continue;
            }

            let (cwd, transcript_unix) = Self::parse_discovery_metadata(path);

            if let Some(filter) = project_filter {
                if !cwd.as_deref().unwrap_or_default().contains(filter) {
                    continue;
                }
            }

            let mtime_unix = file_mtime_unix_seconds(path);
            let updated_unix = transcript_unix.unwrap_or(mtime_unix);

            if let Some(cutoff) = cutoff_unix {
                if updated_unix < cutoff && mtime_unix < cutoff {
                    continue;
                }
            }

            let session_id = path
                .parent()
                .and_then(|parent| parent.file_name())
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_string();

            sessions.push(DiscoveredSession {
                provider: ProviderId::CodexCli,
                session_id,
                path: path.to_path_buf(),
                updated_unix,
            });
        }

        Ok(sessions)
    }
}

impl SessionProvider for CodexProvider {
    fn discover_sessions(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        Ok(self
            .discover_with_metadata(project_filter, since_days)?
            .into_iter()
            .map(|session| session.path)
            .collect())
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        let file =
            fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let reader = BufReader::new(file);

        let session_id = path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mut pending_calls: Vec<(String, String, usize)> = Vec::new();
        let mut call_outputs: HashMap<String, (usize, String, bool)> = HashMap::new();
        let mut sequence_index = 0usize;

        for line in reader.lines().map_while(|line| line.ok()) {
            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };

            if entry.get("type").and_then(|t| t.as_str()) != Some("response_item") {
                continue;
            }

            let payload = match entry.get("payload") {
                Some(payload) => payload,
                None => continue,
            };

            match payload.get("type").and_then(|t| t.as_str()) {
                Some("function_call") => {
                    if payload.get("name").and_then(|n| n.as_str()) != Some("exec_command") {
                        continue;
                    }

                    let call_id = match payload
                        .get("call_id")
                        .and_then(|id| id.as_str())
                        .or_else(|| payload.get("id").and_then(|id| id.as_str()))
                    {
                        Some(call_id) => call_id.to_string(),
                        None => continue,
                    };

                    let arguments = match payload.get("arguments").and_then(|args| args.as_str()) {
                        Some(arguments) => arguments,
                        None => continue,
                    };

                    let args_json: serde_json::Value = match serde_json::from_str(arguments) {
                        Ok(args_json) => args_json,
                        Err(_) => continue,
                    };

                    let command = match args_json.get("cmd").and_then(|cmd| cmd.as_str()) {
                        Some(command) => command.to_string(),
                        None => continue,
                    };

                    pending_calls.push((call_id, command, sequence_index));
                    sequence_index += 1;
                }
                Some("function_call_output") => {
                    let call_id = match payload.get("call_id").and_then(|id| id.as_str()) {
                        Some(call_id) => call_id,
                        None => continue,
                    };

                    let output_value = payload
                        .get("output")
                        .or_else(|| payload.get("content"))
                        .cloned()
                        .unwrap_or(serde_json::Value::Null);

                    let output_content = if let Some(content) = output_value.as_str() {
                        content.to_string()
                    } else if output_value.is_null() {
                        String::new()
                    } else {
                        output_value.to_string()
                    };

                    let is_error = payload
                        .get("is_error")
                        .and_then(|flag| flag.as_bool())
                        .unwrap_or(false)
                        || payload.get("error").is_some()
                        || payload.get("status").and_then(|status| status.as_str())
                            == Some("error");

                    call_outputs.insert(
                        call_id.to_string(),
                        (
                            output_content.len(),
                            output_content.chars().take(1000).collect(),
                            is_error,
                        ),
                    );
                }
                _ => {}
            }
        }

        let mut commands = Vec::new();
        for (call_id, command, sequence_index) in pending_calls {
            let (output_len, output_content, is_error) = call_outputs
                .get(&call_id)
                .map(|(len, content, is_error)| (Some(*len), Some(content.clone()), *is_error))
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

impl CopilotCliProvider {
    fn sessions_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("could not determine home directory")?;
        let dir = home.join(COPILOT_SESSIONS_RELATIVE_PATH);
        if !dir.exists() {
            anyhow::bail!(
                "Copilot CLI sessions directory not found: {}\nMake sure Copilot CLI has been used at least once.",
                dir.display()
            );
        }
        Ok(dir)
    }

    fn parse_discovery_metadata(path: &Path) -> (Option<String>, Option<i64>) {
        let file = match fs::File::open(path) {
            Ok(file) => file,
            Err(_) => return (None, None),
        };

        let reader = BufReader::new(file);
        let mut cwd = None;
        let mut start_unix = None;

        for line in reader.lines().map_while(|line| line.ok()) {
            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };

            if entry.get("type").and_then(|t| t.as_str()) == Some("session.start") {
                if cwd.is_none() {
                    cwd = json_string(&entry, "/data/context/cwd");
                }
                if start_unix.is_none() {
                    start_unix =
                        first_json_unix_timestamp(&entry, &COPILOT_START_TIMESTAMP_POINTERS);
                }
            }
        }

        (cwd, start_unix)
    }

    fn discover_with_metadata(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<DiscoveredSession>> {
        let sessions_dir = Self::sessions_dir()?;
        self.discover_in_dir(&sessions_dir, project_filter, since_days)
    }

    fn discover_in_dir(
        &self,
        sessions_dir: &Path,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<DiscoveredSession>> {
        let cutoff_unix = cutoff_unix_seconds(since_days);
        let mut sessions = Vec::new();

        let entries = fs::read_dir(sessions_dir)
            .with_context(|| format!("failed to read {}", sessions_dir.display()))?;

        for entry in entries.flatten() {
            let session_dir = entry.path();
            if !session_dir.is_dir() {
                continue;
            }

            let events_path = session_dir.join(COPILOT_EVENTS_FILE);
            if !events_path.exists() {
                continue;
            }

            let (cwd, start_unix) = Self::parse_discovery_metadata(&events_path);

            if let Some(filter) = project_filter {
                if !cwd.as_deref().unwrap_or_default().contains(filter) {
                    continue;
                }
            }

            let mtime_unix = file_mtime_unix_seconds(&events_path);
            let updated_unix = start_unix.unwrap_or(mtime_unix);

            if let Some(cutoff) = cutoff_unix {
                if updated_unix < cutoff && mtime_unix < cutoff {
                    continue;
                }
            }

            let session_id = session_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("unknown")
                .to_string();

            sessions.push(DiscoveredSession {
                provider: ProviderId::CopilotCli,
                session_id,
                path: events_path,
                updated_unix,
            });
        }

        Ok(sessions)
    }
}

impl SessionProvider for CopilotCliProvider {
    fn discover_sessions(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        Ok(self
            .discover_with_metadata(project_filter, since_days)?
            .into_iter()
            .map(|session| session.path)
            .collect())
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        let file =
            fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let reader = BufReader::new(file);

        let session_id = path
            .parent()
            .and_then(|parent| parent.file_name())
            .and_then(|name| name.to_str())
            .unwrap_or("unknown")
            .to_string();

        let mut pending_calls: Vec<(String, String, usize)> = Vec::new();
        let mut call_results: HashMap<String, (Option<usize>, Option<String>, bool)> =
            HashMap::new();
        let mut sequence_index = 0usize;

        for line in reader.lines().map_while(|line| line.ok()) {
            let entry: serde_json::Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };

            match entry.get("type").and_then(|t| t.as_str()) {
                Some("tool.execution_start") => {
                    if json_string(&entry, "/data/toolName").as_deref() != Some("bash") {
                        continue;
                    }

                    let command = match json_string(&entry, "/data/arguments/command") {
                        Some(command) => command,
                        None => continue,
                    };

                    let tool_call_id = json_string(&entry, "/data/toolCallId")
                        .or_else(|| json_string(&entry, "/toolCallId"));
                    let call_id = match tool_call_id {
                        Some(call_id) => call_id,
                        None => continue,
                    };

                    pending_calls.push((call_id, command, sequence_index));
                    sequence_index += 1;
                }
                Some("tool.execution_complete") => {
                    let call_id = match json_string(&entry, "/data/toolCallId")
                        .or_else(|| json_string(&entry, "/toolCallId"))
                    {
                        Some(call_id) => call_id,
                        None => continue,
                    };

                    let success = entry
                        .pointer("/data/success")
                        .and_then(|flag| flag.as_bool())
                        .or_else(|| entry.pointer("/success").and_then(|flag| flag.as_bool()));

                    let output = first_json_text(&entry, &COPILOT_OUTPUT_POINTERS);

                    let output_len = output.as_ref().map(|content| content.len());
                    let output_content = output.map(|content| content.chars().take(1000).collect());
                    let is_error = match success {
                        Some(success) => !success,
                        None => {
                            entry.pointer("/data/error").is_some()
                                || entry.pointer("/error").is_some()
                        }
                    };

                    call_results.insert(call_id, (output_len, output_content, is_error));
                }
                _ => {}
            }
        }

        let mut commands = Vec::new();
        for (call_id, command, sequence_index) in pending_calls {
            let (output_len, output_content, is_error) = call_results
                .get(&call_id)
                .cloned()
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
        Ok(self
            .discover_with_metadata(project_filter, since_days)?
            .into_iter()
            .map(|session| session.path)
            .collect())
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
    use chrono::{SecondsFormat, Utc};
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

    fn to_rfc3339_millis(ts: i64) -> String {
        chrono::DateTime::<Utc>::from_timestamp(ts, 0)
            .unwrap()
            .to_rfc3339_opts(SecondsFormat::Millis, true)
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

    #[test]
    fn test_opencode_extract_commands_keeps_latest_row_per_call() {
        let (_temp, db_path) = setup_opencode_db();
        let conn = Connection::open(&db_path).unwrap();
        let t = now_ms();

        conn.execute(
            "INSERT INTO session (id, directory, time_updated) VALUES (?1, ?2, ?3)",
            params!["ses_latest", "/home/user/code/rtk", t],
        )
        .unwrap();

        let running = r#"{"type":"tool","tool":"bash","callID":"call_1","state":{"status":"running","input":{"command":"npm test"}}}"#;
        let completed = r#"{"type":"tool","tool":"bash","callID":"call_1","state":{"status":"completed","input":{"command":"npm test"},"output":"pass"}}"#;
        let overwritten = r#"{"type":"tool","tool":"bash","callID":"call_1","state":{"status":"completed","input":{"command":"npm test"},"output":"pass 2"}}"#;

        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["p1", "ses_latest", t, running],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["p2", "ses_latest", t + 1, completed],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["p3", "ses_latest", t + 2, overwritten],
        )
        .unwrap();

        let provider = OpenCodeProvider {
            db_path: Some(db_path),
        };
        let cmds = provider
            .extract_commands(Path::new("opencode-session:ses_latest"))
            .unwrap();

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "npm test");
        assert_eq!(cmds[0].output_content.as_deref(), Some("pass 2"));
    }

    #[test]
    fn test_opencode_extract_commands_handles_running_without_output() {
        let (_temp, db_path) = setup_opencode_db();
        let conn = Connection::open(&db_path).unwrap();
        let t = now_ms();

        conn.execute(
            "INSERT INTO session (id, directory, time_updated) VALUES (?1, ?2, ?3)",
            params!["ses_running", "/home/user/code/rtk", t],
        )
        .unwrap();

        let running = r#"{"type":"tool","tool":"bash","callID":"call_1","state":{"status":"running","input":{"command":"cargo test"}}}"#;
        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["p1", "ses_running", t, running],
        )
        .unwrap();

        let provider = OpenCodeProvider {
            db_path: Some(db_path),
        };
        let cmds = provider
            .extract_commands(Path::new("opencode-session:ses_running"))
            .unwrap();

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "cargo test");
        assert_eq!(cmds[0].output_len, None);
        assert_eq!(cmds[0].output_content, None);
    }

    #[test]
    fn test_opencode_extract_commands_non_bash_only_is_empty() {
        let (_temp, db_path) = setup_opencode_db();
        let conn = Connection::open(&db_path).unwrap();
        let t = now_ms();

        conn.execute(
            "INSERT INTO session (id, directory, time_updated) VALUES (?1, ?2, ?3)",
            params!["ses_nonbash", "/home/user/code/rtk", t],
        )
        .unwrap();

        let read_data = r#"{"type":"tool","tool":"read","callID":"call_1","state":{"status":"completed","input":{"filePath":"/tmp/a"},"output":"abc"}}"#;
        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["p1", "ses_nonbash", t, read_data],
        )
        .unwrap();

        let provider = OpenCodeProvider {
            db_path: Some(db_path),
        };
        let cmds = provider
            .extract_commands(Path::new("opencode-session:ses_nonbash"))
            .unwrap();
        assert!(cmds.is_empty());
    }

    #[test]
    fn test_opencode_extract_commands_empty_command_session_is_empty() {
        let (_temp, db_path) = setup_opencode_db();
        let conn = Connection::open(&db_path).unwrap();
        let t = now_ms();

        conn.execute(
            "INSERT INTO session (id, directory, time_updated) VALUES (?1, ?2, ?3)",
            params!["ses_empty", "/home/user/code/rtk", t],
        )
        .unwrap();

        let empty_cmd = r#"{"type":"tool","tool":"bash","callID":"call_1","state":{"status":"completed","input":{"command":""},"output":""}}"#;
        conn.execute(
            "INSERT INTO part (id, session_id, time_updated, data) VALUES (?1, ?2, ?3, ?4)",
            params!["p1", "ses_empty", t, empty_cmd],
        )
        .unwrap();

        let provider = OpenCodeProvider {
            db_path: Some(db_path),
        };
        let cmds = provider
            .extract_commands(Path::new("opencode-session:ses_empty"))
            .unwrap();
        assert!(cmds.is_empty());
    }

    #[test]
    fn test_codex_extract_commands_exec_command_only() {
        let jsonl = make_jsonl(&[
            r#"{"type":"response_item","payload":{"type":"function_call","id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"pnpm test\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","id":"call_2","name":"read_file","arguments":"{\"path\":\"README.md\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"ok","is_error":false}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_2","output":"ignored"}}"#,
        ]);

        let provider = CodexProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();

        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "pnpm test");
        assert_eq!(cmds[0].output_content.as_deref(), Some("ok"));
        assert!(!cmds[0].is_error);
    }

    #[test]
    fn test_codex_discover_in_dir_filters_project_and_since() {
        let temp = tempfile::tempdir().unwrap();
        let old_session = temp.path().join("old-session");
        let new_session = temp.path().join("new-session");
        fs::create_dir_all(&old_session).unwrap();
        fs::create_dir_all(&new_session).unwrap();

        let old = old_session.join("rollout-old.jsonl");
        let mut old_file = fs::File::create(&old).unwrap();
        old_file
            .write_all(
                b"{\"type\":\"session_meta\",\"payload\":{\"cwd\":\"/tmp/other\",\"created_at\":\"1970-01-01T00:00:01Z\"}}\n",
            )
            .unwrap();

        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let new = new_session.join("rollout-new.jsonl");
        let mut new_file = fs::File::create(&new).unwrap();
        writeln!(
            new_file,
            "{{\"type\":\"session_meta\",\"payload\":{{\"cwd\":\"/tmp/rtk\",\"created_at\":\"{}\"}}}}",
            to_rfc3339_millis(now_unix)
        )
        .unwrap();

        let provider = CodexProvider;
        let sessions = provider
            .discover_in_dir(temp.path(), Some("rtk"), Some(30))
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider, ProviderId::CodexCli);
        assert_eq!(sessions[0].session_id, "new-session");
    }

    #[test]
    fn test_copilot_extract_commands_pairs_start_and_complete() {
        let jsonl = make_jsonl(&[
            r#"{"type":"tool.execution_start","data":{"toolName":"bash","toolCallId":"tc_1","arguments":{"command":"npm run build"}}}"#,
            r#"{"type":"tool.execution_start","data":{"toolName":"read","toolCallId":"tc_2","arguments":{"path":"README.md"}}}"#,
            r#"{"type":"tool.execution_complete","data":{"toolCallId":"tc_1","success":false,"error":{"code":"ERR"},"result":{"content":"failed"}}}"#,
        ]);

        let provider = CopilotCliProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "npm run build");
        assert_eq!(cmds[0].output_content.as_deref(), Some("failed"));
        assert!(cmds[0].is_error);
    }

    #[test]
    fn test_copilot_discover_in_dir_filters_project_and_since() {
        let temp = tempfile::tempdir().unwrap();
        let session_a = temp.path().join("session-a");
        let session_b = temp.path().join("session-b");
        fs::create_dir_all(&session_a).unwrap();
        fs::create_dir_all(&session_b).unwrap();

        let events_a = session_a.join("events.jsonl");
        let mut file_a = fs::File::create(&events_a).unwrap();
        file_a
            .write_all(
                b"{\"type\":\"session.start\",\"data\":{\"context\":{\"cwd\":\"/tmp/other\"},\"startTime\":\"1970-01-01T00:00:01Z\"}}\n",
            )
            .unwrap();

        let now_unix = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;
        let events_b = session_b.join("events.jsonl");
        let mut file_b = fs::File::create(&events_b).unwrap();
        writeln!(
            file_b,
            "{{\"type\":\"session.start\",\"data\":{{\"context\":{{\"cwd\":\"/tmp/rtk\"}}}},\"timestamp\":\"{}\"}}",
            to_rfc3339_millis(now_unix)
        )
        .unwrap();

        let provider = CopilotCliProvider;
        let sessions = provider
            .discover_in_dir(temp.path(), Some("rtk"), Some(30))
            .unwrap();
        assert_eq!(sessions.len(), 1);
        assert_eq!(sessions[0].provider, ProviderId::CopilotCli);
        assert_eq!(sessions[0].session_id, "session-b");
    }

    #[test]
    fn test_extract_commands_for_session_dispatches_by_provider() {
        let jsonl = make_jsonl(&[
            r#"{"type":"response_item","payload":{"type":"function_call","id":"call_1","name":"exec_command","arguments":"{\"cmd\":\"git status\"}"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"clean"}}"#,
        ]);

        let session = DiscoveredSession {
            provider: ProviderId::CodexCli,
            session_id: "abc123".to_string(),
            path: jsonl.path().to_path_buf(),
            updated_unix: now_ms() / 1000,
        };

        let cmds = extract_commands_for_session(&session).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "git status");
    }
}
