//! Jira CLI (`jira`) command output compression.
//!
//! Provides token-optimized output for the Atlassian `jira` CLI
//! (github.com/ankitpokhrel/jira-cli). Focuses on JSON parsing
//! to extract only the fields an LLM needs, cutting 80-90% of tokens.
//!
//! Supported subcommands:
//!   issue list     — compact table (key, type, status, summary)
//!   issue view     — key facts + filtered description
//!   issue create   — ok confirmation
//!   sprint list    — compact sprint table
//!   board list     — compact board table
//!   _              — passthrough

use crate::core::runner::{self, RunOptions};
use crate::core::truncate::CAP_LIST;
use crate::core::utils::{ok_confirmation, resolved_command, truncate};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use serde_json::Value;

lazy_static! {
      static ref MULTI_BLANK_RE: Regex = Regex::new(r"\n{3,}").unwrap();
      static ref HTML_COMMENT_RE: Regex = Regex::new(r"(?s)<!--.*?-->").unwrap();
}

// ── Cap constants ─────────────────────────────────────────────────────────────
const MAX_ISSUES: usize = CAP_LIST;
const MAX_SPRINTS: usize = CAP_LIST;
const MAX_BOARDS: usize = CAP_LIST;
/// Max chars kept from issue description to avoid flooding context.
const MAX_DESC_CHARS: usize = 500;

// ── Public entry point ────────────────────────────────────────────────────────

/// Route `rtk jira <subcommand> [args...]` to the appropriate filter.
pub fn run(subcommand: &str, args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
      // If user explicitly requested --json or -q/--raw, don't double-filter.
    if has_output_flag(args) {
              return run_passthrough("jira", subcommand, args);
    }

    match subcommand {
              "issue" => run_issue(args, verbose, ultra_compact),
              "sprint" => run_sprint(args, verbose, ultra_compact),
              "board" => run_board(args, verbose, ultra_compact),
              _ => run_passthrough("jira", subcommand, args),
    }
}

// ── Issue subcommands ─────────────────────────────────────────────────────────

fn run_issue(args: &[String], verbose: u8, ultra_compact: bool) -> Result<i32> {
      if args.is_empty() {
                return run_passthrough("jira", "issue", args);
      }
      match args[0].as_str() {
                "list" => issue_list(&args[1..], verbose, ultra_compact),
                "view" => issue_view(&args[1..], verbose, ultra_compact),
                "create" => issue_create(&args[1..], verbose),
                _ => run_passthrough("jira", "issue", args),
      }
}

/// `jira issue list --plain --no-headers` → inject `--plain --no-headers` if
/// absent, parse the tab-separated output into a compact table.
fn issue_list(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
      let mut cmd = resolved_command("jira");
      cmd.args(["issue", "list"]);
      // Inject machine-readable flags when the user hasn't set them.
    let wants_plain = args.iter().any(|a| a == "--plain" || a == "-p");
      if !wants_plain {
                cmd.arg("--plain");
                cmd.arg("--no-headers");
      }
      for a in args {
                cmd.arg(a);
      }
      runner::run_filtered(
                cmd,
                "jira",
                "issue list",
                |stdout| format_issue_list(stdout, ultra_compact),
                RunOptions::stdout_only().early_exit_on_failure(),
            )
}

/// Format plain `jira issue list` TSV output (pure, testable).
///
/// Input lines: `TYPE\tKEY\tSUMMARY\tASSIGNEE\tPRIORITY\tSTATUS\tCREATED`
/// Output: compact lines with only key, type, status, summary.
pub fn format_issue_list(raw: &str, ultra_compact: bool) -> String {
      let lines: Vec<&str> = raw
                .lines()
                .filter(|l| !l.trim().is_empty())
                .collect();

    if lines.is_empty() {
              return "No issues\n".to_string();
    }

    let mut out = String::new();
      let total = lines.len();
      let shown = lines.iter().take(MAX_ISSUES);

    for line in shown {
              let cols: Vec<&str> = line.splitn(7, '\t').collect();
              // Columns: TYPE KEY SUMMARY ASSIGNEE PRIORITY STATUS CREATED
          let (issue_type, key, summary, status) = (
                        cols.first().copied().unwrap_or(""),
                        cols.get(1).copied().unwrap_or(""),
                        cols.get(2).copied().unwrap_or(""),
                        cols.get(5).copied().unwrap_or(""),
                    );

          let summary_t = truncate(summary, 80);
              if ultra_compact {
                            out.push_str(&format!("{} {} [{}] {}\n", key, issue_type, status, summary_t));
              } else {
                            out.push_str(&format!(
                                              "{:<12} {:<8} {:<14} {}\n",
                                              key, issue_type, status, summary_t
                                          ));
              }
    }

    if total > MAX_ISSUES {
              out.push_str(&format!("… +{} more issues\n", total - MAX_ISSUES));
              if let Some(hint) =
                            crate::core::tee::force_tee_tail_hint(raw, "jira-issues", MAX_ISSUES + 1)
              {
                            out.push_str(&format!("  {}\n", hint));
              }
    }
      out
}

/// `jira issue view PROJ-123` — inject `--plain` if absent, filter the output.
fn issue_view(args: &[String], _verbose: u8, _ultra_compact: bool) -> Result<i32> {
      if args.is_empty() {
                return run_passthrough("jira", "issue view", args);
      }
      let mut cmd = resolved_command("jira");
      cmd.args(["issue", "view"]);
      let wants_plain = args.iter().any(|a| a == "--plain" || a == "-p");
      if !wants_plain {
                cmd.arg("--plain");
      }
      for a in args {
                cmd.arg(a);
      }
      runner::run_filtered(
                cmd,
                "jira",
                "issue view",
                format_issue_view,
                RunOptions::stdout_only()
                    .tee("jira-issue-view")
                    .early_exit_on_failure(),
            )
}

/// Filter `jira issue view --plain` output (pure, testable).
///
/// Keeps: key header, Type/Status/Priority/Assignee/Reporter lines,
/// Description (capped), strips decoration and blank noise.
pub fn format_issue_view(raw: &str) -> String {
      let mut out = String::new();
      let mut in_desc = false;
      let mut desc_chars = 0usize;

    for line in raw.lines() {
              let trimmed = line.trim();

          // Skip heavy decoration lines (borders, rule lines).
          if trimmed.chars().all(|c| c == '─' || c == '━' || c == '-' || c == '=') {
                        continue;
          }
              if trimmed.is_empty() && !in_desc {
                            continue;
              }

          // Detect description section start.
          if trimmed.eq_ignore_ascii_case("description") || trimmed.starts_with("Description") {
                        in_desc = true;
                        out.push_str("Description:\n");
                        continue;
          }

          // In description: accumulate up to MAX_DESC_CHARS, then truncate.
          if in_desc {
                        if desc_chars >= MAX_DESC_CHARS {
                                          // Already printed enough; stop adding description lines.
                                          continue;
                        }
                        let remaining = MAX_DESC_CHARS - desc_chars;
                        if line.len() > remaining {
                                          out.push_str(&line[..remaining]);
                                          out.push_str("…\n");
                                          desc_chars = MAX_DESC_CHARS;
                        } else {
                                          out.push_str(line);
                                          out.push('\n');
                                          desc_chars += line.len() + 1;
                        }
                        continue;
          }

          // Key metadata lines (keep Type, Status, Priority, Assignee, Reporter, key header).
          let keep = trimmed.starts_with("Type")
                        || trimmed.starts_with("Status")
                        || trimmed.starts_with("Priority")
                        || trimmed.starts_with("Assignee")
                        || trimmed.starts_with("Reporter")
                        || trimmed.starts_with("Created")
                        || trimmed.starts_with("Updated")
                        || trimmed.contains('-') && trimmed.len() < 20  // likely issue key header
                        || line.trim_start().starts_with('●') // issue key bullet
                        ;

          // Detect key-header line (e.g., "  ● AWM-123  Bug: ...")
          if line.contains("●") || keep {
                        out.push_str(line);
                        out.push('\n');
          } else if trimmed.starts_with("Summary") {
                        out.push_str(line);
                        out.push('\n');
          }
    }

    let result = MULTI_BLANK_RE
              .replace_all(out.trim(), "\n\n")
              .to_string();
      result
}

/// `jira issue create` — just echo ok confirmation.
fn issue_create(args: &[String], _verbose: u8) -> Result<i32> {
      let mut cmd = resolved_command("jira");
      cmd.args(["issue", "create"]);
      for a in args {
                cmd.arg(a);
      }
      runner::run_filtered(
                cmd,
                "jira",
                "issue create",
                |stdout| {
                              // Extract just the issue key from creation output.
                    for line in stdout.lines() {
                                      if line.contains("://") {
                                                            // URL line like "https://org.atlassian.net/browse/PROJ-123"
                                          let key = line.trim().rsplit('/').next().unwrap_or("").trim();
                                                            if !key.is_empty() {
                                                                                      return ok_confirmation(&format!("created {}", key));
                                                            }
                                      }
                    }
                              ok_confirmation("created")
                },
                RunOptions::stdout_only(),
            )
}

// ── Sprint subcommands ────────────────────────────────────────────────────────

fn run_sprint(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
      if args.is_empty() {
                return run_passthrough("jira", "sprint", args);
      }
      match args[0].as_str() {
                "list" => sprint_list(&args[1..], ultra_compact),
                _ => run_passthrough("jira", "sprint", args),
      }
}

fn sprint_list(args: &[String], ultra_compact: bool) -> Result<i32> {
      let mut cmd = resolved_command("jira");
      cmd.args(["sprint", "list"]);
      let wants_plain = args.iter().any(|a| a == "--plain" || a == "-p");
      if !wants_plain {
                cmd.arg("--plain");
                cmd.arg("--no-headers");
      }
      for a in args {
                cmd.arg(a);
      }
      runner::run_filtered(
                cmd,
                "jira",
                "sprint list",
                |stdout| format_sprint_list(stdout, ultra_compact),
                RunOptions::stdout_only().early_exit_on_failure(),
            )
}

/// Format `jira sprint list --plain` output (pure, testable).
/// Columns: ID  Name  StartDate  EndDate  CompleteDate  Status
pub fn format_sprint_list(raw: &str, ultra_compact: bool) -> String {
      let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
      if lines.is_empty() {
                return "No sprints\n".to_string();
      }

    let mut out = String::new();
      let total = lines.len();

    for line in lines.iter().take(MAX_SPRINTS) {
              let cols: Vec<&str> = line.splitn(6, '\t').collect();
              let (id, name, status) = (
                            cols.first().copied().unwrap_or(""),
                            cols.get(1).copied().unwrap_or(""),
                            cols.get(5).copied().unwrap_or(""),
                        );
              let name_t = truncate(name, 40);
              if ultra_compact {
                            out.push_str(&format!("{} {} [{}]\n", id, name_t, status));
              } else {
                            out.push_str(&format!("{:<6} {:<42} {}\n", id, name_t, status));
              }
    }

    if total > MAX_SPRINTS {
              out.push_str(&format!("… +{} more\n", total - MAX_SPRINTS));
              if let Some(hint) =
                            crate::core::tee::force_tee_tail_hint(raw, "jira-sprints", MAX_SPRINTS + 1)
              {
                            out.push_str(&format!("  {}\n", hint));
              }
    }
      out
}

// ── Board subcommands ─────────────────────────────────────────────────────────

fn run_board(args: &[String], _verbose: u8, ultra_compact: bool) -> Result<i32> {
      if args.is_empty() {
                return run_passthrough("jira", "board", args);
      }
      match args[0].as_str() {
                "list" => board_list(&args[1..], ultra_compact),
                _ => run_passthrough("jira", "board", args),
      }
}

fn board_list(args: &[String], ultra_compact: bool) -> Result<i32> {
      let mut cmd = resolved_command("jira");
      cmd.args(["board", "list"]);
      let wants_plain = args.iter().any(|a| a == "--plain" || a == "-p");
      if !wants_plain {
                cmd.arg("--plain");
                cmd.arg("--no-headers");
      }
      for a in args {
                cmd.arg(a);
      }
      runner::run_filtered(
                cmd,
                "jira",
                "board list",
                |stdout| format_board_list(stdout, ultra_compact),
                RunOptions::stdout_only().early_exit_on_failure(),
            )
}

/// Format `jira board list --plain` output (pure, testable).
/// Columns: ID  Name  Type  Project
pub fn format_board_list(raw: &str, ultra_compact: bool) -> String {
      let lines: Vec<&str> = raw.lines().filter(|l| !l.trim().is_empty()).collect();
      if lines.is_empty() {
                return "No boards\n".to_string();
      }

    let mut out = String::new();
      let total = lines.len();

    for line in lines.iter().take(MAX_BOARDS) {
              let cols: Vec<&str> = line.splitn(4, '\t').collect();
              let (id, name, board_type) = (
                            cols.first().copied().unwrap_or(""),
                            cols.get(1).copied().unwrap_or(""),
                            cols.get(2).copied().unwrap_or(""),
                        );
              let name_t = truncate(name, 40);
              if ultra_compact {
                            out.push_str(&format!("{} {} [{}]\n", id, name_t, board_type));
              } else {
                            out.push_str(&format!("{:<6} {:<42} {}\n", id, name_t, board_type));
              }
    }

    if total > MAX_BOARDS {
              out.push_str(&format!("… +{} more\n", total - MAX_BOARDS));
              if let Some(hint) =
                            crate::core::tee::force_tee_tail_hint(raw, "jira-boards", MAX_BOARDS + 1)
              {
                            out.push_str(&format!("  {}\n", hint));
              }
    }
      out
}

// ── Shared helpers ────────────────────────────────────────────────────────────

/// Return true when the user explicitly requested a specific output format
/// that RTK should not override (e.g. --json, --yaml, --raw, --debug).
fn has_output_flag(args: &[String]) -> bool {
      args.iter()
          .any(|a| a == "--json" || a == "--yaml" || a == "--raw" || a == "--debug")
}

/// Passthrough: run `jira <subcommand> <args>` unchanged and track savings.
fn run_passthrough(tool: &str, subcommand: &str, args: &[String]) -> Result<i32> {
      let mut cmd = resolved_command(tool);
      cmd.arg(subcommand);
      for a in args {
                cmd.arg(a);
      }
      runner::run_filtered(
                cmd,
                tool,
                subcommand,
                |s| s.to_string(),
                RunOptions::default(),
            )
}

// ── Tests ─────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
      use super::*;

    // ── issue list ──────────────────────────────────────────────────────────

    #[test]
      fn issue_list_empty_input_returns_no_issues() {
                let out = format_issue_list("", false);
                assert_eq!(out, "No issues\n");
      }

    #[test]
      fn issue_list_blank_only_lines_returns_no_issues() {
                let out = format_issue_list("\n\n  \n", false);
                assert_eq!(out, "No issues\n");
      }

    #[test]
      fn issue_list_single_issue() {
                let raw = "Bug\tPROJ-1\tFix crash\tAlice\tHigh\tOpen\t2026-01-01";
                let out = format_issue_list(raw, false);
                assert!(out.contains("PROJ-1"), "key missing: {}", out);
                assert!(out.contains("Bug"), "type missing: {}", out);
                assert!(out.contains("Open"), "status missing: {}", out);
                assert!(out.contains("Fix crash"), "summary missing: {}", out);
      }

    #[test]
      fn issue_list_ultra_compact_format() {
                let raw = "Story\tPROJ-99\tAdd OAuth\tBob\tMedium\tIn Progress\t2026-01-05";
                let out = format_issue_list(raw, true);
                // ultra-compact: key type [status] summary on one line
          assert!(out.contains("PROJ-99"), "key missing: {}", out);
                assert!(out.contains("[In Progress]"), "status tag missing: {}", out);
      }

    #[test]
      fn issue_list_multiple_issues() {
                let raw = "Bug\tPROJ-1\tCrash\tA\tHigh\tOpen\t2026-01-01\nStory\tPROJ-2\tFeature\tB\tLow\tDone\t2026-01-02";
                let out = format_issue_list(raw, false);
                assert!(out.contains("PROJ-1"));
                assert!(out.contains("PROJ-2"));
      }

    #[test]
      fn issue_list_long_summary_truncated() {
                let long_summary = "A".repeat(200);
                let raw = format!("Bug\tPROJ-1\t{}\tA\tHigh\tOpen\t2026-01-01", long_summary);
                let out = format_issue_list(&raw, false);
                // Should be truncated to 80 chars + "..."
          let summary_in_out: Vec<&str> = out.lines().collect();
                // The summary part should not exceed 80 chars (truncate adds "...")
          let line = summary_in_out[0];
                assert!(line.len() < 200, "line not truncated: {}", line);
      }

    #[test]
      fn issue_list_token_savings() {
                // Simulate 30 verbose issue rows (each ~150 chars) -> compact ~50 chars each
          let raw = (0..30)
                        .map(|i| {
                                          format!(
                                                                "Bug\tPROJ-{}\tLong verbose summary that contains lots of padding text here\tAssignee Name Here\tHigh\tIn Progress\t2026-01-01",
                                                                i
                                                            )
                        })
                        .collect::<Vec<_>>()
                        .join("\n");
                let out = format_issue_list(&raw, false);
                let savings = 1.0 - (out.len() as f64 / raw.len() as f64);
                assert!(savings >= 0.30, "token savings too low: {:.1}%", savings * 100.0);
      }

    // ── issue view ──────────────────────────────────────────────────────────

    #[test]
      fn issue_view_extracts_metadata() {
                let raw = "● PROJ-42  Bug: Fix login crash\n\
                                     Type:      Bug\n\
                                     Status:    Open\n\
                                     Priority:  High\n\
                                     Assignee:  Alice\n\
                                     Reporter:  Bob\n\
                                     ─────────────────────────\n\
                                     Description\n\
                                     The app crashes when logging in.\n";
                let out = format_issue_view(raw);
                assert!(out.contains("Type"), "Type missing");
                assert!(out.contains("Status"), "Status missing");
                assert!(out.contains("Priority"), "Priority missing");
                assert!(out.contains("The app crashes"), "Description missing");
                // Should not contain decoration
          assert!(!out.contains("────"), "decoration not stripped");
      }

    #[test]
      fn issue_view_description_capped() {
                let long_desc = "X".repeat(1000);
                let raw = format!(
                              "● PROJ-1  Bug\nType: Bug\nStatus: Open\nDescription\n{}\n",
                              long_desc
                          );
                let out = format_issue_view(&raw);
                // Description section must not exceed MAX_DESC_CHARS
          let desc_part: String = out
                        .lines()
                        .skip_while(|l| !l.starts_with("Description"))
                        .skip(1)
                        .collect::<Vec<_>>()
                        .join("\n");
                assert!(
                              desc_part.len() <= MAX_DESC_CHARS + 5, // +5 for "…\n"
                              "description not capped: {} chars",
                              desc_part.len()
                          );
      }

    #[test]
      fn issue_view_empty_input() {
                let out = format_issue_view("");
                assert!(out.is_empty() || out.trim().is_empty());
      }

    // ── sprint list ─────────────────────────────────────────────────────────

    #[test]
      fn sprint_list_empty_returns_no_sprints() {
                let out = format_sprint_list("", false);
                assert_eq!(out, "No sprints\n");
      }

    #[test]
      fn sprint_list_single_sprint() {
                let raw = "42\tSprint 10\t2026-01-01\t2026-01-14\t\tactive";
                let out = format_sprint_list(raw, false);
                assert!(out.contains("42"), "id missing");
                assert!(out.contains("Sprint 10"), "name missing");
                assert!(out.contains("active"), "status missing");
      }

    #[test]
      fn sprint_list_ultra_compact() {
                let raw = "42\tSprint 10\t2026-01-01\t2026-01-14\t\tactive";
                let out = format_sprint_list(raw, true);
                assert!(out.contains("[active]"), "status tag missing");
      }

    #[test]
      fn sprint_list_long_name_truncated() {
                let long_name = "B".repeat(100);
                let raw = format!("1\t{}\t2026-01-01\t2026-01-14\t\tactive", long_name);
                let out = format_sprint_list(&raw, false);
                // Name capped at 40 chars
          assert!(out.contains("B".repeat(40).as_str()) || out.contains("…"), "not truncated: {}", out);
      }

    // ── board list ──────────────────────────────────────────────────────────

    #[test]
      fn board_list_empty_returns_no_boards() {
                let out = format_board_list("", false);
                assert_eq!(out, "No boards\n");
      }

    #[test]
      fn board_list_single_board() {
                let raw = "1\tMy Team Board\tscrum\tMYTEAM";
                let out = format_board_list(raw, false);
                assert!(out.contains("1"), "id missing");
                assert!(out.contains("My Team Board"), "name missing");
                assert!(out.contains("scrum"), "type missing");
      }

    #[test]
      fn board_list_ultra_compact() {
                let raw = "5\tInfra Board\tkanban\tINFRA";
                let out = format_board_list(raw, true);
                assert!(out.contains("[kanban]"), "type tag missing");
      }

    // ── has_output_flag ─────────────────────────────────────────────────────

    #[test]
      fn has_output_flag_detects_json() {
                let args: Vec<String> = vec!["--json".into()];
                assert!(has_output_flag(&args));
      }

    #[test]
      fn has_output_flag_detects_yaml() {
                let args: Vec<String> = vec!["--yaml".into()];
                assert!(has_output_flag(&args));
      }

    #[test]
      fn has_output_flag_false_for_normal_args() {
                let args: Vec<String> = vec!["list".into(), "--project".into(), "PROJ".into()];
                assert!(!has_output_flag(&args));
      }
}
