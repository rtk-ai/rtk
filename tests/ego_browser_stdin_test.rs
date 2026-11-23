#![cfg(unix)]
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::Stdio;

mod common;

#[test]
fn ego_browser_forwards_heredoc_stdin_and_preserves_errors_and_exit_status() {
    let directory = common::tempdir();
    let script = directory.path().join("ego-browser");
    std::fs::write(
        &script,
        "#!/bin/sh\n/bin/cat\nprintf 'browser error\\n' >&2\nexit 7\n",
    )
    .expect("write fixture");
    std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755))
        .expect("chmod fixture");
    let mut command = common::rtk_command();
    let mut child = command
        .args(["ego-browser", "nodejs"])
        .env("PATH", directory.path())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn proxy");
    child
        .stdin
        .take()
        .expect("stdin")
        .write_all(b"const unchanged = true;\n")
        .expect("write stdin");
    let output = child.wait_with_output().expect("wait");
    assert_eq!(output.status.code(), Some(7));
    let combined = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(combined.contains("const unchanged = true;"));
    assert!(combined.contains("browser error"));
}
