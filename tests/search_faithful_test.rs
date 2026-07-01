#![cfg(unix)]
//! Engine-faithfulness contract: `rtk grep` runs grep and `rtk rg` runs rg, each
//! with its own regex dialect and ignore semantics. rtk filters output noise only;
//! it never substitutes one engine for the other (the bug this PR removes).

use std::io::Write;
use std::process::{Command, Stdio};

fn rtk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

/// Run rtk with `input` fed on stdin; returns (stdout, exit_code).
fn rtk_stdin(input: &str, args: &[&str]) -> (String, Option<i32>) {
    let mut child = rtk()
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");
    let out = child.wait_with_output().expect("wait");
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        out.status.code(),
    )
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

fn count_tokens(s: &str) -> usize {
    s.split_whitespace().count()
}

// --- regex dialect: the single most important regression guard ---

// Covers #2253 (matches silently rewritten when grep was routed through rg),
// #2301 (rg routed to BSD grep, wrong dialect) and #545 (savings can't be faked
// against a foreign engine's output).
#[test]
fn grep_and_rg_use_their_own_regex_dialect() {
    if !rg_available() {
        return;
    }
    // "alpha" holds "al" and "gamma" holds "ga", but neither line ever contains
    // the literal string "al|ga".
    let (_dir, path) = write_temp("alpha\ngamma\n");
    let p = path.to_str().unwrap();

    // grep BRE: a bare `|` is a literal pipe, so "al|ga" matches nothing -> exit 1.
    let g = rtk().args(["grep", "al|ga", p]).output().expect("rtk grep");
    assert_eq!(
        g.status.code(),
        Some(1),
        "grep must treat | as a literal pipe (0 matches), proving no rg substitution:\n{}",
        String::from_utf8_lossy(&g.stdout)
    );

    // rg: `|` is alternation, so "al|ga" matches both lines -> exit 0.
    let r = rtk().args(["rg", "al|ga", p]).output().expect("rtk rg");
    assert_eq!(
        r.status.code(),
        Some(0),
        "rg must treat | as alternation (matches), proving it runs rg, not grep"
    );
    let r_out = String::from_utf8_lossy(&r.stdout);
    assert!(
        r_out.contains("alpha") && r_out.contains("gamma"),
        "rg alternation must match both lines:\n{r_out}"
    );
}

// --- ignore semantics: rg stays lean, grep -r descends (the node_modules flood fix) ---

// Covers #2606 (rg must keep .gitignore/binary skipping) and #2064 (forced
// --no-ignore-vcs made rg read node_modules and dump minified files).
#[test]
fn rg_honors_ignore_files_grep_does_not() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    // `.ignore` is rg-native and applies without a surrounding git repo.
    std::fs::write(dir.path().join(".ignore"), "skip.txt\n").expect("write");
    std::fs::write(dir.path().join("skip.txt"), "NEEDLE here\n").expect("write");
    std::fs::write(dir.path().join("keep.txt"), "NEEDLE here\n").expect("write");
    let path = dir.path().to_str().unwrap();

    // rg respects the ignore file: skip.txt must not appear (no forced --no-ignore-vcs).
    let r = rtk().args(["rg", "NEEDLE", path]).output().expect("rtk rg");
    let r_out = String::from_utf8_lossy(&r.stdout);
    assert!(
        r_out.contains("keep.txt"),
        "rg must match the non-ignored file:\n{r_out}"
    );
    assert!(
        !r_out.contains("skip.txt"),
        "rg must honor .ignore and skip skip.txt (no forced --no-ignore-vcs flood):\n{r_out}"
    );

    // grep knows nothing of ignore files: -r descends and matches both.
    let g = rtk()
        .args(["grep", "-rn", "NEEDLE", path])
        .output()
        .expect("rtk grep");
    let g_out = String::from_utf8_lossy(&g.stdout);
    assert!(
        g_out.contains("keep.txt") && g_out.contains("skip.txt"),
        "grep -r must descend into every file, ignoring .ignore:\n{g_out}"
    );
}

// --- token savings: rg path compresses as much as the grep path ---

// Covers #545: rg savings are measured against the real rg output, not a
// substituted engine's inflated result.
#[test]
fn bulky_rg_yields_token_savings() {
    if !rg_available() {
        return;
    }
    let filler: String = (0..50).map(|i| format!("word{i} ")).collect();
    let mut content = String::new();
    for i in 0..60 {
        content.push_str(&format!("MATCH line {i} {filler}\n"));
    }
    let (_dir, path) = write_temp(&content);

    let raw = Command::new("rg")
        .args(["-nH", "MATCH", path.to_str().unwrap()])
        .output()
        .expect("rg");
    let raw_tokens = count_tokens(&String::from_utf8_lossy(&raw.stdout));

    let out = rtk()
        .args(["rg", "MATCH", path.to_str().unwrap()])
        .output()
        .expect("rtk rg");
    let rtk_tokens = count_tokens(&String::from_utf8_lossy(&out.stdout));

    let savings = 100.0 - (rtk_tokens as f64 / raw_tokens as f64 * 100.0);
    assert!(
        savings >= 50.0,
        "expected >=50% rg token savings, got {savings:.1}% (raw={raw_tokens}, rtk={rtk_tokens})"
    );
}

// --- engine-specific flags reach their own engine ---

// `-E` selects grep's extended regex; the flag must reach grep, never be swallowed
// as ripgrep's `--encoding` (#2253) or routed to the wrong engine (#2167).
#[test]
fn grep_dash_e_selects_extended_regex() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("alpha\ngamma\n");
    let p = path.to_str().unwrap();

    // Plain BRE: bare `|` is literal -> no match, exit 1.
    let bre = rtk().args(["grep", "al|ga", p]).output().expect("rtk grep");
    assert_eq!(bre.status.code(), Some(1), "BRE | must stay literal");

    // -E (ERE): `|` is alternation -> both lines match, exit 0.
    let ere = rtk()
        .args(["grep", "-E", "al|ga", p])
        .output()
        .expect("rtk grep -E");
    assert_eq!(
        ere.status.code(),
        Some(0),
        "grep -E must enable ERE alternation, proving -E reaches grep and is not read as rg --encoding"
    );
    let out = String::from_utf8_lossy(&ere.stdout);
    assert!(
        out.contains("alpha") && out.contains("gamma"),
        "grep -E alternation must match both lines:\n{out}"
    );
}

// rg's `-g/--glob` filters the file set; it must reach rg with correct ordering,
// not be mangled by a grep that has no such flag (#1824, #2167).
#[test]
fn rg_glob_flag_filters_the_file_set() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.rs"), "NEEDLE in rust\n").expect("write");
    std::fs::write(dir.path().join("b.txt"), "NEEDLE in text\n").expect("write");
    let path = dir.path().to_str().unwrap();

    let out = rtk()
        .args(["rg", "-g", "*.rs", "NEEDLE", path])
        .output()
        .expect("rtk rg -g");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("a.rs"),
        "rg -g '*.rs' must match the rust file:\n{stdout}"
    );
    assert!(
        !stdout.contains("b.txt"),
        "rg -g '*.rs' must exclude files outside the glob:\n{stdout}"
    );
}

// --- piped stdin: read the pipe, never inject a path ---

#[test]
fn grep_reads_piped_stdin() {
    let (out, code) = rtk_stdin("alpha\nmatch me here\nomega\n", &["grep", "match"]);
    assert_eq!(
        code,
        Some(0),
        "stdin match must exit 0, got {code:?}:\n{out}"
    );
    assert!(
        out.contains("match me here"),
        "grep must read stdin and emit the match:\n{out}"
    );
    assert!(
        !out.contains("Is a directory"),
        "RTK must not inject '.' and break a stdin pipe:\n{out}"
    );
}

#[test]
fn rg_reads_piped_stdin_without_flooding_cwd() {
    if !rg_available() {
        return;
    }
    // `fn` matches thousands of repo lines; reading stdin must yield only the pipe.
    let (out, code) = rtk_stdin("fn stdin_only_line\n", &["rg", "fn"]);
    assert_eq!(
        code,
        Some(0),
        "stdin match must exit 0, got {code:?}:\n{out}"
    );
    assert!(
        out.contains("stdin_only_line"),
        "rg must read stdin and emit the match:\n{out}"
    );
    assert!(
        !out.contains(".rs:") && !out.contains("src/"),
        "rg must read stdin, not flood the working tree:\n{out}"
    );
}

#[test]
fn grep_counts_piped_stdin_via_passthrough() {
    // `-c` routes to the passthrough path; it must still read the pipe, not null stdin.
    let (out, code) = rtk_stdin("hit\nmiss\nhit\n", &["grep", "-c", "hit"]);
    assert_eq!(
        code,
        Some(0),
        "stdin count must exit 0, got {code:?}:\n{out}"
    );
    assert_eq!(
        out.trim(),
        "2",
        "grep -c must count matches from stdin:\n{out}"
    );
}

// --- pattern-less commands run instead of being blocked ---

#[test]
fn rg_type_list_passes_through() {
    if !rg_available() {
        return;
    }
    let out = rtk().args(["rg", "--type-list"]).output().expect("rtk rg");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert_eq!(out.status.code(), Some(0), "--type-list must exit 0");
    assert!(
        stdout.lines().count() > 10,
        "--type-list must produce the real type list:\n{stdout}"
    );
    assert!(
        !stdout.contains("pattern required") && !stderr.contains("pattern required"),
        "valid pattern-less command must not be blocked:\n{stdout}{stderr}"
    );
    assert!(
        !stderr.contains("rtk grep:"),
        "an rg command must never be labelled 'rtk grep:':\n{stderr}"
    );
}

#[test]
fn rg_files_lists_the_given_dir() {
    if !rg_available() {
        return;
    }
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("only_here.txt"), "x\n").expect("write");
    let path = dir.path().to_str().unwrap();

    let out = rtk()
        .args(["rg", "--files", path])
        .output()
        .expect("rtk rg");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(out.status.code(), Some(0), "--files must exit 0");
    assert!(
        stdout.contains("only_here.txt"),
        "--files must list the given dir, not the cwd:\n{stdout}"
    );
}

// #1436 (jhagberg): a pattern rg rejects must surface rg's error (exit 2), never
// be swallowed into a silent "0 matches" (exit 1) that reads as a clean no-match.
#[test]
fn rg_surfaces_regex_error_not_silent_zero() {
    if !rg_available() {
        return;
    }
    let (_dir, path) = write_temp("GET /objects/{path}\nfilePath: x\n");
    let out = rtk()
        .args(["rg", "objects|{path}", path.to_str().unwrap()])
        .output()
        .expect("rtk rg");
    assert_eq!(
        out.status.code(),
        Some(2),
        "an rg regex parse error must surface as exit 2, not a silent 0/1"
    );
}
