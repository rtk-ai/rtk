//! Compares RTK-routed vs raw commands in a coding session.

use crate::core::utils::format_tokens;
use crate::discover::provider::{ClaudeProvider, ExtractedCommand, SessionProvider};
use crate::discover::{RewriteConfig, commands_rtk_filters};
use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

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

/// Count RTK-covered commands from extracted commands, as discover reads them
/// ([`commands_rtk_filters`]), under the configuration the hook rewrites under. A command
/// is "covered" if it either:
/// - is an explicit rtk invocation, under any assignments and wrappers, excluding
///   `rtk bypass`, which deliberately runs the raw command unfiltered and must not count
///   as adopted usage. rtk itself does not read `RTK_DISABLED`, so `RTK_DISABLED=1 rtk git
///   status` counts; or
/// - has a rule, and the hook rewrites it where it stands: not behind an
///   `RTK_DISABLED=`, not a command `exclude_commands` names, not one the rewrite declines
///   (a pipe's producer it leaves raw, a consumer it leaves raw), and not on a line the
///   hook cannot attest (`nohup cargo test > test.log 2>&1 &`).
///
/// Chained commands (e.g. "cd ./path && rtk ls") and the lines of a multi-line call are
/// split so each part is classified independently — matching the discover module's behavior.
fn count_rtk_commands(cmds: &[ExtractedCommand], config: &RewriteConfig) -> (usize, usize, usize) {
    let mut total: usize = 0;
    let mut rtk: usize = 0;
    for c in cmds {
        let (commands, filtered) = commands_rtk_filters(&c.command, config);
        total += commands;
        rtk += filtered;
    }
    let output: usize = cmds.iter().filter_map(|c| c.output_len).sum();
    (total, rtk, output)
}

fn progress_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let empty = width.saturating_sub(filled);
    format!("{}{}", "@".repeat(filled), ".".repeat(empty))
}

pub fn run(_verbose: u8) -> Result<()> {
    let provider = ClaudeProvider;
    let sessions = provider
        .discover_sessions(None, Some(30))
        .context("Failed to discover Claude Code sessions")?;

    if sessions.is_empty() {
        println!("No Claude Code sessions found in the last 30 days.");
        println!("Make sure Claude Code has been used at least once.");
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

    // Sort by mtime desc
    session_files.sort_by(|a, b| {
        let ma = fs::metadata(a)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        let mb = fs::metadata(b)
            .and_then(|m| m.modified())
            .unwrap_or(std::time::SystemTime::UNIX_EPOCH);
        mb.cmp(&ma)
    });

    // Take top 10
    session_files.truncate(10);

    let mut summaries: Vec<SessionSummary> = Vec::new();
    let config = RewriteConfig::load();

    for path in &session_files {
        let cmds = match provider.extract_commands(path) {
            Ok(c) => c,
            Err(_) => continue,
        };

        if cmds.is_empty() {
            continue;
        }

        let (total_cmds, rtk_cmds, output_tokens) = count_rtk_commands(&cmds, &config);

        // Extract session ID from filename
        let id = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("unknown");
        let short_id = &id[..id.floor_char_boundary(8)];

        // Extract date from mtime
        let date = fs::metadata(path)
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
            .unwrap_or_else(|_| "?".to_string());

        summaries.push(SessionSummary {
            id: short_id.to_string(),
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

    /// [`super::count_rtk_commands`] with no `exclude_commands` and no transparent prefixes.
    fn count_rtk_commands(cmds: &[ExtractedCommand]) -> (usize, usize, usize) {
        super::count_rtk_commands(cmds, &RewriteConfig::default())
    }

    fn make_cmd(command: &str, output_len: Option<usize>) -> ExtractedCommand {
        ExtractedCommand {
            command: command.to_string(),
            output_len,
            session_id: "test".to_string(),
            tool_use_id: "toolu_test".to_string(),
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
    fn test_count_reads_a_block_with_its_continuations_joined() {
        let cmds = vec![make_cmd("git status \\\n --short\ngit log", None)];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!((total, rtk), (2, 2));
    }

    #[test]
    fn test_count_each_line_of_a_block_also_one_rtk_does_not_handle() {
        let cmds = vec![make_cmd("cd x\ngit status", None)];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!((total, rtk), (2, 1));
    }

    #[test]
    fn test_count_a_refused_block_as_one_command() {
        // The rewrite refuses a block with a subshell line and reaches none of its lines.
        let cmds = vec![make_cmd("( cd x && ls )\ngit status", None)];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!((total, rtk), (1, 0));
    }

    #[test]
    fn test_count_no_command_in_a_call_of_comments() {
        let cmds = vec![make_cmd("# just a note", None), make_cmd("# a\n# b", None)];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!((total, rtk), (0, 0));
    }

    #[test]
    fn test_count_hook_rewritten_commands() {
        // Hook rewrites "git status" → "rtk git status" but JSONL logs the original.
        // count_rtk_commands should detect these by their classification.
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
    fn test_count_rtk_bypass_not_counted_as_adopted() {
        // `rtk bypass <cmd>` deliberately runs the raw command unfiltered — it must
        // not count as adopted rtk usage, or the session report would flatter
        // itself via its own escape hatch (as discover reads it).
        let cmds = vec![
            make_cmd("rtk bypass git log -20", Some(2000)),
            make_cmd("rtk git status", Some(200)),
        ];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!(total, 2);
        assert_eq!(rtk, 1, "only the explicit non-bypass rtk invocation counts");
    }

    /// bash runs `rtk` for a quoted or escaped `rtk`, so those count as well.
    #[test]
    fn test_count_quoted_rtk_as_adopted() {
        let cmds = vec![
            make_cmd("'rtk' git status", Some(200)),
            make_cmd("\\rtk git log", Some(200)),
            make_cmd("\\rtk 'proxy' git log", Some(200)),
        ];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!(total, 3);
        assert_eq!(rtk, 2);
    }

    /// A command is read under its assignments and wrappers, as discover reads it.
    #[test]
    fn test_count_reads_the_command_under_its_wrappers() {
        let cmds = vec![
            make_cmd("timeout 5 rtk git status", Some(200)),
            make_cmd("FOO=1 rtk git status", Some(200)),
            make_cmd("timeout 5 git status", Some(200)),
            make_cmd("time cargo test", Some(200)),
            make_cmd("FOO=1 rtk bypass git status", Some(200)),
        ];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!((total, rtk), (5, 4));
    }

    /// A command with a rule counts only where the hook rewrites it, as discover asks.
    #[test]
    fn test_count_only_what_the_hook_rewrites_where_it_stands() {
        for (command, expected) in [
            // A line the hook cannot attest: it leaves the whole line alone.
            ("nohup cargo test > test.log 2>&1 &", (1, 0)),
            ("git status; echo $(date)", (2, 0)),
            // A pipe consumer the rewrite leaves raw, behind a producer it takes.
            ("git log | head -5", (2, 1)),
            ("cargo test 2>&1 | tail -5", (2, 1)),
            // A pipe whose consumer it does not know is left alone whole.
            ("git log | time head -5", (2, 0)),
            // One only a TOML filter rewrites has no rule; discover lists it unsupported.
            ("stat f", (1, 0)),
        ] {
            let (total, rtk, _) = count_rtk_commands(&[make_cmd(command, Some(200))]);
            assert_eq!((total, rtk), expected, "{command:?}");
        }
    }

    /// The count reads under the configuration the hook rewrites under: a command
    /// `exclude_commands` names is left alone, and one behind a transparent prefix is
    /// rewritten.
    #[test]
    fn test_count_reads_under_the_hooks_configuration() {
        let count = |command: &str, config: &RewriteConfig| {
            let (total, rtk, _) =
                super::count_rtk_commands(&[make_cmd(command, Some(200))], config);
            (total, rtk)
        };
        let excluded = RewriteConfig::of(&["git".to_string()], &[]);
        let prefixed = RewriteConfig::of(&[], &["chronic".to_string(), "sudo".to_string()]);
        let none = RewriteConfig::default();
        assert_eq!(count("git status", &none), (1, 1));
        assert_eq!(count("git status", &excluded), (1, 0));
        assert_eq!(count("cargo test", &excluded), (1, 1));
        assert_eq!(count("chronic git status", &none), (1, 0));
        assert_eq!(count("chronic git status", &prefixed), (1, 1));
        assert_eq!(count("sudo cargo test", &prefixed), (1, 1));
        // The command behind a prefix of several words is the one the rewrite settles at.
        let nice = RewriteConfig::of(&[], &["nice -n 19 ionice".to_string()]);
        assert_eq!(count("nice -n 19 ionice git status", &none), (1, 0));
        assert_eq!(count("nice -n 19 ionice git status", &nice), (1, 1));
    }

    /// A command an `RTK_DISABLED=` keeps the hook off is no adoption, behind a wrapper
    /// too; rtk itself does not read the variable, so an `rtk` behind one still counts.
    #[test]
    fn test_count_rtk_disabled_not_counted_as_adopted() {
        let cmds = vec![
            make_cmd("RTK_DISABLED=1 git status", Some(200)),
            make_cmd("RTK_DISABLED=1 timeout 5 git status", Some(200)),
            make_cmd("RTK_DISABLED=1 time cargo test", Some(200)),
            make_cmd("RTK_DISABLED=1 rtk git status", Some(200)),
        ];
        let (total, rtk, _) = count_rtk_commands(&cmds);
        assert_eq!((total, rtk), (4, 1));
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
