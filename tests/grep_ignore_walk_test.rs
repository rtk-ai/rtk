//! A recursive `rtk grep` skips what the project's ignore files exclude (#4477), the way the
//! `grep` Claude Code's shell provides does, while still searching a path the agent names.
#![cfg(unix)]

use std::path::Path;
use std::process::Output;

mod common;

/// The repository from the issue: `node_modules/`, `scratch/` and `vendor/junk` ignored at the root,
/// `sub/generated/` by a nested `.gitignore`, plus a hidden file and a match inside `.git`.
fn fixture() -> tempfile::TempDir {
    let repo = common::temp_git_repo();
    let root = repo.path();
    for (path, body) in [
        ("src/a.txt", "NEEDLE\n"),
        ("src/build/e.txt", "NEEDLE\n"),
        ("node_modules/pkg/b.txt", "NEEDLE\n"),
        ("scratch/c.txt", "NEEDLE\n"),
        ("sub/generated/d.txt", "NEEDLE\n"),
        ("vendor/junk/f.txt", "NEEDLE\n"),
        (".hidden.txt", "NEEDLE\n"),
        (".git/NEEDLE_MARKER", "NEEDLE\n"),
        (".gitignore", "node_modules\nscratch/\nvendor/junk\n"),
        ("sub/.gitignore", "generated/\n"),
    ] {
        let file = root.join(path);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(file, body).unwrap();
    }
    repo
}

fn rtk_grep(dir: &Path, args: &[&str]) -> Output {
    common::rtk_command()
        .env("RTK_NO_TRACK", "1")
        .current_dir(dir)
        .arg("grep")
        .args(args)
        .output()
        .expect("rtk grep")
}

fn sorted_lines(out: &Output) -> Vec<String> {
    let mut lines: Vec<String> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .map(str::to_string)
        .collect();
    lines.sort();
    lines
}

#[test]
fn recursive_grep_skips_ignored_paths_and_reports_them() {
    let repo = fixture();
    let out = rtk_grep(repo.path(), &["-rn", "NEEDLE", "."]);
    let stderr = String::from_utf8_lossy(&out.stderr);

    assert_eq!(out.status.code(), Some(0), "stderr: {stderr}");
    assert_eq!(
        sorted_lines(&out),
        [
            "./.hidden.txt:1:NEEDLE",
            "./src/a.txt:1:NEEDLE",
            "./src/build/e.txt:1:NEEDLE"
        ]
    );
    assert!(
        stderr.contains("4 paths skipped by ignore rules")
            && stderr.contains("rtk bypass grep -rn NEEDLE ."),
        "the skipped paths must be counted and reachable: {stderr}"
    );
}

#[test]
fn recursive_grep_without_operand_walks_cwd() {
    let repo = fixture();
    let out = rtk_grep(repo.path(), &["-r", "NEEDLE"]);

    assert_eq!(
        sorted_lines(&out),
        [
            ".hidden.txt:NEEDLE",
            "src/a.txt:NEEDLE",
            "src/build/e.txt:NEEDLE"
        ]
    );
}

#[test]
fn recursive_file_list_skips_ignored_paths() {
    let repo = fixture();
    let out = rtk_grep(repo.path(), &["-rl", "NEEDLE", "src", "."]);

    assert_eq!(
        sorted_lines(&out),
        [
            "./.hidden.txt",
            "./src/a.txt",
            "./src/build/e.txt",
            "src/a.txt",
            "src/build/e.txt"
        ]
    );
}

#[test]
fn named_ignored_path_is_still_searched() {
    let repo = fixture();
    let out = rtk_grep(repo.path(), &["-rn", "NEEDLE", "node_modules/pkg"]);

    assert_eq!(sorted_lines(&out), ["node_modules/pkg/b.txt:1:NEEDLE"]);
    assert!(out.stderr.is_empty(), "nothing was skipped");
}

#[test]
fn everything_ignored_exits_without_reading_stdin() {
    let repo = fixture();
    let out = rtk_grep(repo.path(), &["-rn", "NEEDLE", "vendor"]);

    assert_eq!(out.status.code(), Some(1));
    assert!(out.stdout.is_empty());
    assert!(String::from_utf8_lossy(&out.stderr).contains("1 paths skipped"));
}
