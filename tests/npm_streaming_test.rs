#![cfg(unix)]

use std::io::{BufRead, BufReader, Read};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::Duration;

fn npm_is_available() -> bool {
    Command::new("npm")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}

#[test]
fn npm_silent_success_preserves_zero_byte_output() {
    if !npm_is_available() {
        return;
    }

    let dir = tempfile::tempdir().expect("create npm fixture directory");
    std::fs::write(
        dir.path().join("package.json"),
        r#"{
  "private": true,
  "scripts": {
    "silent": "node silent.mjs"
  }
}
"#,
    )
    .expect("write package.json");
    std::fs::write(dir.path().join("silent.mjs"), b"").expect("write silent script");

    let raw = Command::new("npm")
        .args(["run", "--silent", "silent"])
        .current_dir(dir.path())
        .output()
        .expect("run silent npm script directly");
    assert!(raw.status.success(), "raw npm script should succeed");
    assert_eq!(
        raw.stdout, b"",
        "fixture must produce zero raw stdout bytes"
    );
    assert_eq!(
        raw.stderr, b"",
        "fixture must produce zero raw stderr bytes"
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["npm", "run", "--silent", "silent"])
        .current_dir(dir.path())
        .env("RTK_DB_PATH", dir.path().join("tracking.db"))
        .output()
        .expect("run silent npm script through rtk");

    assert!(output.status.success(), "rtk npm should preserve success");
    assert_eq!(output.stdout, b"", "zero raw stdout must stay silent");
    assert_eq!(output.stderr, b"", "zero raw stderr must stay silent");
}

#[test]
fn npm_emits_meaningful_stdout_before_script_exits() {
    if !npm_is_available() {
        return;
    }

    let dir = tempfile::tempdir().expect("create npm fixture directory");
    std::fs::write(
        dir.path().join("package.json"),
        r#"{
  "private": true,
  "scripts": {
    "ready": "node ready.mjs"
  }
}
"#,
    )
    .expect("write package.json");
    std::fs::write(
        dir.path().join("ready.mjs"),
        r#"import { existsSync } from "node:fs";

console.log("READY npm-streaming-regression");
console.error("meaningful npm stderr");
const deadline = Date.now() + 15_000;
const timer = setInterval(() => {
  if (existsSync(process.env.RTK_TEST_STOP_FILE) || Date.now() >= deadline) {
    clearInterval(timer);
  }
}, 20);
"#,
    )
    .expect("write readiness script");

    let stop_file = dir.path().join("stop");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["npm", "run", "--silent", "ready"])
        .current_dir(dir.path())
        .env("RTK_TEST_STOP_FILE", &stop_file)
        .env("RTK_DB_PATH", dir.path().join("tracking.db"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk npm");
    let stdout = child.stdout.take().expect("capture rtk stdout");
    let stderr = child.stderr.take().expect("capture rtk stderr");
    let (tx, rx) = mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut line = String::new();
        BufReader::new(stdout)
            .read_line(&mut line)
            .expect("read readiness line");
        tx.send(line).ok();
    });
    let stderr_reader = std::thread::spawn(move || {
        let mut output = String::new();
        BufReader::new(stderr)
            .read_to_string(&mut output)
            .expect("read rtk stderr");
        output
    });

    let streamed = rx.recv_timeout(Duration::from_secs(5));
    let child_was_alive = child.try_wait().expect("poll rtk npm").is_none();

    std::fs::write(&stop_file, b"stop").expect("signal readiness script to stop");
    let output_status = child.wait().expect("wait for rtk npm");
    reader.join().expect("join stdout reader");
    let stderr = stderr_reader.join().expect("join stderr reader");

    let line = streamed.expect("readiness should be emitted before the npm script exits");
    assert_eq!(line, "READY npm-streaming-regression\n");
    assert!(
        child_was_alive,
        "npm script exited before readiness was read"
    );
    assert!(output_status.success(), "rtk npm should preserve success");
    assert_eq!(stderr, "meaningful npm stderr\n");
}
