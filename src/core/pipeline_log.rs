//! The opt-in pipeline decision log.
//!
//! JSON lines at `~/.local/share/rtk/pipeline.log`, written after stdout is
//! flushed, same discipline as `hook_decisions`. Each entry records
//! **decisions, never content**: no tool output, no file contents, no
//! directive text. Off by default; `rtk pipeline log on` enables it by
//! dropping a marker file next to the log.

use crate::core::user_dirs;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LogEntry {
    pub ts: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<String>,
    pub site: String,
    pub event: String,
    pub subject: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub command: Option<String>,
    pub rule: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub methods: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_before: Option<usize>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tokens_after: Option<usize>,
    #[serde(default)]
    pub shadow: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recovery: Option<String>,
}

pub fn log_path() -> Option<PathBuf> {
    user_dirs::data().map(|dir| dir.join("pipeline.log"))
}

fn marker_path() -> Option<PathBuf> {
    user_dirs::data().map(|dir| dir.join("pipeline-log.enabled"))
}

pub fn enabled() -> bool {
    marker_path().map(|p| p.exists()).unwrap_or(false)
}

pub fn set_enabled(on: bool) -> std::io::Result<()> {
    let Some(marker) = marker_path() else {
        return Ok(());
    };
    if on {
        if let Some(parent) = marker.parent() {
            std::fs::create_dir_all(parent)?;
        }
        std::fs::write(&marker, b"")
    } else {
        match std::fs::remove_file(&marker) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e),
        }
    }
}

pub fn now_ts() -> String {
    chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true)
}

/// Append one entry. Silent and fail-open: the log is a side channel and
/// must never affect a hook's real output.
pub fn append(entry: &LogEntry) {
    if !enabled() {
        return;
    }
    write_now(entry);
}

fn write_now(entry: &LogEntry) {
    let Some(path) = log_path() else {
        return;
    };
    let Ok(line) = serde_json::to_string(entry) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(file, "{line}");
    }
}

std::thread_local! {
    static QUEUE: std::cell::RefCell<Vec<LogEntry>> = const { std::cell::RefCell::new(Vec::new()) };
}

/// Buffer an entry to be written by [`flush`] after the hook's stdout is
/// flushed — the log must never sit in front of the response.
pub fn queue(entry: LogEntry) {
    if !enabled() {
        return;
    }
    QUEUE.with(|q| q.borrow_mut().push(entry));
}

pub fn flush() {
    let entries: Vec<LogEntry> = QUEUE.with(|q| q.borrow_mut().drain(..).collect());
    for entry in &entries {
        write_now(entry);
    }
}

pub fn read_last(limit: usize, rule: Option<&str>) -> Vec<LogEntry> {
    let Some(path) = log_path() else {
        return Vec::new();
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    let mut entries: Vec<LogEntry> = text
        .lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .filter(|e: &LogEntry| rule.map(|r| e.rule == r).unwrap_or(true))
        .collect();
    if entries.len() > limit {
        entries.drain(..entries.len() - limit);
    }
    entries
}

pub fn render(entry: &LogEntry) -> String {
    let position = format!("{}.{}.{}", entry.site, entry.event, entry.subject);
    let what = match (&entry.action, entry.methods.is_empty()) {
        (Some(action), _) => action.clone(),
        (None, false) => entry.methods.join(","),
        (None, true) => "-".to_string(),
    };
    let tokens = match (entry.tokens_before, entry.tokens_after) {
        (Some(before), Some(after)) => format!("  {before} -> {after}"),
        _ => String::new(),
    };
    let shadow = if entry.shadow { "  (shadow)" } else { "" };
    let recovery = entry
        .recovery
        .as_deref()
        .map(|r| format!("  recovery {r}"))
        .unwrap_or_default();
    format!(
        "{}  {:<28} {:<24} {:<16} {}{}{}{}",
        entry.ts,
        position,
        entry
            .command
            .as_deref()
            .unwrap_or(entry.tool.as_deref().unwrap_or("")),
        entry.rule,
        what,
        tokens,
        shadow,
        recovery
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entries_round_trip_and_render_without_content() {
        let entry = LogEntry {
            ts: "2026-09-15T12:04:11Z".to_string(),
            session: Some("abc".to_string()),
            site: "hook".to_string(),
            event: "after_tool".to_string(),
            subject: "shell".to_string(),
            tool: Some("shell".to_string()),
            command: Some("cargo test".to_string()),
            rule: "term-default".to_string(),
            action: None,
            methods: vec!["ansi".to_string(), "cap".to_string()],
            tokens_before: Some(3812),
            tokens_after: Some(640),
            shadow: false,
            recovery: Some("recall 1f3a9c".to_string()),
        };
        let line = serde_json::to_string(&entry).expect("serializes");
        let back: LogEntry = serde_json::from_str(&line).expect("parses");
        assert_eq!(back.rule, "term-default");
        let rendered = render(&back);
        assert!(rendered.contains("hook.after_tool.shell"));
        assert!(rendered.contains("cargo test"));
        assert!(rendered.contains("3812 -> 640"));
    }
}
