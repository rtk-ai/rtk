//! The Semgrep job scans a PR merge, so its baseline must be the base tree
//! included in that merge, not an older SHA from the event payload.

use std::fs;
use std::path::Path;
use std::process::Command;

mod common;

const WORKFLOW: &str = include_str!("../.github/workflows/ci.yml");

fn git(repo: &Path, args: &[&str]) -> String {
    let mut cmd = Command::new("git");
    common::isolate_git(&mut cmd);
    let output = cmd
        .current_dir(repo)
        .args(args)
        .output()
        .expect("run isolated git");
    assert!(output.status.success(), "git {args:?}: {output:?}");
    String::from_utf8(output.stdout)
        .expect("git output is UTF-8")
        .trim()
        .to_string()
}

fn semgrep_commands() -> Vec<&'static str> {
    let commands: Vec<_> = WORKFLOW
        .lines()
        .filter_map(|line| line.trim().strip_prefix("- run: semgrep scan "))
        .collect();
    assert_eq!(
        commands.len(),
        2,
        "expected the ERROR and WARNING scan steps"
    );
    commands
}

fn baseline(command: &str, event_base: &str) -> String {
    let rendered = command.replace("${{ github.event.pull_request.base.sha }}", event_base);
    let mut args = rendered.split_whitespace();
    args.find(|arg| *arg == "--baseline-commit")
        .expect("scan must have a baseline");
    args.next().expect("baseline must have a value").to_string()
}

struct Merge {
    repo: tempfile::TempDir,
    event_base: String,
    merge_base: String,
}

fn stale_event_merge() -> Merge {
    let repo = common::temp_git_repo();
    let dir = repo.path();
    let event_base = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["switch", "-q", "-c", "feature"]);
    fs::write(dir.join("pr.rs"), "fn introduced_by_pr() {}\n").expect("write PR change");
    git(dir, &["add", "pr.rs"]);
    git(dir, &["commit", "-q", "-m", "PR change"]);

    git(dir, &["switch", "-q", "-c", "base", &event_base]);
    fs::write(dir.join("base.rs"), "fn introduced_by_base() {}\n").expect("write base change");
    git(dir, &["add", "base.rs"]);
    git(dir, &["commit", "-q", "-m", "base advanced"]);
    let merge_base = git(dir, &["rev-parse", "HEAD"]);
    git(dir, &["merge", "-q", "--no-ff", "--no-edit", "feature"]);

    Merge {
        repo,
        event_base,
        merge_base,
    }
}

#[test]
fn semgrep_uses_the_base_tree_in_the_checked_out_merge() {
    let merge = stale_event_merge();
    for command in semgrep_commands() {
        let baseline = baseline(command, &merge.event_base);
        assert_eq!(
            git(merge.repo.path(), &["rev-parse", &baseline]),
            merge.merge_base,
            "the event base predates changes already included from base"
        );
    }
}

#[test]
fn semgrep_baseline_keeps_pr_changes_without_blame_for_base_changes() {
    let merge = stale_event_merge();
    for command in semgrep_commands() {
        let baseline = baseline(command, &merge.event_base);
        assert_eq!(
            git(
                merge.repo.path(),
                &["diff", "--name-only", &baseline, "HEAD"]
            ),
            "pr.rs",
            "scan the PR change, not base.rs inherited from the base branch"
        );
    }
}

#[test]
fn semgrep_error_gate_and_advisory_warning_scan_are_preserved() {
    let commands = semgrep_commands();
    assert!(commands[0].ends_with("--severity ERROR --error"));
    assert!(commands[1].ends_with("--severity WARNING || true"));
}

#[test]
fn semgrep_checkout_keeps_the_pr_merge_and_its_parent_history() {
    let job = WORKFLOW
        .split_once("\n  semgrep:\n")
        .expect("Semgrep job")
        .1
        .split("\n  benchmark:\n")
        .next()
        .expect("Semgrep job body");
    assert!(job.contains("uses: actions/checkout@"));
    assert!(
        job.contains("fetch-depth: 0"),
        "the merge's parent must be available"
    );
    assert!(
        !job.lines().any(|line| line.trim().starts_with("ref:")),
        "an explicit checkout ref could replace the synthetic PR merge"
    );
}
