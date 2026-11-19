//! End-to-end verification that `.venv/bin/<tool>` rewrites to `rtk <tool>` and
//! executes the project's local `.venv/bin/<tool>` when no virtual environment
//! is active on `PATH`, while honouring `VIRTUAL_ENV` when one is active.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

mod common;

fn write_stub(path: &Path, marker_file: &Path, label: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("create stub parent");
    }
    fs::write(
        path,
        format!(
            "#!/bin/sh\nprintf '%s:%s\\n' '{label}' \"$*\" > '{}'\necho '[]'\n",
            marker_file.display()
        ),
    )
    .expect("write stub");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod stub");
}

#[test]
fn local_dot_venv_ruff_rewrites_and_executes_without_activation() {
    let dir = common::tempdir();
    let marker = dir.path().join("invoked.txt");
    write_stub(&dir.path().join(".venv/bin/ruff"), &marker, "dot-venv");

    let rewrite = common::rtk_command()
        .current_dir(dir.path())
        .args(["rewrite", ".venv/bin/ruff check src/"])
        .output()
        .expect("run rtk rewrite");
    assert!(matches!(rewrite.status.code(), Some(0 | 3)));
    assert_eq!(
        String::from_utf8(rewrite.stdout)
            .expect("utf8")
            .trim_end_matches('\n'),
        "rtk ruff check src/"
    );

    let path_var = std::ffi::OsStr::new("PATH");
    let run = common::rtk_command()
        .current_dir(dir.path())
        .env(path_var, "/usr/bin:/bin")
        .env_remove("VIRTUAL_ENV")
        .env_remove("CONDA_PREFIX")
        .args(["ruff", "check", "src/"])
        .output()
        .expect("run rtk ruff");
    assert_eq!(run.status.code(), Some(0));
    let recorded = fs::read_to_string(&marker).expect("read marker");
    assert_eq!(recorded.trim(), "dot-venv:check --output-format=json src/");
}

#[test]
fn active_virtual_env_on_path_overrides_local_dot_venv() {
    let dir = common::tempdir();
    let marker = dir.path().join("invoked.txt");
    write_stub(&dir.path().join(".venv/bin/ruff"), &marker, "stale-local");

    let isolated_bin = dir.path().join("isolated/bin");
    write_stub(&isolated_bin.join("ruff"), &marker, "active-isolated");

    let path_var = std::ffi::OsStr::new("PATH");
    let run = common::rtk_command()
        .current_dir(dir.path())
        .env("VIRTUAL_ENV", dir.path().join("isolated"))
        .env(
            path_var,
            format!("{}:/usr/bin:/bin", isolated_bin.display()),
        )
        .args(["ruff", "check", "src/"])
        .output()
        .expect("run rtk ruff");
    assert_eq!(run.status.code(), Some(0));
    let recorded = fs::read_to_string(&marker).expect("read marker");
    assert_eq!(
        recorded.trim(),
        "active-isolated:check --output-format=json src/"
    );
}
