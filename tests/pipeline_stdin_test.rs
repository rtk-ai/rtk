#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};

fn run_with_stdin(command: &mut Command, input: &[u8]) -> Output {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn command");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input)
        .expect("write stdin");
    child.wait_with_output().expect("wait for command")
}

#[test]
fn wc_reads_piped_stdin() {
    let output = run_with_stdin(
        Command::new(env!("CARGO_BIN_EXE_rtk")).args(["wc", "-l"]),
        b"alpha\nbeta\n",
    );

    assert!(output.status.success());
    assert_eq!(String::from_utf8_lossy(&output.stdout).trim(), "2");
}

#[test]
fn wc_preserves_native_failure_exit_code() {
    let invalid_option = "--definitely-invalid-rtk-test-option";
    let rtk = run_with_stdin(
        Command::new(env!("CARGO_BIN_EXE_rtk")).args(["wc", invalid_option]),
        b"input\n",
    );
    let native = run_with_stdin(Command::new("wc").arg(invalid_option), b"input\n");

    assert!(!rtk.status.success());
    assert_eq!(rtk.status.code(), native.status.code());
}

#[test]
fn filtered_gh_pr_actions_preserve_piped_stdin() {
    let temp = tempfile::tempdir().expect("tempdir");
    let gh = temp.path().join("gh");
    let capture = temp.path().join("stdin.txt");
    std::fs::write(
        &gh,
        "#!/bin/sh\ncat > \"$RTK_STDIN_CAPTURE\"\nprintf 'https://github.com/example/repo/pull/42\\n'\n",
    )
    .expect("write fake gh");
    let mut permissions = std::fs::metadata(&gh)
        .expect("fake gh metadata")
        .permissions();
    permissions.set_mode(0o755);
    std::fs::set_permissions(&gh, permissions).expect("make fake gh executable");
    let path = format!(
        "{}:{}",
        temp.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let create = run_with_stdin(
        Command::new(env!("CARGO_BIN_EXE_rtk"))
            .env("PATH", &path)
            .env("RTK_STDIN_CAPTURE", &capture)
            .args(["gh", "pr", "create", "--body-file", "-"]),
        b"body from stdin\n",
    );

    assert!(
        create.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&create.stderr)
    );
    assert_eq!(
        std::fs::read(&capture).expect("read captured create stdin"),
        b"body from stdin\n"
    );

    let comment = run_with_stdin(
        Command::new(env!("CARGO_BIN_EXE_rtk"))
            .env("PATH", path)
            .env("RTK_STDIN_CAPTURE", &capture)
            .args(["gh", "pr", "comment", "42", "--body-file", "-"]),
        b"comment from stdin\n",
    );

    assert!(
        comment.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&comment.stderr)
    );
    assert_eq!(
        std::fs::read(capture).expect("read captured comment stdin"),
        b"comment from stdin\n"
    );
}
