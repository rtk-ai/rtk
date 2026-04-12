//! Compares RTK-routed vs raw commands in a coding session.

use crate::core::utils::format_tokens;
use crate::discover::provider::{
    ClaudeProvider, ExtractedCommand, OpenCodeProvider, SessionProvider,
};
use crate::discover::registry::{classify_command, split_command_chain, Classification};
use anyhow::Result;
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

const OPENCODE_SESSION_PREFIX: &str = "opencode-session:";

/// A summarized session for display.
struct SessionSummary {
    id: String,
    date: String,
    total_cmds: usize,
    rtk_cmds: usize,
    output_tokens: usize,
}

impl SessionSummary {
    fn adoption_pct(&self) -> f64 {
        if self.total_cmds == 0 {
            return 0.0;
        }
        self.rtk_cmds as f64 / self.total_cmds as f64 * 100.0
    }
}

/// Count RTK-covered commands from extracted commands.
/// A command is "covered" if it either:
/// - starts with "rtk " (explicit rtk invocation), or
/// - would be rewritten by the hook (classify_command returns Supported)
///
/// Chained commands (e.g. "cd ./path && rtk ls") are split so each part
/// is classified independently — matching the discover module's behavior.
fn count_rtk_commands(cmds: &[ExtractedCommand]) -> (usize, usize, usize) {
    let mut total: usize = 0;
    let mut rtk: usize = 0;
    for c in cmds {
        let parts = split_command_chain(&c.command);
        for part in &parts {
            total += 1;
            if part.starts_with("rtk ")
                || matches!(classify_command(part), Classification::Supported { .. })
            {
                rtk += 1;
            }
        }
    }
    let output: usize = cmds.iter().filter_map(|c| c.output_len).sum();
    (total, rtk, output)
}

fn progress_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "@".repeat(filled), ".".repeat(empty))
}

fn opencode_session_id(path: &Path) -> Option<String> {
    path.to_string_lossy()
        .strip_prefix(OPENCODE_SESSION_PREFIX)
        .map(str::to_string)
}

fn load_opencode_session_times() -> HashMap<String, i64> {
    let db_path = match dirs::home_dir() {
        Some(h) => h.join(".local/share/opencode/opencode.db"),
        None => return HashMap::new(),
    };

    let conn = match rusqlite::Connection::open(db_path) {
        Ok(c) => c,
        Err(_) => return HashMap::new(),
    };

    let mut stmt = match conn.prepare("SELECT id, time_updated FROM session") {
        Ok(s) => s,
        Err(_) => return HashMap::new(),
    };

    let rows = match stmt.query_map([], |row| {
        let id: String = row.get(0)?;
        let ts: i64 = row.get(1)?;
        Ok((
            id,
            if ts > 1_000_000_000_000 {
                ts / 1000
            } else {
                ts
            },
        ))
    }) {
        Ok(r) => r,
        Err(_) => return HashMap::new(),
    };

    let mut out = HashMap::new();
    for row in rows.flatten() {
        out.insert(row.0, row.1);
    }
    out
}

fn session_updated_unix(path: &Path, opencode_times: &HashMap<String, i64>) -> i64 {
    if let Some(id) = opencode_session_id(path) {
        return *opencode_times.get(&id).unwrap_or(&0);
    }

    fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .and_then(|t| t.duration_since(std::time::SystemTime::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

fn format_relative_date(unix_ts: i64) -> String {
    if unix_ts <= 0 {
        return "?".to_string();
    }

    let now = std::time::SystemTime::now()
        .duration_since(std::time::SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;
    let delta = (now - unix_ts).max(0) as u64;
    let days = delta / 86400;
    if days == 0 {
        "Today".to_string()
    } else if days == 1 {
        "Yesterday".to_string()
    } else {
        format!("{}d ago", days)
    }
}

pub fn run(_verbose: u8) -> Result<()> {
    let provider = ClaudeProvider;
    let opencode_provider = OpenCodeProvider::default();

    let mut sessions = Vec::new();
    let mut available_sources = 0usize;

    if let Ok(mut claude_sessions) = provider.discover_sessions(None, Some(30)) {
        available_sources += 1;
        sessions.append(&mut claude_sessions);
    }

    if let Ok(mut opencode_sessions) = opencode_provider.discover_sessions(None, Some(30)) {
        available_sources += 1;
        sessions.append(&mut opencode_sessions);
    }

    if available_sources == 0 {
        anyhow::bail!("Failed to discover Claude Code/OpenCode sessions");
    }

    if sessions.is_empty() {
        println!("No Claude Code/OpenCode sessions found in the last 30 days.");
        println!("Make sure Claude Code or OpenCode has been used at least once.");
        return Ok(());
    }

    // Group JSONL files by parent session (ignore subagent files)
    let mut session_files: Vec<PathBuf> = sessions
        .into_iter()
        .filter(|p| {
            // Skip subagent files — only top-level session JSONL
            !p.to_string_lossy().contains("subagents")
        })
        .collect();

    let opencode_times = load_opencode_session_times();

    // Sort by updated time desc (filesystem for Claude, DB for OpenCode)
    session_files.sort_by(|a, b| {
        let ta = session_updated_unix(a, &opencode_times);
        let tb = session_updated_unix(b, &opencode_times);
        tb.cmp(&ta)
    });

    // Take top 10
    session_files.truncate(10);

    let mut summaries: Vec<SessionSummary> = Vec::new();

    for path in &session_files {
        let cmds = match provider.extract_commands(path) {
            Ok(c) => c,
            Err(_) => match opencode_provider.extract_commands(path) {
                Ok(c) => c,
                Err(_) => continue,
            },
        };

        if cmds.is_empty() {
            continue;
        }

        let (total_cmds, rtk_cmds, output_tokens) = count_rtk_commands(&cmds);

        // Extract session ID from filename
        let id = opencode_session_id(path).unwrap_or_else(|| {
            path.file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("unknown")
                .to_string()
        });
        let short_id = if id.len() > 8 {
            id[..8].to_string()
        } else {
            id
        };

        let updated_unix = session_updated_unix(path, &opencode_times);
        let date = format_relative_date(updated_unix);

        summaries.push(SessionSummary {
            id: short_id,
            date,
            total_cmds,
            rtk_cmds,
            output_tokens,
        });
    }

    if summaries.is_empty() {
        println!("No sessions with Bash commands found.");
        return Ok(());
    }

    // Display table
    let header = "RTK Session Overview (last 10)";
    println!("{}", header);
    println!("{}", "-".repeat(70));
    println!(
        "{:<12} {:<12} {:>5} {:>5} {:>9} {:<7} {:>8}",
        "Session", "Date", "Cmds", "RTK", "Adoption", "", "Output"
    );
    println!("{}", "-".repeat(70));

    let mut total_cmds = 0;
    let mut total_rtk = 0;

    for s in &summaries {
        let pct = s.adoption_pct();
        let bar = progress_bar(pct, 5);
        total_cmds += s.total_cmds;
        total_rtk += s.rtk_cmds;

        println!(
            "{:<12} {:<12} {:>5} {:>5} {:>8.0}% {:<7} {:>8}",
            s.id,
            s.date,
            s.total_cmds,
            s.rtk_cmds,
            pct,
            bar,
            format_tokens(s.output_tokens),
        );
    }

    println!("{}", "-".repeat(70));

    let avg_adoption = if total_cmds > 0 {
        total_rtk as f64 / total_cmds as f64 * 100.0
    } else {
        0.0
    };
    println!("Average adoption: {:.0}%", avg_adoption);
    println!("Tip: Run `rtk discover` to find missed RTK opportunities");

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::discover::provider::ExtractedCommand;
    use std::io::Write;
    use tempfile::NamedTempFile;

    fn make_cmd(command: &str, output_len: Option<usize>) -> ExtractedCommand {
        ExtractedCommand {
            command: command.to_string(),
            output_len,
            session_id: "test".to_string(),
            output_content: None,
            is_error: false,
            sequence_index: 0,
        }
    }

    // --- Progress bar ---

    #[test]
    fn test_progress_bar_boundaries() {
        assert_eq!(progress_bar(0.0, 5), ".....");
        assert_eq!(progress_bar(100.0, 5), "@@@@@");
        assert_eq!(progress_bar(50.0, 5), "@@@..");
    }

    // --- count_rtk_commands: core counting logic ---

    #[test]
    fn test_count_all_rtk() {
        let cmds = vec![
            make_cmd("rtk git status", Some(200)),
            make_cmd("rtk cargo test", Some(5000)),
            make_cmd("rtk git log -10", Some(800)),
        ];
        let (total, rtk, output) = count_rtk_commands(&cmds);
        assert_eq!(total, 3);
        assert_eq!(rtk, 3);
        assert_eq!(output, 6000);
    }

    #[test]
    fn test_count_hook_rewritten_commands() {
        // Hook rewrites "git status" → "rtk git status" but JSONL logs the original.
        // count_rtk_commands should detect these via classify_command.
        let cmds = vec![
            make_cmd("git status", Some(500)),
            make_cmd("cargo test", Some(3000)),
            make_cmd("echo hello", Some(100)),
        ];
        let (total, rtk, output) = count_rtk_commands(&cmds);
        assert_eq!(total, 3);
        // git status + cargo test are supported by RTK, echo is not
        assert_eq!(rtk, 2);
        assert_eq!(output, 3600);
    }

    #[test]
    fn test_count_mixed_explicit_and_hook() {
        let cmds = vec![
            make_cmd("rtk git status", Some(200)),  // explicit rtk
            make_cmd("git log -5", Some(1000)),     // hook-rewritten (logged as raw)
            make_cmd("rtk cargo test", Some(5000)), // explicit rtk
            make_cmd("echo hello", None),           // not supported
        ];
        let (total, rtk, output) = count_rtk_commands(&cmds);
        assert_eq!(total, 4);
        assert_eq!(rtk, 3); // rtk git status + git log + rtk cargo test
        assert_eq!(output, 6200);
    }

    #[test]
    fn test_count_unsupported_commands_not_counted() {
        let cmds = vec![
            make_cmd("echo hello", Some(100)),
            make_cmd("mkdir -p /tmp/foo", Some(10)),
            make_cmd("cd /tmp", Some(5)),
        ];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!(total, 3);
        assert_eq!(rtk, 0);
    }

    #[test]
    fn test_count_empty_commands() {
        let cmds: Vec<ExtractedCommand> = vec![];
        let (total, rtk, output) = count_rtk_commands(&cmds);
        assert_eq!(total, 0);
        assert_eq!(rtk, 0);
        assert_eq!(output, 0);
    }

    // --- chained commands ---

    #[test]
    fn test_count_chained_commands_split() {
        // "cd ./path && rtk ls" is one ExtractedCommand but two logical commands.
        // cd is ignored/unsupported, ls is supported → 1 out of 2 covered.
        let cmds = vec![make_cmd("cd ./your/app/path && rtk ls", Some(200))];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!(total, 2, "chain should split into 2 commands");
        assert_eq!(rtk, 1, "only 'rtk ls' is RTK-covered");
    }

    #[test]
    fn test_count_chained_all_supported() {
        // Both parts are RTK-supported
        let cmds = vec![make_cmd("git status && git log -5", Some(500))];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!(total, 2, "chain should split into 2 commands");
        assert_eq!(rtk, 2, "both git commands are RTK-covered");
    }

    #[test]
    fn test_count_chained_with_semicolon() {
        let cmds = vec![make_cmd("cd /tmp; git status; echo done", Some(100))];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!(total, 3, "semicolon chain splits into 3 commands");
        assert_eq!(rtk, 1, "only git status is RTK-covered");
    }

    #[test]
    fn test_count_chained_no_false_inflation() {
        // Single command should still count as 1
        let cmds = vec![make_cmd("git status", Some(100))];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!(total, 1);
        assert_eq!(rtk, 1);
    }

    // --- adoption_pct ---

    #[test]
    fn test_adoption_pct_zero_division() {
        let s = SessionSummary {
            id: "x".to_string(),
            date: "Today".to_string(),
            total_cmds: 0,
            rtk_cmds: 0,
            output_tokens: 0,
        };
        assert_eq!(s.adoption_pct(), 0.0);
    }

    #[test]
    fn test_adoption_pct_75_percent() {
        let s = SessionSummary {
            id: "x".to_string(),
            date: "Today".to_string(),
            total_cmds: 20,
            rtk_cmds: 15,
            output_tokens: 0,
        };
        assert_eq!(s.adoption_pct(), 75.0);
    }

    // --- End-to-end: parse real JSONL and count ---

    #[test]
    fn test_parse_jsonl_session_and_count() {
        // Simulate a session with 3 Bash commands: 2 rtk, 1 raw
        let jsonl = [
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"rtk git status"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"On branch main"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Bash","input":{"command":"git log -5"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t2","content":"commit abc123\ncommit def456"}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"rtk cargo test"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t3","content":"test result: ok. 5 passed"}]}}"#,
        ];

        let mut tmp = NamedTempFile::new().expect("create tempfile");
        for line in &jsonl {
            writeln!(tmp, "{}", line).expect("write line");
        }

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(tmp.path()).expect("parse JSONL");

        let (total, rtk, _output) = count_rtk_commands(&cmds);
        assert_eq!(total, 3, "should find 3 Bash commands");
        // All 3 are RTK-covered: 2 explicit "rtk ..." + 1 hook-rewritten "git log"
        assert_eq!(rtk, 3, "all 3 commands should be RTK-covered");
    }

    #[test]
    fn test_parse_jsonl_ignores_non_bash_tools() {
        // Read/Grep/Edit tools should NOT be counted
        let jsonl = [
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Read","input":{"file_path":"/tmp/foo"}}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t2","name":"Grep","input":{"pattern":"TODO"}}]}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t3","name":"Bash","input":{"command":"rtk git status"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t3","content":"clean"}]}}"#,
        ];

        let mut tmp = NamedTempFile::new().expect("create tempfile");
        for line in &jsonl {
            writeln!(tmp, "{}", line).expect("write line");
        }

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(tmp.path()).expect("parse JSONL");

        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!(total, 1, "only Bash tool should be counted");
        assert_eq!(rtk, 1, "the one Bash command is rtk");
    }

    #[test]
    fn test_parse_empty_session() {
        // Session with no Bash commands at all
        let jsonl = [
            r#"{"type":"user","message":{"role":"user","content":"Hello"}}"#,
            r#"{"type":"assistant","message":{"role":"assistant","content":"Hi there!"}}"#,
        ];

        let mut tmp = NamedTempFile::new().expect("create tempfile");
        for line in &jsonl {
            writeln!(tmp, "{}", line).expect("write line");
        }

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(tmp.path()).expect("parse JSONL");

        assert!(cmds.is_empty(), "no Bash commands = empty");
    }

    #[test]
    fn test_parse_jsonl_chained_command() {
        // Claude often runs "cd ./path && git status" as a single Bash call.
        // The adoption metric should split the chain and count each part.
        let jsonl = [
            r#"{"type":"assistant","message":{"role":"assistant","content":[{"type":"tool_use","id":"t1","name":"Bash","input":{"command":"cd ./your/app/path && rtk ls"}}]}}"#,
            r#"{"type":"user","message":{"role":"user","content":[{"type":"tool_result","tool_use_id":"t1","content":"file1.rs\nfile2.rs"}]}}"#,
        ];

        let mut tmp = NamedTempFile::new().expect("create tempfile");
        for line in &jsonl {
            writeln!(tmp, "{}", line).expect("write line");
        }

        let provider = ClaudeProvider;
        let cmds = provider.extract_commands(tmp.path()).expect("parse JSONL");

        assert_eq!(cmds.len(), 1, "one Bash tool call");
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!(total, 2, "chain splits into cd + rtk ls");
        assert_eq!(rtk, 1, "rtk ls is covered, cd is not");
    }
}
