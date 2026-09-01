#![cfg(windows)]
//! Regression for #3727: an argument holding a quote but no space has to reach
//! an MSYS/Cygwin child intact. Skipped when no Git for Windows grep is around.

use std::path::{Path, PathBuf};
use std::process::Command;

/// `git.exe` sits in `<root>/cmd`, `<root>/mingw64/bin` or `<root>/bin`, so walk
/// up until an ancestor also has `usr/bin/grep.exe` under it.
fn msys_grep() -> Option<PathBuf> {
    let git = which::which("git").ok()?;
    for root in git.ancestors() {
        let candidate = root.join("usr").join("bin").join("grep.exe");
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    None
}

/// A `--version` probe cannot tell an MSYS grep from a native one, and only the
/// MSYS build parses its command line by the Cygwin rules under test here.
fn imports_msys_runtime(exe: &Path) -> bool {
    let Ok(bytes) = std::fs::read(exe) else {
        return false;
    };
    String::from_utf8_lossy(&bytes)
        .to_ascii_lowercase()
        .contains("msys-2.0.dll")
}

#[test]
fn quoted_pattern_reaches_msys_grep_intact() {
    let Some(grep) = msys_grep() else {
        return;
    };
    if !imports_msys_runtime(&grep) {
        return;
    }

    let fixture = r#"{"type":"user","id":1}
{"role":"assistant","id":2}
"#;
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::write(dir.path().join("q.jsonl"), fixture).expect("write fixture");

    // Put that grep first on PATH so `rtk grep` resolves to the binary under
    // test whatever else the machine has installed.
    let grep_dir = grep.parent().expect("grep.exe has a parent directory");
    let mut path = grep_dir.as_os_str().to_os_string();
    path.push(";");
    path.push(std::env::var_os("PATH").unwrap_or_default());

    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["grep", "-c", r#""type""#, "q.jsonl"])
        .current_dir(dir.path())
        .env("PATH", &path)
        .output()
        .expect("failed to run rtk");

    let stdout = String::from_utf8_lossy(&out.stdout);
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        out.status.success(),
        "rtk grep exited {:?}, expected 0\nstdout: {stdout:?}\nstderr: {stderr:?}",
        out.status.code()
    );
    assert_eq!(
        stdout.trim(),
        "1",
        "MSYS grep lost the quoted pattern\nstdout: {stdout:?}\nstderr: {stderr:?}"
    );
}
