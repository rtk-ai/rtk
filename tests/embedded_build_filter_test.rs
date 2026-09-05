//! Build/analysis route contracts using an installed native executable.

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn compiler_route_preserves_selected_executable_and_exit_status() {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["compiler", "--program", "cargo", "--", "--version"])
        .output()
        .expect("run compiler route");
    assert!(output.status.success(), "{:?}", output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.to_ascii_lowercase().contains("cargo"));
}

#[test]
fn compiler_machine_diagnostic_mode_is_not_summarized() {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args([
            "compiler",
            "--program",
            "cargo",
            "--",
            "--version",
            "--diagnostics-format=json",
        ])
        .output()
        .expect("run compiler route");
    assert!(!output.status.success());
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("unexpected argument")
            || String::from_utf8_lossy(&output.stdout).contains("unexpected argument")
    );
}

#[test]
fn build_route_forwards_stdin_to_the_embedded_tool() {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["cmake", "-E", "cat", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run build route");
    child
        .stdin
        .take()
        .expect("build route stdin")
        .write_all(b"build-stdin\n")
        .expect("write build route stdin");

    let output = child.wait_with_output().expect("wait for build route");
    assert!(output.status.success(), "stderr={:?}", output.stderr);
    assert_eq!(output.stdout, b"build-stdin\n");
}
