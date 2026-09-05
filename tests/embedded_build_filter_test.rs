//! Build/analysis route contracts using an installed native executable.

use std::process::Command;

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
