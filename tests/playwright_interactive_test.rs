#![cfg(unix)]

use std::fs;
use std::io::{BufRead, BufReader};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;
use std::process::Stdio;
use std::sync::mpsc;
use std::time::Duration;

mod common;

fn fake_npx(path: &Path) {
    fs::write(
        path,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$RTK_TEST_ARGS\"\ntouch \"$RTK_TEST_STARTED\"\nprintf '%s\\n' '### Debugging Instructions' '- attach'\nwhile [ ! -f \"$RTK_TEST_GATE\" ]; do sleep 0.02; done\nexit 7\n",
    )
    .expect("write npx shim");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("make npx executable");
}

fn run_rtk_report(
    temp: &tempfile::TempDir,
    args: &[&str],
    fixture: &Path,
    exit_code: i32,
) -> Output {
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("create bin");
    let npx = bin.join("npx");
    fs::write(
        &npx,
        "#!/bin/sh\nprintf '%s\\n' \"$*\" > \"$RTK_TEST_ARGS\"\ncat \"$RTK_TEST_FIXTURE\"\nexit \"$RTK_TEST_EXIT\"\n",
    )
    .expect("write reporter shim");
    fs::set_permissions(&npx, fs::Permissions::from_mode(0o755)).expect("make npx executable");
    let path = format!("{}:/usr/bin:/bin", bin.display());
    common::rtk_command()
        .args(args)
        .current_dir(temp.path())
        .env("PATH", path)
        .env("RTK_TEST_ARGS", temp.path().join("args"))
        .env("RTK_TEST_FIXTURE", fixture)
        .env("RTK_TEST_EXIT", exit_code.to_string())
        .output()
        .expect("run rtk playwright")
}

#[test]
fn debug_output_streams_before_exit_and_keeps_arguments_and_exit_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let bin = temp.path().join("bin");
    fs::create_dir_all(&bin).expect("create bin");
    let npx = bin.join("npx");
    fake_npx(&npx);
    let args_path = temp.path().join("args");
    let started_path = temp.path().join("started");
    let gate_path = temp.path().join("gate");
    let path = format!("{}:/usr/bin:/bin", bin.display());

    let mut child = common::rtk_command()
        .args(["playwright", "test", "--debug=cli"])
        .current_dir(temp.path())
        .env("PATH", path)
        .env("RTK_TEST_ARGS", &args_path)
        .env("RTK_TEST_STARTED", &started_path)
        .env("RTK_TEST_GATE", &gate_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk");

    let stdout = child.stdout.take().expect("rtk stdout");
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut line = String::new();
        let _ = BufReader::new(stdout).read_line(&mut line);
        sender.send(line).ok();
    });

    for _ in 0..200 {
        if started_path.exists() {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let early_line = receiver.recv_timeout(Duration::from_millis(500)).ok();
    fs::write(&gate_path, "continue").expect("release fake process");
    let output = child.wait_with_output().expect("wait for rtk");

    assert!(
        started_path.exists(),
        "npx shim must run: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let arguments = fs::read_to_string(args_path).expect("captured arguments");
    assert_eq!(early_line.as_deref(), Some("### Debugging Instructions\n"));
    assert!(arguments.contains("--debug=cli"));
    assert!(!arguments.contains("--reporter=json"));
    assert_eq!(output.status.code(), Some(7));
}

#[test]
fn explicit_json_reporter_filters_passing_tests_and_preserves_failure_fallback() {
    let passing_temp = tempfile::tempdir().expect("passing tempdir");
    let passing_fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/playwright/passing.json");
    let passing = run_rtk_report(
        &passing_temp,
        &["playwright", "test", "--reporter=json"],
        &passing_fixture,
        0,
    );
    let passing_output = String::from_utf8_lossy(&passing.stdout);
    let args = fs::read_to_string(passing_temp.path().join("args")).expect("passed args");
    let native_bytes = fs::metadata(passing_fixture)
        .expect("fixture metadata")
        .len();
    assert_eq!(passing.status.code(), Some(0));
    assert!(passing_output.contains("passing scenario"));
    assert!(passing_output.contains("tests/sample.spec.ts:3:1"));
    assert!((passing.stdout.len() as u64) < native_bytes);
    assert_eq!(args.matches("--reporter=json").count(), 1);

    let failing_temp = tempfile::tempdir().expect("failing tempdir");
    let failing_fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/playwright/failing.json");
    let failing = run_rtk_report(
        &failing_temp,
        &["playwright", "test", "--reporter=json"],
        &failing_fixture,
        1,
    );
    let failing_output = String::from_utf8_lossy(&failing.stdout);
    for detail in [
        "failing scenario",
        "Expected 2, received 1",
        "test-results/failure/trace.zip",
        "test-results/failure/error-context.md",
        "allure-results/result.json",
    ] {
        assert!(failing_output.contains(detail), "missing {detail}");
    }
    assert_eq!(failing.status.code(), Some(1));
}

#[test]
fn list_mode_keeps_native_arguments_and_exit_code_without_reporter_injection() {
    let temp = tempfile::tempdir().expect("tempdir");
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/playwright/passing.json");
    let output = run_rtk_report(&temp, &["playwright", "test", "--list"], &fixture, 3);
    let args = fs::read_to_string(temp.path().join("args")).expect("captured args");
    assert_eq!(output.status.code(), Some(3));
    assert!(args.contains("--list"));
    assert!(!args.contains("--reporter=json"));
    assert!(String::from_utf8_lossy(&output.stdout).contains("passing scenario"));
}
