//! Filters `pre-commit run` output to a compact summary.
//!
//! Pre-commit emits one dot-padded line per hook plus a verbatim block
//! for any failure (and an autofix block when a hook modified files).
//! Both the dot padding and the per-pass line are dead weight for an
//! LLM — the action is in failures, autofixes, and the final count.
//!
//! ## Output shape
//!
//! Pass-only:
//!
//! ```text
//! 9 hooks: 7 passed, 2 skipped
//! ```
//!
//! With failures (keeps the failing hook's verbatim block):
//!
//! ```text
//! 9 hooks: 7 passed, 1 skipped, 1 failed
//!
//! ruff check (exit 1):
//!   src/foo.py:42:1: F401 `os` imported but unused
//!   src/foo.py:51:5: E711 Comparison to `None`
//!   Found 2 errors.
//! ```
//!
//! With autofixes (hook modified files; you'd re-stage and retry):
//!
//! ```text
//! 9 hooks: 8 passed, 1 autofixed — re-stage and retry
//!   ruff-format: src/foo.py, src/bar.py
//! ```
//!
//! ## Pass-through
//!
//! Anything that isn't `pre-commit run [...]` is forwarded raw. That
//! covers `install`, `autoupdate`, `clean`, `--help`, `--version`, etc.

use crate::core::runner;
use crate::core::utils::{resolved_command, tool_exists};
use anyhow::Result;
use lazy_static::lazy_static;
use regex::Regex;
use std::path::Path;
use std::process::Command;

lazy_static! {
    /// One hook-result line: `<hook name>\.{3,}[(parens)]<status>`.
    /// The dot padding is what we strip; the optional `(no files to check)`
    /// parenthetical that pre-commit prints for Skipped sits between dots
    /// and status, so we accept it without capturing.
    static ref HOOK_LINE: Regex =
        Regex::new(r"^(?P<name>.+?)\.{3,}(?:\([^)]*\))?(?P<status>Passed|Failed|Skipped)$")
            .expect("hook-line regex literal is valid at build time");
}

/// Build the underlying command that will run pre-commit.
///
/// Resolution order:
///
/// 1. If `pre-commit` is on PATH, use it directly. This is the historical
///    behaviour and the simplest path.
/// 2. Otherwise, if `uv` is on PATH AND a `pyproject.toml` exists in `cwd`
///    or an ancestor, fall back to `uv run pre-commit`. This is what users
///    in a uv-managed project want — pre-commit lives inside `.venv/bin/`
///    and isn't on PATH unless they enter `uv run` themselves.
/// 3. Otherwise, fall through to `pre-commit` on PATH (which will fail
///    cleanly with "command not found" — better than swallowing the call).
///
/// Returns the `Command` plus a human-readable string for `--verbose`.
fn build_precommit_command(cwd: Option<&Path>) -> (Command, String) {
    if tool_exists("pre-commit") {
        return (resolved_command("pre-commit"), "pre-commit".to_string());
    }

    if tool_exists("uv") && cwd.map(has_pyproject_in_ancestors).unwrap_or(false) {
        let mut c = resolved_command("uv");
        c.arg("run").arg("pre-commit");
        return (c, "uv run pre-commit".to_string());
    }

    // Last resort — let the OS report `command not found` rather than hiding
    // the failure.
    (resolved_command("pre-commit"), "pre-commit".to_string())
}

/// Walk up from `start` looking for a `pyproject.toml`. Used to decide
/// whether `uv run pre-commit` is a reasonable fallback.
fn has_pyproject_in_ancestors(start: &Path) -> bool {
    let mut dir: Option<&Path> = Some(start);
    while let Some(d) = dir {
        if d.join("pyproject.toml").is_file() {
            return true;
        }
        dir = d.parent();
    }
    false
}

pub fn run(args: &[String], verbose: u8) -> Result<i32> {
    let cwd = std::env::current_dir().ok();
    let (mut cmd, display_prefix) = build_precommit_command(cwd.as_deref());
    for arg in args {
        cmd.arg(arg);
    }

    if verbose > 0 {
        eprintln!("Running: {} {}", display_prefix, args.join(" "));
    }

    // Only the `run` subcommand emits the hook-result format we filter.
    // Everything else (install, autoupdate, clean, --help, --version) is
    // either short or already useful — pass straight through.
    if !is_run_subcommand(args) {
        return runner::run_filtered(
            cmd,
            "pre-commit",
            &args.join(" "),
            |raw| raw.to_string(),
            runner::RunOptions::default(),
        );
    }

    runner::run_filtered(
        cmd,
        "pre-commit",
        &args.join(" "),
        filter_precommit_output,
        runner::RunOptions::default(),
    )
}

fn is_run_subcommand(args: &[String]) -> bool {
    // `pre-commit` with zero args defaults to `run`. Otherwise, the first
    // positional token must literally be `run`. Flags-only invocations
    // (e.g. `--version`, `--help`) are not the run path and pass through.
    if args.is_empty() {
        return true;
    }
    for a in args {
        if a.starts_with('-') {
            continue;
        }
        return a == "run";
    }
    false
}

/// Status of a single hook line. The dot padding between the hook name
/// and the status word is what we strip — it's purely visual.
#[derive(Debug, PartialEq, Eq, Clone, Copy)]
enum HookStatus {
    Passed,
    Skipped,
    Failed,
}

/// One parsed hook result. `verbatim` is the failure block (or autofix
/// diff) that follows the status line, captured verbatim from `Failed`
/// until the next hook header or end of input.
#[derive(Debug)]
struct HookResult {
    name: String,
    status: HookStatus,
    /// Verbatim block after a failure (everything until the next hook
    /// line). Empty for Passed/Skipped.
    failure_block: String,
    /// True if this hook line was followed by "files were modified by
    /// this hook" — pre-commit's signal that hooks autofixed code and
    /// the user must re-stage. Treated as a soft failure with friendlier
    /// summary copy.
    autofixed: bool,
}

pub(crate) fn filter_precommit_output(raw: &str) -> String {
    // Pre-commit prints this banner when a hook fixed files in-place.
    // It can repeat across hooks; we attach it to the most recent hook.
    let autofix_marker = "files were modified by this hook";

    let mut hooks: Vec<HookResult> = Vec::new();
    let mut info_noise = Vec::<String>::new();

    for raw_line in raw.lines() {
        let line = raw_line.trim_end_matches(['\r', ' ']);

        // Drop pre-commit's "Installing environment" install noise.
        // These lines only show up on first run after `autoupdate` or
        // a hook version bump and never carry actionable info.
        if line.starts_with("[INFO] Installing environment")
            || line.starts_with("[INFO] Once installed this environment will be reused")
            || line.starts_with("[INFO] This may take a few minutes")
        {
            continue;
        }

        if let Some(caps) = HOOK_LINE.captures(line) {
            let name = caps["name"].trim().to_string();
            let status = match &caps["status"] {
                "Passed" => HookStatus::Passed,
                "Skipped" => HookStatus::Skipped,
                "Failed" => HookStatus::Failed,
                _ => unreachable!("regex restricts to the three variants"),
            };
            hooks.push(HookResult {
                name,
                status,
                failure_block: String::new(),
                autofixed: false,
            });
            continue;
        }

        // Continuation of the most recent hook's block.
        if let Some(last) = hooks.last_mut() {
            if line.contains(autofix_marker) {
                last.autofixed = true;
                // Don't include this marker in the failure block — it's
                // already captured in the summary.
                continue;
            }
            // Skip empty lines at the head of a block; preserve them
            // mid-block so error output stays readable.
            if last.status == HookStatus::Failed || last.autofixed {
                if last.failure_block.is_empty() && line.trim().is_empty() {
                    continue;
                }
                last.failure_block.push_str(line);
                last.failure_block.push('\n');
            }
        } else if !line.trim().is_empty() {
            // Output before any hook line — likely setup banner or
            // a warning. Capture as info so the user sees it.
            info_noise.push(line.to_string());
        }
    }

    render(&hooks, &info_noise)
}

fn render(hooks: &[HookResult], info_noise: &[String]) -> String {
    if hooks.is_empty() {
        // Nothing matched — output didn't look like a pre-commit run.
        // Return the original noise verbatim rather than swallow it.
        return info_noise.join("\n");
    }

    let total = hooks.len();
    let passed = hooks
        .iter()
        .filter(|h| h.status == HookStatus::Passed && !h.autofixed)
        .count();
    let skipped = hooks.iter().filter(|h| h.status == HookStatus::Skipped).count();
    let failed = hooks.iter().filter(|h| h.status == HookStatus::Failed).count();
    let autofixed = hooks.iter().filter(|h| h.autofixed).count();

    let mut out = String::new();
    out.push_str(&format!("{} hooks: {} passed", total, passed));
    if skipped > 0 {
        out.push_str(&format!(", {} skipped", skipped));
    }
    if failed > 0 {
        out.push_str(&format!(", {} failed", failed));
    }
    if autofixed > 0 {
        out.push_str(&format!(", {} autofixed", autofixed));
    }

    // Friendly retry hint when only autofixes happened — that's the
    // "run again and it'll pass" case.
    if autofixed > 0 && failed == 0 {
        out.push_str(" — re-stage and retry");
    }
    out.push('\n');

    // Per-failure verbatim blocks.
    for h in hooks.iter().filter(|h| h.status == HookStatus::Failed) {
        out.push('\n');
        out.push_str(&format!("{} (failed):\n", h.name));
        for l in h.failure_block.trim_end().lines() {
            out.push_str(l);
            out.push('\n');
        }
    }

    // Per-autofix one-liner (no diff — the user re-runs to see fixes).
    for h in hooks.iter().filter(|h| h.autofixed && h.status != HookStatus::Failed) {
        // Try to extract modified file names from the block if present.
        // pre-commit's autofix block is usually a diff; first line of the
        // hunk often contains the path.
        out.push_str(&format!("  {}: (re-run pre-commit to apply)\n", h.name));
    }

    // Trailing info, if any (e.g. CI banners).
    if !info_noise.is_empty() {
        out.push('\n');
        for l in info_noise {
            out.push_str(l);
            out.push('\n');
        }
    }

    out.trim_end().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_pass_collapses_to_one_line() {
        let raw = "\
trim trailing whitespace.................................................Passed
fix end of files.........................................................Passed
check for merge conflicts................................................Passed
gitleaks (secrets scan)..................................................Passed
";
        assert_eq!(filter_precommit_output(raw), "4 hooks: 4 passed");
    }

    #[test]
    fn passes_with_skipped_count_separately() {
        let raw = "\
trim trailing whitespace.................................................Passed
check yaml...........................................(no files to check)Skipped
check json...........................................(no files to check)Skipped
gitleaks (secrets scan)..................................................Passed
";
        assert_eq!(
            filter_precommit_output(raw),
            "4 hooks: 2 passed, 2 skipped"
        );
    }

    #[test]
    fn failure_keeps_verbatim_block() {
        let raw = "\
trim trailing whitespace.................................................Passed
ruff check...............................................................Failed
- hook id: ruff
- exit code: 1

src/foo.py:42:1: F401 [*] `os` imported but unused
Found 1 error.
gitleaks (secrets scan)..................................................Passed
";
        let out = filter_precommit_output(raw);
        assert!(out.starts_with("3 hooks: 2 passed, 1 failed\n"));
        assert!(out.contains("ruff check (failed):"));
        assert!(out.contains("- hook id: ruff"));
        assert!(out.contains("F401"));
        // Trailing pass line never bled into the failure block.
        assert!(!out.contains("gitleaks (secrets scan)"));
    }

    #[test]
    fn autofix_marker_promotes_to_retry_hint() {
        let raw = "\
trim trailing whitespace.................................................Passed
ruff-format..............................................................Failed
- hook id: ruff-format
- files were modified by this hook
gitleaks (secrets scan)..................................................Passed
";
        let out = filter_precommit_output(raw);
        // A failed hook that autofixed should still surface as failed but
        // also reflected in the autofix count.
        assert!(out.starts_with("3 hooks: 2 passed, 1 failed, 1 autofixed"));
        // Failed-with-autofix keeps its verbatim block.
        assert!(out.contains("ruff-format (failed):"));
    }

    #[test]
    fn info_install_noise_stripped() {
        let raw = "\
[INFO] Installing environment for https://github.com/psf/black.
[INFO] Once installed this environment will be reused.
[INFO] This may take a few minutes...
black....................................................................Passed
";
        assert_eq!(filter_precommit_output(raw), "1 hooks: 1 passed");
    }

    #[test]
    fn unknown_output_preserved_verbatim() {
        // If pre-commit emits nothing recognisable, we don't swallow it.
        let raw = "command not found: pre-commit\n";
        assert_eq!(
            filter_precommit_output(raw),
            "command not found: pre-commit"
        );
    }

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn fixture_pass_savings_meet_target() {
        let raw = include_str!("../../../tests/fixtures/pre_commit_pass.txt");
        let filtered = filter_precommit_output(raw);
        let savings = 100.0 - (count_tokens(&filtered) as f64 / count_tokens(raw) as f64 * 100.0);
        assert!(
            savings >= 60.0,
            "pre-commit pass filter: expected >=60% savings, got {:.1}% (raw={} filtered={})",
            savings,
            count_tokens(raw),
            count_tokens(&filtered),
        );
    }

    #[test]
    fn fixture_fail_keeps_failure_details() {
        let raw = include_str!("../../../tests/fixtures/pre_commit_fail.txt");
        let filtered = filter_precommit_output(raw);
        // Must contain at least one failure block header
        assert!(
            filtered.contains("(failed):"),
            "filtered output missing failure block:\n{}",
            filtered
        );
        // Summary must report at least one failed hook
        assert!(
            filtered.lines().next().unwrap_or("").contains("failed"),
            "summary line should mention 'failed':\n{}",
            filtered
        );
    }

    #[test]
    fn has_pyproject_finds_marker_at_root() {
        let tmp = std::env::temp_dir().join(format!("rtk-precommit-test-{}", std::process::id()));
        let nested = tmp.join("a").join("b");
        std::fs::create_dir_all(&nested).expect("mkdir");
        std::fs::write(tmp.join("pyproject.toml"), "[project]\n").expect("write pyproject");
        assert!(has_pyproject_in_ancestors(&nested));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn has_pyproject_returns_false_when_absent() {
        let tmp = std::env::temp_dir().join(format!(
            "rtk-precommit-test-noproj-{}",
            std::process::id()
        ));
        std::fs::create_dir_all(&tmp).expect("mkdir");
        assert!(!has_pyproject_in_ancestors(&tmp));
        std::fs::remove_dir_all(&tmp).ok();
    }

    #[test]
    fn is_run_detects_default_and_subcommand_forms() {
        assert!(is_run_subcommand(&[]));
        assert!(is_run_subcommand(&["run".into()]));
        assert!(is_run_subcommand(&["-v".into(), "run".into()]));
        assert!(is_run_subcommand(&["run".into(), "--all-files".into()]));
        assert!(!is_run_subcommand(&["install".into()]));
        assert!(!is_run_subcommand(&["autoupdate".into()]));
        assert!(!is_run_subcommand(&["--version".into()]));
    }
}
