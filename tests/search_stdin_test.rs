#![cfg(unix)]
//! Which stdin the engines actually read. These run in CI as integration tests rather than
//! `#[ignore]`d unit tests: `CARGO_BIN_EXE_rtk` guarantees a built binary, so the fix they
//! cover is exercised by `cargo test --all` instead of only by a flag nobody passes.

use std::process::{Command, Stdio};

fn rtk() -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.env("LC_ALL", "C");
    cmd
}

/// Each test guards on the engine it actually drives -- guarding an rg test on grep (or the
/// reverse) turns a missing tool into a silent pass rather than a skip.
fn engine_available(engine: &str) -> bool {
    Command::new(engine)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

#[test]
fn rg_shows_filenames_when_stdin_is_not_a_pipe() {
    if !engine_available("rg") {
        return;
    }
    // `rtk rg -z foo < /dev/null` in a multi-file dir used to drop filenames: stdin being a
    // non-terminal, non-pipe redirect was misread as "the engine reads stdin," routing into the
    // streaming path, which can't discover "multiple files" the way the buffered path does.
    // Real rg searches the cwd here, not stdin.
    let dir = tempfile::tempdir().expect("test setup");
    std::fs::write(dir.path().join("a.txt"), "foo one\n").expect("test setup");
    std::fs::write(dir.path().join("b.txt"), "foo two\n").expect("test setup");

    let output = rtk()
        .args(["rg", "-z", "foo"])
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .output()
        .expect("failed to run rtk rg");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("a.txt"), "filename missing: {stdout}");
    assert!(stdout.contains("b.txt"), "filename missing: {stdout}");
}

#[test]
fn a_single_matching_file_still_gets_its_name() {
    if !engine_available("rg") {
        return;
    }
    // The engine walked the cwd itself, so the filename is the only way to place the match --
    // real rg prints it even when exactly one file matched.
    let dir = tempfile::tempdir().expect("test setup");
    std::fs::write(dir.path().join("only.txt"), "foo here\n").expect("test setup");
    std::fs::write(dir.path().join("other.txt"), "nothing\n").expect("test setup");

    let output = rtk()
        .args(["rg", "foo"])
        .current_dir(dir.path())
        .stdin(Stdio::null())
        .output()
        .expect("failed to run rtk rg");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("only.txt"), "filename missing: {stdout}");
}

#[test]
fn both_engines_read_a_redirected_file_on_stdin() {
    // A regular file on stdin *is* read by both engines (rg's own is_readable_stdin counts
    // files and sockets, not just FIFOs), so the search must return that file's match rather
    // than the cwd's.
    let dir = tempfile::tempdir().expect("test setup");
    std::fs::write(dir.path().join("a.txt"), "foo from the cwd\n").expect("test setup");
    // The fixture lives outside the searched directory: inside it, an engine that ignored
    // stdin and walked the cwd would still print "foo from stdin" and the test would pass.
    let elsewhere = tempfile::tempdir().expect("test setup");
    let piped = elsewhere.path().join("piped.log");
    std::fs::write(&piped, "foo from stdin\n").expect("test setup");

    for engine in ["grep", "rg"] {
        if !engine_available(engine) {
            continue;
        }
        let output = rtk()
            .args([engine, "foo"])
            .current_dir(dir.path())
            .stdin(std::fs::File::open(&piped).expect("test setup"))
            .output()
            .expect("failed to run rtk");

        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(
            stdout.contains("foo from stdin"),
            "{engine} did not read stdin: {stdout}"
        );
        assert!(
            !stdout.contains("foo from the cwd"),
            "{engine} walked the cwd instead of reading stdin: {stdout}"
        );
    }
}
