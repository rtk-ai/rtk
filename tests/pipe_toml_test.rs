//! `rtk pipe --toml` — apply an arbitrary (untrusted) TOML filter file to stdin.
#![cfg(unix)]

use std::io::Write;
use std::process::{Command, Output, Stdio};

fn run_pipe_toml(toml_path: &std::path::Path, args: &[&str], input: &str) -> Output {
    let mut cmd_args = vec!["pipe", "--toml"];
    let path = toml_path.to_str().expect("utf-8 path");
    cmd_args.push(path);
    cmd_args.extend_from_slice(args);

    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(&cmd_args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk pipe");

    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(input.as_bytes())
        .expect("write stdin");

    child.wait_with_output().expect("wait for rtk pipe")
}

fn write_toml(dir: &tempfile::TempDir, name: &str, contents: &str) -> std::path::PathBuf {
    let path = dir.path().join(name);
    std::fs::write(&path, contents).expect("write toml");
    path
}

#[test]
fn pipe_toml_applies_single_filter_without_trust() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toml = write_toml(
        &dir,
        "draft.toml",
        r#"
schema_version = 1
[filters.draft-noise]
description = "strip NOISE lines"
match_command = "^draft-noise\\b"
strip_lines_matching = ["^NOISE"]
"#,
    );

    let out = run_pipe_toml(&toml, &[], "keep me\nNOISE drop me\nkeep me too\n");

    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    let stdout = String::from_utf8_lossy(&out.stdout);
    // TOML pipeline joins lines with '\n' and does not re-add a trailing newline.
    assert_eq!(stdout, "keep me\nkeep me too");
}

#[test]
fn pipe_toml_selects_named_filter_with_dash_f() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toml = write_toml(
        &dir,
        "multi.toml",
        r#"
schema_version = 1
[filters.keep-errors]
match_command = "^keep-errors\\b"
keep_lines_matching = ["(?i)error"]
[filters.strip-noise]
match_command = "^strip-noise\\b"
strip_lines_matching = ["^noise"]
"#,
    );

    let out = run_pipe_toml(
        &toml,
        &["-f", "keep-errors"],
        "info ok\nERROR boom\nwarn meh\n",
    );

    assert!(
        out.status.success(),
        "stderr={}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert_eq!(String::from_utf8_lossy(&out.stdout), "ERROR boom");
}

#[test]
fn pipe_toml_requires_dash_f_when_multiple_filters() {
    let dir = tempfile::tempdir().expect("tempdir");
    let toml = write_toml(
        &dir,
        "multi.toml",
        r#"
schema_version = 1
[filters.a]
match_command = "^a\\b"
max_lines = 1
[filters.b]
match_command = "^b\\b"
max_lines = 1
"#,
    );

    let out = run_pipe_toml(&toml, &[], "line\n");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("multiple filters") && stderr.contains("-f"),
        "stderr={}",
        stderr
    );
}

#[test]
fn pipe_toml_rejects_missing_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nope.toml");
    let out = run_pipe_toml(&missing, &[], "x\n");
    assert!(!out.status.success());
    let stderr = String::from_utf8_lossy(&out.stderr);
    assert!(
        stderr.contains("failed to read") || stderr.contains("No such file"),
        "stderr={}",
        stderr
    );
}

#[test]
fn pipe_toml_does_not_require_trust_store_entry() {
    // Same as the single-filter case, but assert trusted_filters.json is untouched /
    // not required: filter lives outside gated paths and still applies.
    let dir = tempfile::tempdir().expect("tempdir");
    let toml = write_toml(
        &dir,
        "outside-config.toml",
        r#"
schema_version = 1
[filters.preview]
match_command = "^preview\\b"
strip_lines_matching = ["^zzz"]
"#,
    );

    let out = run_pipe_toml(&toml, &[], "hello\nzzz gone\nworld\n");
    assert!(out.status.success());
    assert_eq!(String::from_utf8_lossy(&out.stdout), "hello\nworld");
}
