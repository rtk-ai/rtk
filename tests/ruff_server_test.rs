//! A language server must receive stdin and respond before either stdin closes
//! or the process exits. Use a fake Ruff so this regression runs without Python.

#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn assert_live_server(args: &[&str], payload_size: usize) {
    let dir = tempfile::tempdir().expect("tempdir");
    let body = format!(
        r#"{{"jsonrpc":"2.0","id":1,"result":"{}"}}"#,
        "x".repeat(payload_size)
    );
    let response = format!("Content-Length: {}\r\n\r\n{}", body.len(), body).into_bytes();
    fs::write(dir.path().join("response.bin"), &response).expect("write response");
    let tool = dir.path().join("ruff");
    fs::write(
        &tool,
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > args.bin\nIFS= read -r request || exit 2\ncat response.bin\nIFS= read -r finish || exit 3\nprintf '%s' \"$request\" >&2\nexit 7\n",
    )
    .expect("write fake Ruff");
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("join PATH");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("ruff")
        .args(args)
        .current_dir(dir.path())
        .env("PATH", path)
        .env("RTK_DB_PATH", dir.path().join("rtk.db"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk");
    let mut stdin = child.stdin.take().expect("stdin");
    let mut stdout = child.stdout.take().expect("stdout");
    let (tx, rx) = mpsc::channel();
    let response_len = response.len();
    let reader = std::thread::spawn(move || {
        let mut bytes = vec![0; response_len];
        let result = stdout.read_exact(&mut bytes).map(|()| bytes);
        let _ = tx.send(result);
    });
    let sent = stdin.write_all(b"initialize\n");
    let received = rx.recv_timeout(Duration::from_secs(5));
    let running = child.try_wait().expect("poll rtk").is_none();
    // Release the fake server on both success and failure before asserting.
    let _ = stdin.write_all(b"exit\n");
    drop(stdin);
    let output = child.wait_with_output().expect("wait for rtk");
    reader.join().expect("reader thread");

    sent.expect("send request");
    let bytes = received
        .expect("response before stdin EOF")
        .expect("complete response");
    assert_eq!(bytes, response, "protocol bytes must not be filtered");
    assert!(running, "response must arrive before server exit");
    assert_eq!(output.status.code(), Some(7), "preserve native exit code");
    assert_eq!(output.stderr, b"initialize", "preserve stderr bytes");
    let expected_args: Vec<u8> = args.iter().flat_map(|arg| arg.bytes().chain([0])).collect();
    assert_eq!(
        fs::read(dir.path().join("args.bin")).expect("args"),
        expected_args
    );
}

#[test]
fn server_keeps_stdio_live_and_byte_exact() {
    assert_live_server(&["server"], 32_768);
}

#[test]
fn global_options_before_server_keep_stdio_live() {
    for args in [
        vec!["--isolated", "server"],
        vec!["--config", "ruff.toml", "server"],
        vec!["--config=ruff.toml", "--color", "never", "server"],
    ] {
        assert_live_server(&args, 32);
    }
}
