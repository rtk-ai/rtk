#![cfg(unix)]
//! Integration tests for the GROUP path with context flags (-A/-B/-C) and the
//! safety-net passthrough for flags that break the NUL-based reparse.

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

fn write_temp(content: &str) -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("test.txt");
    std::fs::write(&path, content).expect("write");
    (dir, path)
}

// --- context compression (the gain win) ---

#[test]
fn after_context_yields_grouped_header_with_correct_match_count() {
    if !rg_available() {
        return;
    }
    let long = "x".repeat(120);
    let content =
        format!("before {long}\nMATCH {long}\nafter1 {long}\nafter2 {long}\nend {long}\n");
    let (_dir, path) = write_temp(&content);
    let out = rtk()
        .args(["grep", "-A2", "MATCH", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("1 matches"),
        "header must count only the match line, not context lines:\n{stdout}"
    );
    assert!(
        stdout.contains("after1"),
        "after-context line 1 missing:\n{stdout}"
    );
    assert!(
        stdout.contains("after2"),
        "after-context line 2 missing:\n{stdout}"
    );
}

#[test]
fn after_context_uses_dash_separator_for_context_lines() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("MATCH\nafter1\n");
    let out = rtk()
        .args(["grep", "-A1", "MATCH", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    let has_context_dash = stdout.lines().any(|l| l.contains("-after1"));
    assert!(
        has_context_dash,
        "context lines must use dash separator:\n{stdout}"
    );
}

#[test]
fn plain_grep_still_shows_grouped_header() {
    if !rg_available() {
        return;
    }
    let long = "x".repeat(120);
    let content = format!("foo {long}\nbaz\nfoo {long}\n");
    let (_dir, path) = write_temp(&content);
    let out = rtk()
        .args(["grep", "foo", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("matches in"),
        "grouped header must show when grouping compresses:\n{stdout}"
    );
}

#[test]
fn true_no_match_exits_1() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("hello world\n");
    let out = rtk()
        .args(["grep", "zzzz_no_match_xyz", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");

    assert_eq!(
        out.status.code(),
        Some(1),
        "true no-match must exit 1, not 0"
    );
}

// --- safety net: flags that break the NUL-based reparse ---

#[test]
fn no_line_number_flag_produces_output_not_zero_matches() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("hello world\n");
    let out = rtk()
        .args(["grep", "-N", "hello", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("0 matches"),
        "-N output must not be reported as '0 matches':\n{stdout}"
    );
    assert!(
        stdout.contains("hello"),
        "-N output must contain the matching line:\n{stdout}"
    );
}

#[test]
fn no_filename_flag_produces_output_not_zero_matches() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("hello world\n");
    let out = rtk()
        .args(["grep", "-I", "hello", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("0 matches"),
        "-I output must not be reported as '0 matches':\n{stdout}"
    );
    assert!(
        stdout.contains("hello"),
        "-I output must contain the matching line:\n{stdout}"
    );
}

#[test]
fn heading_flag_produces_output_not_zero_matches() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("hello world\n");
    let out = rtk()
        .args(["grep", "--heading", "hello", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("0 matches"),
        "--heading output must not be reported as '0 matches':\n{stdout}"
    );
    assert!(
        stdout.contains("hello"),
        "--heading output must contain the matching line:\n{stdout}"
    );
}

#[test]
fn pretty_flag_produces_output_not_zero_matches() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("hello world\n");
    let out = rtk()
        .args(["grep", "-p", "hello", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("0 matches"),
        "-p output must not be reported as '0 matches':\n{stdout}"
    );
    assert!(
        stdout.contains("hello"),
        "-p output must contain the matching line:\n{stdout}"
    );
}

// --- shape flags: passthrough, no NUL leak ---

#[test]
fn column_flag_output_has_no_nul() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("hello world\n");
    let out = rtk()
        .args(["grep", "--column", "hello", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains('\u{0}'),
        "--column output must not contain NUL:\n{stdout:?}"
    );
    assert!(
        stdout.contains("hello"),
        "--column output must contain the match:\n{stdout}"
    );
}

// --- token savings (the compression gain) ---

fn count_tokens(s: &str) -> usize {
    s.split_whitespace().count()
}

#[test]
fn bulky_grep_yields_token_savings() {
    if !rg_available() {
        return;
    }
    let filler: String = (0..50).map(|i| format!("word{i} ")).collect();
    let mut content = String::new();
    for i in 0..20 {
        content.push_str(&format!("MATCH line {i} {filler}\n"));
    }
    let (_dir, path) = write_temp(&content);

    let raw = Command::new("rg")
        .args(["-nH", "MATCH", path.to_str().unwrap()])
        .output()
        .expect("rg");
    let raw_tokens = count_tokens(&String::from_utf8_lossy(&raw.stdout));

    let out = rtk()
        .args(["grep", "MATCH", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let rtk_tokens = count_tokens(&String::from_utf8_lossy(&out.stdout));

    let savings = 100.0 - (rtk_tokens as f64 / raw_tokens as f64 * 100.0);
    assert!(
        savings >= 50.0,
        "expected >=50% token savings, got {savings:.1}% (raw={raw_tokens}, rtk={rtk_tokens})"
    );
}

// --- grep-only syntax falls back to system grep (#2543) ---

#[test]
fn grep_only_flags_fall_back_to_system_grep() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.java"), "DRAFT in java\n").expect("write");
    std::fs::write(dir.path().join("b.txt"), "DRAFT in text\n").expect("write");
    let path = dir.path().to_str().unwrap();

    let out = rtk()
        .args(["grep", "-rn", "DRAFT", "--include=*.java", path])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains("a.java"),
        "--include=*.java should match the java file:\n{stdout}"
    );
    assert!(
        !stdout.contains("b.txt"),
        "--include=*.java should exclude the txt file:\n{stdout}"
    );
    assert!(
        !stdout.contains("grep failed") && !stdout.contains("unrecognized"),
        "rg-incompatible flag must fall back to grep, not error:\n{stdout}"
    );
}

// --- never-worse guard: small greps must not cost more than plain grep ---

#[test]
fn small_grep_not_worse_than_plain() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("foo\n");
    let out = rtk()
        .args(["grep", "foo", path.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        stdout.contains(":1:foo"),
        "should show the match:\n{stdout}"
    );
    assert!(
        !stdout.contains("matches in"),
        "header that costs more than raw must be dropped:\n{stdout}"
    );
}

// --- #2543: bundled files-with-matches cluster (-rln / -ln) lists files, not "0 matches" ---

#[test]
fn bundled_files_with_matches_cluster_lists_files() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.txt"), "TODO alpha\n").expect("write");
    std::fs::write(dir.path().join("b.txt"), "TODO beta\n").expect("write");
    std::fs::write(dir.path().join("c.txt"), "nothing here\n").expect("write");
    let path = dir.path().to_str().unwrap();

    let out = rtk()
        .args(["grep", "-rln", "TODO", path])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert!(
        !stdout.contains("0 matches"),
        "-rln must list files, not report a false '0 matches' (#2543):\n{stdout}"
    );
    assert!(
        stdout.contains("a.txt") && stdout.contains("b.txt"),
        "-rln must list every matching file:\n{stdout}"
    );
    assert!(
        !stdout.contains("c.txt"),
        "-rln must not list non-matching files:\n{stdout}"
    );
}
