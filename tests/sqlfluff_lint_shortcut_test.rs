//! Integration coverage for the generic SQLFluff lint shortcut.
//!
//! The finite stub records argv and returns a native diagnostic result, so the
//! test does not depend on a Python tool being installed in CI.

#![cfg(unix)]

mod common;

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

fn run_rtk(dir: &Path, capture: &Path, fixture: &Path) -> std::process::Output {
    let stub = dir.join("sqlfluff");
    fs::write(fixture, include_str!("fixtures/sqlfluff_lint_v4_raw.json"))
        .expect("write sqlfluff fixture");
    fs::write(
        &stub,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\ncat '{}'\nexit 1\n",
            capture.display(),
            fixture.display()
        ),
    )
    .expect("write sqlfluff stub");
    let mut permissions = fs::metadata(&stub)
        .expect("stat sqlfluff stub")
        .permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&stub, permissions).expect("chmod sqlfluff stub");

    let path = format!(
        "{}:{}",
        stub.parent().expect("stub parent").display(),
        std::env::var("PATH").unwrap_or_default()
    );
    common::rtk_command()
        .current_dir(dir)
        .env("PATH", path)
        .args(["lint", "sqlfluff", "diagnostic.sql"])
        .output()
        .expect("run rtk")
}

#[test]
fn generic_bare_path_runs_sqlfluff_lint_and_preserves_diagnostic_exit() {
    let dir = common::tempdir();
    let capture = dir.path().join("sqlfluff-argv");
    let fixture = dir.path().join("sqlfluff-output.json");
    let out = run_rtk(dir.path(), &capture, &fixture);

    assert_eq!(out.status.code(), Some(1), "rtk failed: {out:?}");
    let argv = fs::read_to_string(capture)
        .expect("read captured sqlfluff argv")
        .lines()
        .map(str::to_string)
        .collect::<Vec<_>>();
    assert_eq!(argv, ["lint", "--format", "json", "diagnostic.sql"]);
    assert!(
        String::from_utf8_lossy(&out.stdout).contains("SQLFluff: 3 violations"),
        "expected filtered diagnostic, got {:?}",
        String::from_utf8_lossy(&out.stdout)
    );
}
