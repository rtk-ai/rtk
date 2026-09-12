use std::io::Write;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

#[test]
fn claude_hook_rewrites_payload_arriving_on_stdin() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk hook claude");

    let mut stdin = child.stdin.take().expect("child stdin");
    stdin
        .write_all(br#"{"tool_name":"Bash","tool_input":{"command":"git status"}}"#)
        .expect("write hook payload");
    drop(stdin);

    let output = child.wait_with_output().expect("collect hook output");
    assert!(
        output.status.success(),
        "hook should exit 0, got {:?}, stderr={}",
        output.status.code(),
        String::from_utf8_lossy(&output.stderr)
    );

    let response: serde_json::Value =
        serde_json::from_slice(&output.stdout).expect("hook JSON response");
    assert_eq!(
        response
            .pointer("/hookSpecificOutput/updatedInput/command")
            .and_then(|v| v.as_str()),
        Some("rtk git status")
    );
}

#[test]
fn claude_hook_fails_open_when_stdin_stays_open_without_payload() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["hook", "claude"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk hook claude");

    let stdin = child.stdin.take().expect("child stdin");
    let deadline = Instant::now() + Duration::from_secs(3);

    loop {
        if child.try_wait().expect("poll child").is_some() {
            drop(stdin);
            let output = child.wait_with_output().expect("collect hook output");
            assert!(
                output.status.success(),
                "hook should fail open with exit 0, got {:?}, stderr={}",
                output.status.code(),
                String::from_utf8_lossy(&output.stderr)
            );
            assert!(
                output.stdout.is_empty(),
                "hook should emit no rewrite when no payload arrives, got {}",
                String::from_utf8_lossy(&output.stdout)
            );
            return;
        }

        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            panic!("rtk hook claude blocked on open stdin without payload");
        }

        thread::sleep(Duration::from_millis(25));
    }
}
