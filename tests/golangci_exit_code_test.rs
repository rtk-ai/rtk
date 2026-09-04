//! #2735 — `rtk golangci-lint run` must not swallow the tool's exit code.
//!
//! golangci-lint exits 1 when it finds issues; pre-commit hooks, Makefile
//! chains and agents gate on that signal. The filtered path used to rewrite
//! exit 1 to 0, reporting success for a failing tree.

#![cfg(unix)]

use std::process::Command;

fn rtk() -> Command {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
}

fn available(bin: &str) -> bool {
    Command::new(bin)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn project_with(main_go: &str) -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    std::process::Command::new("go")
        .args(["mod", "init", "lintcheck"])
        .current_dir(dir.path())
        .output()
        .expect("go mod init");
    std::fs::write(dir.path().join("main.go"), main_go).unwrap();
    std::fs::write(
        dir.path().join(".golangci.yml"),
        "version: \"2\"\nlinters:\n  default: none\n  enable: [ineffassign]\n",
    )
    .unwrap();
    dir
}

#[test]
fn golangci_lint_issues_exit_nonzero() {
    if !available("golangci-lint") || !available("go") {
        return;
    }
    let dir = project_with("package main\n\nfunc main() {\n\tx := 1\n\tx = 2\n\t_ = x\n}\n");

    let out = rtk()
        .args(["golangci-lint", "run", "./..."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        !out.status.success(),
        "lint issues must exit non-zero, got 0. stdout:\n{stdout}"
    );
    assert_eq!(
        out.status.code(),
        Some(1),
        "golangci-lint reports issues with exit 1"
    );
    assert!(stdout.contains("ineffassign"), "findings still shown:\n{stdout}");
}

#[test]
fn golangci_lint_clean_exit_zero() {
    if !available("golangci-lint") || !available("go") {
        return;
    }
    let dir = project_with("package main\n\nfunc main() {\n\t_ = 1\n}\n");

    let out = rtk()
        .args(["golangci-lint", "run", "./..."])
        .current_dir(dir.path())
        .output()
        .unwrap();
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "clean tree must exit 0, got {:?}. stdout:\n{stdout}",
        out.status.code()
    );
}
