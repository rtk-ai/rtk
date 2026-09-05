//! Reads Claude Code session logs from disk and streams their command history.

use crate::hooks::init::resolve_claude_dir;
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
    /// The Claude Code `tool_use_id` for this Bash call — the same id the
    /// PreToolUse hook receives, letting hook-decision logs join back to this
    /// exact transcript entry.
    pub tool_use_id: String,
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

        // `days * 86400` can overflow u64 for a large user-supplied `--since` (same
        // overflow class `core::utils::days_ago_cutoff` fixes for the hook_decisions
        // query) — use checked_mul and fall back to the epoch, which for "days ago"
        // naturally means "no lower bound", instead of panicking. Kept as its own
        // SystemTime-based implementation rather than reusing days_ago_cutoff
        // directly: this filters on file mtimes (SystemTime), not chrono
        // DateTime<Utc>, and days_ago_cutoff's MIN_UTC fallback wouldn't convert to
        // a valid SystemTime anyway. If this overflow-clamping logic needs a fix,
        // check whether the same fix applies to days_ago_cutoff too.
        let cutoff = since_days.map(|days| {
            days.checked_mul(86400)
                .and_then(|secs| SystemTime::now().checked_sub(Duration::from_secs(secs)))
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
    /// Claude Code replaces `/`, `.`, `_`, `\`, `:`, ` `, `[`, `]`, and any
    /// non-ASCII character with `-` when computing the project directory slug
    /// under `~/.claude/projects/`.
    ///
    /// `/Users/foo/bar`          → `-Users-foo-bar`
    /// `/Users/first.last/bar`   → `-Users-first-last-bar`
    /// `/home/chris/2_project`   → `-home-chris-2-project`
    /// `C:\Users\foo\bar`        → `C--Users-foo-bar`
    pub fn encode_project_path(path: &str) -> String {
        // The drive-letter `:` matters on Windows: every cwd carries one, and if
        // it isn't sanitized the slug (`C:-...`) never matches Claude's real
        // folder (`C--...`), so `rtk discover` finds zero sessions (#2919).
        const SANITIZED_CHARS: &[char] = &['/', '.', '_', '\\', ' ', '[', ']', ':'];

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
        Self::discover_sessions_in_projects_dir(&projects_dir, project_filter, since_days)
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
                tool_use_id: tool_id,
                output_content,
                is_error,
                sequence_index,
            });
        }

        Ok(commands)
    }
}

impl CodexProvider {
    fn codex_dir() -> Result<PathBuf> {
        if let Some(home) = std::env::var_os("CODEX_HOME").filter(|value| !value.is_empty()) {
            return Ok(PathBuf::from(home));
        }
        dirs::home_dir()
            .map(|home| home.join(".codex"))
            .context("could not determine Codex directory")
    }

    fn sessions_dir() -> Result<PathBuf> {
        Ok(Self::codex_dir()?.join("sessions"))
    }
}

impl SessionProvider for CodexProvider {
    fn discover_sessions(
        &self,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        let sessions_dir = Self::sessions_dir()?;
        if !sessions_dir.exists() {
            return Ok(Vec::new());
        }
        let cutoff = since_days.and_then(|days| {
            days.checked_mul(86_400)
                .and_then(|seconds| SystemTime::now().checked_sub(Duration::from_secs(seconds)))
        });
        let mut result = Vec::new();
        for entry in WalkDir::new(sessions_dir)
            .follow_links(false)
            .into_iter()
            .filter_map(|entry| entry.ok())
        {
            let path = entry.path();
            if path.extension().and_then(|extension| extension.to_str()) != Some("jsonl") {
                continue;
            }
            if let Some(filter) = project_filter {
                let path_text = path.to_string_lossy();
                if !path_text.contains(filter) {
                    continue;
                }
            }
            if let Some(cutoff) = cutoff {
                if fs::metadata(path)
                    .and_then(|metadata| metadata.modified())
                    .is_ok_and(|modified| modified < cutoff)
                {
                    continue;
                }
            }
            result.push(path.to_path_buf());
        }
        Ok(result)
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        let file = fs::File::open(path)
            .with_context(|| format!("failed to open Codex session {}", path.display()))?;
        let reader = BufReader::new(file);
        let session_id = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("unknown")
            .to_string();
        let mut pending = Vec::new();
        let mut results: HashMap<String, (usize, String, bool)> = HashMap::new();
        let mut sequence_index = 0usize;

        for line in reader.lines() {
            let line = match line {
                Ok(line) => line,
                Err(_) => continue,
            };
            let value: serde_json::Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(_) => continue,
            };
            collect_codex_records(&value, &mut pending, &mut results, &mut sequence_index);
        }

        Ok(pending
            .into_iter()
            .map(|(tool_use_id, command, sequence_index)| {
                let (output_len, output_content, is_error) = results
                    .get(&tool_use_id)
                    .map(|(length, content, error)| (Some(*length), Some(content.clone()), *error))
                    .unwrap_or((None, None, false));
                ExtractedCommand {
                    command,
                    output_len,
                    session_id: session_id.clone(),
                    tool_use_id,
                    output_content,
                    is_error,
                    sequence_index,
                }
            })
            .collect())
    }
}

fn collect_codex_records(
    value: &serde_json::Value,
    pending: &mut Vec<(String, String, usize)>,
    results: &mut HashMap<String, (usize, String, bool)>,
    sequence_index: &mut usize,
) {
    let Some(object) = value.as_object() else {
        if let Some(array) = value.as_array() {
            for child in array {
                collect_codex_records(child, pending, results, sequence_index);
            }
        }
        return;
    };
    let record_type = object
        .get("type")
        .and_then(|value| value.as_str())
        .unwrap_or("");
    let call_id = object
        .get("call_id")
        .or_else(|| object.get("callId"))
        .or_else(|| object.get("id"))
        .and_then(|value| value.as_str());

    if matches!(
        record_type,
        "function_call" | "tool_call" | "function_call_item" | "tool_use"
    ) {
        let name = object.get("name").and_then(|value| value.as_str());
        if name
            .is_some_and(|name| matches!(name, "shell_command" | "run_command" | "Bash" | "bash"))
        {
            let command = object
                .get("arguments")
                .or_else(|| object.get("input"))
                .and_then(codex_command)
                .filter(|command| !command.trim().is_empty());
            if let (Some(call_id), Some(command)) = (call_id, command) {
                pending.push((call_id.to_string(), command, *sequence_index));
                *sequence_index += 1;
            }
        }
    }
    if matches!(
        record_type,
        "function_call_output" | "tool_result" | "function_output"
    ) {
        if let Some(call_id) = call_id {
            let output = object
                .get("output")
                .or_else(|| object.get("content"))
                .map(codex_output_text)
                .unwrap_or_default();
            let is_error = object
                .get("is_error")
                .or_else(|| object.get("isError"))
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            results.insert(
                call_id.to_string(),
                (output.len(), output.chars().take(1000).collect(), is_error),
            );
        }
    }

    for child in object.values() {
        if child.is_object() || child.is_array() {
            collect_codex_records(child, pending, results, sequence_index);
        }
    }
}

fn codex_command(value: &serde_json::Value) -> Option<String> {
    match value {
        serde_json::Value::String(text) => serde_json::from_str::<serde_json::Value>(text)
            .ok()
            .and_then(|parsed| codex_command(&parsed))
            .or_else(|| Some(text.clone())),
        serde_json::Value::Object(object) => object
            .get("command")
            .and_then(|command| command.as_str())
            .map(str::to_string),
        _ => None,
    }
}

fn codex_output_text(value: &serde_json::Value) -> String {
    value
        .as_str()
        .map(str::to_string)
        .or_else(|| serde_json::to_string(value).ok())
        .unwrap_or_default()
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
    fn test_extract_codex_shell_call_and_result() {
        let jsonl = make_jsonl(&[
            r#"{"type":"function_call","call_id":"call_1","name":"shell_command","arguments":"{\"command\":\"git status\"}"}"#,
            r#"{"type":"function_call_output","call_id":"call_1","output":"On branch main\nnothing to commit","is_error":false}"#,
        ]);
        let provider = CodexProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "git status");
        assert_eq!(
            cmds[0].output_len,
            Some("On branch main\nnothing to commit".len())
        );
        assert!(!cmds[0].is_error);
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
        // Windows backslashes AND the drive-letter colon are replaced with '-'.
        // A real `C:\Users\foo\bar` dir lands in ~/.claude/projects/C--Users-foo-bar,
        // so keeping the colon (C:-...) made `rtk discover` find zero sessions on
        // Windows (#2919).
        assert_eq!(
            ClaudeProvider::encode_project_path(r"C:\Users\foo\bar"),
            "C--Users-foo-bar"
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
}
