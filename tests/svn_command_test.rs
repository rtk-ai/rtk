#![cfg(unix)]

use std::fs;
use std::io::{Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

const PROMPT: &[u8] = b"Password for test realm: ";

#[test]
fn svn_log_relays_prompt_before_stdin_and_preserves_success_stderr() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_svn = temp.path().join("svn");
    let argv_file = temp.path().join("argv.txt");
    fs::write(
        &fake_svn,
        r#"#!/bin/sh
printf '%s' 'Password for test realm: ' >&2
IFS= read -r answer
printf '%s\n' 'warning: certificate accepted' >&2
printf '%s\n' "$@" > "$SVN_ARGV_FILE"
printf '%s\n' \
  '------------------------------------------------------------------------' \
  'r1 | dev | 2026-01-01 | 1 line' \
  '' \
  'Initialize project structure' \
  '------------------------------------------------------------------------'
"#,
    )
    .expect("write fake svn");
    let mut permissions = fs::metadata(&fake_svn)
        .expect("fake svn metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_svn, permissions).expect("make fake svn executable");

    let path = std::env::join_paths(std::iter::once(temp.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("compose PATH");

    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["svn", "log"])
        .env("PATH", path)
        .env("SVN_ARGV_FILE", &argv_file)
        .env("RTK_SUPPRESS_HOOK_WARNING", "1")
        .env("RTK_DB_PATH", temp.path().join("history.db"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk svn log");

    let mut child_stderr = child.stderr.take().expect("piped stderr");
    let (prompt_tx, prompt_rx) = mpsc::channel();
    let stderr_thread = thread::spawn(move || {
        let mut prompt = vec![0u8; PROMPT.len()];
        child_stderr
            .read_exact(&mut prompt)
            .expect("read live SVN prompt");
        prompt_tx.send(prompt.clone()).expect("send prompt");

        let mut rest = Vec::new();
        child_stderr
            .read_to_end(&mut rest)
            .expect("read remaining stderr");
        prompt.extend(rest);
        prompt
    });

    let prompt = match prompt_rx.recv_timeout(Duration::from_secs(5)) {
        Ok(prompt) => prompt,
        Err(error) => {
            let _ = child.kill();
            let _ = child.wait();
            panic!("SVN prompt was not relayed before stdin was provided: {error}");
        }
    };
    assert_eq!(prompt, PROMPT);

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(b"test-answer\n")
        .expect("answer prompt");
    let output = child.wait_with_output().expect("wait for rtk svn log");
    let stderr = stderr_thread.join().expect("join stderr reader");

    assert!(output.status.success());
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "r1 | dev | 2026-01-01 | 1 line\n\nInitialize project structure\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&stderr),
        "Password for test realm: warning: certificate accepted\n"
    );
    assert_eq!(
        fs::read_to_string(argv_file).expect("read forwarded argv"),
        "log\n--limit\n10\n"
    );
}

#[test]
fn svn_log_preserves_failure_exit_code_and_both_streams() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_svn = temp.path().join("svn");
    fs::write(
        &fake_svn,
        r#"#!/bin/sh
printf '%s\n' 'partial native stdout'
printf '%s\n' 'svn: E999999: synthetic failure' >&2
exit 7
"#,
    )
    .expect("write fake svn");
    let mut permissions = fs::metadata(&fake_svn)
        .expect("fake svn metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_svn, permissions).expect("make fake svn executable");

    let path = std::env::join_paths(std::iter::once(temp.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("compose PATH");
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["svn", "log", "-l", "1"])
        .env("PATH", path)
        .env("RTK_SUPPRESS_HOOK_WARNING", "1")
        .env("RTK_DB_PATH", temp.path().join("history.db"))
        .output()
        .expect("run failing fake svn");

    assert_eq!(output.status.code(), Some(7));
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        "partial native stdout\n"
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "svn: E999999: synthetic failure\n"
    );
}

#[test]
fn svn_log_localized_output_keeps_recovery_hint() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_svn = temp.path().join("svn");
    fs::write(
        &fake_svn,
        r#"#!/bin/sh
i=10
while [ "$i" -ge 1 ]; do
  printf '%s\n' '------------------------------------------------------------------------'
  printf 'r%s | dev | 2026-01-01 | 1 ligne\n\nmessage %s\n' "$i" "$i"
  i=$((i - 1))
done
printf '%s\n' '------------------------------------------------------------------------'
"#,
    )
    .expect("write fake svn");
    let mut permissions = fs::metadata(&fake_svn)
        .expect("fake svn metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_svn, permissions).expect("make fake svn executable");

    let path = std::env::join_paths(std::iter::once(temp.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("compose PATH");
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["svn", "log"])
        .env("PATH", path)
        .env("RTK_SUPPRESS_HOOK_WARNING", "1")
        .env("RTK_DB_PATH", temp.path().join("history.db"))
        .output()
        .expect("run localized fake svn");
    let stdout = String::from_utf8_lossy(&output.stdout);

    assert!(output.status.success());
    assert_eq!(stdout.matches(" | dev | 2026-01-01 | 1 ligne").count(), 10);
    assert!(
        !stdout
            .contains("------------------------------------------------------------------------")
    );
    assert!(stdout.contains("[default log limit: 10; add -l/--limit to show more]"));
}

#[test]
fn svn_log_streams_complete_output_above_capture_limit() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fake_svn = temp.path().join("svn");
    fs::write(
        &fake_svn,
        r#"#!/bin/sh
printf '%s\n' 'warning before large output' >&2
dd if=/dev/zero bs=1048576 count=11 2>/dev/null | tr '\000' 'x'
printf '%s\n' 'warning after large output' >&2
"#,
    )
    .expect("write fake svn");
    let mut permissions = fs::metadata(&fake_svn)
        .expect("fake svn metadata")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&fake_svn, permissions).expect("make fake svn executable");

    let path = std::env::join_paths(std::iter::once(temp.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("compose PATH");
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["svn", "log", "-l", "100000"])
        .env("PATH", path)
        .env("RTK_SUPPRESS_HOOK_WARNING", "1")
        .env("RTK_DB_PATH", temp.path().join("history.db"))
        .output()
        .expect("run large fake svn");

    assert!(output.status.success());
    assert_eq!(output.stdout.len(), 11 * 1024 * 1024);
    assert!(output.stdout.iter().all(|byte| *byte == b'x'));
    let stderr = String::from_utf8_lossy(&output.stderr);
    for expected in [
        "warning before large output",
        "svn log output exceeds 10 MiB; streaming the complete native output",
        "warning after large output",
    ] {
        assert!(stderr.contains(expected), "missing stderr: {expected}");
    }
}

#[test]
fn svn_log_relays_cancellation_and_reaps_child() {
    for signal in ["-TERM", "-INT"] {
        assert_svn_cancellation(signal, false);
    }
}

#[test]
fn svn_log_escalates_when_child_ignores_cancellation() {
    assert_svn_cancellation("-TERM", true);
}

fn assert_svn_cancellation(signal: &str, ignore_signal: bool) {
    use std::os::unix::process::ExitStatusExt;
    use std::time::Instant;

    let temp = tempfile::tempdir().expect("tempdir");
    let fake_svn = temp.path().join("svn");
    let pid_file = temp.path().join("svn.pid");
    let trap = if ignore_signal {
        "trap '' TERM INT\n"
    } else {
        ""
    };
    fs::write(
        &fake_svn,
        format!("#!/bin/sh\n{trap}echo $$ > \"$SVN_PID_FILE\"\nexec sleep 300\n"),
    )
    .expect("write fake svn");
    fs::set_permissions(&fake_svn, fs::Permissions::from_mode(0o755)).expect("chmod svn");
    let path = std::env::join_paths(std::iter::once(temp.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("compose PATH");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["svn", "log"])
        .env("PATH", path)
        .env("SVN_PID_FILE", &pid_file)
        .env("RTK_SUPPRESS_HOOK_WARNING", "1")
        .env("RTK_DB_PATH", temp.path().join("history.db"))
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rtk svn log");
    let deadline = Instant::now() + Duration::from_secs(5);
    while !pid_file.exists() && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(25));
    }
    if !pid_file.exists() {
        let _ = child.kill();
        let _ = child.wait();
        panic!("SVN child never started");
    }
    let svn_pid = fs::read_to_string(&pid_file).expect("read child PID");
    // The shim runs only after RTK has installed the relay and spawned it.
    let sent = Command::new("kill")
        .args([signal, &child.id().to_string()])
        .status()
        .expect("send cancellation");
    assert!(sent.success());
    let deadline = Instant::now() + Duration::from_secs(5);
    let status = loop {
        if let Some(status) = child.try_wait().expect("poll RTK") {
            break Some(status);
        }
        if Instant::now() >= deadline {
            break None;
        }
        thread::sleep(Duration::from_millis(25));
    };
    let alive = Command::new("kill")
        .args(["-0", svn_pid.trim()])
        .stderr(Stdio::null())
        .status()
        .expect("check child liveness")
        .success();
    // Clean up even on the broken implementation, so a failing test cannot
    // leave the shim alive or hang the rest of the suite.
    if alive {
        let _ = Command::new("kill")
            .args(["-KILL", svn_pid.trim()])
            .status();
    }
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    assert!(!alive, "cancelled RTK left its SVN child running");
    let status = status.expect("RTK did not finish after cancellation");
    let expected = if signal == "-INT" { 2 } else { 15 };
    assert_eq!(status.signal(), Some(expected));
}
