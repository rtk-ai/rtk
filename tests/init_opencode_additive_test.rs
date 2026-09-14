//! `rtk init -g --opencode` must install the Claude Code hook *in addition to*
//! the OpenCode plugin (regression: it used to skip Claude entirely).

use std::process::Command;

use tempfile::TempDir;

#[test]
fn opencode_flag_also_installs_claude_hook() {
    let home = TempDir::new().expect("tempdir");
    let out = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .env("LC_ALL", "C")
        .env("HOME", home.path())
        .args(["init", "-g", "--opencode", "--dry-run"])
        .output()
        .expect("spawn rtk");

    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        stdout.contains("OpenCode plugin"),
        "missing OpenCode plugin install: {stdout}"
    );
    assert!(
        stdout.contains("RTK.md") && stdout.contains("settings.json"),
        "missing Claude hook install: {stdout}"
    );
}
