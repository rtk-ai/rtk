//! `-l` / `-L` / `--files` passthrough folds the directory prefix every path shares into a
//! `<prefix> (N files)` header. Any other shape flag on top of the list leaves it verbatim.
#![cfg(unix)]

use std::path::Path;
use std::process::Command;

fn rtk() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.env("RTK_NO_TRACK", "1");
    cmd
}

fn rg_available() -> bool {
    Command::new("rg")
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// `<root>/src/a/foo.rs`, `<root>/src/a/bar.rs`, `<root>/src/b/baz.rs` all contain `needle`;
/// `<root>/src/b/none.rs` does not.
fn fixture() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let src = dir.path().join("src");
    std::fs::create_dir_all(src.join("a")).unwrap();
    std::fs::create_dir_all(src.join("b")).unwrap();
    std::fs::write(src.join("a/foo.rs"), "needle\n").unwrap();
    std::fs::write(src.join("a/bar.rs"), "needle\n").unwrap();
    std::fs::write(src.join("b/baz.rs"), "needle\n").unwrap();
    std::fs::write(src.join("b/none.rs"), "hay\n").unwrap();
    dir
}

fn src_prefix(root: &Path) -> String {
    format!("{}/src/", root.display())
}

#[test]
fn grep_l_folds_shared_prefix_into_header() {
    let dir = fixture();
    let src = dir.path().join("src");
    let out = rtk()
        .args(["grep", "-rl", "needle", src.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(0),
        "stderr: {}",
        String::from_utf8_lossy(&out.stderr)
    );

    let prefix = src_prefix(dir.path());
    let mut lines = stdout.lines();
    assert_eq!(
        lines.next(),
        Some(format!("{prefix} (3 files)").as_str()),
        "header must name the shared prefix and the count:\n{stdout}"
    );
    let mut tails: Vec<&str> = lines.collect();
    tails.sort_unstable();
    assert_eq!(tails, ["a/bar.rs", "a/foo.rs", "b/baz.rs"], "{stdout}");
    assert!(
        !stdout.contains(&format!("{prefix}a/")),
        "no path may keep the folded prefix:\n{stdout}"
    );
}

#[test]
fn grep_big_l_folds_too() {
    let dir = fixture();
    let src = dir.path().join("src");
    let out = rtk()
        .args(["grep", "-rL", "needle", src.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);
    // Only one file lacks the needle: a single line has no prefix to fold, so the path
    // prints in full, exactly as grep printed it.
    assert_eq!(
        stdout.trim_end(),
        src.join("b/none.rs").to_str().unwrap(),
        "{stdout}"
    );

    // Add a second non-matching file and the fold kicks in.
    std::fs::write(src.join("a/none.rs"), "hay\n").unwrap();
    let out = rtk()
        .args(["grep", "-rL", "needle", src.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with(&format!("{} (2 files)\n", src_prefix(dir.path()))),
        "{stdout}"
    );
}

#[test]
fn single_file_list_line_is_verbatim() {
    let dir = fixture();
    let only = dir.path().join("src/a/foo.rs");
    let out = rtk()
        .args(["grep", "-l", "needle", only.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim_end(), only.to_str().unwrap(), "{stdout}");
    assert!(
        !stdout.contains("files)"),
        "no header for one line:\n{stdout}"
    );
}

#[test]
fn null_joined_list_is_verbatim() {
    let dir = fixture();
    let src = dir.path().join("src");
    let out = rtk()
        .args(["grep", "-rl", "--null", "needle", src.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains('\0'),
        "--null output must stay NUL-joined:\n{stdout:?}"
    );
    assert!(
        !stdout.contains("files)"),
        "no header under --null:\n{stdout:?}"
    );
    assert_eq!(
        stdout.matches(&src_prefix(dir.path())).count(),
        3,
        "every path keeps its full prefix under -Z:\n{stdout:?}"
    );
}

#[test]
fn count_with_list_is_verbatim() {
    let dir = fixture();
    let src = dir.path().join("src");
    let out = rtk()
        .args(["grep", "-rlc", "needle", src.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(!stdout.contains("files)"), "no header under -c:\n{stdout}");
    // GNU grep lets -l win over -c, BSD grep prints both; either way rtk forwards it as is.
    let raw = Command::new("grep")
        .args(["-rlc", "needle", src.to_str().unwrap()])
        .output()
        .expect("grep");
    assert_eq!(stdout, String::from_utf8_lossy(&raw.stdout));
}

#[test]
fn no_match_list_exits_1_with_empty_stdout() {
    let dir = fixture();
    let src = dir.path().join("src");
    let out = rtk()
        .args(["grep", "-rl", "absent", src.to_str().unwrap()])
        .output()
        .expect("rtk grep");
    assert_eq!(out.status.code(), Some(1));
    assert!(
        out.stdout.is_empty(),
        "{:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}

#[test]
fn rg_l_and_files_fold() {
    if !rg_available() {
        return;
    }
    let dir = fixture();
    let src = dir.path().join("src");
    let prefix = src_prefix(dir.path());

    let out = rtk()
        .args(["rg", "-l", "needle", src.to_str().unwrap()])
        .output()
        .expect("rtk rg");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with(&format!("{prefix} (3 files)\n")),
        "{stdout}"
    );

    let out = rtk()
        .args(["rg", "--files", src.to_str().unwrap()])
        .output()
        .expect("rtk rg");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.starts_with(&format!("{prefix} (4 files)\n")),
        "{stdout}"
    );
    let mut tails: Vec<&str> = stdout.lines().skip(1).collect();
    tails.sort_unstable();
    assert_eq!(tails, ["a/bar.rs", "a/foo.rs", "b/baz.rs", "b/none.rs"]);
}

#[test]
fn rg_l_from_piped_stdin_is_verbatim() {
    if !rg_available() {
        return;
    }
    use std::io::Write;
    use std::process::Stdio;
    let mut child = rtk()
        .args(["rg", "-l", "needle"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()
        .expect("spawn");
    child.stdin.take().unwrap().write_all(b"needle\n").unwrap();
    let out = child.wait_with_output().unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(stdout.trim_end(), "<stdin>", "{stdout}");
}
