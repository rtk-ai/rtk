use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

fn command(temp: &TempDir) -> Command {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_rtk"));
    cmd.current_dir(temp.path())
        .env("HOME", temp.path())
        .env("WORKBUDDY_CONFIG_DIR", temp.path().join("workbuddy"))
        .env("CLAUDE_CONFIG_DIR", temp.path().join("claude"))
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("RTK_DB_PATH", temp.path().join("rtk.db"));
    cmd
}

fn hook(temp: &TempDir, input: &str) -> Output {
    let mut child = command(temp)
        .args(["hook", "workbuddy"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    output
}

#[test]
fn workbuddy_rewrites_bom_payload_preserving_fields_without_approval() {
    let temp = TempDir::new().unwrap();
    // Another host's rules must not turn this adapter into an approval source.
    fs::create_dir(temp.path().join("claude")).unwrap();
    fs::write(
        temp.path().join("claude/settings.json"),
        r#"{"permissions":{"deny":["Bash(git *)"]}}"#,
    )
    .unwrap();
    for tool in ["Bash", "execute_command"] {
        let input = json!({"hook_event_name":"PreToolUse", "tool_name":tool,
            "tool_input":{"command":"git status","timeout":321,"description":"Inspect","extra":{"a":true}}});
        let output = hook(&temp, &format!("\u{feff}{input}"));
        let result: Value = serde_json::from_slice(&output.stdout).unwrap();
        let mut expected = input["tool_input"].clone();
        expected["command"] = json!("rtk git status");
        assert_eq!(result["hookSpecificOutput"]["updatedInput"], expected);
        assert_eq!(result["continue"], true);
        assert!(result.get("permissionDecision").is_none());
        assert!(
            result["hookSpecificOutput"]
                .get("permissionDecision")
                .is_none()
        );
        assert!(result["hookSpecificOutput"].get("modifiedInput").is_none());
    }
}

#[test]
fn workbuddy_ignores_malformed_wrong_event_non_shell_and_unsafe_commands() {
    let temp = TempDir::new().unwrap();
    for input in [
        "",
        "not json",
        "{}",
        r#"{"tool_name":"Read","tool_input":{"command":"git status"}}"#,
        r#"{"tool_name":"Bash","hook_event_name":"PostToolUse","tool_input":{"command":"git status"}}"#,
    ] {
        assert!(hook(&temp, input).stdout.is_empty(), "{input}");
    }
    for cmd in [
        "rtk git status",
        "htop",
        "git status > out.txt",
        "git status $(echo hi)",
        "cat <<EOF\nhi\nEOF",
    ] {
        let input = json!({"tool_name":"Bash","tool_input":{"command":cmd}});
        assert!(hook(&temp, &input.to_string()).stdout.is_empty(), "{cmd}");
    }
}

#[test]
fn workbuddy_install_dry_run_idempotency_and_scoped_uninstall() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("workbuddy");
    let path = dir.join("settings.json");
    fs::create_dir(&dir).unwrap();
    let original = json!({"theme":"dark", "hooks":{"PreToolUse":[
        {"matcher":"Bash", "hooks":[{"type":"command","command":"rtk hook codebuddy"},{"type":"command","command":"echo user"}]},
        {"matcher":"Bash|execute_command", "hooks":[{"type":"prompt","command":"rtk hook workbuddy"}]}
    ]}});
    let initial = format!("\u{feff}{original}");
    fs::write(&path, &initial).unwrap();
    let run = |args: &[&str]| {
        let output = command(&temp).args(args).output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    run(&[
        "init",
        "-g",
        "--agent",
        "workbuddy",
        "--dry-run",
        "--auto-patch",
    ]);
    assert_eq!(fs::read_to_string(&path).unwrap(), initial);
    assert!(!path.with_extension("json.bak").exists());
    assert!(run(&["init", "-g", "--agent", "workbuddy", "--no-patch"]).contains("not installed"));
    assert_eq!(fs::read_to_string(&path).unwrap(), initial);
    run(&["init", "-g", "--agent", "workbuddy", "--auto-patch"]);
    let installed = fs::read_to_string(&path).unwrap();
    let value: Value = serde_json::from_str(&installed).unwrap();
    assert_eq!(value["hooks"]["PreToolUse"].as_array().unwrap().len(), 3);
    assert_eq!(
        fs::read_to_string(path.with_extension("json.bak")).unwrap(),
        initial
    );
    assert!(run(&["init", "-g", "--agent", "workbuddy", "--show"]).contains("installed"));
    assert!(
        run(&["init", "-g", "--agent", "workbuddy", "--auto-patch"]).contains("already installed")
    );
    assert_eq!(fs::read_to_string(&path).unwrap(), installed);
    run(&[
        "init",
        "-g",
        "--agent",
        "workbuddy",
        "--uninstall",
        "--dry-run",
    ]);
    assert_eq!(fs::read_to_string(&path).unwrap(), installed);
    run(&["init", "-g", "--agent", "workbuddy", "--uninstall"]);
    assert_eq!(
        serde_json::from_str::<Value>(&fs::read_to_string(&path).unwrap()).unwrap(),
        original
    );
    assert!(
        run(&["init", "-g", "--agent", "workbuddy", "--uninstall"]).contains("nothing to remove")
    );
}

#[test]
fn workbuddy_backup_errors_preserve_file_on_install_and_uninstall() {
    for installed in [false, true] {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("workbuddy");
        fs::create_dir(&dir).unwrap();
        let path = dir.join("settings.json");
        let content = if installed {
            r#"{"hooks":{"PreToolUse":[{"matcher":"Bash|execute_command","hooks":[{"command":"rtk hook workbuddy"}]}]}}"#
        } else {
            "{}"
        };
        fs::write(&path, content).unwrap();
        fs::create_dir(path.with_extension("json.bak")).unwrap();
        let output = command(&temp)
            .args([
                "init",
                "-g",
                "--agent",
                "workbuddy",
                if installed {
                    "--uninstall"
                } else {
                    "--auto-patch"
                },
            ])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert!(String::from_utf8_lossy(&output.stderr).contains("backup"));
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
    }
}

#[test]
fn workbuddy_rejects_local_install_and_invalid_json_without_writes() {
    let temp = TempDir::new().unwrap();
    for args in [
        vec!["init", "--agent", "workbuddy", "--auto-patch"],
        vec!["init", "--agent", "workbuddy", "--uninstall"],
        vec!["init", "--agent", "workbuddy", "--show"],
        vec!["init", "-g", "--agent", "workbuddy", "--codex"],
    ] {
        let output = command(&temp).args(args).output().unwrap();
        assert!(!output.status.success());
    }
    assert!(!temp.path().join("workbuddy").exists());
    let dir = temp.path().join("workbuddy");
    fs::create_dir(&dir).unwrap();
    let path = dir.join("settings.json");
    for content in ["{broken", "null", r#"{"hooks":[]}"#] {
        fs::write(&path, content).unwrap();
        let output = command(&temp)
            .args(["init", "-g", "--agent", "workbuddy", "--auto-patch"])
            .output()
            .unwrap();
        assert!(!output.status.success());
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
    }
    fs::write(&path, "").unwrap();
    assert!(
        command(&temp)
            .args(["init", "-g", "--agent", "workbuddy", "--auto-patch"])
            .status()
            .unwrap()
            .success()
    );
}
