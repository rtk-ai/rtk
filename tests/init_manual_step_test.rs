//! `rtk init -g` without a terminal declines to patch settings.json and prints
//! a manual step. That step must name the matchers the pipeline needs, the
//! same ones `--auto-patch` writes, or a user who follows it never gets the
//! pipeline's Read rules (#5167).

// Without the engine no pipeline is loaded, so the only required matcher is Bash.
#![cfg(all(unix, feature = "engine"))]

use std::path::Path;
use std::process::{Command, Stdio};

mod common;

const BASH_ONLY: &str = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook claude"}]}]}}"#;

/// rtk with `home` as its home and `rtk:delegate-reads` as the user pipeline.
fn rtk(home: &Path) -> Command {
    let config = home.join("rtk-config");
    std::fs::create_dir_all(&config).expect("create rtk config dir");
    std::fs::write(
        config.join("pipeline.toml"),
        "[pipeline]\nextends = \"rtk:delegate-reads\"\n",
    )
    .expect("write pipeline.toml");
    let mut cmd = common::rtk_command();
    cmd.env("HOME", home)
        .env("CLAUDE_CONFIG_DIR", home.join(".claude"))
        .env("RTK_CONFIG_DIR", &config)
        .env("RTK_DB_PATH", home.join("rtk.db"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .stdin(Stdio::null());
    cmd
}

fn settings(home: &Path) -> std::path::PathBuf {
    home.join(".claude").join("settings.json")
}

/// Runs `rtk init -g --hook-only` with no terminal and returns stdout.
fn init_without_tty(home: &Path) -> String {
    let out = rtk(home)
        .args(["init", "-g", "--hook-only"])
        .output()
        .expect("run rtk init");
    assert!(
        out.status.success(),
        "rtk init failed: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

/// The JSON printed under "MANUAL STEP: Add this to", up to the next blank line.
fn added_entries(stdout: &str) -> serde_json::Value {
    let snippet: String = stdout
        .lines()
        .skip_while(|line| !line.contains("MANUAL STEP: Add this to"))
        .skip(1)
        .take_while(|line| !line.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    serde_json::from_str(&snippet).unwrap_or_else(|e| panic!("{e}: {snippet:?} in\n{stdout}"))
}

#[test]
fn existing_bash_entry_is_told_to_change_its_matcher() {
    let home = common::tempdir();
    std::fs::create_dir_all(home.path().join(".claude")).expect("create .claude");
    std::fs::write(settings(home.path()), BASH_ONLY).expect("write settings.json");

    let stdout = init_without_tty(home.path());

    assert!(
        stdout.contains(
            "set the matcher of the PreToolUse entry that runs `rtk hook claude` to \"Bash|Read\""
        ),
        "{stdout}"
    );
    assert!(
        !stdout.contains("Add this to"),
        "the entry exists; adding another would run the hook twice:\n{stdout}"
    );
    assert_eq!(
        std::fs::read_to_string(settings(home.path())).expect("read settings.json"),
        BASH_ONLY,
        "declining must leave settings.json untouched"
    );
}

#[test]
fn fresh_install_is_told_to_add_the_read_matcher() {
    let home = common::tempdir();

    let stdout = init_without_tty(home.path());

    assert_eq!(
        added_entries(&stdout),
        serde_json::json!({ "hooks": { "PreToolUse": [{
            "matcher": "Bash|Read",
            "hooks": [{ "type": "command", "command": "rtk hook claude" }]
        }]}})
    );
    assert!(
        stdout.contains(&format!("Add this to {}:", settings(home.path()).display())),
        "{stdout}"
    );
    assert!(!settings(home.path()).exists(), "declining writes nothing");
}

#[test]
fn following_the_manual_step_leaves_no_hook_gap() {
    let home = common::tempdir();
    std::fs::create_dir_all(home.path().join(".claude")).expect("create .claude");
    std::fs::write(settings(home.path()), BASH_ONLY).expect("write settings.json");
    let before = rtk(home.path())
        .args(["pipeline", "validate"])
        .output()
        .expect("run rtk pipeline validate");
    let before = String::from_utf8_lossy(&before.stdout);
    assert!(
        before.contains("PreToolUse does not cover Read"),
        "the Bash-only entry should start with a gap:\n{before}"
    );
    // Plain `rtk init -g` declines without a terminal, so the hint names the flag that patches.
    assert!(
        before.contains("run `rtk init -g --auto-patch`"),
        "{before}"
    );

    let stdout = init_without_tty(home.path());
    assert!(stdout.contains("to \"Bash|Read\""), "{stdout}");
    // Do what the step says.
    std::fs::write(
        settings(home.path()),
        BASH_ONLY.replace(r#""matcher":"Bash""#, r#""matcher":"Bash|Read""#),
    )
    .expect("write settings.json");

    let after = rtk(home.path())
        .args(["pipeline", "validate"])
        .output()
        .expect("run rtk pipeline validate");
    let after = String::from_utf8_lossy(&after.stdout);
    assert!(!after.contains("hook needed"), "{after}");
}
