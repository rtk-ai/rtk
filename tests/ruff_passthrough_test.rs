//! Complete Ruff documents bypass captured filtering. Replays graph/rule fixtures,
//! checks exact argv and line endings, and exercises both streams above 10 MiB.

#![cfg(unix)]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::process::Output;

mod common;

const GRAPH: &str = include_str!("fixtures/ruff_analyze_graph_raw.json");
const RULE_E501: &str = include_str!("fixtures/ruff_rule_e501_raw.txt");

/// Writes an executable `ruff` in `dir` that prints `fixture` when called with as many
/// arguments as `expected_args`, joined to the same text, and exits 99 otherwise.
fn fake_ruff(dir: &Path, expected_args: &[&str], fixture: &str) {
    let (count, joined) = (expected_args.len(), expected_args.join(" "));
    fs::write(dir.join("captured"), fixture).expect("write fixture");
    let path = dir.join("ruff");
    fs::write(
        &path,
        format!(
            "#!/bin/sh\n[ $# = {count} ] && [ \"$*\" = \"{joined}\" ] || {{ echo \"ruff got $# args: $*\" >&2; exit 99; }}\ncat \"$(dirname \"$0\")/captured\"\n"
        ),
    )
    .expect("write fake ruff");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).expect("chmod fake ruff");
}

/// Runs rtk with `dir` first on PATH, so the fake ruff shadows any real one.
fn rtk_ruff(dir: &Path, args: &[&str]) -> Output {
    let path = format!(
        "{}:{}",
        dir.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    common::rtk_command()
        .arg("ruff")
        .args(args)
        .env("PATH", path)
        .output()
        .expect("run rtk")
}

/// What a failed assertion needs to show: the exit status and stderr.
fn context(out: &Output) -> String {
    format!(
        "exit {:?}, stderr {:?}",
        out.status.code(),
        String::from_utf8_lossy(&out.stderr)
    )
}

#[test]
fn analyze_graph_json_reaches_the_caller_whole() {
    let dir = common::tempdir();
    // Both fixtures exceed the old passthrough cap.
    assert!(GRAPH.len() > RULE_E501.len());
    fake_ruff(dir.path(), &["analyze", "graph", "big"], GRAPH);

    let out = rtk_ruff(dir.path(), &["analyze", "graph", "big"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(out.status.code(), Some(0), "{}", context(&out));
    assert_eq!(stdout.trim(), GRAPH.trim(), "{}", context(&out));
    serde_json::from_str::<serde_json::Value>(&stdout).expect("graph must stay valid JSON");
}

#[test]
fn rule_documentation_reaches_the_caller_whole() {
    let dir = common::tempdir();
    fake_ruff(dir.path(), &["rule", "E501"], RULE_E501);
    let out = rtk_ruff(dir.path(), &["rule", "E501"]);
    assert_eq!(out.status.code(), Some(0), "{}", context(&out));
    assert_eq!(out.stdout, RULE_E501.as_bytes(), "{}", context(&out));
}

#[test]
fn selected_formats_reach_real_stdout_without_an_extra_newline() {
    let raw = format!("[{}]\n\n", " ".repeat(4096));
    for args in [
        vec!["check", "--output-format", "sarif", "big"],
        vec!["check", "--output-format=json", "big"],
        vec!["format", "--check", "--output-format=json-lines", "big"],
        vec!["rule", "--all", "--output-format=json"],
    ] {
        let dir = common::tempdir();
        fake_ruff(dir.path(), &args, &raw);
        let out = rtk_ruff(dir.path(), &args);
        assert_eq!(out.status.code(), Some(0), "{args:?}: {}", context(&out));
        assert_eq!(out.stdout, raw.as_bytes(), "{args:?}");
    }
}

#[test]
fn documents_beyond_capture_limit_keep_both_streams_and_exit_status() {
    let dir = common::tempdir();
    let raw = format!("[\"{}\"]", "x".repeat(11 * 1024 * 1024));
    fs::write(dir.path().join("captured"), &raw).unwrap();
    let path = dir.path().join("ruff");
    fs::write(&path, "#!/bin/sh\ncat \"$(dirname \"$0\")/captured\"\nprintf 'diagnostic without newline' >&2\nexit 1\n").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o755)).unwrap();
    let out = rtk_ruff(dir.path(), &["check", "--output-format=json", "big"]);
    assert_eq!(out.status.code(), Some(1), "{}", context(&out));
    assert_eq!(out.stdout, raw.as_bytes());
    assert_eq!(out.stderr, b"diagnostic without newline");
}

#[test]
fn shell_completion_is_a_complete_document() {
    let script = format!(
        "# generated completion\r\n{}\r\n",
        "completion() { :; }\r\n".repeat(300)
    );
    for args in [
        vec!["generate-shell-completion", "bash"],
        vec!["--isolated", "generate-shell-completion", "bash"],
        vec!["--config", "ruff.toml", "generate-shell-completion", "bash"],
    ] {
        let dir = common::tempdir();
        fake_ruff(dir.path(), &args, &script);
        let out = rtk_ruff(dir.path(), &args);
        assert_eq!(out.status.code(), Some(0), "{}", context(&out));
        assert_eq!(out.stdout, script.as_bytes(), "args: {args:?}");
        assert!(out.stderr.is_empty());
    }
}
