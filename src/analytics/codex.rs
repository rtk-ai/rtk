//! Codex session analytics: parses ~/.codex/sessions/**/*.jsonl to extract
//! tool call counts and cumulative token usage per session.
//!
//! Each Codex session file is a newline-delimited JSON log. Relevant entry
//! types:
//! - `session_meta`  — session UUID, cwd, model_provider
//! - `response_item` — tool calls (`type=function_call`, `name=exec_command`)
//! - `event_msg`     — token snapshots (`type=token_count`, `info.total_token_usage`)
//!
//! Token counts in `total_token_usage` are cumulative across the session;
//! the last entry is the authoritative total.

use anyhow::Result;
use serde::Deserialize;
use std::fs;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

const CODEX_SESSIONS_DIR: &str = ".codex/sessions";

// ── Public Types ──

/// Per-session statistics parsed from a single Codex JSONL log file.
#[derive(Debug, Default)]
pub struct CodexSessionSummary {
    /// Short session identifier (first 8 chars of UUID from `session_meta`).
    pub id: String,
    /// Human-readable age: "Today", "Yesterday", "Nd ago".
    pub date: String,
    /// Number of `exec_command` function calls (CLI commands run by Codex).
    pub tool_calls: usize,
    /// Cumulative total tokens (input + output + reasoning) at session end.
    pub total_tokens: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cached_input_tokens: u64,
}

// ── JSONL Deserialization ──

#[derive(Deserialize)]
struct RawEntry {
    #[serde(rename = "type")]
    kind: String,
}

#[derive(Deserialize)]
struct SessionMetaEntry {
    payload: SessionMetaPayload,
}

#[derive(Deserialize)]
struct SessionMetaPayload {
    id: String,
}

#[derive(Deserialize)]
struct ResponseItemEntry {
    payload: ResponseItemPayload,
}

#[derive(Deserialize)]
struct ResponseItemPayload {
    #[serde(rename = "type")]
    kind: String,
    name: Option<String>,
}

#[derive(Deserialize)]
struct EventMsgEntry {
    payload: EventMsgPayload,
}

#[derive(Deserialize)]
struct EventMsgPayload {
    #[serde(rename = "type")]
    kind: String,
    info: Option<TokenCountInfo>,
}

#[derive(Deserialize)]
struct TokenCountInfo {
    total_token_usage: TotalTokenUsage,
}

#[derive(Deserialize)]
struct TotalTokenUsage {
    input_tokens: u64,
    #[serde(default)]
    cached_input_tokens: u64,
    output_tokens: u64,
    total_tokens: u64,
}

// ── Public API ──

/// Return all Codex JSONL session files modified within the last `since_days`
/// days, sorted by modification time (most recent first).
///
/// Returns an empty vec if `~/.codex/sessions` does not exist.
pub fn discover_sessions(since_days: Option<u32>) -> Vec<PathBuf> {
    let base = match dirs::home_dir() {
        Some(h) => h.join(CODEX_SESSIONS_DIR),
        None => return Vec::new(),
    };

    if !base.exists() {
        return Vec::new();
    }

    let cutoff_secs = since_days.map(|d| d as u64 * 86400);

    let mut paths: Vec<PathBuf> = WalkDir::new(&base)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_type().is_file())
        .filter(|e| {
            e.path()
                .extension()
                .and_then(|x| x.to_str())
                .map(|ext| ext == "jsonl")
                .unwrap_or(false)
        })
        .filter(|e| {
            if let Some(max_age) = cutoff_secs {
                fs::metadata(e.path())
                    .and_then(|m| m.modified())
                    .and_then(|t| {
                        t.elapsed()
                            .map_err(|err| std::io::Error::new(std::io::ErrorKind::Other, err))
                    })
                    .map(|elapsed| elapsed.as_secs() <= max_age)
                    .unwrap_or(false)
            } else {
                true
            }
        })
        .map(|e| e.path().to_path_buf())
        .collect();

    paths.sort_by(|a, b| {
        let ma = fs::metadata(a)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let mb = fs::metadata(b)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        mb.cmp(&ma)
    });

    paths
}

/// Parse a single Codex JSONL session file into a [`CodexSessionSummary`].
///
/// Counts every `exec_command` function call and reads the last cumulative
/// `total_token_usage` snapshot. Silently skips malformed lines.
pub fn parse_session(path: &Path) -> Result<CodexSessionSummary> {
    let content = fs::read_to_string(path)?;
    let mut summary = CodexSessionSummary::default();
    let mut last_usage: Option<TotalTokenUsage> = None;

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        let Ok(raw) = serde_json::from_str::<RawEntry>(line) else {
            continue;
        };

        match raw.kind.as_str() {
            "session_meta" => {
                if let Ok(meta) = serde_json::from_str::<SessionMetaEntry>(line) {
                    summary.id = meta.payload.id;
                }
            }
            "response_item" => {
                if let Ok(item) = serde_json::from_str::<ResponseItemEntry>(line) {
                    if item.payload.kind == "function_call"
                        && item.payload.name.as_deref() == Some("exec_command")
                    {
                        summary.tool_calls += 1;
                    }
                }
            }
            "event_msg" => {
                if let Ok(event) = serde_json::from_str::<EventMsgEntry>(line) {
                    if event.payload.kind == "token_count" {
                        if let Some(info) = event.payload.info {
                            last_usage = Some(info.total_token_usage);
                        }
                    }
                }
            }
            _ => {}
        }
    }

    if let Some(usage) = last_usage {
        summary.total_tokens = usage.total_tokens;
        summary.input_tokens = usage.input_tokens;
        summary.output_tokens = usage.output_tokens;
        summary.cached_input_tokens = usage.cached_input_tokens;
    }

    summary.date = mtime_label(path);

    // Shorten UUID for display; fall back to filename stem
    let raw_id = if summary.id.is_empty() {
        path.file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown")
            .to_string()
    } else {
        summary.id.clone()
    };
    summary.id = raw_id.chars().take(8).collect();

    Ok(summary)
}

// ── Helpers ──

fn mtime_label(path: &Path) -> String {
    fs::metadata(path)
        .and_then(|m| m.modified())
        .map(|t| {
            let elapsed = std::time::SystemTime::now()
                .duration_since(t)
                .unwrap_or_default();
            let days = elapsed.as_secs() / 86400;
            if days == 0 {
                "Today".to_string()
            } else if days == 1 {
                "Yesterday".to_string()
            } else {
                format!("{}d ago", days)
            }
        })
        .unwrap_or_else(|_| "?".to_string())
}

// ── Tests ──

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn write_session(lines: &[&str]) -> NamedTempFile {
        let mut f = NamedTempFile::new().unwrap();
        for line in lines {
            writeln!(f, "{}", line).unwrap();
        }
        f
    }

    #[test]
    fn test_parse_session_counts_exec_command() {
        let f = write_session(&[
            r#"{"type":"session_meta","payload":{"id":"019e2704-19e7-7a81-b927-399029e88020","timestamp":"2026-05-14T15:04:00.743Z","cwd":"/home/user/proj","originator":"codex-tui","cli_version":"0.130.0","source":{},"thread_source":"user","model_provider":"openai","base_instructions":{"text":""},"session_context_window":null}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-14T15:04:01Z","payload":{"type":"function_call","name":"exec_command","arguments":"{}","call_id":"c1"}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-14T15:04:02Z","payload":{"type":"function_call","name":"exec_command","arguments":"{}","call_id":"c2"}}"#,
            r#"{"type":"response_item","timestamp":"2026-05-14T15:04:03Z","payload":{"type":"function_call","name":"write_stdin","arguments":"{}","call_id":"c3"}}"#,
        ]);
        let s = parse_session(f.path()).unwrap();
        assert_eq!(s.tool_calls, 2, "only exec_command counts");
        assert_eq!(s.total_tokens, 0, "no token_count entry");
    }

    #[test]
    fn test_parse_session_reads_last_token_count() {
        let f = write_session(&[
            r#"{"type":"session_meta","payload":{"id":"aabbccdd-0000-0000-0000-000000000000","timestamp":"2026-05-14T15:00:00Z","cwd":"/","originator":"codex-tui","cli_version":"0.130.0","source":{},"thread_source":"user","model_provider":"openai","base_instructions":{"text":""},"session_context_window":null}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-14T15:00:01Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":1000,"cached_input_tokens":800,"output_tokens":100,"reasoning_output_tokens":20,"total_tokens":1100},"last_token_usage":{"input_tokens":1000,"cached_input_tokens":800,"output_tokens":100,"reasoning_output_tokens":20,"total_tokens":1100},"model_context_window":258400},"rate_limits":{"limit_id":"codex","limit_name":null,"primary":{"used_percent":5.0,"window_minutes":300,"resets_at":1778799411},"secondary":{"used_percent":10.0,"window_minutes":10080,"resets_at":1779368122},"credits":null,"plan_type":"plus","rate_limit_reached_type":null}}}"#,
            r#"{"type":"event_msg","timestamp":"2026-05-14T15:00:02Z","payload":{"type":"token_count","info":{"total_token_usage":{"input_tokens":5000,"cached_input_tokens":4000,"output_tokens":300,"reasoning_output_tokens":50,"total_tokens":5300},"last_token_usage":{"input_tokens":4000,"cached_input_tokens":3200,"output_tokens":200,"reasoning_output_tokens":30,"total_tokens":4200},"model_context_window":258400},"rate_limits":{"limit_id":"codex","limit_name":null,"primary":{"used_percent":7.0,"window_minutes":300,"resets_at":1778799411},"secondary":{"used_percent":15.0,"window_minutes":10080,"resets_at":1779368122},"credits":null,"plan_type":"plus","rate_limit_reached_type":null}}}"#,
        ]);
        let s = parse_session(f.path()).unwrap();
        // Should use the LAST token_count entry
        assert_eq!(s.total_tokens, 5300);
        assert_eq!(s.input_tokens, 5000);
        assert_eq!(s.output_tokens, 300);
        assert_eq!(s.cached_input_tokens, 4000);
    }

    #[test]
    fn test_parse_session_id_shortened() {
        let f = write_session(&[
            r#"{"type":"session_meta","payload":{"id":"019e2704-19e7-7a81-b927-399029e88020","timestamp":"2026-05-14T15:04:00.743Z","cwd":"/home/user/proj","originator":"codex-tui","cli_version":"0.130.0","source":{},"thread_source":"user","model_provider":"openai","base_instructions":{"text":""},"session_context_window":null}}"#,
        ]);
        let s = parse_session(f.path()).unwrap();
        assert_eq!(s.id.len(), 8);
        assert_eq!(s.id, "019e2704");
    }

    #[test]
    fn test_parse_session_empty_file() {
        let f = write_session(&[]);
        let s = parse_session(f.path()).unwrap();
        assert_eq!(s.tool_calls, 0);
        assert_eq!(s.total_tokens, 0);
    }

    #[test]
    fn test_parse_session_malformed_lines_skipped() {
        let f = write_session(&[
            r#"{"type":"session_meta","payload":{"id":"aaaabbbb-0000-0000-0000-000000000000","timestamp":"2026-05-14T15:04:00.743Z","cwd":"/","originator":"codex-tui","cli_version":"0.130.0","source":{},"thread_source":"user","model_provider":"openai","base_instructions":{"text":""},"session_context_window":null}}"#,
            "not valid json at all",
            r#"{"type":"response_item","timestamp":"t","payload":{"type":"function_call","name":"exec_command","arguments":"{}","call_id":"x"}}"#,
        ]);
        let s = parse_session(f.path()).unwrap();
        assert_eq!(s.tool_calls, 1);
    }

    #[test]
    fn test_discover_sessions_missing_dir() {
        // Should not panic if ~/.codex/sessions doesn't exist
        let paths = discover_sessions(Some(30));
        // On CI without codex, this returns empty; on dev machine it may have data.
        // Just assert it doesn't panic.
        let _ = paths;
    }
}
