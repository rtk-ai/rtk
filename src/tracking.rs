use anyhow::Result;
use chrono::{DateTime, Utc};
use rusqlite::{Connection, params};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::Mutex;

const HISTORY_DAYS: i64 = 90;

lazy_static::lazy_static! {
    static ref TRACKER: Mutex<Option<Tracker>> = Mutex::new(None);
}

pub struct Tracker {
    conn: Connection,
}

#[derive(Debug)]
pub struct CommandRecord {
    pub timestamp: DateTime<Utc>,
    pub original_cmd: String,
    pub rtk_cmd: String,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub saved_tokens: usize,
    pub savings_pct: f64,
}

#[derive(Debug)]
pub struct GainSummary {
    pub total_commands: usize,
    pub total_input: usize,
    pub total_output: usize,
    pub total_saved: usize,
    pub avg_savings_pct: f64,
    pub by_command: Vec<(String, usize, usize, f64)>,
    pub by_day: Vec<(String, usize)>,
}

#[derive(Debug, Serialize)]
pub struct DayStats {
    pub date: String,
    pub commands: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub saved_tokens: usize,
    pub savings_pct: f64,
}

#[derive(Debug, Serialize)]
pub struct WeekStats {
    pub week_start: String,
    pub week_end: String,
    pub commands: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub saved_tokens: usize,
    pub savings_pct: f64,
}

#[derive(Debug, Serialize)]
pub struct MonthStats {
    pub month: String,
    pub commands: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub saved_tokens: usize,
    pub savings_pct: f64,
}

impl Tracker {
    pub fn new() -> Result<Self> {
        let db_path = get_db_path()?;
        if let Some(parent) = db_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let conn = Connection::open(&db_path)?;
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

        Ok(Self { conn })
    }

    pub fn record(&self, original_cmd: &str, rtk_cmd: &str, input_tokens: usize, output_tokens: usize) -> Result<()> {
        let saved = input_tokens.saturating_sub(output_tokens);
        let pct = if input_tokens > 0 {
            (saved as f64 / input_tokens as f64) * 100.0
        } else {
            0.0
        };

        self.conn.execute(
            "INSERT INTO commands (timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                Utc::now().to_rfc3339(),
                original_cmd,
                rtk_cmd,
                input_tokens as i64,
                output_tokens as i64,
                saved as i64,
                pct
            ],
        )?;

        self.cleanup_old()?;
        Ok(())
    }

    fn cleanup_old(&self) -> Result<()> {
        let cutoff = Utc::now() - chrono::Duration::days(HISTORY_DAYS);
        self.conn.execute(
            "DELETE FROM commands WHERE timestamp < ?1",
            params![cutoff.to_rfc3339()],
        )?;
        Ok(())
    }

    pub fn get_summary(&self) -> Result<GainSummary> {
        let mut total_commands = 0usize;
        let mut total_input = 0usize;
        let mut total_output = 0usize;
        let mut total_saved = 0usize;

        let mut stmt = self.conn.prepare(
            "SELECT input_tokens, output_tokens, saved_tokens FROM commands"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)? as usize,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i64>(2)? as usize,
            ))
        })?;

        for row in rows {
            let (input, output, saved) = row?;
            total_commands += 1;
            total_input += input;
            total_output += output;
            total_saved += saved;
        }

        let avg_savings_pct = if total_input > 0 {
            (total_saved as f64 / total_input as f64) * 100.0
        } else {
            0.0
        };

        let by_command = self.get_by_command()?;
        let by_day = self.get_by_day()?;

        Ok(GainSummary {
            total_commands,
            total_input,
            total_output,
            total_saved,
            avg_savings_pct,
            by_command,
            by_day,
        })
    }

    fn get_by_command(&self) -> Result<Vec<(String, usize, usize, f64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT rtk_cmd, COUNT(*), SUM(saved_tokens), AVG(savings_pct)
             FROM commands
             GROUP BY rtk_cmd
             ORDER BY SUM(saved_tokens) DESC
             LIMIT 10"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as usize,
                row.get::<_, i64>(2)? as usize,
                row.get::<_, f64>(3)?,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    fn get_by_day(&self) -> Result<Vec<(String, usize)>> {
        let mut stmt = self.conn.prepare(
            "SELECT DATE(timestamp), SUM(saved_tokens)
             FROM commands
             GROUP BY DATE(timestamp)
             ORDER BY DATE(timestamp) DESC
             LIMIT 30"
        )?;

        let rows = stmt.query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)? as usize,
            ))
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        result.reverse();
        Ok(result)
    }

    pub fn get_all_days(&self) -> Result<Vec<DayStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                DATE(timestamp) as date,
                COUNT(*) as commands,
                SUM(input_tokens) as input,
                SUM(output_tokens) as output,
                SUM(saved_tokens) as saved
             FROM commands
             GROUP BY DATE(timestamp)
             ORDER BY DATE(timestamp) DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            let input = row.get::<_, i64>(2)? as usize;
            let saved = row.get::<_, i64>(4)? as usize;
            let savings_pct = if input > 0 {
                (saved as f64 / input as f64) * 100.0
            } else {
                0.0
            };

            Ok(DayStats {
                date: row.get(0)?,
                commands: row.get::<_, i64>(1)? as usize,
                input_tokens: input,
                output_tokens: row.get::<_, i64>(3)? as usize,
                saved_tokens: saved,
                savings_pct,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        result.reverse();
        Ok(result)
    }

    pub fn get_by_week(&self) -> Result<Vec<WeekStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                DATE(timestamp, 'weekday 0', '-6 days') as week_start,
                DATE(timestamp, 'weekday 0') as week_end,
                COUNT(*) as commands,
                SUM(input_tokens) as input,
                SUM(output_tokens) as output,
                SUM(saved_tokens) as saved
             FROM commands
             GROUP BY week_start
             ORDER BY week_start DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            let input = row.get::<_, i64>(3)? as usize;
            let saved = row.get::<_, i64>(5)? as usize;
            let savings_pct = if input > 0 {
                (saved as f64 / input as f64) * 100.0
            } else {
                0.0
            };

            Ok(WeekStats {
                week_start: row.get(0)?,
                week_end: row.get(1)?,
                commands: row.get::<_, i64>(2)? as usize,
                input_tokens: input,
                output_tokens: row.get::<_, i64>(4)? as usize,
                saved_tokens: saved,
                savings_pct,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        result.reverse();
        Ok(result)
    }

    pub fn get_by_month(&self) -> Result<Vec<MonthStats>> {
        let mut stmt = self.conn.prepare(
            "SELECT
                strftime('%Y-%m', timestamp) as month,
                COUNT(*) as commands,
                SUM(input_tokens) as input,
                SUM(output_tokens) as output,
                SUM(saved_tokens) as saved
             FROM commands
             GROUP BY month
             ORDER BY month DESC"
        )?;

        let rows = stmt.query_map([], |row| {
            let input = row.get::<_, i64>(2)? as usize;
            let saved = row.get::<_, i64>(4)? as usize;
            let savings_pct = if input > 0 {
                (saved as f64 / input as f64) * 100.0
            } else {
                0.0
            };

            Ok(MonthStats {
                month: row.get(0)?,
                commands: row.get::<_, i64>(1)? as usize,
                input_tokens: input,
                output_tokens: row.get::<_, i64>(3)? as usize,
                saved_tokens: saved,
                savings_pct,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        result.reverse();
        Ok(result)
    }

    pub fn get_recent(&self, limit: usize) -> Result<Vec<CommandRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT timestamp, original_cmd, rtk_cmd, input_tokens, output_tokens, saved_tokens, savings_pct
             FROM commands
             ORDER BY timestamp DESC
             LIMIT ?1"
        )?;

        let rows = stmt.query_map(params![limit as i64], |row| {
            Ok(CommandRecord {
                timestamp: DateTime::parse_from_rfc3339(&row.get::<_, String>(0)?)
                    .map(|dt| dt.with_timezone(&Utc))
                    .unwrap_or_else(|_| Utc::now()),
                original_cmd: row.get(1)?,
                rtk_cmd: row.get(2)?,
                input_tokens: row.get::<_, i64>(3)? as usize,
                output_tokens: row.get::<_, i64>(4)? as usize,
                saved_tokens: row.get::<_, i64>(5)? as usize,
                savings_pct: row.get(6)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }
}

fn get_db_path() -> Result<PathBuf> {
    let data_dir = dirs::data_local_dir()
        .unwrap_or_else(|| PathBuf::from("."));
    Ok(data_dir.join("rtk").join("history.db"))
}

pub fn estimate_tokens(text: &str) -> usize {
    // ~4 chars per token on average
    (text.len() as f64 / 4.0).ceil() as usize
}

/// Track a command execution
/// original_cmd: the equivalent standard command (e.g., "ls -la")
/// rtk_cmd: the rtk command used (e.g., "rtk ls")
/// input: estimated raw output that would have been produced
/// output: actual rtk output produced
pub fn track(original_cmd: &str, rtk_cmd: &str, input: &str, output: &str) {
    let input_tokens = estimate_tokens(input);
    let output_tokens = estimate_tokens(output);

    if let Ok(tracker) = Tracker::new() {
        let _ = tracker.record(original_cmd, rtk_cmd, input_tokens, output_tokens);
    }
}

/// Track with pre-calculated token counts
pub fn track_tokens(original_cmd: &str, rtk_cmd: &str, input_tokens: usize, output_tokens: usize) {
    if let Ok(tracker) = Tracker::new() {
        let _ = tracker.record(original_cmd, rtk_cmd, input_tokens, output_tokens);
    }
}
