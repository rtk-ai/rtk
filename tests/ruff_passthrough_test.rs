//! `rtk ruff` passes subcommands it has no filter for through its passthrough cap,
//! except `analyze graph`, whose JSON is only useful whole. These run the real `run()`
//! path against a stub ruff that replays output captured with ruff 0.16.6: the graph of
//! a 300-module package (`ruff analyze graph big`) and `ruff rule E501`.

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
    // `other_passthrough_subcommands_keep_the_cap` shows the cap is below RULE_E501's length.
    assert!(GRAPH.len() > RULE_E501.len());
    fake_ruff(dir.path(), &["analyze", "graph", "big"], GRAPH);

    let out = rtk_ruff(dir.path(), &["analyze", "graph", "big"]);
    let stdout = String::from_utf8_lossy(&out.stdout);

    assert_eq!(out.status.code(), Some(0), "{}", context(&out));
    assert_eq!(stdout.trim(), GRAPH.trim(), "{}", context(&out));
    serde_json::from_str::<serde_json::Value>(&stdout).expect("graph must stay valid JSON");
}

#[test]
fn other_passthrough_subcommands_keep_the_cap() {
    let dir = common::tempdir();
    fake_ruff(dir.path(), &["rule", "E501"], RULE_E501);

    let out = rtk_ruff(dir.path(), &["rule", "E501"]);
    let stdout = String::from_utf8_lossy(&out.stdout);
    let shown = stdout.trim_end();

    assert_eq!(out.status.code(), Some(0), "{}", context(&out));
    assert!(
        shown.ends_with("...") && RULE_E501.starts_with(shown.trim_end_matches("...")),
        "rule output must be a capped prefix of ruff's, got {} of {} bytes; {}",
        shown.len(),
        RULE_E501.len(),
        context(&out)
    );
}
