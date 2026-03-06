use std::path::Path;
use std::process::Command;

fn repo_root() -> &'static Path {
    Path::new(env!("CARGO_MANIFEST_DIR"))
}

fn run_rtk(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(args)
        .current_dir(repo_root())
        .output()
        .expect("failed to execute rtk test binary")
}

fn stdout(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &std::process::Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

#[test]
fn version_flag_succeeds() {
    let output = run_rtk(&["--version"]);

    assert!(
        output.status.success(),
        "expected success, stderr was: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("rtk"),
        "expected version output to mention rtk, stdout was: {}",
        stdout(&output)
    );
}

#[test]
fn ls_current_directory_succeeds() {
    let output = run_rtk(&["ls", "."]);

    assert!(
        output.status.success(),
        "expected success, stderr was: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("Cargo.toml"),
        "expected ls output to include Cargo.toml, stdout was: {}",
        stdout(&output)
    );
}

#[test]
fn read_cargo_toml_succeeds() {
    let output = run_rtk(&["read", "Cargo.toml"]);

    assert!(
        output.status.success(),
        "expected success, stderr was: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("[package]"),
        "expected read output to include Cargo.toml content, stdout was: {}",
        stdout(&output)
    );
}

#[test]
fn rewrite_git_status_succeeds() {
    let output = run_rtk(&["rewrite", "git status"]);

    assert!(
        output.status.success(),
        "expected success, stderr was: {}",
        stderr(&output)
    );
    assert_eq!(stdout(&output).trim(), "rtk git status");
}

#[test]
fn init_show_succeeds() {
    let output = run_rtk(&["init", "--show"]);

    assert!(
        output.status.success(),
        "expected success, stderr was: {}",
        stderr(&output)
    );
    assert!(
        stdout(&output).contains("rtk Configuration:") || stdout(&output).contains("settings.json"),
        "expected init --show output, stdout was: {}",
        stdout(&output)
    );
}
