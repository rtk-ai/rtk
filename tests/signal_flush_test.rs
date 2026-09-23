#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Stdio};
use std::thread::sleep;
use std::time::{Duration, Instant};
fn shim_dir() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let cargo = dir.path().join("cargo");
    let marker = dir.path().join("emitted");
    fs::write(
        &cargo,
        format!(
            "#!/usr/bin/env bash\n\
             for i in $(seq 1 50); do echo \"   Compiling crate-$i v0.1.0\"; done\n\
             echo 'error[E0308]: mismatched types'\n\
             echo '    --> src/lib.rs:12:5'\n\
             touch {}\n\
             exec sleep 300\n",
            marker.display()
        ),
    )
    .expect("write shim");
    fs::set_permissions(&cargo, fs::Permissions::from_mode(0o755)).expect("chmod shim");
    dir
}

fn wait_for(path: &std::path::Path, limit: Duration) -> bool {
    let deadline = Instant::now() + limit;
    while Instant::now() < deadline {
        if path.exists() {
            return true;
        }
        sleep(Duration::from_millis(50));
    }
    false
}

#[test]
fn signalled_run_still_prints_captured_output() {
    let dir = shim_dir();
    let home = dir.path().join("home");
    fs::create_dir_all(&home).expect("home");
    let path = format!(
        "{}:{}",
        dir.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );

    let child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cargo", "clippy"])
        .env("PATH", path)
        .env("HOME", &home)
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn rtk");

    assert!(
        wait_for(&dir.path().join("emitted"), Duration::from_secs(60)),
        "shim never produced output"
    );
    sleep(Duration::from_millis(500));

    let killed = Command::new("kill")
        .args(["-TERM", &child.id().to_string()])
        .status()
        .expect("send SIGTERM");
    assert!(killed.success(), "kill failed");

    let out = child.wait_with_output().expect("collect rtk output");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !stdout.trim().is_empty(),
        "signalled run printed nothing; captured output was lost"
    );
    assert!(
        stdout.contains("E0308"),
        "signalled run dropped the diagnostic: {}",
        stdout
    );
}
