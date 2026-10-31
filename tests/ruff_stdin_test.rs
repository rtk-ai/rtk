#![cfg(unix)]

use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

mod common;

fn check_stdin(args: &[&str], input: &[u8], redirected: bool) {
    let dir = tempfile::tempdir().expect("temporary Ruff directory");
    let capture = dir.path().join("received");
    let stub = dir.path().join("ruff");
    std::fs::write(
        &stub,
        "#!/bin/sh\ncat > \"$RUFF_STDIN_CAPTURE\"\nprintf '[]\\n'\nexit 7\n",
    )
    .expect("write Ruff stand-in");
    std::fs::set_permissions(&stub, std::fs::Permissions::from_mode(0o755))
        .expect("executable Ruff stand-in");
    let mut cmd = common::rtk_command();
    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    cmd.arg("ruff")
        .args(args)
        .env("PATH", path)
        .env("RUFF_STDIN_CAPTURE", &capture)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let output = if redirected {
        let source = dir.path().join("source");
        std::fs::write(&source, input).expect("write redirected stdin");
        cmd.stdin(std::fs::File::open(source).expect("open redirected stdin"))
            .output()
            .expect("run rtk ruff")
    } else {
        let mut child = cmd.stdin(Stdio::piped()).spawn().expect("spawn rtk ruff");
        let mut stdin = child.stdin.take().expect("piped stdin");
        // A writer thread also handles payloads larger than the pipe buffer.
        let bytes = input.to_vec();
        let writer = std::thread::spawn(move || stdin.write_all(&bytes));
        let output = child.wait_with_output().expect("wait for rtk ruff");
        let _ = writer.join().expect("stdin writer");
        output
    };
    assert_eq!(
        output.status.code(),
        Some(7),
        "native exit code: {output:?}"
    );
    assert_eq!(
        std::fs::read(capture).expect("captured stdin"),
        input,
        "Ruff must receive the original stdin bytes"
    );
}

#[test]
fn check_receives_piped_source() {
    check_stdin(
        &["check", "--stdin-filename", "sample.py", "-"],
        b"import sys\n",
        false,
    );
}

#[test]
fn format_receives_redirected_source() {
    check_stdin(
        &["format", "--stdin-filename", "sample.py", "-"],
        b"x=1\n",
        true,
    );
}

#[test]
fn check_receives_large_binary_payload_unchanged() {
    let bytes: Vec<u8> = (0..131072).map(|i| (i % 256) as u8).collect();
    check_stdin(
        &["check", "--stdin-filename", "sample.py", "-"],
        &bytes,
        false,
    );
}

#[test]
fn empty_stdin_reaches_eof() {
    check_stdin(&["check", "--stdin-filename", "sample.py", "-"], b"", false);
}
