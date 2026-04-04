---
title: Tracking System
description: How RTK records token savings in SQLite and exposes aggregation APIs
sidebar:
  order: 3
---

# Tracking System

## Overview

Every RTK command execution is recorded in a local SQLite database. This data powers `rtk gain` and `rtk cc-economics`.

**Storage locations:**
- Linux: `~/.local/share/rtk/tracking.db`
- macOS: `~/Library/Application Support/rtk/tracking.db`
- Windows: `%APPDATA%\rtk\tracking.db`

**Retention:** Records older than 90 days are automatically deleted on each write.

## Data flow

```
rtk command execution
  ↓
TimedExecution::start()
  ↓
[command runs]
  ↓
TimedExecution::track(original_cmd, rtk_cmd, input, output)
  ↓
Tracker::record(original_cmd, rtk_cmd, input_tokens, output_tokens, exec_time_ms)
  ↓
SQLite INSERT
  ↓
Aggregation APIs (get_summary, get_all_days, etc.)
  ↓
rtk gain output
```

## Core API

```rust
pub struct Tracker {
    conn: Connection,    // SQLite connection
}

impl Tracker {
    pub fn new() -> Result<Self>;

    pub fn record(
        &self,
        original_cmd: &str,       // e.g., "ls -la"
        rtk_cmd: &str,             // e.g., "rtk ls"
        input_tokens: usize,       // estimate_tokens(raw_output)
        output_tokens: usize,      // estimate_tokens(filtered_output)
        exec_time_ms: u64,
    ) -> Result<()>;

    pub fn get_summary(&self) -> Result<Summary>;
    pub fn get_all_days(&self) -> Result<Vec<DailyRecord>>;
    pub fn get_weekly(&self) -> Result<Vec<WeeklyRecord>>;
    pub fn get_monthly(&self) -> Result<Vec<MonthlyRecord>>;
}
```

## Token estimation

```
estimate_tokens(text) = text.len() / 4
```

~4 characters per token average. Accuracy: ±10% vs actual LLM tokenization.

## Database schema

```sql
CREATE TABLE commands (
    id          INTEGER PRIMARY KEY,
    timestamp   DATETIME DEFAULT CURRENT_TIMESTAMP,
    original_cmd TEXT NOT NULL,
    rtk_cmd     TEXT NOT NULL,
    input_tokens INTEGER NOT NULL,
    output_tokens INTEGER NOT NULL,
    saved_tokens INTEGER GENERATED ALWAYS AS (input_tokens - output_tokens),
    savings_pct  REAL GENERATED ALWAYS AS (
        CASE WHEN input_tokens > 0
        THEN (1.0 - CAST(output_tokens AS REAL) / input_tokens) * 100
        ELSE 0 END
    ),
    exec_time_ms INTEGER
);
```

## Reporting query

```sql
SELECT
    COUNT(*) as total_commands,
    SUM(saved_tokens) as total_saved,
    AVG(savings_pct) as avg_savings,
    SUM(exec_time_ms) as total_time_ms
FROM commands
WHERE timestamp > datetime('now', '-90 days')
```

## Configuration

```toml
[tracking]
enabled = true
history_days = 90
database_path = "/custom/path/tracking.db"    # optional override
```

Environment variable override: `RTK_DB_PATH=/custom/path.db`

## Thread safety

Single-threaded execution with `Mutex<Option<Tracker>>` for future-proofing. No multi-threading currently used.
