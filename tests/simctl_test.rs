//! Fixture captured from Xcode 27's `xcrun simctl listapps`, reduced to three
//! complete app records. User paths, device/container IDs and the user app are anonymized.

#![cfg(unix)]

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command, Output, Stdio};

const RAW: &str = include_str!("fixtures/simctl_listapps_raw.txt");

fn run(args: &[&str], raw: impl AsRef<[u8]>, exit_code: i32, input: Option<&[u8]>) -> Output {
    let dir = tempfile::tempdir().expect("tempdir");
    fs::write(dir.path().join("stdout.txt"), raw.as_ref()).expect("fixture");
    let tool = dir.path().join("xcrun");
    fs::write(&tool, format!(
        "#!/bin/sh\nprintf '%s\\0' \"$@\" > args.bin\n{}\ncat stdout.txt\nprintf 'native diagnostic\\n' >&2\nexit {exit_code}\n",
        if input.is_some() { "cat" } else { ":" },
    )).expect("fake xcrun");
    fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).expect("chmod");
    let path = std::env::join_paths(std::iter::once(dir.path().to_path_buf()).chain(
        std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
    ))
    .expect("PATH");
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("xcrun")
        .args(args)
        .current_dir(dir.path())
        .env("PATH", path)
        .env("RTK_DB_PATH", dir.path().join("rtk.db"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("rtk");
    if let Some(input) = input {
        child
            .stdin
            .take()
            .expect("stdin")
            .write_all(input)
            .expect("write input");
    }
    let output = child.wait_with_output().expect("output");
    let expected_args: Vec<u8> = args.iter().flat_map(|arg| arg.bytes().chain([0])).collect();
    assert_eq!(
        fs::read(dir.path().join("args.bin")).expect("args"),
        expected_args
    );
    output
}

#[test]
fn app_inventory_retains_every_app_and_native_fields() {
    let output = run(&["simctl", "listapps", "booted"], RAW, 0, None);
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).expect("utf8");
    for id in [
        "com.example.DemoCamera",
        "com.apple.Bridge",
        "com.apple.CarPlayApp",
    ] {
        assert!(stdout.contains(id), "missing {id}");
    }
    assert!(stdout.contains("CFBundleDisplayName = DemoCamera;"));
    assert!(stdout.contains("ApplicationType = User;"));
    assert!(stdout.contains("Path = "));
    assert!(!stdout.contains("GroupContainers"));
    assert!(
        stdout.len() * 100 < RAW.len() * 40,
        "expected at least 60% fewer bytes"
    );
    assert_eq!(output.stderr, b"native diagnostic\n");
}

#[test]
fn explicit_output_options_and_other_commands_are_byte_exact() {
    for args in [
        vec!["simctl", "list", "devices", "--json"],
        vec!["simctl", "list", "devices", "-v"],
        vec!["simctl", "list", "runtimes"],
        vec!["simctl", "listapps", "booted", "--help"],
        vec!["--find", "simctl"],
        vec!["--sdk", "simctl", "clang", "--version"],
        vec!["simctl", "listapps", "--", "booted"],
    ] {
        let output = run(&args, RAW, 0, None);
        assert!(output.status.success());
        assert_eq!(output.stdout, RAW.as_bytes(), "{args:?}");
    }
}

#[test]
fn app_inventory_failure_preserves_diagnostics_and_exit_code() {
    let output = run(&["simctl", "listapps", "booted"], RAW, 42, None);
    assert_eq!(output.status.code(), Some(42));
    assert_eq!(output.stdout, RAW.as_bytes());
    assert_eq!(output.stderr, b"native diagnostic\n");
}

#[test]
fn other_simulator_commands_receive_stdin() {
    let output = run(
        &["simctl", "pbcopy", "booted"],
        "",
        0,
        Some(b"clipboard bytes\0\xff"),
    );
    assert!(output.status.success());
    assert_eq!(output.stdout, b"clipboard bytes\0\xff");
}

#[test]
fn unrecognized_or_empty_inventory_is_preserved() {
    for raw in ["", "{}", "{\n}\n", "unexpected output\n", "{partial"] {
        let output = run(&["simctl", "listapps", "booted"], raw, 0, None);
        assert!(output.status.success());
        assert_eq!(output.stdout, raw.as_bytes());
    }
}

#[test]
fn invalid_utf8_inventory_is_preserved() {
    let raw = b"{invalid \xff bytes}\r\n";
    let output = run(&["simctl", "listapps", "booted"], raw, 0, None);
    assert!(output.status.success());
    assert_eq!(output.stdout, raw);
}
