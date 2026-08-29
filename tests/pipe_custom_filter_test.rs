#![cfg(unix)]
//! #2179: `rtk pipe -f <name>` must accept built-in TOML filters and trusted
//! project/global custom filters, not just the hard-coded Rust filter names.

use std::io::Write;
use std::process::{Command, Stdio};

fn write_probe_filter(dir: &std::path::Path) {
    let rtk_dir = dir.join(".rtk");
    std::fs::create_dir_all(&rtk_dir).unwrap();
    std::fs::write(
        rtk_dir.join("filters.toml"),
        "schema_version = 1\n\
         [filters.probe-echo]\n\
         description = \"probe echo filter\"\n\
         match_command = \"^echo\"\n\
         strip_lines_matching = [\"^noise\"]\n",
    )
    .unwrap();
}

const PROBE_INPUT: &str = "PROBE keep\nnoise a\nnoise b\nnoise c\nnoise d\nnoise e\n";

/// Run `rtk pipe -f <name>` in `dir` with `stdin`, returning (stdout, stderr, success).
fn run_pipe(dir: &std::path::Path, name: &str, stdin: &str, trust: bool) -> (String, String, bool) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.args(["pipe", "-f", name])
        .current_dir(dir)
        .env("HOME", dir)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if trust {
        // Trust project filters without mutating the real trust store.
        cmd.env("RTK_TRUST_PROJECT_FILTERS", "1").env("CI", "1");
    }
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(stdin.as_bytes())
        .unwrap();
    let out = child.wait_with_output().unwrap();
    (
        String::from_utf8_lossy(&out.stdout).into_owned(),
        String::from_utf8_lossy(&out.stderr).into_owned(),
        out.status.success(),
    )
}

#[test]
fn trusted_custom_filter_is_applied() {
    let dir = tempfile::tempdir().unwrap();
    write_probe_filter(dir.path());
    let (stdout, _stderr, ok) = run_pipe(dir.path(), "probe-echo", PROBE_INPUT, true);
    assert!(ok, "pipe should succeed for trusted custom filter");
    assert!(stdout.contains("PROBE keep"), "stdout={stdout}");
    assert!(
        !stdout.contains("noise"),
        "noise lines should be stripped: {stdout}"
    );
}

#[test]
fn untrusted_custom_filter_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    write_probe_filter(dir.path());
    let (_stdout, stderr, ok) = run_pipe(dir.path(), "probe-echo", PROBE_INPUT, false);
    assert!(!ok, "untrusted custom filter must not be accepted");
    assert!(
        stderr.contains("Unknown filter 'probe-echo'"),
        "stderr={stderr}"
    );
}

#[test]
fn builtin_toml_filter_works_without_trust() {
    // Built-in TOML filters (e.g. `make`) are always trusted and usable via -f.
    let dir = tempfile::tempdir().unwrap();
    let input = "make[1]: Entering directory '/x'\ngcc -c foo.c\nmake[1]: Leaving directory '/x'\n";
    let (stdout, _stderr, ok) = run_pipe(dir.path(), "make", input, false);
    assert!(ok, "built-in TOML filter should work without trust");
    assert!(!stdout.is_empty(), "stdout={stdout}");
}

#[test]
fn rtk_no_toml_disables_toml_filters_in_pipe() {
    // RTK_NO_TOML=1 must bypass the TOML engine, so even a built-in TOML
    // filter name becomes unknown via pipe -f.
    let dir = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.args(["pipe", "-f", "make"])
        .current_dir(dir.path())
        .env("HOME", dir.path())
        .env("RTK_NO_TOML", "1")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(b"make[1]: Entering directory '/x'\n")
        .unwrap();
    let out = child.wait_with_output().unwrap();
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        !out.status.success(),
        "RTK_NO_TOML must reject TOML filters"
    );
    assert!(stderr.contains("Unknown filter 'make'"), "stderr={stderr}");
}

#[test]
fn builtin_rust_filter_still_works() {
    let dir = tempfile::tempdir().unwrap();
    let input = "src/main.rs:42:fn main() {\nsrc/lib.rs:10:pub fn helper() {}\n";
    let (stdout, _stderr, ok) = run_pipe(dir.path(), "grep", input, false);
    assert!(ok, "built-in Rust filter must keep working");
    assert!(stdout.contains("main.rs"), "stdout={stdout}");
}
