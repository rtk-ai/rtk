#![cfg(unix)]
//! Regressions from the #2333 grep overhaul: -A/-B/-C must keep context lines,
//! -c/-o must not leak ripgrep's NUL separator.

use std::process::Command;

fn rtk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn after_context_lines_are_shown() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("ctx.txt");
    std::fs::write(&f, "before1\nbefore2\nMATCHME\nafter1\nafter2\nafter3\n").unwrap();
    let out = rtk()
        .args(["grep", "-A2", "MATCHME", f.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("after1"),
        "missing after-context line 1:\n{stdout}"
    );
    assert!(
        stdout.contains("after2"),
        "missing after-context line 2:\n{stdout}"
    );
    assert!(!stdout.contains("after3"), "leaked beyond -A2:\n{stdout}");
}

#[test]
fn before_context_lines_are_shown() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("ctx.txt");
    std::fs::write(&f, "before1\nbefore2\nMATCHME\nafter1\n").unwrap();
    let out = rtk()
        .args(["grep", "-B1", "MATCHME", f.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("before2"),
        "missing before-context line:\n{stdout}"
    );
    assert!(!stdout.contains("before1"), "leaked beyond -B1:\n{stdout}");
}

#[test]
fn count_output_has_no_nul_separator() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("nul.txt");
    std::fs::write(&f, "TODO one\nnope\nTODO two\n").unwrap();
    let out = rtk()
        .args(["grep", "-c", "TODO", f.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains('\u{0}'),
        "NUL leaked into -c output: {stdout:?}"
    );
    assert!(
        stdout.contains('2'),
        "count missing from -c output: {stdout:?}"
    );
}

#[test]
fn group_separator_between_non_adjacent_matches() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("sep.txt");
    std::fs::write(
        &f,
        "alpha match here\nfiller 1\nfiller 2\nfiller 3\nfiller 4\nfiller 5\nbeta match here\n",
    )
    .unwrap();

    let out = rtk()
        .args(["grep", "-A1", "match", f.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("--"),
        "missing group separator between non-adjacent match blocks:\n{stdout}"
    );
    assert!(
        stdout.contains("alpha match here"),
        "first match must be present:\n{stdout}"
    );
    assert!(
        stdout.contains("beta match here"),
        "second match must be present:\n{stdout}"
    );
}

#[test]
fn no_separator_without_context_flag() {
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("nosep.txt");
    std::fs::write(
        &f,
        "match line 1\nfiller\nfiller\nfiller\nmatch line 2\nfiller\nfiller\nmatch line 3\n",
    )
    .unwrap();

    let out = rtk()
        .args(["grep", "-n", "match", f.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("--"),
        "plain grep without -A/-B/-C must not insert -- separators:\n{stdout}"
    );
    assert!(
        stdout.contains("match line 1"),
        "first match must be present:\n{stdout}"
    );
    assert!(
        stdout.contains("match line 3"),
        "third match must be present:\n{stdout}"
    );
}

#[test]
fn only_matching_output_has_no_nul_separator() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("nul.txt");
    std::fs::write(&f, "TODO one\nnope\nTODO two\n").unwrap();
    let out = rtk()
        .args(["grep", "-o", "TODO", f.to_str().unwrap()])
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.contains('\u{0}'),
        "NUL leaked into -o output: {stdout:?}"
    );
    assert!(
        stdout.contains("TODO"),
        "match missing from -o output: {stdout:?}"
    );
    assert!(
        !stdout.contains("1:TODO"),
        "line-number prefix leaked into -o output: {stdout:?}"
    );
}
