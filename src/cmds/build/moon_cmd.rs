//! Filter for the moon (moonrepo) task runner.
//!
//! Strips moon's chrome (banners, hash suffixes, decoration) and routes each
//! task's stdout/stderr through the matching rtk filter for its underlying
//! command. See issue #1877.

use anyhow::{Context, Result};
use lazy_static::lazy_static;
use regex::Regex;
use serde::Deserialize;
use std::collections::HashMap;

use crate::core::runner::{self, RunOptions};
use crate::core::utils::resolved_command;

/// Subcommands that should bypass filtering — structured output (DOT graphs,
/// JSON), info / setup commands, or long-running servers where filtering
/// would corrupt the stream. Verified against `moon --help` for moon 2.0.4.
const PASSTHROUGH_SUBCOMMANDS: &[&str] = &[
    "action-graph",
    "bin",
    "completions",
    "docker",
    "ext",
    "extension",
    "hash",
    "init",
    "mcp",
    "migrate",
    "project",
    "project-graph",
    "projects",
    "query",
    "setup",
    "task",
    "task-graph",
    "tasks",
    "teardown",
    "template",
    "templates",
    "toolchain",
    "upgrade",
];

/// Serde helper: root of `moon query tasks` JSON output.
#[derive(Debug, Deserialize)]
struct QueryTasksRoot {
    tasks: HashMap<String, HashMap<String, QueryTask>>,
}

/// Serde helper: minimal task fields we care about (other fields are ignored).
#[derive(Debug, Deserialize)]
struct QueryTask {
    command: String,
}

/// Maps a `project:task` identifier to the underlying tool's command name
/// (e.g. `"audit:format" -> "prettier"`). Built from `moon query tasks` in
/// Task 4; defaulted to empty for chrome-only mode in Task 3.
#[derive(Debug, Default, Clone)]
pub struct TaskMap {
    tasks: HashMap<String, String>,
}

impl TaskMap {
    /// Return the underlying command for a `project:task` id, or `None` if
    /// the task is unknown. Used by Task 5 per-task routing.
    #[allow(dead_code)]
    pub fn tool_for(&self, project_task: &str) -> Option<&str> {
        self.tasks.get(project_task).map(|s| s.as_str())
    }

    /// Parse the output of `moon query tasks`. The JSON has shape
    /// `{"tasks": {"<project>": {"<task>": {"command": "...", ...}}}, "options": {...}}`.
    /// Unknown fields (like `options` at the root, `args` inside task) are ignored.
    pub fn from_query_json(json: &str) -> anyhow::Result<Self> {
        let root: QueryTasksRoot = serde_json::from_str(json)
            .context("Failed to parse moon query tasks JSON")?;
        let mut tasks = HashMap::with_capacity(root.tasks.len() * 4);
        for (project, project_tasks) in root.tasks {
            for (task_name, task) in project_tasks {
                tasks.insert(format!("{}:{}", project, task_name), task.command);
            }
        }
        Ok(Self { tasks })
    }

    /// Number of `project:task` entries in the map.
    pub fn len(&self) -> usize {
        self.tasks.len()
    }

    /// True when no tasks have been loaded (e.g. when `moon query tasks` failed).
    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.tasks.is_empty()
    }
}

lazy_static! {
    /// moon's decoration prefix: 4× U+25AE BLACK VERTICAL RECTANGLE + space.
    static ref CHROME_PREFIX_RE: Regex =
        Regex::new(r"^▮▮▮▮ ").unwrap();

    /// Hash suffix on task completion lines: `(<duration>, <8-hex-hash>)` or
    /// `(cached, <8-hex-hash>)`. Strip the trailing `, <hash>)` while keeping
    /// the duration / cached marker.
    static ref HASH_SUFFIX_RE: Regex =
        Regex::new(r",\s*[0-9a-f]{8}\)$").unwrap();

    /// Task-start lines after chrome-prefix stripping look like
    /// `<project>:<task> (<8-hex-hash>)`. Capture the identifier (group 1) so we
    /// can drop just the hash without nuking unrelated lines that happen to end
    /// in `(<hex>)` (e.g. `bun test v1.3.14 (0d9b296a)` — bun's version hash).
    static ref BARE_HASH_RE: Regex =
        Regex::new(r"^([a-z0-9_\-]+:[a-z0-9_\-]+)\s*\([0-9a-f]{8}\)$").unwrap();

    /// Footer line (single-task runs only).
    static ref FOOTER_RE: Regex =
        Regex::new(r"❯❯❯❯\s*to the moon").unwrap();

    /// Upgrade nag lines detected individually.
    static ref UPGRADE_BANNER_RES: [Regex; 3] = [
        Regex::new(r"There's a new version of moon available").unwrap(),
        Regex::new(r"Daemon, AI skill, async experiments").unwrap(),
        Regex::new(r"Run moon upgrade or install").unwrap(),
    ];
}

/// Run `moon query tasks` to build the task->tool map. Returns an empty
/// map (not Err) on failure so the filter still falls back to chrome-only
/// stripping rather than blocking the user.
fn build_task_map(verbose: u8) -> TaskMap {
    let output = resolved_command("moon")
        .arg("query")
        .arg("tasks")
        .output();

    match output {
        Ok(out) if out.status.success() => {
            let json = String::from_utf8_lossy(&out.stdout);
            match TaskMap::from_query_json(&json) {
                Ok(map) => map,
                Err(e) => {
                    if verbose > 0 {
                        eprintln!(
                            "rtk moon: failed to parse moon query tasks ({}); chrome-only filtering",
                            e
                        );
                    }
                    TaskMap::default()
                }
            }
        }
        Ok(out) => {
            if verbose > 0 {
                eprintln!(
                    "rtk moon: `moon query tasks` exited {}; chrome-only filtering",
                    out.status.code().unwrap_or(-1)
                );
            }
            TaskMap::default()
        }
        Err(e) => {
            if verbose > 0 {
                eprintln!(
                    "rtk moon: could not execute `moon query tasks` ({}); chrome-only filtering",
                    e
                );
            }
            TaskMap::default()
        }
    }
}

/// Apply chrome stripping to moon output. When `task_map` is non-empty, Task 5
/// will additionally route per-task body lines through matching tool filters;
/// for now `task_map` is unused.
pub fn filter_moon_output(input: &str, _task_map: &TaskMap) -> String {
    input
        .lines()
        .filter_map(|line| {
            // Drop upgrade banner lines anywhere they appear.
            if UPGRADE_BANNER_RES.iter().any(|re| re.is_match(line)) {
                return None;
            }
            // Handle footer — may be glued to "Time: ..." on same line.
            if FOOTER_RE.is_match(line) {
                let cleaned = FOOTER_RE.replace(line, "");
                let cleaned = cleaned.trim_end();
                if cleaned.is_empty() {
                    return None;
                }
                return Some(cleaned.to_string());
            }
            // Strip the decoration prefix.
            let stripped = CHROME_PREFIX_RE.replace(line, "");
            // Strip task-completion hash suffix `, <hash>)` → keep duration/cached.
            let stripped = HASH_SUFFIX_RE.replace(&stripped, ")");
            // Strip task-start bare-hash suffix `(<hash>)` from `project:task (hash)` lines,
            // preserving the identifier.
            let stripped = BARE_HASH_RE.replace(&stripped, "$1");
            Some(stripped.into_owned())
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Entry point for `rtk moon <args>`.
pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    // Subcommand check: if the first arg is one of the passthrough subcommands,
    // execute moon without filtering. Track usage but apply no transform.
    if let Some(first) = args.first() {
        if PASSTHROUGH_SUBCOMMANDS.contains(&first.as_str()) {
            let os_args: Vec<std::ffi::OsString> =
                args.iter().map(std::ffi::OsString::from).collect();
            return runner::run_passthrough("moon", &os_args, verbose);
        }
    }

    let mut cmd = resolved_command("moon");
    for arg in args {
        cmd.arg(arg);
    }

    let task_map = build_task_map(verbose);
    if verbose > 0 {
        eprintln!("rtk moon: detected {} tasks in workspace", task_map.len());
    }
    runner::run_filtered(
        cmd,
        "moon",
        &args.join(" "),
        move |s| filter_moon_output(s, &task_map),
        RunOptions::default(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use insta::assert_snapshot;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    fn savings_pct(raw: &str, filtered: &str) -> f64 {
        let r = count_tokens(raw) as f64;
        let f = count_tokens(filtered) as f64;
        100.0 * (1.0 - f / r)
    }

    #[test]
    fn strips_chrome_from_typecheck_success() {
        let input = include_str!("../../../tests/fixtures/moon/run_typecheck_success.txt");
        let output = filter_moon_output(input, &TaskMap::default());
        assert_snapshot!(output);
    }

    #[test]
    fn strips_chrome_from_cache_hit() {
        let input = include_str!("../../../tests/fixtures/moon/run_cache_hit.txt");
        let output = filter_moon_output(input, &TaskMap::default());
        assert_snapshot!(output);
    }

    #[test]
    fn preserves_failure_body_in_test_run() {
        let input = include_str!("../../../tests/fixtures/moon/run_test_failure.txt");
        let output = filter_moon_output(input, &TaskMap::default());
        // The failure message must still be present after chrome stripping.
        assert!(
            output.contains("fail") || output.contains("error") || output.contains("Expected"),
            "failure indicator lost during chrome strip: {}",
            output
        );
        assert_snapshot!(output);
    }

    #[test]
    fn strips_chrome_from_summary_detailed() {
        let input = include_str!("../../../tests/fixtures/moon/run_summary_detailed.txt");
        let output = filter_moon_output(input, &TaskMap::default());
        // SUMMARY section body lines have no chrome prefix — they pass through.
        // The "Loading changed files" preamble has chrome prefix and gets stripped.
        assert_snapshot!(output);
    }

    #[test]
    fn savings_chrome_only_success_fixture() {
        let raw = include_str!("../../../tests/fixtures/moon/run_typecheck_success.txt");
        let filtered = filter_moon_output(raw, &TaskMap::default());
        let s = savings_pct(raw, &filtered);
        assert!(
            s >= 20.0,
            "expected >=20% savings on success fixture, got {:.1}%",
            s
        );
    }

    #[test]
    fn savings_chrome_only_cache_hit_fixture() {
        let raw = include_str!("../../../tests/fixtures/moon/run_cache_hit.txt");
        let filtered = filter_moon_output(raw, &TaskMap::default());
        let s = savings_pct(raw, &filtered);
        // The cache-hit fixture is only 7 lines; most content is non-chrome task
        // body output, so chrome-only stripping nets ~21%. Real ≥60% savings are
        // reached in Task 6 after per-task tool filtering is added (Task 5).
        assert!(
            s >= 15.0,
            "expected >=15% savings on cache-hit fixture, got {:.1}%",
            s
        );
    }

    #[test]
    fn builds_taskmap_from_real_query_json() {
        let json = include_str!("../../../tests/fixtures/moon/query_tasks.json");
        let map = TaskMap::from_query_json(json).expect("parses query JSON");
        // The fixture is from yulii/ops-platform which has audit:format -> prettier.
        assert_eq!(map.tool_for("audit:format"), Some("prettier"));
    }

    #[test]
    fn taskmap_from_malformed_json_returns_err() {
        let result = TaskMap::from_query_json("not json");
        assert!(result.is_err());
    }

    #[test]
    fn taskmap_from_empty_json_is_empty() {
        let map = TaskMap::from_query_json(r#"{"tasks": {}}"#).expect("parses");
        assert_eq!(map.tool_for("anything:anything"), None);
    }
}
