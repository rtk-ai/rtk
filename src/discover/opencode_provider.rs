//! Reads OpenCode session history from its SQLite database and streams
//! their command history.
//!
//! OpenCode stores every session in one WAL-mode SQLite database at
//! `$XDG_DATA_HOME/opencode/opencode.db` (fallback `~/.local/share`). There
//! are two storage generations (design D8). The OpenCode 2.x generation
//! writes `session_v2` + `session_message`, nesting tool calls in each
//! message's `content[]` as `{"type":"tool","id":…,"name":"shell",
//! "state":{"status","input":{"command"},"content":[{"type":"text","text"}],
//! "metadata":{"exit"}}}`. The older 1.x generation wrote `session`/`part`
//! with a shell call as `{"type":"tool","tool":"bash","callID":…,
//! "state":{"status","input":{"command"},"output","metadata":{"exit"}}}`.
//! OpenCode 2.x migrates 1.x history into the v2 tables, so when
//! `session_v2` exists it is read exclusively; otherwise the 1.x tables are.
//!
//! Access is strictly read-only (design D4): rtk never creates, migrates or
//! recovers another application's database. A missing database means "no
//! OpenCode history" (silent); an unreadable one is a clean
//! "provider unavailable" error, never a panic and never a silent zero.
//!
//! Verified against OpenCode 1.19.0 (legacy tables) and 2.0.32 (v2 tables).

use super::provider::{
    ExtractedCommand, ProjectFilter, SessionProvider, SessionRef, dir_matches_filter,
};
use anyhow::{Context, Result};
use rusqlite::{Connection, OpenFlags, params};
use std::path::PathBuf;
use std::time::{SystemTime, UNIX_EPOCH};

/// Max chars of tool output kept for error detection, mirroring the ~1000-char
/// window Claude's extractor uses.
const OUTPUT_CONTENT_MAX: usize = 1000;

/// Default database location: `$XDG_DATA_HOME/opencode/opencode.db`, falling
/// back to `~/.local/share/opencode/opencode.db` where the XDG base is unset.
pub fn db_path() -> Option<PathBuf> {
    crate::core::user_dirs::data_local().map(|d| d.join("opencode").join("opencode.db"))
}

/// Session provider backed by OpenCode's SQLite store.
pub struct OpenCodeProvider {
    db: PathBuf,
}

impl Default for OpenCodeProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl OpenCodeProvider {
    /// Provider for the user's real OpenCode database (if the platform
    /// resolves a data dir at all).
    pub fn new() -> Self {
        Self {
            db: db_path().unwrap_or_else(|| PathBuf::from("opencode/opencode.db")),
        }
    }

    /// Provider for an explicit database path (tests).
    #[cfg(test)]
    pub fn with_db_path(db: PathBuf) -> Self {
        Self { db }
    }

    /// Open the database strictly read-only. WAL-mode files that need
    /// `-wal`/`-shm` recovery fail here with `SQLITE_CANTOPEN` while OpenCode
    /// runs; that surfaces as a "provider unavailable" error the caller turns
    /// into a warning — rtk never recovers another application's WAL.
    fn connect(&self) -> Result<Connection> {
        Connection::open_with_flags(&self.db, OpenFlags::SQLITE_OPEN_READ_ONLY).with_context(|| {
            format!(
                "OpenCode provider unavailable: cannot open {} read-only",
                self.db.display()
            )
        })
    }
}

fn now_millis() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}

/// Which storage generation a store uses (design D8). OpenCode 2.x writes
/// `session_v2`/`session_message` and migrates 1.x history into them, so
/// when the v2 tables exist they are the exclusive source; the 1.x
/// `session`/`part` tables are read only on stores 2.x never touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Schema {
    V2,
    Legacy,
}

fn detect_schema(conn: &Connection) -> Result<Schema> {
    let has_v2: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = 'session_v2'",
            [],
            |row| row.get(0),
        )
        .context("OpenCode provider unavailable: schema probe failed")?;
    Ok(if has_v2 > 0 {
        Schema::V2
    } else {
        Schema::Legacy
    })
}

/// One shell tool call as extracted from either generation:
/// (call id, command, output text, status, exit code).
type ToolRow = (
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
);

impl SessionProvider for OpenCodeProvider {
    /// Whether OpenCode storage exists at all. A never-installed OpenCode must
    /// stay fully silent (design D4).
    fn available(&self) -> bool {
        self.db.is_file()
    }

    fn discover_sessions(
        &self,
        filter: ProjectFilter<'_>,
        since_days: Option<u64>,
    ) -> Result<Vec<SessionRef>> {
        if !self.available() {
            return Ok(Vec::new());
        }
        let conn = self.connect()?;
        let table = match detect_schema(&conn)? {
            Schema::V2 => "session_v2",
            Schema::Legacy => "session",
        };

        // `time_created` is epoch milliseconds in both generations (verified
        // against a live store); the cutoff converts days to ms.
        let mut conditions: Vec<String> = Vec::new();
        let mut args: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::new();
        if let Some(days) = since_days {
            conditions.push("time_created >= ?".to_string());
            args.push(Box::new(now_millis() - (days as i64) * 86_400_000));
        }
        // Substring scoping is filtered in Rust through the shared
        // `dir_matches_filter` so `--project` means the same literal substring
        // match on every provider (SQL LIKE would treat `%`/`_` in user input
        // as wildcards and ignore case on all platforms).
        let substring = match filter {
            ProjectFilter::All => None,
            ProjectFilter::CurrentProject(dir) => {
                // OpenCode records the plain working directory per session.
                conditions.push("directory = ?".to_string());
                args.push(Box::new(dir.to_string_lossy().into_owned()));
                None
            }
            ProjectFilter::Substring(s) => Some(s),
        };
        let sql = format!(
            "SELECT id, directory FROM {table}{} ORDER BY time_created DESC, id",
            if conditions.is_empty() {
                String::new()
            } else {
                format!(" WHERE {}", conditions.join(" AND "))
            }
        );
        let mut stmt = conn.prepare_cached(&sql).context(
            "OpenCode provider unavailable: session query failed (corrupt or locked store)",
        )?;
        let rows = stmt
            .query_map(
                rusqlite::params_from_iter(args.iter().map(|p| p.as_ref())),
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
            )
            .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
            .context("OpenCode provider unavailable: session scan failed")?;
        let refs = rows
            .into_iter()
            .filter(|(_, directory)| substring.is_none_or(|s| dir_matches_filter(directory, s)))
            .map(|(session_id, _)| SessionRef::OpenCode { session_id })
            .collect();
        Ok(refs)
    }

    fn extract_commands(&self, session: &SessionRef) -> Result<Vec<ExtractedCommand>> {
        let SessionRef::OpenCode { session_id } = session else {
            return Err(anyhow::anyhow!(
                "OpenCodeProvider cannot read {}",
                session.display()
            ));
        };
        let conn = self.connect()?;
        // Filtering and field mapping happen in SQL (design D5); malformed
        // rows yield NULLs and are skipped per fail-open, never an error.
        let rows = match detect_schema(&conn)? {
            Schema::V2 => query_v2_rows(&conn, session_id)?,
            Schema::Legacy => query_legacy_rows(&conn, session_id)?,
        };

        let mut commands = Vec::new();
        for (call_id, command, output, status, exit) in rows {
            let Some(command) = command else { continue };
            let is_error = status.as_deref() != Some("completed") || exit.is_some_and(|e| e != 0);
            let output_content = output
                .as_ref()
                .map(|o| truncate_chars(o, OUTPUT_CONTENT_MAX).to_string());
            commands.push(ExtractedCommand {
                command,
                output_len: output.as_ref().map(|o| o.len()),
                session_id: session_id.clone(),
                tool_use_id: call_id.unwrap_or_default(),
                output_content,
                is_error,
                sequence_index: commands.len(),
            });
        }
        Ok(commands)
    }
}

/// Shell tool calls from the 2.x generation. Tool entries are nested in each
/// message's `content[]`; `seq` orders the session, `key` the entries within
/// a message. Output text is the concatenation of the `state.content[]`
/// text entries.
fn query_v2_rows(conn: &Connection, session_id: &str) -> Result<Vec<ToolRow>> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT json_extract(j.value, '$.id'),
                    json_extract(j.value, '$.state.input.command'),
                    (SELECT group_concat(json_extract(c.value, '$.text'), '')
                       FROM json_each(json_extract(j.value, '$.state.content')) c
                      WHERE json_extract(c.value, '$.type') = 'text'),
                    json_extract(j.value, '$.state.status'),
                    json_extract(j.value, '$.state.metadata.exit')
             FROM session_message m, json_each(json_extract(m.data, '$.content')) j
             WHERE m.session_id = ?1
               AND json_extract(j.value, '$.type') = 'tool'
               AND json_extract(j.value, '$.name') IN ('bash', 'shell')
             ORDER BY m.seq, j.key",
        )
        .context(
            "OpenCode provider unavailable: session_message query failed (corrupt or locked store)",
        )?;
    let rows = stmt
        .query_map(params![session_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .context("OpenCode provider unavailable: session_message scan failed")?;
    Ok(rows)
}

/// Shell tool calls from the 1.x generation (`part` rows, one per call).
fn query_legacy_rows(conn: &Connection, session_id: &str) -> Result<Vec<ToolRow>> {
    let mut stmt = conn
        .prepare_cached(
            "SELECT json_extract(data, '$.callID'),
                    json_extract(data, '$.state.input.command'),
                    json_extract(data, '$.state.output'),
                    json_extract(data, '$.state.status'),
                    json_extract(data, '$.state.metadata.exit')
             FROM part
             WHERE session_id = ?1
               AND json_extract(data, '$.type') = 'tool'
               AND json_extract(data, '$.tool') IN ('bash', 'shell')
             ORDER BY time_created, id",
        )
        .context("OpenCode provider unavailable: part query failed (corrupt or locked store)")?;
    let rows = stmt
        .query_map(params![session_id], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<String>>(3)?,
                row.get::<_, Option<i64>>(4)?,
            ))
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
        .context("OpenCode provider unavailable: part scan failed")?;
    Ok(rows)
}

/// First `max` chars of `s` (char-safe, unlike a byte slice).
fn truncate_chars(s: &str, max: usize) -> &str {
    match s.char_indices().nth(max) {
        Some((idx, _)) => &s[..idx],
        None => s,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::params;
    use std::path::Path;
    use tempfile::NamedTempFile;

    /// Minimal mirror of the OpenCode 1.x storage generation with the columns
    /// this provider reads; seeded JSON only, never real user data.
    fn seed_db() -> (NamedTempFile, Connection) {
        let file = NamedTempFile::new().expect("tempfile");
        let conn = Connection::open(file.path()).expect("open sqlite");
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                directory TEXT NOT NULL,
                time_created INTEGER NOT NULL
            );
            CREATE TABLE message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                message_id TEXT NOT NULL,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .expect("schema");
        (file, conn)
    }

    fn insert_session(conn: &Connection, id: &str, directory: &str, time_created: i64) {
        conn.execute(
            "INSERT INTO session (id, directory, time_created) VALUES (?1, ?2, ?3)",
            params![id, directory, time_created],
        )
        .unwrap();
    }

    /// Insert one tool part. `exit` of `None` omits `metadata.exit` entirely,
    /// matching how OpenCode writes errored calls.
    fn insert_tool_part(
        conn: &Connection,
        session_id: &str,
        part_id: &str,
        time_created: i64,
        _tool: &str,
        data_json: &str,
    ) {
        conn.execute(
            "INSERT INTO message (id, session_id, time_created, data) VALUES (?1, ?2, ?3, '{}')",
            params![format!("m_{part_id}"), session_id, time_created],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part (id, message_id, session_id, time_created, data) VALUES (?1, ?2, ?3, ?4, ?5)",
            params![part_id, format!("m_{part_id}"), session_id, time_created, data_json],
        )
        .unwrap();
    }

    pub(super) fn bash_json(
        command: &str,
        output: &str,
        status: &str,
        exit: Option<i64>,
    ) -> String {
        let mut state = serde_json::Map::new();
        state.insert("status".into(), status.into());
        state.insert("input".into(), serde_json::json!({ "command": command }));
        state.insert("output".into(), output.into());
        if let Some(e) = exit {
            state.insert("metadata".into(), serde_json::json!({ "exit": e }));
        }
        let mut root = serde_json::Map::new();
        root.insert("type".into(), "tool".into());
        root.insert("tool".into(), "bash".into());
        root.insert("callID".into(), format!("call_{command}").into());
        root.insert("state".into(), serde_json::Value::Object(state));
        serde_json::Value::Object(root).to_string()
    }

    pub(super) fn opencode_ref(id: &str) -> SessionRef {
        SessionRef::OpenCode {
            session_id: id.to_string(),
        }
    }

    fn extract(file: &Path, session: &str) -> Vec<ExtractedCommand> {
        OpenCodeProvider::with_db_path(file.to_path_buf())
            .extract_commands(&opencode_ref(session))
            .expect("extract")
    }

    #[test]
    fn extracts_bash_commands_with_fields() {
        let (f, conn) = seed_db();
        insert_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        insert_tool_part(
            &conn,
            "s1",
            "p1",
            1_700_000_000_001,
            "bash",
            &bash_json("git status", "clean", "completed", Some(0)),
        );
        insert_tool_part(
            &conn,
            "s1",
            "p2",
            1_700_000_000_002,
            "bash",
            &bash_json("cargo test", "ok", "completed", Some(0)),
        );

        let cmds = extract(f.path(), "s1");
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "git status");
        assert_eq!(cmds[0].tool_use_id, "call_git status");
        assert_eq!(cmds[0].output_len, Some(5));
        assert_eq!(cmds[0].output_content.as_deref(), Some("clean"));
        assert!(!cmds[0].is_error);
        assert_eq!(cmds[1].command, "cargo test");
        assert_eq!(cmds[0].session_id, "s1");
    }

    #[test]
    fn ignores_non_bash_tools() {
        let (f, conn) = seed_db();
        insert_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        for (id, tool) in [("p1", "edit"), ("p2", "read"), ("p3", "grep")] {
            insert_tool_part(
                &conn,
                "s1",
                id,
                1_700_000_000_000,
                tool,
                &format!(
                    "{{\"type\":\"tool\",\"tool\":\"{tool}\",\"callID\":\"c\",\"state\":{{\"status\":\"completed\",\"input\":{{\"command\":\"not-a-shell\"}},\"output\":\"x\"}}}}"
                ),
            );
        }
        assert!(extract(f.path(), "s1").is_empty());
    }

    #[test]
    fn ignores_non_tool_parts() {
        let (f, conn) = seed_db();
        insert_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        for (id, kind) in [("p1", "text"), ("p2", "reasoning"), ("p3", "step-finish")] {
            insert_tool_part(
                &conn,
                "s1",
                id,
                1_700_000_000_000,
                kind,
                &format!("{{\"type\":\"{kind}\",\"text\":\"hi\"}}"),
            );
        }
        assert!(extract(f.path(), "s1").is_empty());
    }

    #[test]
    fn detects_error_via_status() {
        let (f, conn) = seed_db();
        insert_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        insert_tool_part(
            &conn,
            "s1",
            "p1",
            1_700_000_000_001,
            "bash",
            &bash_json("false", "boom", "error", None),
        );

        let cmds = extract(f.path(), "s1");
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].is_error);
    }

    #[test]
    fn detects_error_via_exit_code() {
        let (f, conn) = seed_db();
        insert_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        insert_tool_part(
            &conn,
            "s1",
            "p1",
            1_700_000_000_001,
            "bash",
            &bash_json("test -f nope", "no", "completed", Some(1)),
        );

        let cmds = extract(f.path(), "s1");
        assert_eq!(cmds.len(), 1);
        assert!(cmds[0].is_error);
    }

    #[test]
    fn truncates_output_content_keeps_full_length() {
        let (f, conn) = seed_db();
        insert_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        let big = "x".repeat(5_000);
        insert_tool_part(
            &conn,
            "s1",
            "p1",
            1_700_000_000_001,
            "bash",
            &bash_json("ls", &big, "completed", Some(0)),
        );

        let cmds = extract(f.path(), "s1");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].output_len, Some(5_000));
        let content = cmds[0].output_content.as_deref().unwrap();
        assert_eq!(content.chars().count(), OUTPUT_CONTENT_MAX);
    }

    #[test]
    fn returns_commands_in_chronological_order() {
        let (f, conn) = seed_db();
        insert_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        // Insert newest first so insertion order cannot mask the sort.
        insert_tool_part(
            &conn,
            "s1",
            "p3",
            1_700_000_000_030,
            "bash",
            &bash_json("third", "", "completed", Some(0)),
        );
        insert_tool_part(
            &conn,
            "s1",
            "p1",
            1_700_000_000_010,
            "bash",
            &bash_json("first", "", "completed", Some(0)),
        );
        insert_tool_part(
            &conn,
            "s1",
            "p2",
            1_700_000_000_020,
            "bash",
            &bash_json("second", "", "completed", Some(0)),
        );

        let cmds = extract(f.path(), "s1");
        let order: Vec<&str> = cmds.iter().map(|c| c.command.as_str()).collect();
        assert_eq!(order, ["first", "second", "third"]);
        let idx: Vec<usize> = cmds.iter().map(|c| c.sequence_index).collect();
        assert_eq!(idx, [0, 1, 2]);
    }

    // -- fail-open (task 3.3) ----------------------------------------------

    #[test]
    fn missing_db_stays_silent() {
        let provider = OpenCodeProvider::with_db_path(PathBuf::from("/nonexistent/opencode.db"));
        assert!(!provider.available());
        let sessions = provider
            .discover_sessions(ProjectFilter::All, None)
            .expect("missing store is not an error");
        assert!(sessions.is_empty());
    }

    #[test]
    fn corrupt_db_is_a_clean_unavailable_error() {
        let f = NamedTempFile::new().expect("tempfile");
        std::fs::write(f.path(), b"definitely not a sqlite database").expect("write");
        let provider = OpenCodeProvider::with_db_path(f.path().to_path_buf());
        assert!(provider.available());
        let err = provider
            .discover_sessions(ProjectFilter::All, None)
            .expect_err("corrupt store must surface an error, never a silent zero");
        assert!(err.to_string().contains("unavailable"), "{err}");
        // Extraction path maps the same failure.
        let err = provider
            .extract_commands(&opencode_ref("s1"))
            .expect_err("corrupt store must surface an error");
        assert!(err.to_string().contains("unavailable"), "{err}");
    }

    #[cfg(unix)]
    #[test]
    fn unreadable_db_maps_to_unavailable_not_panic() {
        use std::os::unix::fs::PermissionsExt;
        // A file that stats but cannot be opened yields SQLITE_CANTOPEN — the
        // same error class as WAL recovery being needed while OpenCode runs.
        let dir = tempfile::tempdir().expect("tempdir");
        let db = dir.path().join("opencode.db");
        drop(Connection::open(&db).expect("create"));
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o000)).expect("chmod");
        let provider = OpenCodeProvider::with_db_path(db.clone());
        assert!(provider.available());
        let err = provider
            .discover_sessions(ProjectFilter::All, None)
            .expect_err("unreadable store must surface an error, never a silent zero");
        assert!(err.to_string().contains("unavailable"), "{err}");
        std::fs::set_permissions(&db, std::fs::Permissions::from_mode(0o644)).expect("restore");
    }

    // -- filters (task 3.4) --------------------------------------------------

    pub(super) fn ids(refs: &[SessionRef]) -> Vec<String> {
        refs.iter()
            .map(|r| match r {
                SessionRef::OpenCode { session_id } => session_id.clone(),
                SessionRef::File(_) => unreachable!(),
            })
            .collect()
    }

    fn seeded_store() -> (NamedTempFile, Connection) {
        let (f, conn) = seed_db();
        let now = now_millis();
        insert_session(&conn, "old", "/home/u/alpha", now - 40 * 86_400_000);
        insert_session(&conn, "recent", "/home/u/alpha", now - 86_400_000);
        insert_session(&conn, "other", "/home/u/beta", now - 2 * 86_400_000);
        (f, conn)
    }

    #[test]
    fn since_cutoff_excludes_older_sessions() {
        let (f, _conn) = seeded_store();
        let provider = OpenCodeProvider::with_db_path(f.path().to_path_buf());
        let recent = provider
            .discover_sessions(ProjectFilter::All, Some(30))
            .unwrap();
        assert_eq!(ids(&recent), ["recent", "other"]); // newest first
        let everything = provider
            .discover_sessions(ProjectFilter::All, None)
            .unwrap();
        assert_eq!(everything.len(), 3);
    }

    #[test]
    fn current_project_matches_directory_exactly() {
        let (f, conn) = seeded_store();
        insert_session(&conn, "nested", "/home/u/alpha/sub", 1);
        let provider = OpenCodeProvider::with_db_path(f.path().to_path_buf());
        let found = provider
            .discover_sessions(
                ProjectFilter::CurrentProject(Path::new("/home/u/alpha")),
                None,
            )
            .unwrap();
        // Newest first; the `/sub` sibling must not leak in via exact match.
        assert_eq!(ids(&found), ["recent", "old"]);
        assert!(!ids(&found).contains(&"nested".to_string()));
    }

    #[test]
    fn substring_filter_matches_directory_like() {
        let (f, _conn) = seeded_store();
        let provider = OpenCodeProvider::with_db_path(f.path().to_path_buf());
        let found = provider
            .discover_sessions(ProjectFilter::Substring("bet"), None)
            .unwrap();
        assert_eq!(ids(&found), ["other"]);
    }

    #[test]
    fn all_filter_includes_every_session() {
        let (f, _conn) = seeded_store();
        let provider = OpenCodeProvider::with_db_path(f.path().to_path_buf());
        assert_eq!(
            provider
                .discover_sessions(ProjectFilter::All, None)
                .unwrap()
                .len(),
            3
        );
    }
}

/// Tests for the OpenCode 2.x storage generation (`session_v2` +
/// `session_message`, tool entries nested in message `content[]`), design D8.
#[cfg(test)]
mod v2_tests {
    use super::tests::{bash_json, ids, opencode_ref};
    use super::*;
    use rusqlite::params;
    use std::path::Path;
    use tempfile::NamedTempFile;

    /// Minimal mirror of the generation OpenCode 2.x writes, with the columns
    /// this provider reads; seeded JSON only, never real user data.
    fn seed_db_v2() -> (NamedTempFile, Connection) {
        let file = NamedTempFile::new().expect("tempfile");
        let conn = Connection::open(file.path()).expect("open sqlite");
        conn.execute_batch(
            "CREATE TABLE session_v2 (
                id TEXT PRIMARY KEY,
                directory TEXT NOT NULL,
                time_created INTEGER NOT NULL
            );
            CREATE TABLE session_message (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                type TEXT NOT NULL,
                seq INTEGER NOT NULL,
                time_created INTEGER NOT NULL,
                time_updated INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .expect("schema");
        (file, conn)
    }

    fn insert_v2_session(conn: &Connection, id: &str, directory: &str, time_created: i64) {
        conn.execute(
            "INSERT INTO session_v2 (id, directory, time_created) VALUES (?1, ?2, ?3)",
            params![id, directory, time_created],
        )
        .unwrap();
    }

    /// One assistant message whose `content` array holds `entries`, ordered
    /// within the session by `seq` exactly as 2.x writes them.
    fn insert_v2_message(
        conn: &Connection,
        session_id: &str,
        msg_id: &str,
        seq: i64,
        entries: &[String],
    ) {
        let content: Vec<serde_json::Value> = entries
            .iter()
            .map(|e| serde_json::from_str(e).expect("entry json"))
            .collect();
        let data = serde_json::json!({ "content": content }).to_string();
        conn.execute(
            "INSERT INTO session_message (id, session_id, type, seq, time_created, time_updated, data)
             VALUES (?1, ?2, 'assistant', ?3, 1700000000000, 1700000000000, ?4)",
            params![msg_id, session_id, seq, data],
        )
        .unwrap();
    }

    /// One tool entry as 2.x nests it in a message's `content[]`. `exit` of
    /// `None` omits `metadata.exit` entirely, as errored calls do.
    fn v2_tool_json(
        name: &str,
        call_id: &str,
        command: &str,
        outputs: &[&str],
        status: &str,
        exit: Option<i64>,
    ) -> String {
        let content: Vec<serde_json::Value> = outputs
            .iter()
            .map(|t| serde_json::json!({ "type": "text", "text": t }))
            .collect();
        let mut state = serde_json::json!({
            "status": status,
            "input": { "command": command },
            "content": content,
        });
        if let Some(e) = exit {
            state["metadata"] = serde_json::json!({ "exit": e });
        }
        serde_json::json!({
            "type": "tool", "id": call_id, "name": name, "state": state
        })
        .to_string()
    }

    fn extract_v2(file: &Path, session: &str) -> Vec<ExtractedCommand> {
        OpenCodeProvider::with_db_path(file.to_path_buf())
            .extract_commands(&opencode_ref(session))
            .expect("extract")
    }

    #[test]
    fn extracts_shell_commands_with_fields() {
        let (f, conn) = seed_db_v2();
        insert_v2_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        insert_v2_message(
            &conn,
            "s1",
            "m1",
            0,
            &[
                v2_tool_json(
                    "shell",
                    "call_a",
                    "git status",
                    &["clean"],
                    "completed",
                    Some(0),
                ),
                v2_tool_json(
                    "bash",
                    "call_b",
                    "cargo test",
                    &["ok"],
                    "completed",
                    Some(0),
                ),
                v2_tool_json(
                    "edit",
                    "call_c",
                    "not-a-shell",
                    &["x"],
                    "completed",
                    Some(0),
                ),
                serde_json::json!({"type": "text", "text": "hi"}).to_string(),
            ],
        );

        let cmds = extract_v2(f.path(), "s1");
        assert_eq!(cmds.len(), 2);
        assert_eq!(cmds[0].command, "git status");
        assert_eq!(cmds[0].tool_use_id, "call_a");
        assert_eq!(cmds[0].output_len, Some(5));
        assert_eq!(cmds[0].output_content.as_deref(), Some("clean"));
        assert!(!cmds[0].is_error);
        assert_eq!(cmds[0].session_id, "s1");
        assert_eq!(cmds[1].command, "cargo test");
    }

    #[test]
    fn concatenates_multiple_output_chunks() {
        let (f, conn) = seed_db_v2();
        insert_v2_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        insert_v2_message(
            &conn,
            "s1",
            "m1",
            0,
            &[v2_tool_json(
                "shell",
                "call_a",
                "ls",
                &["a\n", "b"],
                "completed",
                Some(0),
            )],
        );

        let cmds = extract_v2(f.path(), "s1");
        assert_eq!(cmds[0].output_content.as_deref(), Some("a\nb"));
        assert_eq!(cmds[0].output_len, Some(3));
    }

    #[test]
    fn detects_error_via_status() {
        let (f, conn) = seed_db_v2();
        insert_v2_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        insert_v2_message(
            &conn,
            "s1",
            "m1",
            0,
            &[v2_tool_json(
                "shell",
                "c",
                "false",
                &["boom"],
                "error",
                None,
            )],
        );
        assert!(extract_v2(f.path(), "s1")[0].is_error);
    }

    #[test]
    fn detects_error_via_exit_code() {
        let (f, conn) = seed_db_v2();
        insert_v2_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        insert_v2_message(
            &conn,
            "s1",
            "m1",
            0,
            &[v2_tool_json(
                "shell",
                "c",
                "cargo test",
                &["failures"],
                "completed",
                Some(101),
            )],
        );
        assert!(extract_v2(f.path(), "s1")[0].is_error);
    }

    #[test]
    fn truncates_output_content_keeps_full_length() {
        let (f, conn) = seed_db_v2();
        insert_v2_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        let big = "x".repeat(1200);
        insert_v2_message(
            &conn,
            "s1",
            "m1",
            0,
            &[v2_tool_json(
                "shell",
                "c",
                "cat big",
                &[&big],
                "completed",
                Some(0),
            )],
        );

        let cmds = extract_v2(f.path(), "s1");
        assert_eq!(
            cmds[0].output_content.as_deref().unwrap().chars().count(),
            1000
        );
        assert_eq!(cmds[0].output_len, Some(1200));
    }

    #[test]
    fn orders_by_seq_then_content_position() {
        let (f, conn) = seed_db_v2();
        insert_v2_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        // Inserted out of seq order on purpose: extraction must follow seq.
        insert_v2_message(
            &conn,
            "s1",
            "m2",
            5,
            &[v2_tool_json(
                "shell",
                "c3",
                "third",
                &[],
                "completed",
                Some(0),
            )],
        );
        insert_v2_message(
            &conn,
            "s1",
            "m1",
            2,
            &[
                v2_tool_json("shell", "c1", "first", &[], "completed", Some(0)),
                v2_tool_json("shell", "c2", "second", &[], "completed", Some(0)),
            ],
        );

        let cmds = extract_v2(f.path(), "s1");
        let order: Vec<&str> = cmds.iter().map(|c| c.command.as_str()).collect();
        assert_eq!(order, ["first", "second", "third"]);
        assert_eq!(
            cmds.iter().map(|c| c.sequence_index).collect::<Vec<_>>(),
            vec![0, 1, 2]
        );
    }

    #[test]
    fn v2_preferred_over_legacy_when_both_exist() {
        // A migrated store: the same session lives in both generations; the v2
        // copy is the migrated superset and must be the one read, once.
        let (f, conn) = seed_db_v2();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY,
                directory TEXT NOT NULL,
                time_created INTEGER NOT NULL
            );
            CREATE TABLE part (
                id TEXT PRIMARY KEY,
                session_id TEXT NOT NULL,
                time_created INTEGER NOT NULL,
                data TEXT NOT NULL
            );",
        )
        .unwrap();
        insert_v2_session(&conn, "s1", "/home/u/proj", 1_700_000_000_000);
        conn.execute(
            "INSERT INTO session VALUES ('s1', '/home/u/proj', 1700000000000)",
            [],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO part VALUES ('p1', 's1', 1700000000001, ?1)",
            params![bash_json("legacy-cmd", "x", "completed", Some(0))],
        )
        .unwrap();
        insert_v2_message(
            &conn,
            "s1",
            "m1",
            0,
            &[v2_tool_json(
                "shell",
                "c",
                "v2-cmd",
                &[],
                "completed",
                Some(0),
            )],
        );

        let provider = OpenCodeProvider::with_db_path(f.path().to_path_buf());
        let cmds = extract_v2(f.path(), "s1");
        assert_eq!(cmds.len(), 1);
        assert_eq!(cmds[0].command, "v2-cmd");

        let found = provider
            .discover_sessions(ProjectFilter::All, None)
            .unwrap();
        assert_eq!(ids(&found), ["s1"]);
    }

    #[test]
    fn discover_honors_filters() {
        let (f, conn) = seed_db_v2();
        let now = now_millis();
        insert_v2_session(&conn, "recent", "/home/u/proj", now - 86_400_000);
        insert_v2_session(&conn, "old", "/home/u/proj", now - 40 * 86_400_000);
        insert_v2_session(&conn, "other", "/home/u/elsewhere", now);

        let provider = OpenCodeProvider::with_db_path(f.path().to_path_buf());
        let found = provider
            .discover_sessions(
                ProjectFilter::CurrentProject(Path::new("/home/u/proj")),
                Some(30),
            )
            .unwrap();
        assert_eq!(ids(&found), ["recent"]);
    }

    #[test]
    fn substring_is_literal_not_sql_wildcard() {
        // `--project` must be a literal substring on every provider: `_` and
        // `%` in the filter match themselves, not LIKE wildcards.
        let (f, conn) = seed_db_v2();
        let now = now_millis();
        insert_v2_session(&conn, "under", "/home/u/my_project", now);
        insert_v2_session(&conn, "dash", "/home/u/my-project", now - 1000);
        insert_v2_session(&conn, "pct", "/home/u/50x", now - 2000);

        let provider = OpenCodeProvider::with_db_path(f.path().to_path_buf());
        let found = provider
            .discover_sessions(ProjectFilter::Substring("my_project"), None)
            .unwrap();
        assert_eq!(ids(&found), ["under"]);

        let found = provider
            .discover_sessions(ProjectFilter::Substring("50%"), None)
            .unwrap();
        assert!(found.is_empty());
    }
}
