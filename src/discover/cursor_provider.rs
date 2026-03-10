// Cursor stores conversation data in internal SQLite databases (state.vscdb for
// desktop, store.db for agent CLI). These schemas are undocumented and may change
// without notice across Cursor updates. All row/column parsing errors are silently
// skipped so that schema changes degrade to empty results rather than hard failures.

use anyhow::{Context, Result};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

use super::provider::{
    cutoff_from_days, is_recent, join_tool_uses_with_results, ExtractedCommand, SessionProvider,
    ToolSource, OUTPUT_PREVIEW_CHARS,
};

/// Terminal tool names Cursor may use (undocumented, best-effort).
const TERMINAL_TOOL_NAMES: &[&str] = &[
    "run_terminal_command",
    "terminal",
    "execute_command",
    "run_command",
];

pub struct CursorProvider;

impl CursorProvider {
    /// Find all Cursor database files (state.vscdb for desktop, store.db for agent CLI).
    fn find_db_paths() -> Vec<PathBuf> {
        let Some(home) = dirs::home_dir() else {
            return vec![];
        };

        let mut paths = Vec::new();

        // Desktop: state.vscdb
        let desktop_candidates = [
            // macOS
            home.join("Library/Application Support/Cursor/User/globalStorage/state.vscdb"),
            // Linux
            home.join(".config/Cursor/User/globalStorage/state.vscdb"),
        ];

        for candidate in &desktop_candidates {
            if candidate.exists() {
                paths.push(candidate.clone());
            }
        }

        // Agent CLI: store.db files in chat directories
        let agent_dirs = [
            home.join(".config/cursor/chats"),
            home.join(".cursor/chats"),
        ];

        for agent_dir in &agent_dirs {
            if !agent_dir.exists() {
                continue;
            }
            if let Ok(entries) = fs::read_dir(agent_dir) {
                for entry in entries.flatten() {
                    let store_db = entry.path().join("store.db");
                    if store_db.exists() {
                        paths.push(store_db);
                    }
                }
            }
        }

        paths
    }
}

impl SessionProvider for CursorProvider {
    fn tool_source(&self) -> ToolSource {
        ToolSource::Cursor
    }

    fn discover_sessions(
        &self,
        _project_filter: Option<&str>,
        since_days: Option<u64>,
    ) -> Result<Vec<PathBuf>> {
        let cutoff = cutoff_from_days(since_days);
        let paths: Vec<PathBuf> = Self::find_db_paths()
            .into_iter()
            .filter(|p| is_recent(p, cutoff))
            .collect();
        Ok(paths)
    }

    fn extract_commands(&self, path: &Path) -> Result<Vec<ExtractedCommand>> {
        let filename = path.file_name().and_then(|n| n.to_str()).unwrap_or("");

        if filename == "state.vscdb" {
            extract_from_desktop_db(path)
        } else {
            extract_from_agent_db(path)
        }
    }
}

/// Extract commands from Cursor Desktop's state.vscdb (cursorDiskKV table).
fn extract_from_desktop_db(path: &Path) -> Result<Vec<ExtractedCommand>> {
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open Cursor DB: {}", path.display()))?;

    // Check if cursorDiskKV table exists
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='cursorDiskKV'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        return Ok(vec![]);
    }

    let mut stmt = conn
        .prepare("SELECT key, value FROM cursorDiskKV WHERE key LIKE 'bubbleId:%'")
        .context("failed to query cursorDiskKV")?;

    let session_id = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("cursor-desktop")
        .to_string();

    let mut commands = Vec::new();
    let mut sequence_counter = 0;

    let rows = stmt
        .query_map([], |row| {
            let value: String = row.get(1)?;
            Ok(value)
        })
        .context("failed to read cursorDiskKV rows")?;

    for row in rows {
        let value = match row {
            Ok(v) => v,
            Err(_) => continue,
        };

        let parsed: serde_json::Value = match serde_json::from_str(&value) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Look for assistant bubbles (type == 2) with tool calls
        let bubble_type = parsed.get("type").and_then(|t| t.as_i64()).unwrap_or(0);
        if bubble_type != 2 {
            continue;
        }

        // Extract from richText or text fields
        let text = parsed
            .get("richText")
            .or_else(|| parsed.get("text"))
            .and_then(|t| t.as_str())
            .unwrap_or("");

        // Look for tool call patterns in the text
        if let Some(cmd) = extract_command_from_bubble_text(text) {
            commands.push(ExtractedCommand {
                command: cmd,
                output_len: None,
                session_id: session_id.clone(),
                output_content: None,
                is_error: false,
                sequence_index: sequence_counter,
            });
            sequence_counter += 1;
        }
    }

    Ok(commands)
}

/// Extract commands from Cursor Agent CLI's store.db (blobs table).
fn extract_from_agent_db(path: &Path) -> Result<Vec<ExtractedCommand>> {
    let conn = rusqlite::Connection::open_with_flags(
        path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .with_context(|| format!("failed to open Cursor agent DB: {}", path.display()))?;

    // Check if blobs table exists
    let table_exists: bool = conn
        .query_row(
            "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='blobs'",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);

    if !table_exists {
        return Ok(vec![]);
    }

    // Try both possible column layouts
    let query = if has_column(&conn, "blobs", "data") {
        "SELECT data FROM blobs"
    } else if has_column(&conn, "blobs", "value") {
        "SELECT value FROM blobs"
    } else {
        return Ok(vec![]);
    };

    let mut stmt = conn.prepare(query).context("failed to query blobs")?;

    let session_id = path
        .parent()
        .and_then(|p| p.file_name())
        .and_then(|n| n.to_str())
        .unwrap_or("cursor-agent")
        .to_string();

    let mut tool_calls: Vec<(String, String, usize)> = Vec::new();
    let mut tool_responses: HashMap<String, (usize, String, bool)> = HashMap::new();
    let mut sequence_counter = 0;

    let rows = stmt
        .query_map([], |row| {
            let value: String = row.get(0)?;
            Ok(value)
        })
        .context("failed to read blobs")?;

    for row in rows {
        let value = match row {
            Ok(v) => v,
            Err(_) => continue,
        };

        let parsed: serde_json::Value = match serde_json::from_str(&value) {
            Ok(v) => v,
            Err(_) => continue,
        };

        // Handle both single objects and arrays of content items
        let items = if parsed.is_array() {
            parsed.as_array().cloned().unwrap_or_default()
        } else if let Some(content) = parsed.get("content").and_then(|c| c.as_array()) {
            content.clone()
        } else {
            vec![parsed.clone()]
        };

        for item in &items {
            let item_type = item.get("type").and_then(|t| t.as_str()).unwrap_or("");

            match item_type {
                "tool_call" => {
                    let name = item.get("name").and_then(|n| n.as_str()).unwrap_or("");
                    if !TERMINAL_TOOL_NAMES.contains(&name) {
                        continue;
                    }
                    let tool_call_id = item
                        .get("tool_call_id")
                        .or_else(|| item.get("id"))
                        .and_then(|i| i.as_str())
                        .unwrap_or("");
                    if tool_call_id.is_empty() {
                        continue;
                    }

                    // Arguments may be a JSON string or an object
                    let command = extract_command_from_args(item);
                    if let Some(cmd) = command {
                        tool_calls.push((tool_call_id.to_string(), cmd, sequence_counter));
                        sequence_counter += 1;
                    }
                }
                "tool" | "tool_result" => {
                    let tool_call_id = item
                        .get("tool_call_id")
                        .and_then(|i| i.as_str())
                        .unwrap_or("");
                    if tool_call_id.is_empty() {
                        continue;
                    }
                    let output = item.get("content").and_then(|c| c.as_str()).unwrap_or("");
                    let is_error = item
                        .get("is_error")
                        .and_then(|e| e.as_bool())
                        .unwrap_or(false);
                    let preview: String = output.chars().take(OUTPUT_PREVIEW_CHARS).collect();
                    tool_responses
                        .insert(tool_call_id.to_string(), (output.len(), preview, is_error));
                }
                _ => {}
            }
        }
    }

    Ok(join_tool_uses_with_results(
        tool_calls,
        &tool_responses,
        &session_id,
    ))
}

fn has_column(conn: &rusqlite::Connection, table: &str, column: &str) -> bool {
    if !table.chars().all(|c| c.is_alphanumeric() || c == '_') {
        return false;
    }
    let query = format!("PRAGMA table_info(\"{}\")", table);
    let mut stmt = match conn.prepare(&query) {
        Ok(s) => s,
        Err(_) => return false,
    };
    let cols: Vec<String> = stmt
        .query_map([], |row| row.get::<_, String>(1))
        .ok()
        .map(|rows| rows.filter_map(|r| r.ok()).collect())
        .unwrap_or_default();
    cols.iter().any(|c| c == column)
}

/// Try to extract a shell command from a Cursor desktop bubble's text.
fn extract_command_from_bubble_text(text: &str) -> Option<String> {
    // Look for patterns like: ```bash\n<command>\n``` or tool_call markers
    if text.contains("```bash") || text.contains("```sh") || text.contains("```shell") {
        let start_markers = ["```bash\n", "```sh\n", "```shell\n"];
        for marker in &start_markers {
            if let Some(start) = text.find(marker) {
                let cmd_start = start + marker.len();
                if let Some(end) = text[cmd_start..].find("```") {
                    let cmd = text[cmd_start..cmd_start + end].trim();
                    if !cmd.is_empty() {
                        return Some(cmd.to_string());
                    }
                }
            }
        }
    }
    None
}

/// Extract command string from tool_call arguments.
fn extract_command_from_args(item: &serde_json::Value) -> Option<String> {
    let args = item.get("arguments")?;

    // Arguments might be a JSON string that needs parsing
    if let Some(args_str) = args.as_str() {
        if let Ok(parsed) = serde_json::from_str::<serde_json::Value>(args_str) {
            return parsed
                .get("command")
                .and_then(|c| c.as_str())
                .map(|s| s.to_string());
        }
        // If not JSON, treat the whole string as the command
        if !args_str.is_empty() {
            return Some(args_str.to_string());
        }
    }

    // Arguments might already be an object
    if let Some(obj) = args.as_object() {
        return obj
            .get("command")
            .and_then(|c| c.as_str())
            .map(|s| s.to_string());
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::Connection;
    use tempfile::tempdir;

    #[test]
    fn test_extract_from_agent_db_tool_calls() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("store.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE blobs (id INTEGER PRIMARY KEY, data TEXT)", [])
            .unwrap();

        // Insert a tool_call and tool_result
        let tool_call = serde_json::json!([
            {"type":"tool_call","name":"run_terminal_command","tool_call_id":"tc_1","arguments":"{\"command\":\"git status\"}"},
            {"type":"tool","tool_call_id":"tc_1","content":"On branch main","is_error":false}
        ]);
        conn.execute(
            "INSERT INTO blobs (data) VALUES (?1)",
            [tool_call.to_string()],
        )
        .unwrap();
        drop(conn);

        let cmds = extract_from_agent_db(&db_path).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "git status");
        assert!(!cmds[0].is_error);
        assert_eq!(cmds[0].output_len.unwrap(), "On branch main".len());
    }

    #[test]
    fn test_extract_from_agent_db_non_terminal_ignored() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("store.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE blobs (id INTEGER PRIMARY KEY, data TEXT)", [])
            .unwrap();

        let tool_call = serde_json::json!([
            {"type":"tool_call","name":"read_file","tool_call_id":"tc_1","arguments":"{\"path\":\"/tmp/foo\"}"}
        ]);
        conn.execute(
            "INSERT INTO blobs (data) VALUES (?1)",
            [tool_call.to_string()],
        )
        .unwrap();
        drop(conn);

        let cmds = extract_from_agent_db(&db_path).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_extract_from_agent_db_missing_table() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("store.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE other_table (id INTEGER PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();
        drop(conn);

        let cmds = extract_from_agent_db(&db_path).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_extract_from_desktop_db_bubbles() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("state.vscdb");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();

        let bubble = serde_json::json!({
            "type": 2,
            "richText": "Let me run this:\n```bash\nnpm test\n```\nDone."
        });
        conn.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES ('bubbleId:abc', ?1)",
            [bubble.to_string()],
        )
        .unwrap();
        drop(conn);

        let cmds = extract_from_desktop_db(&db_path).unwrap();
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "npm test");
    }

    #[test]
    fn test_extract_from_desktop_db_user_bubble_ignored() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("state.vscdb");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute(
            "CREATE TABLE cursorDiskKV (key TEXT PRIMARY KEY, value TEXT)",
            [],
        )
        .unwrap();

        // User bubble (type == 1) should be ignored
        let bubble = serde_json::json!({
            "type": 1,
            "text": "```bash\ngit status\n```"
        });
        conn.execute(
            "INSERT INTO cursorDiskKV (key, value) VALUES ('bubbleId:xyz', ?1)",
            [bubble.to_string()],
        )
        .unwrap();
        drop(conn);

        let cmds = extract_from_desktop_db(&db_path).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_extract_from_desktop_db_missing_table() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("state.vscdb");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE other (key TEXT PRIMARY KEY)", [])
            .unwrap();
        drop(conn);

        let cmds = extract_from_desktop_db(&db_path).unwrap();
        assert_eq!(cmds.len(), 0);
    }

    #[test]
    fn test_extract_command_from_bubble_text() {
        assert_eq!(
            extract_command_from_bubble_text("Run:\n```bash\ngit status\n```\nDone."),
            Some("git status".to_string())
        );
        assert_eq!(
            extract_command_from_bubble_text("```sh\nls -la\n```"),
            Some("ls -la".to_string())
        );
        assert_eq!(extract_command_from_bubble_text("No code here."), None);
    }

    #[test]
    fn test_extract_command_from_args_json_string() {
        let item = serde_json::json!({
            "type": "tool_call",
            "name": "run_terminal_command",
            "tool_call_id": "tc_1",
            "arguments": "{\"command\":\"cargo test\"}"
        });
        assert_eq!(
            extract_command_from_args(&item),
            Some("cargo test".to_string())
        );
    }

    #[test]
    fn test_extract_command_from_args_object() {
        let item = serde_json::json!({
            "type": "tool_call",
            "name": "run_terminal_command",
            "tool_call_id": "tc_1",
            "arguments": {"command": "npm install"}
        });
        assert_eq!(
            extract_command_from_args(&item),
            Some("npm install".to_string())
        );
    }

    #[test]
    fn test_tool_source() {
        let provider = CursorProvider;
        assert_eq!(provider.tool_source(), ToolSource::Cursor);
    }

    #[test]
    fn test_multiple_terminal_tool_names() {
        let dir = tempdir().unwrap();
        let db_path = dir.path().join("store.db");

        let conn = Connection::open(&db_path).unwrap();
        conn.execute("CREATE TABLE blobs (id INTEGER PRIMARY KEY, data TEXT)", [])
            .unwrap();

        // Test different tool names
        for (i, name) in [
            "run_terminal_command",
            "terminal",
            "execute_command",
            "run_command",
        ]
        .iter()
        .enumerate()
        {
            let tool_call = serde_json::json!([
                {"type":"tool_call","name":name,"tool_call_id":format!("tc_{}", i),"arguments":format!("{{\"command\":\"cmd_{}\"}}", i)}
            ]);
            conn.execute(
                "INSERT INTO blobs (data) VALUES (?1)",
                [tool_call.to_string()],
            )
            .unwrap();
        }
        drop(conn);

        let cmds = extract_from_agent_db(&db_path).unwrap();
        assert_eq!(cmds.len(), 4);
    }
}
