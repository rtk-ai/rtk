//! Filters codegraph CLI output — strips per-file progress, truncates result lists.
//!
//! IMPORTANT: `codegraph affected-tests` is NEVER truncated — its output is consumed
//! by CI scripts. The MCP server (`codegraph serve`) is never routed here.

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;

const RESULT_LIMIT: usize = 20;

lazy_static! {
    static ref PARSING_RE: Regex = Regex::new(r"^Parsing\s+\S+\s+ok\s+\(\d+\s+symbols?\)").unwrap();
    static ref UPDATE_FILE_RE: Regex =
        Regex::new(r"^(Updating|Reparsing|Removing)\s+\S+").unwrap();
    static ref STATS_LINE_RE: Regex =
        Regex::new(r"^\s*(Files|Symbols|Edges|Errors|Languages|Repos|Indexed)\s*:").unwrap();
    static ref ERROR_RE: Regex = Regex::new(r"(?i)\b(error|failed)\b").unwrap();
}

pub fn run_index(args: &[String], verbose: u8) -> Result<i32> {
    run_index_like("index", args, verbose)
}

pub fn run_update(args: &[String], verbose: u8) -> Result<i32> {
    run_index_like("update", args, verbose)
}

fn run_index_like(sub: &'static str, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("codegraph");
    cmd.arg(sub);
    for a in args {
        cmd.arg(a);
    }
    if verbose > 0 {
        eprintln!("Running: codegraph {} {}", sub, args.join(" "));
    }
    runner::run_filtered(
        cmd,
        &format!("codegraph {}", sub),
        &args.join(" "),
        move |raw| filter_index(raw, sub),
        RunOptions::with_tee("codegraph"),
    )
}

pub fn run_stats(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("codegraph");
    cmd.arg("stats");
    for a in args {
        cmd.arg(a);
    }
    if verbose > 0 {
        eprintln!("Running: codegraph stats {}", args.join(" "));
    }
    runner::run_filtered(
        cmd,
        "codegraph stats",
        &args.join(" "),
        filter_stats,
        RunOptions::default(),
    )
}

pub fn run_search_like(sub: &'static str, args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("codegraph");
    cmd.arg(sub);
    for a in args {
        cmd.arg(a);
    }
    if verbose > 0 {
        eprintln!("Running: codegraph {} {}", sub, args.join(" "));
    }
    runner::run_filtered(
        cmd,
        &format!("codegraph {}", sub),
        &args.join(" "),
        |raw| filter_results(raw, RESULT_LIMIT),
        RunOptions::default(),
    )
}

pub fn run_affected_tests(args: &[String], verbose: u8) -> Result<i32> {
    let mut cmd = resolved_command("codegraph");
    cmd.arg("affected-tests");
    for a in args {
        cmd.arg(a);
    }
    if verbose > 0 {
        eprintln!("Running: codegraph affected-tests {}", args.join(" "));
    }
    // Pass-through completely unchanged — output is consumed by CI scripts.
    runner::run_filtered(
        cmd,
        "codegraph affected-tests",
        &args.join(" "),
        |raw| raw.trim_end().to_string(),
        RunOptions::default(),
    )
}

fn filter_index(raw: &str, sub: &str) -> String {
    let mut errors = Vec::new();
    let mut summary = SummaryStats::default();

    for line in raw.lines() {
        let trimmed = line.trim();
        if PARSING_RE.is_match(trimmed) || UPDATE_FILE_RE.is_match(trimmed) {
            continue;
        }
        if let Some(rest) = trimmed.strip_prefix("Files:") {
            summary.files = parse_first_num(rest);
        } else if let Some(rest) = trimmed.strip_prefix("Files changed:") {
            summary.files = parse_first_num(rest);
            summary.has_changed = true;
        } else if let Some(rest) = trimmed.strip_prefix("Symbols:") {
            summary.symbols = parse_first_num(rest);
        } else if let Some(rest) = trimmed.strip_prefix("New symbols:") {
            summary.new_symbols = parse_first_num(rest);
        } else if let Some(rest) = trimmed.strip_prefix("Edges:") {
            summary.edges = parse_first_num(rest);
        } else if let Some(rest) = trimmed.strip_prefix("Errors:") {
            summary.errors = parse_first_num(rest);
        } else if let Some(rest) = trimmed.strip_prefix("Time:") {
            summary.time = rest.trim().to_string();
        } else if trimmed.contains("error:") || trimmed.starts_with("Error") {
            errors.push(trimmed.to_string());
        }
    }

    let mut out = String::new();
    if sub == "update" {
        out.push_str(&format!(
            "codegraph update: {} files changed",
            summary.files.unwrap_or(0)
        ));
        if let Some(n) = summary.new_symbols {
            out.push_str(&format!("  +{} symbols", n));
        }
    } else {
        out.push_str(&format!(
            "codegraph index: {} files",
            summary.files.unwrap_or(0)
        ));
        if let Some(n) = summary.symbols {
            out.push_str(&format!("  {} symbols", n));
        }
        if let Some(n) = summary.edges {
            out.push_str(&format!("  {} edges", n));
        }
    }
    if !summary.time.is_empty() {
        out.push_str(&format!("  {}", summary.time));
    }
    if let Some(n) = summary.errors {
        if n > 0 {
            out.push_str(&format!("\nErrors: {}", n));
            for e in errors.iter().take(20) {
                out.push('\n');
                out.push_str(e);
            }
        }
    }
    out
}

#[derive(Default)]
struct SummaryStats {
    files: Option<usize>,
    symbols: Option<usize>,
    new_symbols: Option<usize>,
    edges: Option<usize>,
    errors: Option<usize>,
    time: String,
    #[allow(dead_code)]
    has_changed: bool,
}

fn parse_first_num(s: &str) -> Option<usize> {
    s.split(|c: char| !c.is_ascii_digit())
        .find(|t| !t.is_empty())
        .and_then(|t| t.parse().ok())
}

fn filter_stats(raw: &str) -> String {
    let mut out = Vec::new();
    for line in raw.lines() {
        let trimmed = line.trim_end();
        if trimmed.is_empty() {
            continue;
        }
        let stripped = trimmed.trim_start();
        if stripped.chars().all(|c| matches!(c, '-' | '=' | '*' | '+')) {
            continue;
        }
        out.push(trimmed.to_string());
    }
    out.join("\n")
}

fn filter_results(raw: &str, limit: usize) -> String {
    let mut lines: Vec<&str> = raw
        .lines()
        .filter(|l| !l.trim().is_empty())
        .collect();
    let total = lines.len();
    if total <= limit {
        return raw.trim_end().to_string();
    }
    lines.truncate(limit);
    let mut out = lines.join("\n");
    out.push_str(&format!(
        "\n(+{} more — run codegraph search directly for full results)",
        total - limit
    ));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_index_strips_parsing_lines() {
        let raw = "Parsing src/foo.rs ok (12 symbols)\n\
            Parsing src/bar.rs ok (8 symbols)\n\
            Parsing src/baz.rs ok (3 symbols)\n\
            Files: 3\n\
            Symbols: 23\n\
            Edges: 47\n\
            Errors: 0\n\
            Time: 0.4s\n";
        let out = filter_index(raw, "index");
        assert!(out.starts_with("codegraph index:"));
        assert!(out.contains("3 files"));
        assert!(out.contains("23 symbols"));
        assert!(!out.contains("Parsing"));
    }

    #[test]
    fn test_search_truncates() {
        let raw = (1..=50)
            .map(|i| format!("result_{}: foo at file{}.rs:10", i, i))
            .collect::<Vec<_>>()
            .join("\n");
        let out = filter_results(&raw, 20);
        assert!(out.contains("result_1:"));
        assert!(out.contains("result_20:"));
        assert!(!out.contains("result_21:"));
        assert!(out.contains("(+30 more"));
    }

    #[test]
    fn test_search_no_truncate_under_limit() {
        let raw = "a\nb\nc";
        assert_eq!(filter_results(raw, 20), "a\nb\nc");
    }

    #[test]
    fn test_fixture_index_verbose() {
        let raw = include_str!("../../../tests/fixtures/cpp/codegraph_index_verbose.txt");
        let out = filter_index(raw, "index");
        assert!(out.starts_with("codegraph index:"));
        assert!(out.contains("21 files"));
        assert!(out.contains("771 symbols"));
        assert!(out.contains("2412 edges"));
        assert!(!out.contains("Parsing src/"));
    }

    #[test]
    fn test_fixture_search_results_truncate() {
        let raw = include_str!("../../../tests/fixtures/cpp/codegraph_search_results.txt");
        let out = filter_results(raw, RESULT_LIMIT);
        assert!(out.contains("(+30 more"));
        assert!(out.contains("parse_expr at src/parser.rs:42"));
    }

    #[test]
    fn test_stats_strips_separators() {
        let raw = "==========\nFiles: 47\n----------\nSymbols: 2841\n";
        let out = filter_stats(raw);
        assert!(out.contains("Files: 47"));
        assert!(out.contains("Symbols: 2841"));
        assert!(!out.contains("===="));
        assert!(!out.contains("----"));
    }
}
