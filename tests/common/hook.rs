//! `rtk hook <agent>` run the way a host runs it, from an isolated home.

use std::process::Output;

/// A pipeline rule that swaps `git status` for the `check-build` task, whose
/// replacement command is `rtk check-build`. Its `command_matches` is an
/// unanchored regex on purpose: tests rely on it matching any command that
/// contains `git status`, such as `git status $(date)`, which the hook defers.
pub const SWAP_GIT_STATUS: &str = r#"[upstream.haiku]
provider = "anthropic"
model = "claude-haiku-4-5"

[task.check-build]
kind = "digest"
upstream = "haiku"

[[hook.before_tool.shell]]
name = "swap-git"
when = { command_matches = "git status" }
action = "route"
task = "check-build"
mode = "swap"
"#;

/// Where a user's `pipeline.toml` sits under an isolated home:
/// `dirs::config_dir()` resolves differently per platform, so both.
pub const PIPELINE_PATHS: [&str; 2] = [
    "rtk/pipeline.toml",
    "Library/Application Support/rtk/pipeline.toml",
];

/// An isolated home carrying the files the test names, each written at
/// `(relative path, contents)`.
pub fn isolated_home(files: &[(&str, &str)]) -> tempfile::TempDir {
    let home = tempfile::tempdir().expect("create isolated home");
    for (path, contents) in files {
        let target = home.path().join(path);
        std::fs::create_dir_all(target.parent().expect("file has a parent")).expect("create dir");
        std::fs::write(&target, contents).expect("write file");
    }
    home
}

/// A Claude Code PreToolUse payload for a Bash `command`, reporting `cwd` as
/// the session's working directory when given.
pub fn claude_payload(command: &str, cwd: Option<&str>) -> String {
    let mut payload = serde_json::json!({
        "tool_name": "Bash",
        "tool_input": { "command": command },
    });
    if let Some(cwd) = cwd {
        payload["cwd"] = cwd.into();
    }
    payload.to_string()
}

/// `rtk hook check --agent <agent>` — the diagnostic that must answer for the
/// agent named, including whose rules that agent's hook reads.
///
/// It exits 0 with a rewrite and 1 without one, so anything else is a crash
/// or an agent it does not know, which would pass for "no rewrite".
pub fn hook_check(agent: &str, command: &str, home: &tempfile::TempDir) -> Output {
    let output = super::rtk_command()
        .args(["hook", "check", "--agent", agent, "--", command])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .output()
        .expect("run rtk hook check");
    assert!(
        matches!(output.status.code(), Some(0 | 1)),
        "rtk hook check --agent {agent} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

/// `rtk hook <agent>` fed a PreToolUse payload on stdin — the path a host
/// actually takes, as opposed to the `rtk rewrite` CLI.
///
/// A hook exits 0 on every path, whatever it decides, so anything else is a
/// crash, and a crash prints nothing, which would pass for a decision to stay
/// out of the way.
pub fn hook_stdin(agent: &str, payload: &str, home: &tempfile::TempDir) -> Output {
    use std::io::Write;

    let mut child = super::rtk_command()
        .args(["hook", agent])
        .current_dir(home.path())
        .env("HOME", home.path())
        .env("XDG_CONFIG_HOME", home.path())
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("spawn rtk hook");
    child
        .stdin
        .as_mut()
        .expect("hook stdin")
        .write_all(payload.as_bytes())
        .expect("write payload");
    let output = child.wait_with_output().expect("run rtk hook");
    assert!(
        output.status.success(),
        "rtk hook {agent} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}
