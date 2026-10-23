#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

mod common;

fn write_executable(path: &Path, body: &str) {
    fs::write(path, format!("#!/bin/sh\n{body}\n")).expect("write executable");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("set executable mode");
}

fn rtk_lint_in(project: &Path, path: &Path, marker: &Path) -> Output {
    common::rtk_command()
        .arg("lint")
        .current_dir(project)
        .env("PATH", path)
        .env("RTK_ESLINT_MARKER", marker)
        .output()
        .expect("run rtk lint")
}

fn setup_project(temp: &tempfile::TempDir, body: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    let project = temp.path().join("workspace/app");
    let local_bin = temp.path().join("workspace/node_modules/.bin");
    fs::create_dir_all(&local_bin).expect("create local bin");
    fs::create_dir_all(&project).expect("create project");
    fs::write(project.join("eslint.config.js"), "export default [];\n").expect("flat config");
    fs::write(
        project.join("package.json"),
        r#"{"devDependencies":{"eslint":"^9.0.0"}}"#,
    )
    .expect("project package manifest");
    write_executable(&local_bin.join("eslint"), body);
    (project, local_bin)
}

#[test]
fn prefers_project_eslint_over_an_unrelated_global_binary() {
    let temp = tempfile::tempdir().expect("tempdir");
    let marker = temp.path().join("selected-eslint");
    let (project, local_bin) = setup_project(
        &temp,
        "printf local > \"$RTK_ESLINT_MARKER\"\nprintf '%s\\n' '[]'",
    );
    let global_bin = temp.path().join("global-bin");
    fs::create_dir_all(&global_bin).expect("create global bin");
    write_executable(
        &global_bin.join("eslint"),
        "printf global > \"$RTK_ESLINT_MARKER\"\nprintf '%s\\n' 'unrelated global eslint'",
    );
    let path = format!("{}:{}", global_bin.display(), local_bin.display());

    let output = rtk_lint_in(&project, Path::new(&path), &marker);
    assert_eq!(output.status.code(), Some(0));
    assert_eq!(
        fs::read_to_string(marker).expect("selected marker"),
        "local"
    );
}

#[test]
fn failed_eslint_preserves_stderr_and_exit_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let marker = temp.path().join("selected-eslint");
    let (project, local_bin) = setup_project(
        &temp,
        "printf '%s\\n' 'ESLint flat-config failure' >&2\nexit 2",
    );
    let output = rtk_lint_in(&project, &local_bin, &marker);
    assert_eq!(output.status.code(), Some(2));
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(diagnostics.contains("ESLint flat-config failure"));
    assert!(!diagnostics.contains("JSON parse failed"));
}

#[test]
fn malformed_json_failure_preserves_both_streams_and_exit_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let marker = temp.path().join("selected-eslint");
    let (project, local_bin) = setup_project(
        &temp,
        "printf '%s\\n' 'partial JSON'\nprintf '%s\\n' 'invalid flat config' >&2\nexit 2",
    );
    let output = rtk_lint_in(&project, &local_bin, &marker);
    assert_eq!(output.status.code(), Some(2));
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(diagnostics.contains("partial JSON"));
    assert!(diagnostics.contains("invalid flat config"));
    assert!(!diagnostics.contains("JSON parse failed"));
}

#[test]
fn eslint_process_crash_preserves_abort_exit_code() {
    let temp = tempfile::tempdir().expect("tempdir");
    let marker = temp.path().join("selected-eslint");
    let (project, local_bin) = setup_project(&temp, "kill -ABRT $$");
    let output = rtk_lint_in(&project, &local_bin, &marker);
    assert_eq!(output.status.code(), Some(134));
}

#[test]
fn missing_eslint_does_not_become_a_json_parse_error() {
    let temp = tempfile::tempdir().expect("tempdir");
    let project = temp.path().join("project");
    let path = temp.path().join("empty-path");
    fs::create_dir_all(&project).expect("create project");
    fs::create_dir_all(&path).expect("create empty PATH");
    let output = rtk_lint_in(&project, &path, &temp.path().join("marker"));
    assert_ne!(output.status.code(), Some(0));
    let diagnostics = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(!diagnostics.contains("JSON parse failed"));
}
