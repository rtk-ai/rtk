//! `rtk hook opencode-after --probe` answers whether the resolved pipeline has
//! an after-tool rule at all.
//!
//! The OpenCode plugin asks this before it registers `tool.execute.after`: a
//! pipeline with no `hook.after_tool.*` rule would otherwise spawn a process on
//! every tool call, read its payload and print nothing.

use std::path::Path;
use std::process::{Command, Stdio};

mod common;

/// Runs rtk with `home` as its home and `home/.config/rtk/pipeline.toml` as the
/// user pipeline, in a project directory of its own, so neither the developer's
/// config nor this repository's `.rtk/` shapes the answer.
fn rtk(home: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.env("HOME", home)
        // `RTK_CONFIG_DIR` is the documented override and, unlike
        // `XDG_CONFIG_HOME`, means the same directory on every platform.
        .env("RTK_CONFIG_DIR", home.join("config/rtk"))
        .env("XDG_CONFIG_HOME", home.join(".config"))
        .env("XDG_DATA_HOME", home.join(".local").join("share"))
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    cmd
}

fn write_pipeline(home: &Path, body: &str) {
    let dir = home.join("config").join("rtk");
    std::fs::create_dir_all(&dir).expect("create config dir");
    std::fs::write(dir.join("pipeline.toml"), body).expect("write pipeline");
}

fn probe(home: &Path) -> serde_json::Value {
    let out = rtk(home)
        .args(["hook", "opencode-after", "--probe"])
        .output()
        .expect("rtk runs");
    assert!(
        out.status.success(),
        "probe failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    serde_json::from_slice(&out.stdout).expect("probe prints one JSON object")
}

fn home() -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("tempdir");
    // The project directory the child starts in must not carry a pipeline of
    // its own; `rtk_command` isolates it in the test's scratch directory.
    let _ = home;
    home
}

#[test]
fn a_pipeline_with_an_after_tool_rule_is_active() {
    let home = home();
    write_pipeline(
        home.path(),
        "[[hook.after_tool.shell]]\nname = \"squeeze\"\nuse = [\"dedup\"]\n",
    );
    let answer = probe(home.path());
    assert_eq!(answer["v"], 1);
    assert_eq!(answer["active"], true, "{answer}");
}

#[test]
fn a_pipeline_without_one_is_inactive() {
    let home = home();
    write_pipeline(home.path(), "[pipeline]\nname = \"before-only\"\n");
    let answer = probe(home.path());
    assert_eq!(answer["v"], 1);
    assert_eq!(answer["active"], false, "{answer}");
}

#[test]
fn no_pipeline_file_answers_inactive() {
    // The fallback is `rtk:default`, which ships no after_tool rules.
    let home = home();
    let answer = probe(home.path());
    assert_eq!(answer["v"], 1);
    assert_eq!(answer["active"], false, "{answer}");
}

#[test]
fn a_pro_only_after_tool_rule_answers_inactive() {
    // `locked` is a pro-edition key: this edition loads the file but marks the
    // rule inert, so it can never fire and the hook must not be registered.
    let home = home();
    write_pipeline(
        home.path(),
        "[[hook.after_tool.shell]]\nname = \"pro\"\nuse = [\"dedup\"]\nlocked = true\n",
    );
    assert_eq!(probe(home.path())["active"], false);
}

#[test]
fn a_disabled_after_tool_rule_answers_inactive() {
    let home = home();
    write_pipeline(
        home.path(),
        "[[hook.after_tool.shell]]\nname = \"off\"\nenabled = false\nuse = [\"dedup\"]\n",
    );
    assert_eq!(probe(home.path())["active"], false);
}
