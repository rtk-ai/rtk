//! Token savings tracking and analytics system.
//!
//! This module provides comprehensive tracking of RTK command executions,
//! recording token savings, execution times, and providing aggregation APIs
//! for daily/weekly/monthly statistics.
//!
//! # Architecture
//!
//! - Storage: SQLite database (~/.local/share/rtk/tracking.db)
//! - Retention: 90-day automatic cleanup
//! - Metrics: Input/output tokens, savings %, execution time
//!
//! # Quick Start
//!
//! ```no_run
//! use rtk::tracking::{TimedExecution, Tracker};
//!
//! // Track a command execution
//! let timer = TimedExecution::start();
//! let input = "raw output";
//! let output = "filtered output";
//! timer.track("ls -la", "rtk ls", input, output);
//!
//! // Query statistics
//! let tracker = Tracker::new().unwrap();
//! let summary = tracker.get_summary().unwrap();
//! println!("Saved {} tokens", summary.total_saved);
//! ```
//!
//! See [docs/tracking.md](../docs/tracking.md) for full documentation.

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::collections::HashMap;
use std::ffi::OsString;
use std::path::PathBuf;
use std::time::Instant;

// ── Project path helpers ── // added: project-scoped tracking support

/// Get the canonical project path string for the current working directory.
fn current_project_path_string() -> String {
    std::env::current_dir()
        .ok()
        .and_then(|p| p.canonicalize().ok())
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// Build SQL filter params for project-scoped queries.
/// Returns (exact_match, glob_prefix) for WHERE clause.
/// Uses GLOB instead of LIKE to avoid `_` and `%` in paths acting as wildcards. // changed: GLOB
fn project_filter_params(project_path: Option<&str>) -> (Option<String>, Option<String>) {
    match project_path {
        Some(p) => (
            Some(p.to_string()),
            Some(format!("{}{}*", p, std::path::MAIN_SEPARATOR)), // changed: GLOB pattern with * wildcard
        ),
        None => (None, None),
    }
}

use super::constants::{DEFAULT_HISTORY_DAYS, HISTORY_DB, RTK_DATA_DIR};

/// Main tracking interface for recording and querying command history.
///
/// Manages SQLite database connection and provides methods for:
/// - Recording command executions with token counts and timing
/// - Querying aggregated statistics (summary, daily, weekly, monthly)
/// - Retrieving recent command history
///
/// # Database Location
///
/// - Linux: `~/.local/share/rtk/tracking.db`
/// - macOS: `~/Library/Application Support/rtk/tracking.db`
/// - Windows: `%APPDATA%\rtk\tracking.db`
///
/// # Examples
///
/// ```no_run
/// use rtk::tracking::Tracker;
///
/// let tracker = Tracker::new()?;
/// tracker.record("ls -la", "rtk ls", 1000, 200, 50)?;
///
/// let summary = tracker.get_summary()?;
/// println!("Total saved: {} tokens", summary.total_saved);
/// # Ok::<(), anyhow::Error>(())
/// ```
pub struct Tracker {
    conn: Connection,
}

/// Individual command record from tracking history.
///
/// Contains timestamp, command name, and savings metrics for a single execution.
#[derive(Debug)]
pub struct CommandRecord {
    /// UTC timestamp when command was executed
    pub timestamp: DateTime<Utc>,
    /// RTK command that was executed (e.g., "rtk ls")
    pub rtk_cmd: String,
    /// Number of tokens saved (input - output)
    pub saved_tokens: usize,
    /// Savings percentage ((saved / input) * 100)
    pub savings_pct: f64,
}

/// The real outcome a PreToolUse hook reached for one Bash call.
///
/// Shared by both ends of the `hook_decisions` log: `hooks::hook_cmd` writes it at
/// the moment the hook actually runs, and `discover` reads it back to know whether
/// a historical command was truly covered. `Display`/`FromStr` are the only
/// string boundary — everywhere else this stays a typed enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HookOutcome {
    /// Rewritten and auto-allowed.
    Allow,
    /// Rewritten, but Claude Code still prompts the user.
    Ask,
    /// A deny rule matched — the hook never rewrites, defers to native deny handling.
    Deny,
    /// An unattestable construct (substitution, etc.) — the hook defers, no rewrite.
    Defer,
}

impl HookOutcome {
    /// Whether this outcome means the command was actually routed through RTK.
    pub fn is_covered(self) -> bool {
        matches!(self, HookOutcome::Allow | HookOutcome::Ask)
    }

    /// The `'static` string this variant serializes to in the `hook_decisions`
    /// table. Exists so `record_hook_decision` can bind it directly as a param
    /// (rusqlite accepts `&str`) instead of heap-allocating via `.to_string()` on
    /// every single hook invocation for what's always one of four literals.
    fn as_str(self) -> &'static str {
        match self {
            HookOutcome::Allow => "allow",
            HookOutcome::Ask => "ask",
            HookOutcome::Deny => "deny",
            HookOutcome::Defer => "defer",
        }
    }
}

impl std::fmt::Display for HookOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for HookOutcome {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s {
            "allow" => Ok(HookOutcome::Allow),
            "ask" => Ok(HookOutcome::Ask),
            "deny" => Ok(HookOutcome::Deny),
            "defer" => Ok(HookOutcome::Defer),
            _ => Err(()),
        }
    }
}

/// A single PreToolUse hook decision, logged at the moment the hook actually ran.
///
/// Keyed by `tool_use_id` so it can be joined 1:1 against the same id Claude Code
/// stores on the transcript's `tool_use`/`tool_result` blocks — giving `rtk discover`
/// ground truth about historical hook coverage instead of re-deriving a guess from
/// today's hook-install state and registry.
///
/// Only carries what `discover` actually consumes (the decision). The
/// `hook_decisions` table itself also stores `timestamp`/`raw_cmd`/`rewritten_cmd`/
/// `rtk_version` for direct inspection (`sqlite3 tracking.db`), but nothing in Rust
/// reads those back today — add fields here if/when something does.
#[derive(Debug, Clone, Copy)]
pub struct HookDecisionRecord {
    pub decision: HookOutcome,
}

/// Aggregated statistics across all recorded commands.
///
/// Provides overall metrics and breakdowns by command and by day.
/// Returned by [`Tracker::get_summary`].
#[derive(Debug)]
pub struct GainSummary {
    /// Total number of commands recorded
    pub total_commands: usize,
    /// Total input tokens across all commands
    pub total_input: usize,
    /// Total output tokens across all commands
    pub total_output: usize,
    /// Total tokens saved (input - output)
    pub total_saved: usize,
    /// Average savings percentage across all commands
    pub avg_savings_pct: f64,
    /// Total execution time across all commands (milliseconds)
    pub total_time_ms: u64,
    /// Average execution time per command (milliseconds)
    pub avg_time_ms: u64,
    /// Top 10 commands by tokens saved: (cmd, count, saved, avg_pct, avg_time_ms)
    pub by_command: Vec<(String, usize, usize, f64, u64)>,
    /// Last 30 days of activity: (date, saved_tokens)
    pub by_day: Vec<(String, usize)>,
}

/// Daily statistics for token savings and execution metrics.
///
/// Serializable to JSON for export via `rtk gain --daily --format json`.
///
/// # JSON Schema
///
/// ```json
/// {
///   "date": "2026-02-03",
///   "commands": 42,
///   "input_tokens": 15420,
///   "output_tokens": 3842,
///   "saved_tokens": 11578,
///   "savings_pct": 75.08,
///   "total_time_ms": 8450,
///   "avg_time_ms": 201
/// }
/// ```
#[derive(Debug, Serialize)]
pub struct DayStats {
    /// ISO date (YYYY-MM-DD)
    pub date: String,
    /// Number of commands executed this day
    pub commands: usize,
    /// Total input tokens for this day
    pub input_tokens: usize,
    /// Total output tokens for this day
    pub output_tokens: usize,
    /// Total tokens saved this day
    pub saved_tokens: usize,
    /// Savings percentage for this day
    pub savings_pct: f64,
    /// Total execution time for this day (milliseconds)
    pub total_time_ms: u64,
    /// Average execution time per command (milliseconds)
    pub avg_time_ms: u64,
}

/// Weekly statistics for token savings and execution metrics.
///
/// Serializable to JSON for export via `rtk gain --weekly --format json`.
/// Weeks start on Sunday (SQLite default).
#[derive(Debug, Serialize)]
pub struct WeekStats {
    /// Week start date (YYYY-MM-DD)
    pub week_start: String,
    /// Week end date (YYYY-MM-DD)
    pub week_end: String,
    /// Number of commands executed this week
    pub commands: usize,
    /// Total input tokens for this week
    pub input_tokens: usize,
    /// Total output tokens for this week
    pub output_tokens: usize,
    /// Total tokens saved this week
    pub saved_tokens: usize,
    /// Savings percentage for this week
    pub savings_pct: f64,
    /// Total execution time for this week (milliseconds)
    pub total_time_ms: u64,
    /// Average execution time per command (milliseconds)
    pub avg_time_ms: u64,
}

/// Monthly statistics for token savings and execution metrics.
///
/// Serializable to JSON for export via `rtk gain --monthly --format json`.
#[derive(Debug, Serialize)]
pub struct MonthStats {
    /// Month identifier (YYYY-MM)
    pub month: String,
    /// Number of commands executed this month
    pub commands: usize,
    /// Total input tokens for this month
    pub input_tokens: usize,
    /// Total output tokens for this month
    pub output_tokens: usize,
    /// Total tokens saved this month
    pub saved_tokens: usize,
    /// Savings percentage for this month
    pub savings_pct: f64,
    /// Total execution time for this month (milliseconds)
    pub total_time_ms: u64,
    /// Average execution time per command (milliseconds)
    pub avg_time_ms: u64,
}

/// Type alias for command statistics tuple: (command, count, saved_tokens, avg_savings_pct, avg_time_ms)
type CommandStats = (String, usize, usize, f64, u64);

/// Current tracking-DB schema version, stored in the SQLite `user_version` pragma.
///
/// `Tracker::new()` is on the hot path (every `rtk <cmd>` invocation and every
/// PreToolUse hook call), so schema creation/migration must not re-run on every
/// call. Bump this whenever `run_schema_migrations` gains a new statement; a stale
/// `user_version` triggers exactly one re-run of the full migration sequence, then
/// the pragma is updated so subsequent opens skip straight past it.
const SCHEMA_VERSION: i64 = 1;

/// Create all tables/indexes, run column migrations, and stamp `user_version` to
/// `SCHEMA_VERSION` for the on-disk tracker DB.
///
/// Runs once per database, gated by `SCHEMA_VERSION` in `Tracker::new()` — not on
/// every call, since this is otherwise on the hot path (every `rtk <cmd>` and every
/// PreToolUse hook invocation).
fn run_schema_migrations(conn: &Connection) -> Result<()> {
    conn.execute(
        "CREATE TABLE IF NOT EXISTS commands (
            id INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            original_cmd TEXT NOT NULL,
            rtk_cmd TEXT NOT NULL,
            input_tokens INTEGER NOT NULL,
            output_tokens INTEGER NOT NULL,
            saved_tokens INTEGER NOT NULL,
            savings_pct REAL NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_timestamp ON commands(timestamp)",
        [],
    )?;

    // Migration: add exec_time_ms column if it doesn't exist
    let _ = conn.execute(
        "ALTER TABLE commands ADD COLUMN exec_time_ms INTEGER DEFAULT 0",
        [],
    );
    // Migration: add project_path column with DEFAULT '' for new rows // changed: added DEFAULT
    let _ = conn.execute(
        "ALTER TABLE commands ADD COLUMN project_path TEXT DEFAULT ''",
        [],
    );
    // One-time migration: normalize NULLs from pre-default schema // changed: guarded with EXISTS
    let has_nulls: bool = conn
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM commands WHERE project_path IS NULL)",
            [],
            |row| row.get(0),
        )
        .unwrap_or(false);
    if has_nulls {
        let _ = conn.execute(
            "UPDATE commands SET project_path = '' WHERE project_path IS NULL",
            [],
        );
    }
    // Index for fast project-scoped gain queries // added
    let _ = conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_project_path_timestamp ON commands(project_path, timestamp)",
        [],
    );

    conn.execute(
        "CREATE TABLE IF NOT EXISTS parse_failures (
            id INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            raw_command TEXT NOT NULL,
            error_message TEXT NOT NULL,
            fallback_succeeded INTEGER NOT NULL DEFAULT 0
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_pf_timestamp ON parse_failures(timestamp)",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS hook_decisions (
            id INTEGER PRIMARY KEY,
            timestamp TEXT NOT NULL,
            session_id TEXT NOT NULL,
            tool_use_id TEXT NOT NULL,
            project_path TEXT DEFAULT '',
            raw_cmd TEXT NOT NULL,
            decision TEXT NOT NULL,
            rewritten_cmd TEXT,
            rtk_version TEXT NOT NULL
        )",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_hook_decisions_tool_use_id ON hook_decisions(tool_use_id)",
        [],
    )?;
    conn.execute(
        "CREATE INDEX IF NOT EXISTS idx_hook_decisions_timestamp ON hook_decisions(timestamp)",
        [],
    )?;

    conn.pragma_update(None, "user_version", SCHEMA_VERSION)?;

    Ok(())
}

/// Warn the user when a tracking-DB write fails because a table is missing.
///
/// Migrations only run once per database (gated by `SCHEMA_VERSION`/
/// `user_version` in `Tracker::new()`), so if a table is dropped or corrupted
/// out-of-band (manual `sqlite3` surgery, disk issue, partial restore) after
/// `user_version` is already current, it silently stays missing forever —
/// there's no automatic re-migration to fall back on the way there used to be
/// when every open re-ran the full schema unconditionally. Point the user at
/// the fix instead of failing silently.
fn warn_if_missing_table(context: &str, err: &rusqlite::Error) {
    if err.to_string().contains("no such table") {
        eprintln!(
            "rtk: tracking database looks corrupted ({context}: {err}). Run `rtk init` to recreate it."
        );
    }
}

/// Pure core of `Tracker::maybe_cleanup_hook_decisions`'s sampling decision.
///
/// Hashes `key` (the call's `tool_use_id`, always distinct per hook invocation)
/// rather than reading the wall clock: a nanosecond-based sample (an earlier
/// version of this used `Utc::now().timestamp_subsec_nanos() % rate`) silently
/// breaks on any clock source whose resolution isn't fine enough to hit every
/// residue mod `rate` — e.g. a coarse virtualized/Windows timer ticking in
/// large, evenly-divisible steps could make the check fire on nearly 100% of
/// calls (defeating the sampling entirely) or effectively 0% (undoing the
/// backstop against unbounded growth), depending on the tick size, with no
/// visible symptom short of profiling. Doesn't need to be cryptographically
/// random, just cheap and evenly distributed across calls — `DefaultHasher`
/// over an already-unique string avoids pulling in a `rand` dependency just for
/// a sampling backstop.
fn should_sample_cleanup(key: &str, rate: u32) -> bool {
    use std::hash::{Hash, Hasher};
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    key.hash(&mut hasher);
    hasher.finish().is_multiple_of(rate as u64)
}

impl Tracker {
    /// Create a new tracker instance.
    ///
    /// Opens or creates the SQLite database at the platform-specific location.
    /// Automatically creates the `commands` table if it doesn't exist and runs
    /// any necessary schema migrations.
    ///
    /// # Errors
    ///
    /// Returns error if:
    /// - Cannot determine database path
    /// - Cannot create parent directories
    /// - Cannot open/create SQLite database
    /// - Schema creation/migration fails
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn new() -> Result<Self> {
        Ok(Self {
            conn: open_and_prepare(MigrationMode::GatedByVersion)?,
        })
    }

    /// Create an isolated in-memory tracker for tests.
    ///
    /// Runs the same `run_schema_migrations` the real on-disk `Tracker::new()`
    /// path uses, rather than hand-duplicating the DDL — a second copy would
    /// silently drift from the real schema the next time `SCHEMA_VERSION` bumps.
    #[cfg(test)]
    pub fn new_in_memory() -> Result<Self> {
        let conn = Connection::open_in_memory().context("Failed to open in-memory DB")?;
        run_schema_migrations(&conn)?;
        Ok(Self { conn })
    }

    /// Record a command execution with token counts and timing.
    ///
    /// Calculates savings metrics and stores the record in the database.
    /// Automatically cleans up records older than 90 days after insertion.
    ///
    /// # Arguments
    ///
    /// - `original_cmd`: The standard command (e.g., "ls -la")
    /// - `rtk_cmd`: The RTK command used (e.g., "rtk ls")
    /// - `input_tokens`: Estimated tokens from standard command output
    /// - `output_tokens`: Actual tokens from RTK output
    /// - `exec_time_ms`: Execution time in milliseconds
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// tracker.record("ls -la", "rtk ls", 1000, 200, 50)?;
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn record(
        &self,
        original_cmd: &str,
        rtk_cmd: &str,
        input_tokens: usize,
        output_tokens: usize,
        exec_time_ms: u64,
    ) -> Result<()> {
        let saved = input_tokens.saturating_sub(output_tokens);
        let pct = if input_tokens > 0 {
            (saved as f64 / input_tokens as f64) * 100.0
        } else {
            0.0
        };

        let project_path = current_project_path_string(); // added: record cwd

        self.conn.execute(
            "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, project_path, input_tokens, output_tokens, saved_tokens, savings_pct, exec_time_ms)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)", // added: project_path
            params![
                Utc::now().to_rfc3339(),
                original_cmd,
                rtk_cmd,
                project_path, // added
                input_tokens as i64,
                output_tokens as i64,
                saved as i64,
                pct,
                exec_time_ms as i64
            ],
        )
        .inspect_err(|e| warn_if_missing_table("record", e))?;

        self.cleanup_old()?;
        Ok(())
    }

    fn cleanup_old(&self) -> Result<()> {
        let cutoff = Utc::now() - chrono::Duration::days(DEFAULT_HISTORY_DAYS);
        self.conn.execute(
            "DELETE FROM commands WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        self.conn.execute(
            "DELETE FROM parse_failures WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        self.conn.execute(
            "DELETE FROM hook_decisions WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Delete all tracked data (commands + parse_failures + hook_decisions), resetting all stats to zero.
    pub fn reset_all(&self) -> Result<()> {
        self.conn
            .execute_batch(
                "BEGIN;
                 DELETE FROM commands;
                 DELETE FROM parse_failures;
                 DELETE FROM hook_decisions;
                 COMMIT;",
            )
            .context("Failed to reset tracking database")?;
        Ok(())
    }

    /// Record a parse failure for analytics.
    pub fn record_parse_failure(
        &self,
        raw_command: &str,
        error_message: &str,
        fallback_succeeded: bool,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO parse_failures (timestamp, raw_command, error_message, fallback_succeeded)
             VALUES (?1, ?2, ?3, ?4)",
            params![
                Utc::now().to_rfc3339(),
                raw_command,
                error_message,
                fallback_succeeded as i32,
            ],
        )
        .inspect_err(|e| warn_if_missing_table("record_parse_failure", e))?;
        self.cleanup_old()?;
        Ok(())
    }

    /// Record a PreToolUse hook decision at the moment the hook actually ran.
    ///
    /// Deliberately does *not* run the full `cleanup_old()` (which sweeps
    /// `commands`/`parse_failures` too) unconditionally — this fires on every
    /// single Bash tool call (the hook's hot path, under the project's <10ms
    /// latency budget), so a 3-table DELETE sweep here would tax every command,
    /// not just RTK-covered ones. Retention mostly piggybacks on whatever cadence
    /// `record()`/`record_parse_failure()` already run cleanup at, but a user whose
    /// commands are almost entirely `Deny`/`Defer`'d may rarely trigger either of
    /// those, so `hook_decisions` alone still gets a `maybe_cleanup_hook_decisions`
    /// sweep — sampled, not every call — as a backstop against unbounded growth.
    #[allow(clippy::too_many_arguments)]
    pub fn record_hook_decision(
        &self,
        session_id: &str,
        tool_use_id: &str,
        project_path: &str,
        raw_cmd: &str,
        decision: HookOutcome,
        rewritten_cmd: Option<&str>,
        rtk_version: &str,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT INTO hook_decisions (timestamp, session_id, tool_use_id, project_path, raw_cmd, decision, rewritten_cmd, rtk_version)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            params![
                Utc::now().to_rfc3339(),
                session_id,
                tool_use_id,
                project_path,
                raw_cmd,
                decision.as_str(),
                rewritten_cmd,
                rtk_version,
            ],
        )
        .inspect_err(|e| warn_if_missing_table("record_hook_decision", e))?;

        // The INSERT above already committed as its own autocommit statement —
        // whatever happens to the retention sweep must not be reported as if it
        // were *this* write failing (a caller like `log_hook_decision` prints
        // "hook_decisions logging failed" on `Err`, which would be a lie here:
        // the decision genuinely was recorded and is readable by `rtk discover`).
        // Best-effort only, same as the rest of this best-effort side channel.
        if let Err(e) = self.maybe_cleanup_hook_decisions(tool_use_id) {
            eprintln!("rtk: warning: hook_decisions retention sweep failed: {e}");
        }
        Ok(())
    }

    /// Sampled retention sweep for `hook_decisions` alone (not the full 3-table
    /// `cleanup_old()`), run on roughly 1-in-`HOOK_DECISIONS_CLEANUP_SAMPLE_RATE`
    /// calls to `record_hook_decision`. Bounds the table's growth for a user whose
    /// commands are mostly `Deny`/`Defer`'d — and so rarely trigger `record()`'s or
    /// `record_parse_failure()`'s own cleanup — without paying a DELETE on every
    /// single hook invocation.
    fn maybe_cleanup_hook_decisions(&self, tool_use_id: &str) -> Result<()> {
        const HOOK_DECISIONS_CLEANUP_SAMPLE_RATE: u32 = 500;
        if !should_sample_cleanup(tool_use_id, HOOK_DECISIONS_CLEANUP_SAMPLE_RATE) {
            return Ok(());
        }
        self.cleanup_hook_decisions()
    }

    /// The actual `hook_decisions` DELETE sweep, split out from
    /// `maybe_cleanup_hook_decisions` so it's directly callable (bypassing the
    /// sampling gate) from tests without needing to hit the 1-in-N chance for real.
    fn cleanup_hook_decisions(&self) -> Result<()> {
        let cutoff = Utc::now() - chrono::Duration::days(DEFAULT_HISTORY_DAYS);
        self.conn.execute(
            "DELETE FROM hook_decisions WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        Ok(())
    }

    /// Bulk-fetch every hook decision at or after `cutoff`, keyed by `tool_use_id`.
    ///
    /// Meant to be called once per `rtk discover` run (mirroring the existing
    /// single `Config::load()`/`hook_status()` snapshot pattern), so per-command
    /// lookups during the scan loop are in-memory instead of one query each.
    pub fn hook_decisions_since(
        &self,
        cutoff: DateTime<Utc>,
    ) -> Result<HashMap<String, HookDecisionRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT tool_use_id, decision
             FROM hook_decisions
             WHERE timestamp >= ?1",
        )?;
        let rows = stmt.query_map(params![cutoff.to_rfc3339()], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;

        // Single pass: insert directly instead of collecting into an intermediate
        // Vec first and looping over that separately — this scales with
        // hook_decisions table size and runs once per `rtk discover` invocation.
        let mut out = HashMap::new();
        for row in rows {
            let (tool_use_id, decision) = row?;
            // Skip rows with an unrecognized decision string (e.g. written by a
            // future rtk version with a decision this build doesn't know) rather
            // than failing the whole query — `discover` just falls back to its
            // estimate heuristic for that command.
            if let Ok(decision) = decision.parse::<HookOutcome>() {
                out.insert(tool_use_id, HookDecisionRecord { decision });
            }
        }
        Ok(out)
    }

    /// Earliest timestamp among *currently-retained* hook decision rows, if any
    /// exist. NOT the date logging first began: `hook_decisions` is pruned by the
    /// same `DEFAULT_HISTORY_DAYS` window as the rest of the tracking DB (see
    /// `cleanup_hook_decisions`), so this rolls forward over time — on a
    /// long-running install it settles at roughly "now minus `DEFAULT_HISTORY_DAYS`",
    /// not the true install date.
    ///
    /// Used to tell `rtk discover` where the "measured" window currently starts —
    /// any scan range before this point falls back to the estimate-based heuristic,
    /// permanently (not just transitionally), since retention keeps pruning the
    /// tail as new rows come in.
    pub fn earliest_hook_decision_timestamp(&self) -> Result<Option<DateTime<Utc>>> {
        let ts: Option<String> =
            self.conn
                .query_row("SELECT MIN(timestamp) FROM hook_decisions", [], |row| {
                    row.get(0)
                })?;
        Ok(ts.and_then(|t| {
            DateTime::parse_from_rfc3339(&t)
                .ok()
                .map(|dt| dt.with_timezone(&Utc))
        }))
    }

    /// Get parse failure summary for `rtk gain --failures`.
    pub fn get_parse_failure_summary(&self) -> Result<ParseFailureSummary> {
        let total: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM parse_failures", [], |row| row.get(0))?;

        let succeeded: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM parse_failures WHERE fallback_succeeded = 1",
            [],
            |row| row.get(0),
        )?;

        let recovery_rate = if total > 0 {
            (succeeded as f64 / total as f64) * 100.0
        } else {
            0.0
        };

        // Top commands by frequency
        let mut stmt = self.conn.prepare(
            "SELECT raw_command, COUNT(*) as cnt
             FROM parse_failures
             GROUP BY raw_command
             ORDER BY cnt DESC
             LIMIT 10",
        )?;
        let top_commands = stmt
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
            })?
            .collect::<Result<Vec<_>, _>>()?;

        // Recent 10
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, raw_command, error_message, fallback_succeeded
             FROM parse_failures
             ORDER BY timestamp DESC
             LIMIT 10",
        )?;
        let recent = stmt
            .query_map([], |row| {
                Ok(ParseFailureRecord {
                    timestamp: row.get(0)?,
                    raw_command: row.get(1)?,
                    error_message: row.get(2)?,
                    fallback_succeeded: row.get::<_, i32>(3)? != 0,
                })
            })?
            .collect::<Result<Vec<_>, _>>()?;

        Ok(ParseFailureSummary {
            total: total as usize,
            recovery_rate,
            top_commands,
            recent,
        })
    }

    /// Get overall summary statistics across all recorded commands.
    ///
    /// Returns aggregated metrics including:
    /// - Total commands, tokens (input/output/saved)
    /// - Average savings percentage and execution time
    /// - Top 10 commands by tokens saved
    /// - Last 30 days of activity
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let summary = tracker.get_summary()?;
    /// println!("Saved {} tokens ({:.1}%)",
    ///     summary.total_saved, summary.avg_savings_pct);
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    #[allow(dead_code)]
    pub fn get_summary(&self) -> Result<GainSummary> {
        self.get_summary_filtered(None) // delegate to filtered variant
    }

    /// Get summary statistics filtered by project path. // added
    ///
    /// When `project_path` is `Some`, matches the exact working directory
    /// or any subdirectory (prefix match with path separator).
    pub fn get_summary_filtered(&self, project_path: Option<&str>) -> Result<GainSummary> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let mut total_commands = 0usize;
        let mut total_input = 0usize;
        let mut total_output = 0usize;
        let mut total_saved = 0usize;
        let mut total_time_ms = 0u64;

        let mut stmt = self.conn.prepare(
            "SELECT input_tokens, output_tokens, saved_tokens, exec_time_ms
             FROM commands
             WHERE (?1 IS NULL OR project_path = ?1 OR project_path GLOB ?2)", // added: project filter
        )?;

        let rows = stmt.query_map(params![project_exact, project_glob], |row| {
            // added: params
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i64>(2)? as usize,
                row.get::<_, i64>(3)? as u64,
            ))
        })?;

        for row in rows {
            let (input, output, saved, time_ms) = row?;
            total_commands += 1;
            total_input += input;
            total_output += output;
            total_saved += saved;
            total_time_ms += time_ms;
        }

        let avg_savings_pct = if total_input > 0 {
            (total_saved as f64 / total_input as f64) * 100.0
        } else {
            0.0
        };

        let avg_time_ms = if total_commands > 0 {
            total_time_ms / total_commands as u64
        } else {
            0
        };

        let by_command = self.get_by_command(project_path)?; // added: pass project filter
        let by_day = self.get_by_day(project_path)?; // added: pass project filter

        Ok(GainSummary {
            total_commands,
            total_input,
            total_output,
            total_saved,
            avg_savings_pct,
            total_time_ms,
            avg_time_ms,
            by_command,
            by_day,
        })
    }

    fn get_by_command(
        &self,
        project_path: Option<&str>, // added
    ) -> Result<Vec<CommandStats>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let mut stmt = self.conn.prepare(
            "SELECT rtk_cmd, COUNT(*), SUM(saved_tokens), AVG(savings_pct), AVG(exec_time_ms)
             FROM commands
             WHERE (?1 IS NULL OR project_path = ?1 OR project_path GLOB ?2)
             GROUP BY rtk_cmd
             ORDER BY SUM(saved_tokens) DESC
             LIMIT 10", // added: project filter in WHERE
        )?;

        let rows = stmt.query_map(params![project_exact, project_glob], |row| {
            // added: params
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i64>(2)? as usize,
                row.get::<_, f64>(3)?,
                row.get::<_, f64>(4)? as u64,
            ))
        })?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    fn get_by_day(
        &self,
        project_path: Option<&str>, // added
    ) -> Result<Vec<(String, usize)>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let mut stmt = self.conn.prepare(
            "SELECT DATE(timestamp), SUM(saved_tokens)
             FROM commands
             WHERE (?1 IS NULL OR project_path = ?1 OR project_path GLOB ?2)
             GROUP BY DATE(timestamp)
             ORDER BY DATE(timestamp) DESC
             LIMIT 30", // added: project filter in WHERE
        )?;

        let rows = stmt.query_map(params![project_exact, project_glob], |row| {
            // added: params
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as usize))
        })?;

        let mut result: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        result.reverse();
        Ok(result)
    }

    /// Get daily statistics for all recorded days.
    ///
    /// Returns one [`DayStats`] per day with commands executed, tokens saved,
    /// and execution time metrics. Results are ordered chronologically (oldest first).
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let days = tracker.get_all_days()?;
    /// for day in days.iter().take(7) {
    ///     println!("{}: {} commands, {} tokens saved",
    ///         day.date, day.commands, day.saved_tokens);
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn get_all_days(&self) -> Result<Vec<DayStats>> {
        self.get_all_days_filtered(None) // delegate to filtered variant
    }

    /// Get daily statistics filtered by project path. // added
    pub fn get_all_days_filtered(&self, project_path: Option<&str>) -> Result<Vec<DayStats>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let mut stmt = self.conn.prepare(
            "SELECT
                DATE(timestamp) as date,
                COUNT(*) as commands,
                SUM(input_tokens) as input,
                SUM(output_tokens) as output,
                SUM(saved_tokens) as saved,
                SUM(exec_time_ms) as total_time
             FROM commands
             WHERE (?1 IS NULL OR project_path = ?1 OR project_path GLOB ?2)
             GROUP BY DATE(timestamp)
             ORDER BY DATE(timestamp) DESC", // added: project filter
        )?;

        let rows = stmt.query_map(params![project_exact, project_glob], |row| {
            // added: params
            let input = row.get::<_, i64>(2)? as usize;
            let saved = row.get::<_, i64>(4)? as usize;
            let commands = row.get::<_, i64>(1)? as usize;
            let total_time = row.get::<_, i64>(5)? as u64;
            let savings_pct = if input > 0 {
                (saved as f64 / input as f64) * 100.0
            } else {
                0.0
            };
            let avg_time_ms = if commands > 0 {
                total_time / commands as u64
            } else {
                0
            };

            Ok(DayStats {
                date: row.get(0)?,
                commands,
                input_tokens: input,
                output_tokens: row.get::<_, i64>(3)? as usize,
                saved_tokens: saved,
                savings_pct,
                total_time_ms: total_time,
                avg_time_ms,
            })
        })?;

        let mut result: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        result.reverse();
        Ok(result)
    }

    /// Get weekly statistics grouped by week.
    ///
    /// Returns one [`WeekStats`] per week with aggregated metrics.
    /// Weeks start on Sunday (SQLite default). Results ordered chronologically.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let weeks = tracker.get_by_week()?;
    /// for week in weeks {
    ///     println!("{} to {}: {} tokens saved",
    ///         week.week_start, week.week_end, week.saved_tokens);
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn get_by_week(&self) -> Result<Vec<WeekStats>> {
        self.get_by_week_filtered(None) // delegate to filtered variant
    }

    /// Get weekly statistics filtered by project path. // added
    pub fn get_by_week_filtered(&self, project_path: Option<&str>) -> Result<Vec<WeekStats>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let mut stmt = self.conn.prepare(
            "SELECT
                DATE(timestamp, 'weekday 0', '-6 days') as week_start,
                DATE(timestamp, 'weekday 0') as week_end,
                COUNT(*) as commands,
                SUM(input_tokens) as input,
                SUM(output_tokens) as output,
                SUM(saved_tokens) as saved,
                SUM(exec_time_ms) as total_time
             FROM commands
             WHERE (?1 IS NULL OR project_path = ?1 OR project_path GLOB ?2)
             GROUP BY week_start
             ORDER BY week_start DESC", // added: project filter
        )?;

        let rows = stmt.query_map(params![project_exact, project_glob], |row| {
            // added: params
            let input = row.get::<_, i64>(3)? as usize;
            let saved = row.get::<_, i64>(5)? as usize;
            let commands = row.get::<_, i64>(2)? as usize;
            let total_time = row.get::<_, i64>(6)? as u64;
            let savings_pct = if input > 0 {
                (saved as f64 / input as f64) * 100.0
            } else {
                0.0
            };
            let avg_time_ms = if commands > 0 {
                total_time / commands as u64
            } else {
                0
            };

            Ok(WeekStats {
                week_start: row.get(0)?,
                week_end: row.get(1)?,
                commands,
                input_tokens: input,
                output_tokens: row.get::<_, i64>(4)? as usize,
                saved_tokens: saved,
                savings_pct,
                total_time_ms: total_time,
                avg_time_ms,
            })
        })?;

        let mut result: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        result.reverse();
        Ok(result)
    }

    /// Get monthly statistics grouped by month.
    ///
    /// Returns one [`MonthStats`] per month (YYYY-MM format) with aggregated metrics.
    /// Results ordered chronologically.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let months = tracker.get_by_month()?;
    /// for month in months {
    ///     println!("{}: {} tokens saved ({:.1}%)",
    ///         month.month, month.saved_tokens, month.savings_pct);
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    pub fn get_by_month(&self) -> Result<Vec<MonthStats>> {
        self.get_by_month_filtered(None) // delegate to filtered variant
    }

    /// Get monthly statistics filtered by project path. // added
    pub fn get_by_month_filtered(&self, project_path: Option<&str>) -> Result<Vec<MonthStats>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let mut stmt = self.conn.prepare(
            "SELECT
                strftime('%Y-%m', timestamp) as month,
                COUNT(*) as commands,
                SUM(input_tokens) as input,
                SUM(output_tokens) as output,
                SUM(saved_tokens) as saved,
                SUM(exec_time_ms) as total_time
             FROM commands
             WHERE (?1 IS NULL OR project_path = ?1 OR project_path GLOB ?2)
             GROUP BY month
             ORDER BY month DESC", // added: project filter
        )?;

        let rows = stmt.query_map(params![project_exact, project_glob], |row| {
            // added: params
            let input = row.get::<_, i64>(2)? as usize;
            let saved = row.get::<_, i64>(4)? as usize;
            let commands = row.get::<_, i64>(1)? as usize;
            let total_time = row.get::<_, i64>(5)? as u64;
            let savings_pct = if input > 0 {
                (saved as f64 / input as f64) * 100.0
            } else {
                0.0
            };
            let avg_time_ms = if commands > 0 {
                total_time / commands as u64
            } else {
                0
            };

            Ok(MonthStats {
                month: row.get(0)?,
                commands,
                input_tokens: input,
                output_tokens: row.get::<_, i64>(3)? as usize,
                saved_tokens: saved,
                savings_pct,
                total_time_ms: total_time,
                avg_time_ms,
            })
        })?;

        let mut result: Vec<_> = rows.collect::<Result<Vec<_>, _>>()?;
        result.reverse();
        Ok(result)
    }

    /// Get recent command history.
    ///
    /// Returns up to `limit` most recent command records, ordered by timestamp (newest first).
    ///
    /// # Arguments
    ///
    /// - `limit`: Maximum number of records to return
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::Tracker;
    ///
    /// let tracker = Tracker::new()?;
    /// let recent = tracker.get_recent(10)?;
    /// for cmd in recent {
    ///     println!("{}: {} saved {:.1}%",
    ///         cmd.timestamp, cmd.rtk_cmd, cmd.savings_pct);
    /// }
    /// # Ok::<(), anyhow::Error>(())
    /// ```
    #[allow(dead_code)]
    pub fn get_recent(&self, limit: usize) -> Result<Vec<CommandRecord>> {
        self.get_recent_filtered(limit, None) // delegate to filtered variant
    }

    /// Get recent command history filtered by project path. // added
    pub fn get_recent_filtered(
        &self,
        limit: usize,
        project_path: Option<&str>,
    ) -> Result<Vec<CommandRecord>> {
        let (project_exact, project_glob) = project_filter_params(project_path); // added
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, rtk_cmd, saved_tokens, savings_pct
             FROM commands
             WHERE (?1 IS NULL OR project_path = ?1 OR project_path GLOB ?2)
             ORDER BY timestamp DESC
             LIMIT ?3", // added: project filter
        )?;

        let rows = stmt.query_map(
            params![project_exact, project_glob, limit as i64], // added: project params
            |row| {
                Ok(CommandRecord {
                    timestamp: DateTime::parse_from_rfc3339(&row.get::<_, String>(0)?)
                        .map(|dt| dt.with_timezone(&Utc))
                        .unwrap_or_else(|_| Utc::now()),
                    rtk_cmd: row.get(1)?,
                    saved_tokens: row.get::<_, i64>(2)? as usize,
                    savings_pct: row.get(3)?,
                })
            },
        )?;

        Ok(rows.collect::<Result<Vec<_>, _>>()?)
    }

    /// Count commands since a given timestamp (for telemetry).
    pub fn count_commands_since(&self, since: chrono::DateTime<chrono::Utc>) -> Result<i64> {
        let ts = since.format("%Y-%m-%dT%H:%M:%S").to_string();
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM commands WHERE timestamp >= ?1",
            params![ts],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Get top N commands by frequency (for telemetry).
    pub fn top_commands(&self, limit: usize) -> Result<Vec<String>> {
        let mut stmt = self.conn.prepare(
            "SELECT rtk_cmd, COUNT(*) as cnt FROM commands
             GROUP BY rtk_cmd ORDER BY cnt DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let cmd: String = row.get(0)?;
            // Extract just the command name (e.g. "rtk git status" → "git")
            Ok(cmd.split_whitespace().nth(1).unwrap_or(&cmd).to_string())
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Get overall savings percentage (for telemetry).
    pub fn overall_savings_pct(&self) -> Result<f64> {
        let (total_input, total_saved): (i64, i64) = self.conn.query_row(
            "SELECT COALESCE(SUM(input_tokens), 0), COALESCE(SUM(saved_tokens), 0) FROM commands",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if total_input > 0 {
            Ok((total_saved as f64 / total_input as f64) * 100.0)
        } else {
            Ok(0.0)
        }
    }

    /// Get total tokens saved across all tracked commands (for telemetry).
    pub fn total_tokens_saved(&self) -> Result<i64> {
        let saved: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(saved_tokens), 0) FROM commands",
            [],
            |row| row.get(0),
        )?;
        Ok(saved)
    }

    /// Get tokens saved in the last 24 hours (for telemetry).
    pub fn tokens_saved_24h(&self, since: chrono::DateTime<chrono::Utc>) -> Result<i64> {
        let ts = since.format("%Y-%m-%dT%H:%M:%S").to_string();
        let saved: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(saved_tokens), 0) FROM commands WHERE timestamp >= ?1",
            params![ts],
            |row| row.get(0),
        )?;
        Ok(saved)
    }

    /// Top N passthrough commands (0% savings) — commands missing a filter.
    /// Groups by first word only to avoid leaking arguments into telemetry.
    pub fn top_passthrough(&self, limit: usize) -> Result<Vec<(String, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT TRIM(SUBSTR(original_cmd, 1, INSTR(original_cmd || ' ', ' ') - 1)) as tool,
             COUNT(*) as cnt FROM commands
             WHERE input_tokens = 0 AND output_tokens = 0
             GROUP BY tool ORDER BY cnt DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let cmd: String = row.get(0)?;
            let count: i64 = row.get(1)?;
            Ok((cmd, count))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Count parse failures in the last 24 hours.
    pub fn parse_failures_since(&self, since: chrono::DateTime<chrono::Utc>) -> Result<i64> {
        let ts = since.format("%Y-%m-%dT%H:%M:%S").to_string();
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM parse_failures WHERE timestamp >= ?1",
            params![ts],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Count commands with low savings (<30%) — filters that need improvement.
    pub fn low_savings_commands(&self, limit: usize) -> Result<Vec<(String, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT rtk_cmd, AVG(savings_pct) as avg_sav FROM commands
             WHERE input_tokens > 0
             GROUP BY rtk_cmd
             HAVING avg_sav < 30.0 AND avg_sav > 0.0
             ORDER BY COUNT(*) DESC LIMIT ?1",
        )?;
        let rows = stmt.query_map(params![limit as i64], |row| {
            let cmd: String = row.get(0)?;
            let sav: f64 = row.get(1)?;
            let short = cmd.split_whitespace().take(3).collect::<Vec<_>>().join(" ");
            Ok((short, sav))
        })?;
        Ok(rows.filter_map(|r| r.ok()).collect())
    }

    /// Average savings percentage per command (unweighted — each command name counts once).
    pub fn avg_savings_per_command(&self) -> Result<f64> {
        let avg: f64 = self.conn.query_row(
            "SELECT COALESCE(AVG(avg_sav), 0.0) FROM (
                SELECT rtk_cmd, AVG(savings_pct) as avg_sav
                FROM commands WHERE input_tokens > 0
                GROUP BY rtk_cmd
            )",
            [],
            |row| row.get(0),
        )?;
        Ok(avg)
    }

    /// Count invocations of a specific meta-command (by rtk_cmd suffix).
    pub fn count_meta_command(&self, name: &str) -> Result<i64> {
        let pattern = format!("rtk {}", name);
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(*) FROM commands WHERE rtk_cmd LIKE ?1 || '%'",
            params![pattern],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Days since first recorded command (installation age).
    pub fn first_seen_days(&self) -> Result<i64> {
        let oldest: Option<String> =
            match self
                .conn
                .query_row("SELECT MIN(timestamp) FROM commands", [], |row| row.get(0))
            {
                Ok(v) => v,
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(e) => return Err(anyhow::anyhow!("Failed to query first seen timestamp: {e}")),
            };
        match oldest {
            Some(ts) => {
                let first = chrono::NaiveDateTime::parse_from_str(&ts, "%Y-%m-%dT%H:%M:%S")
                    .or_else(|_| chrono::NaiveDateTime::parse_from_str(&ts, "%Y-%m-%d %H:%M:%S"))
                    .map(|dt| dt.and_utc())
                    .unwrap_or_else(|_| chrono::Utc::now());
                let days = (chrono::Utc::now() - first).num_days();
                Ok(days.max(0))
            }
            None => Ok(0),
        }
    }

    /// Number of distinct active days in the last 30 days.
    pub fn active_days_30d(&self) -> Result<i64> {
        let since = (chrono::Utc::now() - chrono::Duration::days(30))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT DATE(timestamp)) FROM commands WHERE timestamp >= ?1",
            params![since],
            |row| row.get(0),
        )?;
        Ok(count)
    }

    /// Total number of recorded commands.
    pub fn commands_total(&self) -> Result<i64> {
        let count: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM commands", [], |row| row.get(0))?;
        Ok(count)
    }

    /// Ecosystem distribution as percentages (top categories by command prefix).
    pub fn ecosystem_mix(&self) -> Result<Vec<(String, f64)>> {
        let total: f64 = self.conn.query_row(
            "SELECT COUNT(*) FROM commands WHERE input_tokens > 0 AND timestamp >= datetime('now', '-90 days')",
            [],
            |row| row.get(0),
        )?;
        if total == 0.0 {
            return Ok(vec![]);
        }
        let mut stmt = self.conn.prepare(
            "SELECT rtk_cmd, COUNT(*) as cnt FROM commands
             WHERE input_tokens > 0 AND timestamp >= datetime('now', '-90 days')
             GROUP BY rtk_cmd ORDER BY cnt DESC",
        )?;
        let mut categories: std::collections::HashMap<String, f64> =
            std::collections::HashMap::new();
        let rows = stmt.query_map([], |row| {
            let cmd: String = row.get(0)?;
            let cnt: f64 = row.get(1)?;
            Ok((cmd, cnt))
        })?;
        for row in rows.flatten() {
            let cat = categorize_command(&row.0);
            *categories.entry(cat).or_default() += row.1;
        }
        let mut result: Vec<(String, f64)> = categories
            .into_iter()
            .map(|(cat, cnt)| (cat, (cnt / total * 100.0).round()))
            .collect();
        result.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
        result.truncate(8);
        Ok(result)
    }

    /// Tokens saved in the last 30 days.
    pub fn tokens_saved_30d(&self) -> Result<i64> {
        let since = (chrono::Utc::now() - chrono::Duration::days(30))
            .format("%Y-%m-%dT%H:%M:%S")
            .to_string();
        let saved: i64 = self.conn.query_row(
            "SELECT COALESCE(SUM(saved_tokens), 0) FROM commands WHERE timestamp >= ?1",
            params![since],
            |row| row.get(0),
        )?;
        Ok(saved)
    }

    /// Number of distinct project paths.
    pub fn projects_count(&self) -> Result<i64> {
        let count: i64 = self.conn.query_row(
            "SELECT COUNT(DISTINCT project_path) FROM commands WHERE project_path != ''",
            [],
            |row| row.get(0),
        )?;
        Ok(count)
    }
}

/// Map an rtk_cmd to an ecosystem category for telemetry.
fn categorize_command(rtk_cmd: &str) -> String {
    let parts: Vec<&str> = rtk_cmd.split_whitespace().collect();
    let tool = parts.get(1).copied().unwrap_or("other");
    match tool {
        "git" | "gh" | "gt" => "git",
        "cargo" => "cargo",
        "npm" | "npx" | "pnpm" | "bun" | "bunx" | "deno" | "vitest" | "tsc" | "lint"
        | "prettier" | "next" | "playwright" | "prisma" => "js",
        "pytest" | "ruff" | "mypy" | "pip" => "python",
        "go" | "golangci-lint" => "go",
        "docker" | "kubectl" => "cloud",
        "rspec" | "rubocop" | "rake" => "ruby",
        "dotnet" => "dotnet",
        "ctest" => "cpp",
        "ls" | "tree" | "grep" | "find" | "wc" | "read" | "env" | "json" | "log" | "smart"
        | "diff" | "deps" | "summary" | "format" => "system",
        _ => "other",
    }
    .to_string()
}

/// SQLite appends `-wal`/`-shm` to the whole filename, so these are siblings
/// rather than extension swaps. Concatenate on `OsString`, not `PathBuf::push`,
/// which would append a component and silently target `history.db/-wal`.
fn restrict_db_files(db_path: &std::path::Path) {
    crate::core::utils::restrict_file(db_path);
    for sidecar in db_sidecars(db_path) {
        crate::core::utils::restrict_file(&sidecar);
    }
}

fn db_sidecars(db_path: &std::path::Path) -> Vec<PathBuf> {
    ["-wal", "-shm"]
        .iter()
        .map(|suffix| {
            let mut name = db_path.as_os_str().to_os_string();
            name.push(suffix);
            PathBuf::from(name)
        })
        .collect()
}

pub(crate) fn get_db_path() -> Result<PathBuf> {
    // Priority 1: Environment variable RTK_DB_PATH
    if let Ok(custom_path) = std::env::var("RTK_DB_PATH") {
        return Ok(PathBuf::from(custom_path));
    }

    // Priority 2: Configuration file. Reads the process-wide cached config (see
    // `config::cached_config`), not a fresh `Config::load()`: this runs inside
    // `Tracker::new()`, which `log_hook_decision` now calls on every single
    // PreToolUse hook invocation — `hook_rewrite_params()` (called earlier in the
    // same hook invocation, via `get_rewritten`) already reads config too, so
    // without caching that's two full disk-read-plus-TOML-parse round trips per
    // Bash tool call instead of one.
    if let Some(db_path) = crate::core::config::cached_config()
        .tracking
        .database_path
        .clone()
    {
        return Ok(db_path);
    }

    // Priority 3: Default platform-specific location
    let data_dir = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    Ok(data_dir.join(RTK_DATA_DIR).join(HISTORY_DB))
}

/// Whether to gate schema migrations behind `user_version` (the hot-path
/// default) or always re-run them (used by `rtk init`, which isn't a hot path
/// and wants to self-heal a dropped/corrupted table — see `ensure_schema_fresh`).
enum MigrationMode {
    GatedByVersion,
    Always,
}

/// Open (creating if needed) the tracking DB, apply pragmas, and run schema
/// migrations per `mode`. Shared by `Tracker::new()` (hot path, gated) and
/// `ensure_schema_fresh` (`rtk init`, unconditional).
fn open_and_prepare(mode: MigrationMode) -> Result<Connection> {
    let db_path = get_db_path()?;

    // `create_private_dir` is cheap even when the directory already exists (its own
    // chmod is a no-op past the first call — see `utils::set_owner_only`), so it
    // stays unconditional.
    //
    // For a brand-new DB, pre-create the file ourselves via `open_private` so
    // SQLite derives the -wal/-shm modes from an already-private DB instead of the
    // umask. For an already-existing DB, skip that pre-create's own redundant
    // open()/close() pair (a second file open on top of the `Connection::open`
    // right after it) — but still re-chmod the file *before* `Connection::open`
    // via the now-cheap `restrict_file` (a no-op stat once permissions already
    // match), rather than only healing it via `restrict_db_files` at the very
    // end. Skipping the pre-open heal entirely would let `Connection::open` and
    // any migration writes touch a file whose permissions had drifted (external
    // tampering, restored backup) before anything corrected them — the same
    // "chmod before SQLite ever opens it" invariant the original pre-create dance
    // provided, just without paying its extra open. `Tracker::new()` is on the
    // hot path for every `rtk <cmd>` and, since rtk-ai/rtk#3206's hook_decisions
    // ground-truth logging, every single PreToolUse hook call (not just
    // RTK-covered ones), so avoiding that redundant open on the already-set-up
    // common case matters for the <10ms budget.
    if let Some(parent) = db_path.parent() {
        crate::core::utils::create_private_dir(parent)?;
    }
    if db_path.exists() {
        crate::core::utils::restrict_file(&db_path);
    } else {
        crate::core::utils::open_private(
            std::fs::OpenOptions::new().write(true).create(true),
            &db_path,
        )
        .with_context(|| {
            format!(
                "Failed to pre-create private DB file: {}",
                db_path.display()
            )
        })?;
    }

    let conn = Connection::open(&db_path)?;
    // WAL mode + busy_timeout for concurrent access (multiple Claude Code instances).
    // Non-fatal: NFS/read-only filesystems may not support WAL. Cheap on every
    // open (WAL mode persists in the DB file; busy_timeout is a per-connection
    // in-memory setting), so these stay unconditional.
    let _ = conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA busy_timeout=5000;",
    );

    match mode {
        MigrationMode::Always => run_schema_migrations(&conn)?,
        MigrationMode::GatedByVersion => {
            // Schema creation/migration is comparatively expensive (several CREATE
            // TABLE/INDEX/ALTER statements plus a table scan) and `Tracker::new()` is
            // called on every `rtk <cmd>` invocation and every PreToolUse hook call, so
            // gate it behind a single cheap `user_version` read instead of re-running it
            // every time.
            let current_version: i64 =
                conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            if current_version < SCHEMA_VERSION {
                run_schema_migrations(&conn)?;
            }
        }
    }

    restrict_db_files(&db_path);

    Ok(conn)
}

/// Unconditionally re-run schema migrations, bypassing the `user_version` gate
/// `Tracker::new()` uses on its hot path. Meant to be called from `rtk init`
/// (not a hot path): this both pre-warms the schema during install/upgrade and
/// self-heals a table dropped/corrupted out-of-band after `user_version` was
/// already stamped current (see `warn_if_missing_table`) — `CREATE TABLE IF NOT
/// EXISTS`/`ALTER TABLE` are additive, so existing history is left untouched.
pub fn ensure_schema_fresh() -> Result<()> {
    open_and_prepare(MigrationMode::Always).map(|_| ())
}

/// Individual parse failure record.
#[derive(Debug)]
pub struct ParseFailureRecord {
    pub timestamp: String,
    pub raw_command: String,
    #[allow(dead_code)]
    pub error_message: String,
    pub fallback_succeeded: bool,
}

/// Aggregated parse failure summary.
#[derive(Debug)]
pub struct ParseFailureSummary {
    pub total: usize,
    pub recovery_rate: f64,
    pub top_commands: Vec<(String, usize)>,
    pub recent: Vec<ParseFailureRecord>,
}

/// Record a parse failure without ever crashing.
/// Silently ignores all errors — used in the fallback path.
pub fn record_parse_failure_silent(raw_command: &str, error_message: &str, succeeded: bool) {
    if let Ok(tracker) = Tracker::new() {
        let _ = tracker.record_parse_failure(raw_command, error_message, succeeded);
    }
}

/// Estimate token count from text using ~4 chars = 1 token heuristic.
///
/// This is a fast approximation suitable for tracking purposes.
/// For precise counts, integrate with your LLM's tokenizer API.
///
/// # Formula
///
/// `tokens = ceil(chars / 4)`
///
/// # Examples
///
/// ```
/// use rtk::tracking::estimate_tokens;
///
/// assert_eq!(estimate_tokens(""), 0);
/// assert_eq!(estimate_tokens("abcd"), 1);  // 4 chars = 1 token
/// assert_eq!(estimate_tokens("abcde"), 2); // 5 chars = ceil(1.25) = 2
/// assert_eq!(estimate_tokens("hello world"), 3); // 11 chars = ceil(2.75) = 3
/// ```
pub fn estimate_tokens(text: &str) -> usize {
    // ~4 chars per token on average
    (text.len() as f64 / 4.0).ceil() as usize
}

/// Helper struct for timing command execution
/// Helper for timing command execution and tracking results.
///
/// Preferred API for tracking commands. Automatically measures execution time
/// and records token savings. Use instead of the deprecated [`track`] function.
///
/// # Examples
///
/// ```no_run
/// use rtk::tracking::TimedExecution;
///
/// let timer = TimedExecution::start();
/// let input = execute_standard_command()?;
/// let output = execute_rtk_command()?;
/// timer.track("ls -la", "rtk ls", &input, &output);
/// # Ok::<(), anyhow::Error>(())
/// ```
pub struct TimedExecution {
    start: Instant,
}

impl TimedExecution {
    /// Start timing a command execution.
    ///
    /// Creates a new timer that starts measuring elapsed time immediately.
    /// Call [`track`](Self::track) or [`track_passthrough`](Self::track_passthrough)
    /// when the command completes.
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::TimedExecution;
    ///
    /// let timer = TimedExecution::start();
    /// // ... execute command ...
    /// timer.track("cmd", "rtk cmd", "input", "output");
    /// ```
    pub fn start() -> Self {
        Self {
            start: Instant::now(),
        }
    }

    /// Track the command with elapsed time and token counts.
    ///
    /// Records the command execution with:
    /// - Elapsed time since [`start`](Self::start)
    /// - Token counts estimated from input/output strings
    /// - Calculated savings metrics
    ///
    /// # Arguments
    ///
    /// - `original_cmd`: Standard command (e.g., "ls -la")
    /// - `rtk_cmd`: RTK command used (e.g., "rtk ls")
    /// - `input`: Standard command output (for token estimation)
    /// - `output`: RTK command output (for token estimation)
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::TimedExecution;
    ///
    /// let timer = TimedExecution::start();
    /// let input = "long output...";
    /// let output = "short output";
    /// timer.track("ls -la", "rtk ls", input, output);
    /// ```
    pub fn track(&self, original_cmd: &str, rtk_cmd: &str, input: &str, output: &str) {
        let elapsed_ms = self.start.elapsed().as_millis() as u64;
        let input_tokens = estimate_tokens(input);
        let output_tokens = estimate_tokens(output);

        if let Ok(tracker) = Tracker::new() {
            let _ = tracker.record(
                original_cmd,
                rtk_cmd,
                input_tokens,
                output_tokens,
                elapsed_ms,
            );
        }
    }

    /// Track passthrough commands (timing-only, no token counting).
    ///
    /// For commands that stream output or run interactively where output
    /// cannot be captured. Records execution time but sets tokens to 0
    /// (does not dilute savings statistics).
    ///
    /// # Arguments
    ///
    /// - `original_cmd`: Standard command (e.g., "git tag --list")
    /// - `rtk_cmd`: RTK command used (e.g., "rtk git tag --list")
    ///
    /// # Examples
    ///
    /// ```no_run
    /// use rtk::tracking::TimedExecution;
    ///
    /// let timer = TimedExecution::start();
    /// // ... execute streaming command ...
    /// timer.track_passthrough("git tag", "rtk git tag");
    /// ```
    pub fn track_passthrough(&self, original_cmd: &str, rtk_cmd: &str) {
        let elapsed_ms = self.start.elapsed().as_millis() as u64;
        // input_tokens=0, output_tokens=0 won't dilute savings statistics
        if let Ok(tracker) = Tracker::new() {
            let _ = tracker.record(original_cmd, rtk_cmd, 0, 0, elapsed_ms);
        }
    }
}

/// Format OsString args for tracking display.
///
/// Joins arguments with spaces, converting each to UTF-8 (lossy).
/// Useful for displaying command arguments in tracking records.
///
/// # Examples
///
/// ```
/// use std::ffi::OsString;
/// use rtk::tracking::args_display;
///
/// let args = vec![OsString::from("status"), OsString::from("--short")];
/// assert_eq!(args_display(&args), "status --short");
/// ```
pub fn args_display(args: &[OsString]) -> String {
    args.iter()
        .map(|a| a.to_string_lossy())
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Serializes tests that mutate the process-global `RTK_DB_PATH` env var.
    /// Must be a single shared static: a `static` declared inside each test
    /// function body is a distinct static per function, not a shared lock, so
    /// tests using separate locals don't actually serialize against each other
    /// and can race on the same global env var under parallel `cargo test`.
    static ENV_LOCK: Mutex<()> = Mutex::new(());

    // 1. estimate_tokens — verify ~4 chars/token ratio
    #[test]
    fn test_estimate_tokens() {
        assert_eq!(estimate_tokens(""), 0);
        assert_eq!(estimate_tokens("abcd"), 1); // 4 chars = 1 token
        assert_eq!(estimate_tokens("abcde"), 2); // 5 chars = ceil(1.25) = 2
        assert_eq!(estimate_tokens("a"), 1); // 1 char = ceil(0.25) = 1
        assert_eq!(estimate_tokens("12345678"), 2); // 8 chars = 2 tokens
    }

    // 2. args_display — format OsString vec
    #[test]
    fn test_args_display() {
        let args = vec![OsString::from("status"), OsString::from("--short")];
        assert_eq!(args_display(&args), "status --short");
        assert_eq!(args_display(&[]), "");

        let single = vec![OsString::from("log")];
        assert_eq!(args_display(&single), "log");
    }

    // 3. Tracker::record + get_recent — round-trip DB
    #[test]
    fn test_tracker_record_and_recent() {
        // In-memory: isolated per-test DB, immune to the shared on-disk DB's
        // cross-test races under parallel `cargo test` (see get_recent(N) racing
        // against concurrent inserts from other tests hitting the same file).
        let tracker = Tracker::new_in_memory().expect("Failed to create tracker");

        let test_cmd = "rtk git status test";

        tracker
            .record("git status", test_cmd, 100, 20, 50)
            .expect("Failed to record");

        let recent = tracker.get_recent(10).expect("Failed to get recent");

        // Find our specific test record
        let test_record = recent
            .iter()
            .find(|r| r.rtk_cmd == test_cmd)
            .expect("Test record not found in recent commands");

        assert_eq!(test_record.saved_tokens, 80);
        assert_eq!(test_record.savings_pct, 80.0);
    }

    // 4. track_passthrough doesn't dilute stats (input=0, output=0)
    #[test]
    fn test_track_passthrough_no_dilution() {
        // In-memory: isolated per-test DB, see test_tracker_record_and_recent.
        let tracker = Tracker::new_in_memory().expect("Failed to create tracker");

        let cmd1 = "rtk cmd1_test";
        let cmd2 = "rtk cmd2_passthrough_test";

        // Record one real command with 80% savings
        tracker
            .record("cmd1", cmd1, 1000, 200, 10)
            .expect("Failed to record cmd1");

        // Record passthrough (0, 0)
        tracker
            .record("cmd2", cmd2, 0, 0, 5)
            .expect("Failed to record passthrough");

        // Verify both records exist in recent history
        let recent = tracker.get_recent(20).expect("Failed to get recent");

        let record1 = recent
            .iter()
            .find(|r| r.rtk_cmd == cmd1)
            .expect("cmd1 record not found");
        let record2 = recent
            .iter()
            .find(|r| r.rtk_cmd == cmd2)
            .expect("passthrough record not found");

        // Verify cmd1 has 80% savings
        assert_eq!(record1.saved_tokens, 800);
        assert_eq!(record1.savings_pct, 80.0);

        // Verify passthrough has 0% savings
        assert_eq!(record2.saved_tokens, 0);
        assert_eq!(record2.savings_pct, 0.0);

        // This validates that passthrough (0 input, 0 output) doesn't dilute stats
        // because the savings calculation is correct for both cases
    }

    // 5. TimedExecution::track records with exec_time > 0
    //
    // TimedExecution::track() hardcodes Tracker::new() internally (real
    // on-disk DB via get_db_path()), so it can't take an in-memory tracker
    // like the tests above. Point RTK_DB_PATH at a private temp file instead —
    // otherwise get_recent(5) races against every other test concurrently
    // inserting into the same shared default DB and can miss this test's own
    // record once 5+ other rows land first.
    #[test]
    fn test_timed_execution_records_time() {
        use std::env;
        let _guard = ENV_LOCK.lock().unwrap();
        let db_path = env::temp_dir().join(format!(
            "rtk_test_timed_exec_records_{}.db",
            std::process::id()
        ));
        // nosemgrep: filesystem-deletion -- test-only cleanup of this test's own throwaway temp DB file, not production/user data.
        let _ = std::fs::remove_file(&db_path);
        temp_env::with_var("RTK_DB_PATH", Some(&db_path), || {
            let timer = TimedExecution::start();
            std::thread::sleep(std::time::Duration::from_millis(10));
            timer.track("test cmd", "rtk test", "raw input data", "filtered");

            // Verify via DB that record exists
            let tracker = Tracker::new().expect("Failed to create tracker");
            let recent = tracker.get_recent(5).expect("Failed to get recent");
            assert!(recent.iter().any(|r| r.rtk_cmd == "rtk test"));
        });
        // nosemgrep: filesystem-deletion -- test-only cleanup of this test's own throwaway temp DB file, not production/user data.
        let _ = std::fs::remove_file(&db_path);
    }

    // 6. TimedExecution::track_passthrough records with 0 tokens
    // Same isolation rationale as test_timed_execution_records_time above.
    #[test]
    fn test_timed_execution_passthrough() {
        use std::env;
        let _guard = ENV_LOCK.lock().unwrap();
        let db_path = env::temp_dir().join(format!(
            "rtk_test_timed_exec_passthrough_{}.db",
            std::process::id()
        ));
        // nosemgrep: filesystem-deletion -- test-only cleanup of this test's own throwaway temp DB file, not production/user data.
        let _ = std::fs::remove_file(&db_path);
        temp_env::with_var("RTK_DB_PATH", Some(&db_path), || {
            let timer = TimedExecution::start();
            timer.track_passthrough("git tag", "rtk git tag (passthrough)");

            let tracker = Tracker::new().expect("Failed to create tracker");
            let recent = tracker.get_recent(5).expect("Failed to get recent");

            let pt = recent
                .iter()
                .find(|r| r.rtk_cmd.contains("passthrough"))
                .expect("Passthrough record not found");

            // savings_pct should be 0 for passthrough
            assert_eq!(pt.savings_pct, 0.0);
            assert_eq!(pt.saved_tokens, 0);
        });
        // nosemgrep: filesystem-deletion -- test-only cleanup of this test's own throwaway temp DB file, not production/user data.
        let _ = std::fs::remove_file(&db_path);
    }

    // 7. get_db_path respects environment variable RTK_DB_PATH
    // 8. get_db_path falls back to default when no custom config
    // Combined into one test so the set and unset cases cannot interleave.
    #[test]
    fn test_db_path_env_and_default() {
        use std::env;
        let _guard = ENV_LOCK.lock().unwrap();

        let custom_path = env::temp_dir().join("rtk_test_custom.db");
        temp_env::with_var("RTK_DB_PATH", Some(&custom_path), || {
            let db_path = get_db_path().expect("Failed to get db path");
            assert_eq!(db_path, custom_path);
        });

        temp_env::with_var_unset("RTK_DB_PATH", || {
            let db_path = get_db_path().expect("Failed to get db path");
            assert!(
                db_path.ends_with("rtk/history.db"),
                "expected default path ending with rtk/history.db, got: {}",
                db_path.display()
            );
        });
    }

    // 8b. Tracker::new() gates schema migration behind PRAGMA user_version, so a
    // fresh DB gets stamped to SCHEMA_VERSION and a second open on the same file
    // still works (and doesn't re-run/fail the migration).
    #[test]
    fn test_schema_migration_gated_by_user_version() {
        use std::env;
        let _guard = ENV_LOCK.lock().unwrap();

        let db_path =
            env::temp_dir().join(format!("rtk_test_schema_version_{}.db", std::process::id()));
        // nosemgrep: filesystem-deletion -- test-only cleanup of this test's own throwaway temp DB file, not production/user data.
        let _ = std::fs::remove_file(&db_path);
        temp_env::with_var("RTK_DB_PATH", Some(&db_path), || {
            let tracker = Tracker::new().expect("first open should run migrations");
            let version: i64 = tracker
                .conn
                .query_row("PRAGMA user_version", [], |row| row.get(0))
                .expect("user_version should be readable");
            assert_eq!(version, SCHEMA_VERSION);
            drop(tracker);

            // Second open on the same file must skip migrations without erroring, and
            // the DB must still be fully usable (tables from the first open persist).
            let tracker2 = Tracker::new().expect("second open should skip migrations cleanly");
            tracker2
                .record("git status", "rtk git status", 100, 20, 50)
                .expect("commands table should already exist and accept writes");
        });
        // nosemgrep: filesystem-deletion -- test-only cleanup of this test's own throwaway temp DB file, not production/user data.
        let _ = std::fs::remove_file(&db_path);
    }

    // 8c. Re-running run_schema_migrations() unconditionally (what
    // ensure_schema_fresh()/MigrationMode::Always does, vs. Tracker::new()'s
    // gated hot path) recreates a table dropped out-of-band, even though
    // user_version is already at SCHEMA_VERSION. This is the self-heal `rtk
    // init` relies on instead of a dedicated repair flag.
    //
    // Exercises the migration function directly on its own throwaway on-disk
    // connection rather than going through ensure_schema_fresh()/Tracker::new()
    // (which read the process-global RTK_DB_PATH env var): this test doesn't
    // need the ENV_LOCK serialization those need, and — critically — never
    // leaves the *shared default* tracking DB in a dropped-table state where
    // an unrelated, concurrently-running test that opens Tracker::new()
    // without its own RTK_DB_PATH override could observe it.
    #[test]
    fn test_run_schema_migrations_heals_dropped_table_when_forced() {
        let db_path = std::env::temp_dir().join(format!(
            "rtk_test_heal_migrations_{}.db",
            std::process::id()
        ));
        // nosemgrep: filesystem-deletion -- test-only cleanup of this test's own throwaway temp DB file, not production/user data.
        let _ = std::fs::remove_file(&db_path);
        let conn = Connection::open(&db_path).expect("open should succeed");

        run_schema_migrations(&conn).expect("first migration run should succeed");
        conn.execute("DROP TABLE hook_decisions", [])
            .expect("drop should succeed");
        assert!(
            conn.execute(
                "INSERT INTO hook_decisions (timestamp, session_id, tool_use_id, raw_cmd, decision, rtk_version) VALUES ('t','s','u','c','allow','0')",
                [],
            )
            .is_err(),
            "table should genuinely be gone after DROP"
        );

        // Forced re-run (what ensure_schema_fresh does) recreates it, even
        // though user_version is already stamped to SCHEMA_VERSION.
        run_schema_migrations(&conn).expect("forced re-run should recreate the dropped table");
        conn.execute(
            "INSERT INTO hook_decisions (timestamp, session_id, tool_use_id, raw_cmd, decision, rtk_version) VALUES ('t','s','u','c','allow','0')",
            [],
        )
        .expect("hook_decisions table should exist and accept writes again");

        drop(conn);
        // nosemgrep: filesystem-deletion -- test-only cleanup of this test's own throwaway temp DB file, not production/user data.
        let _ = std::fs::remove_file(&db_path);
    }

    // 9. project_filter_params uses GLOB pattern with * wildcard // added
    #[test]
    fn test_project_filter_params_glob_pattern() {
        let (exact, glob) = project_filter_params(Some("/home/user/project"));
        assert_eq!(exact.unwrap(), "/home/user/project");
        // Must use * (GLOB) not % (LIKE) for subdirectory prefix matching
        let glob_val = glob.unwrap();
        assert!(glob_val.ends_with('*'), "GLOB pattern must end with *");
        assert!(!glob_val.contains('%'), "Must not contain LIKE wildcard %");
        assert_eq!(
            glob_val,
            format!("/home/user/project{}*", std::path::MAIN_SEPARATOR)
        );
    }

    // 10. project_filter_params returns None for None input // added
    #[test]
    fn test_project_filter_params_none() {
        let (exact, glob) = project_filter_params(None);
        assert!(exact.is_none());
        assert!(glob.is_none());
    }

    // 11. GLOB pattern safe with underscores in path names // added
    #[test]
    fn test_project_filter_params_underscore_safe() {
        // In LIKE, _ matches any single char; in GLOB, _ is literal
        let (exact, glob) = project_filter_params(Some("/home/user/my_project"));
        assert_eq!(exact.unwrap(), "/home/user/my_project");
        let glob_val = glob.unwrap();
        // _ must be preserved literally (GLOB treats _ as literal, LIKE does not)
        assert!(glob_val.contains("my_project"));
        assert_eq!(
            glob_val,
            format!("/home/user/my_project{}*", std::path::MAIN_SEPARATOR)
        );
    }

    // 12. record_parse_failure + get_parse_failure_summary roundtrip
    #[test]
    fn test_parse_failure_roundtrip() {
        // In-memory: isolated per-test DB, see test_tracker_record_and_recent.
        let tracker = Tracker::new_in_memory().expect("Failed to create tracker");
        let test_cmd = "git -C /path status test";

        tracker
            .record_parse_failure(test_cmd, "unrecognized subcommand", true)
            .expect("Failed to record parse failure");

        let summary = tracker
            .get_parse_failure_summary()
            .expect("Failed to get summary");

        assert!(summary.total >= 1);
        assert!(summary.recent.iter().any(|r| r.raw_command == test_cmd));
    }

    // 13. recovery_rate calculation
    #[test]
    fn test_parse_failure_recovery_rate() {
        // In-memory: isolated per-test DB, see test_tracker_record_and_recent.
        let tracker = Tracker::new_in_memory().expect("Failed to create tracker");

        // 2 successes, 1 failure
        tracker
            .record_parse_failure("cmd_ok1", "err", true)
            .unwrap();
        tracker
            .record_parse_failure("cmd_ok2", "err", true)
            .unwrap();
        tracker
            .record_parse_failure("cmd_fail", "err", false)
            .unwrap();

        let summary = tracker.get_parse_failure_summary().unwrap();
        // Isolated DB, so the rate is now exact: 2/3 successes ≈ 66.7%.
        assert!((summary.recovery_rate - 66.7).abs() < 0.1);
    }

    #[test]
    fn test_reset_all_clears_both_tables() {
        let tracker = Tracker::new_in_memory().expect("Failed to create in-memory tracker");
        let pid = std::process::id();

        // Insert into commands
        tracker
            .record(
                "git status",
                &format!("rtk git status reset_test_{}", pid),
                100,
                20,
                50,
            )
            .expect("Failed to record command");

        // Insert into parse_failures
        tracker
            .record_parse_failure(&format!("bad_cmd_reset_test_{}", pid), "parse error", false)
            .expect("Failed to record parse failure");

        // Reset everything
        tracker.reset_all().expect("Failed to reset");

        // Both tables should be empty
        let summary = tracker.get_summary().expect("Failed to get summary");
        assert_eq!(
            summary.total_commands, 0,
            "commands table should be empty after reset"
        );

        let failures = tracker
            .get_parse_failure_summary()
            .expect("Failed to get failure summary");
        assert_eq!(
            failures.total, 0,
            "parse_failures table should be empty after reset"
        );
    }

    #[test]
    fn test_db_sidecars_are_siblings_not_children() {
        let got = db_sidecars(std::path::Path::new("/data/rtk/history.db"));
        assert_eq!(
            got,
            vec![
                PathBuf::from("/data/rtk/history.db-wal"),
                PathBuf::from("/data/rtk/history.db-shm"),
            ],
            "PathBuf::push would yield history.db/-wal and silently harden nothing"
        );
    }

    #[test]
    #[cfg(unix)]
    fn test_restrict_db_files_covers_wal_sidecars() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().expect("tempdir");
        let db = tmp.path().join("history.db");
        let wal = tmp.path().join("history.db-wal");
        let shm = tmp.path().join("history.db-shm");
        for p in [&db, &wal, &shm] {
            std::fs::write(p, b"x").expect("write");
            std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o644)).expect("chmod");
        }

        restrict_db_files(&db);

        for p in [&db, &wal, &shm] {
            let mode = std::fs::metadata(p).expect("metadata").permissions().mode() & 0o777;
            assert_eq!(mode, 0o600, "expected 0600 on {}", p.display());
        }
    }

    // rtk-ai/rtk#3148: ground-truth hook-decision logging, so `discover` can join
    // real hook outcomes back to transcript entries by `tool_use_id` instead of
    // re-deriving a guess from today's hook/config state.
    #[test]
    fn test_record_and_lookup_hook_decision_by_tool_use_id() {
        let tracker = Tracker::new_in_memory().expect("Failed to create in-memory tracker");

        tracker
            .record_hook_decision(
                "session-1",
                "toolu_abc123",
                "/home/user/project",
                "git status",
                HookOutcome::Allow,
                Some("rtk git status"),
                "0.42.4",
            )
            .expect("Failed to record hook decision");

        let cutoff = Utc::now() - chrono::Duration::days(1);
        let log = tracker
            .hook_decisions_since(cutoff)
            .expect("Failed to query hook decisions");

        let record = log
            .get("toolu_abc123")
            .expect("record not found by tool_use_id");
        assert_eq!(record.decision, HookOutcome::Allow);

        // The other columns aren't exposed on HookDecisionRecord (nothing in Rust
        // reads them back yet), but they must still be persisted correctly for
        // direct DB inspection.
        let (raw_cmd, rewritten_cmd, rtk_version): (String, Option<String>, String) = tracker
            .conn
            .query_row(
                "SELECT raw_cmd, rewritten_cmd, rtk_version FROM hook_decisions WHERE tool_use_id = ?1",
                params!["toolu_abc123"],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("row not found");
        assert_eq!(raw_cmd, "git status");
        assert_eq!(rewritten_cmd.as_deref(), Some("rtk git status"));
        assert_eq!(rtk_version, "0.42.4");
    }

    #[test]
    fn test_hook_decisions_since_excludes_older_rows() {
        let tracker = Tracker::new_in_memory().expect("Failed to create in-memory tracker");

        tracker
            .record_hook_decision(
                "session-1",
                "toolu_old",
                "",
                "git status",
                HookOutcome::Deny,
                None,
                "0.42.4",
            )
            .expect("Failed to record hook decision");

        // Cutoff in the future — the row above should not appear.
        let cutoff = Utc::now() + chrono::Duration::days(1);
        let log = tracker
            .hook_decisions_since(cutoff)
            .expect("Failed to query hook decisions");

        assert!(!log.contains_key("toolu_old"));
    }

    #[test]
    fn test_earliest_hook_decision_timestamp() {
        let tracker = Tracker::new_in_memory().expect("Failed to create in-memory tracker");

        assert!(
            tracker
                .earliest_hook_decision_timestamp()
                .expect("query failed")
                .is_none()
        );

        tracker
            .record_hook_decision(
                "s",
                "toolu_1",
                "",
                "ls",
                HookOutcome::Allow,
                Some("rtk ls"),
                "0.42.4",
            )
            .expect("Failed to record hook decision");

        assert!(
            tracker
                .earliest_hook_decision_timestamp()
                .expect("query failed")
                .is_some()
        );
    }

    #[test]
    fn test_reset_all_clears_hook_decisions() {
        let tracker = Tracker::new_in_memory().expect("Failed to create in-memory tracker");

        tracker
            .record_hook_decision(
                "s",
                "toolu_1",
                "",
                "ls",
                HookOutcome::Allow,
                Some("rtk ls"),
                "0.42.4",
            )
            .expect("Failed to record hook decision");

        tracker.reset_all().expect("Failed to reset");

        assert!(
            tracker
                .earliest_hook_decision_timestamp()
                .expect("query failed")
                .is_none()
        );
    }

    // rtk-ai/rtk#3206 review: hook_decisions grows unbounded for a user whose
    // commands are mostly Deny/Defer'd, since record_hook_decision deliberately
    // skips the full cleanup_old() sweep and those decisions rarely trigger
    // record()/record_parse_failure()'s own cleanup. maybe_cleanup_hook_decisions
    // is a sampled backstop against that.

    #[test]
    fn test_should_sample_cleanup_pure() {
        // Deterministic for a given key (same tool_use_id always samples the same
        // way), and observably not "always true"/"always false" across a spread of
        // distinct keys — the actual hit rate isn't asserted (that would pin the
        // hash implementation), just that it's a real subset.
        let sampled: Vec<bool> = (0..2000)
            .map(|i| should_sample_cleanup(&format!("toolu_{i}"), 500))
            .collect();
        assert!(sampled.iter().any(|&s| s), "some keys should sample");
        assert!(sampled.iter().any(|&s| !s), "most keys should not sample");

        // Same key, same rate → same decision every time.
        let a = should_sample_cleanup("toolu_stable", 500);
        let b = should_sample_cleanup("toolu_stable", 500);
        assert_eq!(a, b);
    }

    #[test]
    fn test_cleanup_hook_decisions_deletes_only_stale_rows() {
        let tracker = Tracker::new_in_memory().expect("Failed to create in-memory tracker");

        // Recent row, via the real API.
        tracker
            .record_hook_decision(
                "s",
                "toolu_recent",
                "",
                "ls",
                HookOutcome::Allow,
                Some("rtk ls"),
                "0.42.4",
            )
            .expect("Failed to record hook decision");

        // Stale row, inserted directly: record_hook_decision always stamps
        // Utc::now(), so there's no other way to get an old row into the table.
        let stale_ts = (Utc::now() - chrono::Duration::days(DEFAULT_HISTORY_DAYS + 1)).to_rfc3339();
        tracker
            .conn
            .execute(
                "INSERT INTO hook_decisions (timestamp, session_id, tool_use_id, project_path, raw_cmd, decision, rewritten_cmd, rtk_version)
                 VALUES (?1, 's', 'toolu_stale', '', 'ls', 'allow', 'rtk ls', '0.42.4')",
                params![stale_ts],
            )
            .expect("Failed to insert stale row");

        // Call the sweep directly, bypassing maybe_cleanup_hook_decisions' sampling
        // gate — this test isn't exercising the sampling decision, just the DELETE.
        tracker
            .cleanup_hook_decisions()
            .expect("cleanup should succeed");

        let far_past = DateTime::<Utc>::MIN_UTC;
        let all = tracker
            .hook_decisions_since(far_past)
            .expect("query failed");
        assert!(
            all.contains_key("toolu_recent"),
            "recent row should survive cleanup"
        );
        assert!(
            !all.contains_key("toolu_stale"),
            "stale row should be deleted by cleanup"
        );
    }

    #[test]
    fn test_categorize_bun_and_deno_as_js() {
        for cmd in ["rtk bun install", "rtk bunx cowsay", "rtk deno test"] {
            assert_eq!(categorize_command(cmd), "js", "{cmd}");
        }
    }
}
