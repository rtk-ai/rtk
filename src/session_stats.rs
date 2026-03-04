//! Session statistics and cache compounding savings calculator.
//!
//! Scans Claude Code JSONL session files to determine average session length,
//! then computes how RTK's direct token savings compound through prompt caching.
//!
//! Cache compounding logic:
//! - Saved tokens avoid a 1.25x cache write on the turn they're generated
//! - On every subsequent turn, saved tokens avoid a 0.1x cache read
//! - Multiplier = 1.25 + 0.1 * avg_remaining_turns

use crate::cc_economics::{WEIGHT_CACHE_CREATE, WEIGHT_CACHE_READ};
use crate::discover::provider::{ClaudeProvider, SessionProvider};
use crate::tracking::Tracker;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, IsTerminal, Write};
use std::path::Path;

const DEFAULT_AVG_TURNS: f64 = 20.0;
const CACHE_KEY: &str = "session_stats";
const CACHE_TTL_SECS: u64 = 86400; // 24 hours

#[derive(Debug, Serialize, Deserialize)]
pub struct SessionStats {
    pub sessions_analyzed: usize,
    pub avg_turns_per_session: f64,
    pub avg_remaining_turns: f64,
    pub cache_multiplier: f64,
    pub is_estimated: bool,
}

#[derive(Debug, Serialize)]
pub struct CacheCompoundingSavings {
    pub direct_saved: usize,
    pub theoretical_max_saved: usize,
    pub multiplier: f64,
    pub dollar_savings: Option<f64>,
    pub stats: SessionStats,
}

/// Count assistant turns in a single JSONL session file.
/// Uses fast string matching without full JSON parse.
pub fn count_turns_in_session(path: &Path) -> Result<usize> {
    let file = std::fs::File::open(path)
        .with_context(|| format!("failed to open session file: {}", path.display()))?;
    let reader = BufReader::new(file);
    let mut count = 0;

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.contains("\"type\":\"assistant\"") || line.contains("\"type\": \"assistant\"") {
            count += 1;
        }
    }

    Ok(count)
}

/// Compute session stats from turn counts.
/// If no session data, falls back to DEFAULT_AVG_TURNS.
fn stats_from_turns(turn_counts: &[usize]) -> SessionStats {
    if turn_counts.is_empty() {
        let avg_remaining = DEFAULT_AVG_TURNS / 2.0;
        return SessionStats {
            sessions_analyzed: 0,
            avg_turns_per_session: DEFAULT_AVG_TURNS,
            avg_remaining_turns: avg_remaining,
            cache_multiplier: WEIGHT_CACHE_CREATE + WEIGHT_CACHE_READ * avg_remaining,
            is_estimated: true,
        };
    }

    let total: usize = turn_counts.iter().sum();
    let avg = total as f64 / turn_counts.len() as f64;
    let avg_remaining = avg / 2.0;

    SessionStats {
        sessions_analyzed: turn_counts.len(),
        avg_turns_per_session: avg,
        avg_remaining_turns: avg_remaining,
        cache_multiplier: WEIGHT_CACHE_CREATE + WEIGHT_CACHE_READ * avg_remaining,
        is_estimated: false,
    }
}

/// Compute session stats with 24h SQLite cache.
/// Shows a spinner on cache miss since scanning sessions can take a few seconds.
pub fn compute_session_stats(since_days: u64) -> Result<SessionStats> {
    if let Ok(tracker) = Tracker::new() {
        if let Ok(Some(cached)) = tracker.cache_get(CACHE_KEY) {
            if let Ok(stats) = serde_json::from_str::<SessionStats>(&cached) {
                return Ok(stats);
            }
        }
    }

    let is_tty = std::io::stderr().is_terminal();
    if is_tty {
        eprint!("Computing avg session duration... ");
        let _ = std::io::stderr().flush();
    }

    let stats = scan_sessions(since_days)?;

    if is_tty {
        eprintln!("done.");
    }

    if let Ok(tracker) = Tracker::new() {
        if let Ok(json) = serde_json::to_string(&stats) {
            let _ = tracker.cache_set(CACHE_KEY, &json, CACHE_TTL_SECS);
        }
    }

    Ok(stats)
}

/// Scan Claude Code JSONL sessions and compute average turn stats.
/// Excludes subagent sessions (paths containing "/subagents/").
fn scan_sessions(since_days: u64) -> Result<SessionStats> {
    let provider = ClaudeProvider;
    let sessions = match provider.discover_sessions(None, Some(since_days)) {
        Ok(s) => s,
        Err(_) => return Ok(stats_from_turns(&[])),
    };

    let mut turn_counts = Vec::new();
    for path in &sessions {
        if path.to_string_lossy().contains("/subagents/") {
            continue;
        }

        match count_turns_in_session(path) {
            Ok(count) if count > 0 => turn_counts.push(count),
            _ => continue,
        }
    }

    Ok(stats_from_turns(&turn_counts))
}

/// Apply cache compounding multiplier to direct savings.
pub fn compute_compounding(
    direct_saved: usize,
    stats: SessionStats,
    weighted_input_cpt: Option<f64>,
) -> CacheCompoundingSavings {
    let theoretical_max = (direct_saved as f64 * stats.cache_multiplier).round() as usize;
    let dollar_savings = weighted_input_cpt.map(|cpt| theoretical_max as f64 * cpt);

    CacheCompoundingSavings {
        direct_saved,
        theoretical_max_saved: theoretical_max,
        multiplier: stats.cache_multiplier,
        dollar_savings,
        stats,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn test_default_session_stats() {
        let stats = stats_from_turns(&[]);
        assert_eq!(stats.sessions_analyzed, 0);
        assert_eq!(stats.avg_turns_per_session, 20.0);
        assert_eq!(stats.avg_remaining_turns, 10.0);
        // 1.25 + 0.1 * 10 = 2.25
        assert!((stats.cache_multiplier - 2.25).abs() < 1e-6);
        assert!(stats.is_estimated);
    }

    #[test]
    fn test_session_stats_from_turns() {
        let stats = stats_from_turns(&[100, 200, 300]);
        assert_eq!(stats.sessions_analyzed, 3);
        assert!((stats.avg_turns_per_session - 200.0).abs() < 1e-6);
        assert!((stats.avg_remaining_turns - 100.0).abs() < 1e-6);
        // 1.25 + 0.1 * 100 = 11.25
        assert!((stats.cache_multiplier - 11.25).abs() < 1e-6);
        assert!(!stats.is_estimated);
    }

    #[test]
    fn test_compute_compounding_tokens_only() {
        let stats = SessionStats {
            sessions_analyzed: 10,
            avg_turns_per_session: 200.0,
            avg_remaining_turns: 100.0,
            cache_multiplier: 11.25,
            is_estimated: false,
        };

        let result = compute_compounding(1_000_000, stats, None);
        assert_eq!(result.direct_saved, 1_000_000);
        assert_eq!(result.theoretical_max_saved, 11_250_000);
        assert!((result.multiplier - 11.25).abs() < 1e-6);
        assert!(result.dollar_savings.is_none());
    }

    #[test]
    fn test_compute_compounding_with_dollars() {
        let stats = SessionStats {
            sessions_analyzed: 10,
            avg_turns_per_session: 200.0,
            avg_remaining_turns: 100.0,
            cache_multiplier: 11.25,
            is_estimated: false,
        };

        // cpt = $3/MTok = 0.000003 per token
        let cpt = 0.000003;
        let result = compute_compounding(1_000_000, stats, Some(cpt));
        assert_eq!(result.theoretical_max_saved, 11_250_000);
        let expected_dollars = 11_250_000.0 * 0.000003; // $33.75
        assert!((result.dollar_savings.unwrap() - expected_dollars).abs() < 0.01);
    }

    #[test]
    fn test_count_turns_from_fixture() {
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tmpfile,
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[]}}}}"#
        )
        .unwrap();
        writeln!(
            tmpfile,
            r#"{{"type":"user","message":{{"role":"user","content":[]}}}}"#
        )
        .unwrap();
        writeln!(
            tmpfile,
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[]}}}}"#
        )
        .unwrap();
        writeln!(
            tmpfile,
            r#"{{"type":"user","message":{{"role":"user","content":[]}}}}"#
        )
        .unwrap();
        writeln!(
            tmpfile,
            r#"{{"type":"assistant","message":{{"role":"assistant","content":[]}}}}"#
        )
        .unwrap();
        tmpfile.flush().unwrap();

        let count = count_turns_in_session(tmpfile.path()).unwrap();
        assert_eq!(count, 3);
    }

    #[test]
    fn test_count_turns_no_assistant() {
        let mut tmpfile = tempfile::NamedTempFile::new().unwrap();
        writeln!(
            tmpfile,
            r#"{{"type":"user","message":{{"role":"user","content":[]}}}}"#
        )
        .unwrap();
        writeln!(tmpfile, r#"{{"type":"system","message":{{}}}}"#).unwrap();
        tmpfile.flush().unwrap();

        let count = count_turns_in_session(tmpfile.path()).unwrap();
        assert_eq!(count, 0);
    }

    #[test]
    fn test_single_session_stats() {
        let stats = stats_from_turns(&[50]);
        assert_eq!(stats.sessions_analyzed, 1);
        assert!((stats.avg_turns_per_session - 50.0).abs() < 1e-6);
        assert!((stats.avg_remaining_turns - 25.0).abs() < 1e-6);
        // 1.25 + 0.1 * 25 = 3.75
        assert!((stats.cache_multiplier - 3.75).abs() < 1e-6);
    }

    #[test]
    fn test_compute_compounding_zero_saved() {
        let stats = stats_from_turns(&[100]);
        let result = compute_compounding(0, stats, Some(0.000003));
        assert_eq!(result.theoretical_max_saved, 0);
        assert!((result.dollar_savings.unwrap() - 0.0).abs() < 1e-6);
    }
}
