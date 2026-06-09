//! Reads Claude Code and Codex session logs from disk and streams their command history.

use crate::hooks::constants::CLAUDE_DIR;
use anyhow::{Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};
use walkdir::WalkDir;

const CODEX_DIR: &str = ".codex";

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
    fn home_dir() -> Result<PathBuf> {
        dirs::home_dir().context("could not determine home directory")
    }

    /// Get the base directory for Claude Code projects.
    fn projects_dir() -> Result<PathBuf> {
        let home = dirs::home_dir().context("could not determine home directory")?;
        Ok(Self::projects_dir_for_home(&home))
    }

    fn projects_dir_for_home(home: &Path) -> PathBuf {
        home.join(CLAUDE_DIR).join("projects")
    }

    /// Get the known Codex session roots.
    fn codex_session_roots() -> Result<Vec<PathBuf>> {
        let codex_dir = Self::home_dir()?.join(CODEX_DIR);
        Ok(vec![
            codex_dir.join("sessions"),
            codex_dir.join("archived_sessions"),
        ])
    }

    fn matches_cutoff(path: &Path, cutoff: Option<SystemTime>) -> bool {
        let Some(cutoff_time) = cutoff else {
            return true;
        };

        fs::metadata(path)
            .and_then(|m| m.modified())
            .map(|mtime| mtime >= cutoff_time)
            .unwrap_or(true)
    }

    fn codex_session_cwd(path: &Path) -> Option<String> {
        let file = fs::File::open(path).ok()?;
        let reader = BufReader::new(file);

        for line in reader.lines().take(20) {
            let line = line.ok()?;
            if !line.contains("\"session_meta\"") || !line.contains("\"cwd\"") {
                continue;
            }

            let entry: Value = serde_json::from_str(&line).ok()?;
            if entry.get("type").and_then(|t| t.as_str()) != Some("session_meta") {
                continue;
            }

            if let Some(cwd) = entry.pointer("/payload/cwd").and_then(|c| c.as_str()) {
                return Some(cwd.to_string());
            }
        }

        None
    }

    fn codex_matches_project_filter(path: &Path, filter: &str) -> bool {
        Self::codex_session_cwd(path)
            .map(|cwd| cwd.contains(filter) || Self::encode_project_path(&cwd).contains(filter))
            .unwrap_or(false)
    }

    fn output_from_value(value: Option<&Value>) -> (usize, String) {
        let text = match value {
            Some(Value::String(s)) => s.clone(),
            Some(Value::Null) | None => String::new(),
            Some(other) => other.to_string(),
        };
        let preview: String = text.chars().take(1000).collect();
        (text.len(), preview)
    }

    fn cutoff_since_days(since_days: Option<u64>) -> Option<SystemTime> {
        since_days.map(|days| {
            SystemTime::now()
                .checked_sub(Duration::from_secs(days * 86400))
                .unwrap_or(SystemTime::UNIX_EPOCH)
        })
    }

    fn discover_sessions_from_roots(
        projects_dir: &Path,
        codex_roots: &[PathBuf],
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        let cutoff = Self::cutoff_since_days(since_days);
        let mut sessions = Vec::new();

        if projects_dir
            .try_exists()
            .with_context(|| format!("failed to access {}", projects_dir.display()))?
        {
            let entries = fs::read_dir(projects_dir)
                .with_context(|| format!("failed to read {}", projects_dir.display()))?;

            for entry in entries.flatten() {
                let path = entry.path();
                if !path.is_dir() {
                    continue;
                }

                if let Some(filter) = project_filter {
                    let dir_name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                    if !dir_name.contains(filter) {
                        continue;
                    }
                }

                for walk_entry in WalkDir::new(&path)
                    .follow_links(false)
                    .into_iter()
                    .filter_map(|e| e.ok())
                {
                    let file_path = walk_entry.path();
                    if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                        continue;
                    }
                    if !Self::matches_cutoff(file_path, cutoff) {
                        continue;
                    }
                    sessions.push(file_path.to_path_buf());
                }
            }
        }

        for root in codex_roots {
            if !root.exists() {
                continue;
            }

            for walk_entry in WalkDir::new(&root)
                .follow_links(false)
                .into_iter()
                .filter_map(|e| e.ok())
            {
                let file_path = walk_entry.path();
                if file_path.extension().and_then(|e| e.to_str()) != Some("jsonl") {
                    continue;
                }
                if !Self::matches_cutoff(file_path, cutoff) {
                    continue;
                }
                if let Some(filter) = project_filter {
                    if !Self::codex_matches_project_filter(file_path, filter) {
                        continue;
                    }
                }
                sessions.push(file_path.to_path_buf());
            }
        }

        Ok(sessions)
    }

    fn discover_sessions_in_projects_dir(
        projects_dir: &Path,
        project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        Self::discover_sessions_from_roots(
            projects_dir,
            &Self::codex_session_roots()?,
            project_filter,
            since_days,
        )
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
    ) -> Result<Vec<PathBuf>> {
        let projects_dir = Self::projects_dir()?;
        Self::discover_sessions_in_projects_dir(&projects_dir, project_filter, since_days)
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        let file =
            fs::File::open(path).with_context(|| format!("failed to open {}", path.display()))?;
        let reader = BufReader::new(file);

        let mut session_id = path
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

            // Pre-filter: skip lines that can't contain Claude Bash or Codex exec_command events.
            if !line.contains("\"Bash\"")
                && !line.contains("\"tool_result\"")
                && !line.contains("\"exec_command\"")
                && !line.contains("\"function_call_output\"")
                && !line.contains("\"session_meta\"")
            {
                continue;
            }

            let entry: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };

            let entry_type = entry.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match entry_type {
                "session_meta" => {
                    if let Some(id) = entry.pointer("/payload/id").and_then(|i| i.as_str()) {
                        session_id = id.to_string();
                    }
                }
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
                "response_item" => {
                    let payload_type = entry
                        .pointer("/payload/type")
                        .and_then(|t| t.as_str())
                        .unwrap_or("");

                    match payload_type {
                        "function_call" => {
                            if entry.pointer("/payload/name").and_then(|n| n.as_str())
                                != Some("exec_command")
                            {
                                continue;
                            }

                            let Some(id) =
                                entry.pointer("/payload/call_id").and_then(|i| i.as_str())
                            else {
                                continue;
                            };
                            let Some(arguments) =
                                entry.pointer("/payload/arguments").and_then(|a| a.as_str())
                            else {
                                continue;
                            };
                            let args: Value = match serde_json::from_str(arguments) {
                                Ok(v) => v,
                                Err(_) => continue,
                            };
                            let Some(cmd) = args
                                .get("cmd")
                                .and_then(|c| c.as_str())
                                .or_else(|| args.get("command").and_then(|c| c.as_str()))
                            else {
                                continue;
                            };

                            pending_tool_uses.push((
                                id.to_string(),
                                cmd.to_string(),
                                sequence_counter,
                            ));
                            sequence_counter += 1;
                        }
                        "function_call_output" => {
                            let Some(id) =
                                entry.pointer("/payload/call_id").and_then(|i| i.as_str())
                            else {
                                continue;
                            };
                            let (output_len, content_preview) =
                                Self::output_from_value(entry.pointer("/payload/output"));
                            tool_results
                                .insert(id.to_string(), (output_len, content_preview, false));
                        }
                        _ => {}
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
        let missing_projects_dir = temp_home.path().join(CLAUDE_DIR).join("projects");

        let sessions = ClaudeProvider::discover_sessions_from_roots(
            &missing_projects_dir,
            &[],
            None,
            Some(30),
        )
        .unwrap();

        assert!(sessions.is_empty());
    }

    #[test]
    fn test_discover_sessions_scans_codex_when_claude_projects_missing() {
        let temp_home = tempfile::tempdir().unwrap();
        let missing_projects_dir = temp_home.path().join(CLAUDE_DIR).join("projects");
        let codex_root = temp_home.path().join(".codex/sessions/2026/06/09");
        std::fs::create_dir_all(&codex_root).unwrap();
        std::fs::write(
            codex_root.join("rollout.jsonl"),
            r#"{"type":"session_meta","payload":{"id":"abc","cwd":"/tmp/codex-only"}}"#,
        )
        .unwrap();

        let sessions = ClaudeProvider::discover_sessions_from_roots(
            &missing_projects_dir,
            &[codex_root],
            Some("codex-only"),
            Some(30),
        )
        .unwrap();

        assert_eq!(sessions.len(), 1);
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

        let sessions = ClaudeProvider::discover_sessions_from_roots(
            projects_dir.path(),
            &[],
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
            ClaudeProvider::discover_sessions_from_roots(projects_file.path(), &[], None, None)
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

    #[test]
    fn test_extract_codex_exec_command() {
        let jsonl = make_jsonl(&[
            r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"rtk git status\",\"workdir\":\"/tmp\"}","call_id":"call_1"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"Command: /bin/zsh -lc 'rtk git status'\nOriginal token count: 11\nOutput:\nclean"}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "rtk git status");
        assert_eq!(
            cmds[0].output_len,
            Some(
                "Command: /bin/zsh -lc 'rtk git status'\nOriginal token count: 11\nOutput:\nclean"
                    .len()
            )
        );
        assert_eq!(
            cmds[0].output_content.as_deref(),
            Some(
                "Command: /bin/zsh -lc 'rtk git status'\nOriginal token count: 11\nOutput:\nclean"
            )
        );
    }

    #[test]
    fn test_codex_session_cwd_matches_filter() {
        let jsonl = make_jsonl(&[
            r#"{"type":"session_meta","payload":{"id":"abc","cwd":"/home/dev/my-project"}}"#,
            r#"{"type":"response_item","payload":{"type":"message","role":"assistant"}}"#,
        ]);

        assert!(ClaudeProvider::codex_matches_project_filter(
            jsonl.path(),
            "-home-dev-my-project"
        ));
        assert!(ClaudeProvider::codex_matches_project_filter(
            jsonl.path(),
            "/home/dev/my-project"
        ));
        assert!(!ClaudeProvider::codex_matches_project_filter(
            jsonl.path(),
            "other-project"
        ));
    }

    #[test]
    fn test_extract_codex_session_id_from_session_meta() {
        let jsonl = make_jsonl(&[
            r#"{"type":"session_meta","payload":{"id":"019cb2e0-438f-77f3-b9e4-854a431d49a9","cwd":"/home/dev/my-project"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call","name":"exec_command","arguments":"{\"cmd\":\"rtk git status\"}","call_id":"call_1"}}"#,
            r#"{"type":"response_item","payload":{"type":"function_call_output","call_id":"call_1","output":"clean"}}"#,
        ]);

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(jsonl.path()).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].session_id, "019cb2e0-438f-77f3-b9e4-854a431d49a9");
    }
}
