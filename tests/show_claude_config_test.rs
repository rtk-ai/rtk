//! #5206: `rtk init --show` must not call prose that merely mentions @RTK.md
//! a configured reference. The check is a whole trimmed line, not a substring.
use std::fs;
use std::path::Path;
use tempfile::TempDir;

mod common;

fn show(claude_dir: &Path, home: &Path) -> String {
    let output = common::rtk_command()
        .args(["init", "--show"])
        .env("CLAUDE_CONFIG_DIR", claude_dir)
        .env("HOME", home)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).unwrap()
}

#[test]
fn show_does_not_call_a_prose_mention_a_reference() {
    let temp = TempDir::new().unwrap();
    let claude = temp.path().join("claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(claude.join("CLAUDE.md"), "Read @RTK.md for details.\n").unwrap();

    let status = show(&claude, temp.path());
    assert!(
        !status.contains("@RTK.md reference"),
        "prose must not be reported as configured: {status}"
    );
    assert!(
        status.contains("[--] Global (~/.claude/CLAUDE.md): exists but rtk not configured"),
        "expected explicit not-configured status: {status}"
    );
}

#[test]
fn show_still_reports_a_reference_on_its_own_line() {
    let temp = TempDir::new().unwrap();
    let claude = temp.path().join("claude");
    fs::create_dir_all(&claude).unwrap();
    fs::write(claude.join("CLAUDE.md"), "Notes\n\n@RTK.md\n").unwrap();

    let status = show(&claude, temp.path());
    assert!(
        status.contains("[ok] Global (~/.claude/CLAUDE.md): @RTK.md reference"),
        "a whole-line reference must still be reported: {status}"
    );
}
