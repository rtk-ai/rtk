use serde_json::{Value, json};
use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};
use tempfile::TempDir;

mod common;

fn command(temp: &TempDir) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(temp.path())
        .env("HOME", temp.path())
        .env("WORKBUDDY_CONFIG_DIR", temp.path().join("workbuddy"))
        .env("CLAUDE_CONFIG_DIR", temp.path().join("claude"))
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("RTK_DB_PATH", temp.path().join("rtk.db"))
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("RTK_HOOK_AUDIT", "0");
    cmd
}

fn hook(temp: &TempDir, input: &str) -> Output {
    hook_bytes(temp, input.as_bytes())
}

fn hook_bytes(temp: &TempDir, input: &[u8]) -> Output {
    let mut child = command(temp)
        .args(["hook", "workbuddy"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child.stdin.take().unwrap().write_all(input).unwrap();
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
fn workbuddy_fresh_install_and_verbose_dry_run_use_shared_json_writer() {
    let temp = TempDir::new().unwrap();
    let dir = temp.path().join("workbuddy");
    let path = dir.join("settings.json");
    let output = command(&temp)
        .args([
            "-v",
            "init",
            "-g",
            "--agent",
            "workbuddy",
            "--auto-patch",
            "--dry-run",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("would patch WorkBuddy settings"));
    assert!(stdout.contains("rtk hook workbuddy"));
    assert!(!dir.exists());

    let output = command(&temp)
        .args(["init", "-g", "--agent", "workbuddy", "--auto-patch"])
        .output()
        .unwrap();
    assert!(output.status.success());
    let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        root["hooks"]["PreToolUse"][0]["matcher"],
        "Bash|execute_command"
    );
    assert_eq!(
        root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "rtk hook workbuddy"
    );
    assert!(!path.with_extension("json.bak").exists());
}

#[test]
fn workbuddy_recognizes_existing_binary_paths_and_preserves_other_hooks() {
    for hook_command in [
        "/opt/bin/rtk hook workbuddy",
        r#""C:\Program Files\rtk.exe" hook workbuddy"#,
    ] {
        let temp = TempDir::new().unwrap();
        let dir = temp.path().join("workbuddy");
        fs::create_dir(&dir).unwrap();
        let path = dir.join("settings.json");
        let original = json!({"hooks":{"PreToolUse":[{"matcher":"Bash|execute_command","hooks":[
            {"type":"command","command":hook_command},
            {"type":"command","command":"echo rtk hook workbuddy"},
            {"type":"command","command":"rtk hook codex"}
        ]}]}})
        .to_string();
        fs::write(&path, &original).unwrap();
        let output = command(&temp)
            .args(["init", "-g", "--agent", "workbuddy", "--auto-patch"])
            .output()
            .unwrap();
        assert!(output.status.success());
        assert!(String::from_utf8_lossy(&output.stdout).contains("already installed"));
        assert_eq!(fs::read_to_string(&path).unwrap(), original);
        let output = command(&temp)
            .args(["init", "-g", "--agent", "workbuddy", "--uninstall"])
            .output()
            .unwrap();
        assert!(output.status.success());
        let root: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            root["hooks"]["PreToolUse"][0]["hooks"],
            json!([
                {"type":"command","command":"echo rtk hook workbuddy"},
                {"type":"command","command":"rtk hook codex"}
            ])
        );
    }
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

#[test]
fn workbuddy_invalid_utf8_and_oversized_input_pass_through() {
    let temp = TempDir::new().unwrap();
    for input in [vec![0xff, 0xfe], vec![b' '; 1_048_577]] {
        let output = hook_bytes(&temp, &input);
        assert!(output.stdout.is_empty());
        assert!(
            !output.stderr.is_empty(),
            "input failure should be diagnosed"
        );
    }
}

#[test]
fn workbuddy_does_not_interpret_other_hosts_permission_rules() {
    let temp = TempDir::new().unwrap();
    let claude_dir = temp.path().join("claude");
    fs::create_dir(&claude_dir).unwrap();
    for rule in ["default", "allow", "ask", "deny"] {
        let settings = if rule == "default" {
            json!({})
        } else {
            json!({"permissions": {rule: ["Bash(git *)", "Bash(rtk *)"]}})
        };
        fs::write(claude_dir.join("settings.json"), settings.to_string()).unwrap();
        for tool in ["Bash", "execute_command"] {
            let input = json!({"hook_event_name":"PreToolUse", "tool_name":tool,
                "tool_input":{"command":"git status", "description":"Keep metadata"}});
            let output = hook(&temp, &input.to_string());
            let result: Value = serde_json::from_slice(&output.stdout).unwrap();
            assert_eq!(
                result,
                json!({
                    "continue": true,
                    "hookSpecificOutput": {
                        "hookEventName": "PreToolUse",
                        "permissionDecisionReason": "RTK auto-rewrite",
                        "updatedInput": {"command":"rtk git status", "description":"Keep metadata"}
                    }
                }),
                "unexpected approval response for Claude rule {rule}"
            );
        }
    }
}

#[test]
fn workbuddy_override_takes_precedence_and_preserves_project_settings() {
    let temp = TempDir::new().unwrap();
    let original = r#"{"permissions":{"ask":["Bash(git *)"]},"theme":"dark"}"#;
    for name in [".workbuddy", ".codebuddy"] {
        fs::create_dir(temp.path().join(name)).unwrap();
        fs::write(temp.path().join(name).join("settings.json"), original).unwrap();
    }
    for action in ["--auto-patch", "--show", "--uninstall"] {
        let output = command(&temp)
            .args(["init", "-g", "--agent", "workbuddy", action])
            .output()
            .unwrap();
        assert!(output.status.success(), "{output:?}");
        for name in [".workbuddy", ".codebuddy"] {
            let path = temp.path().join(name).join("settings.json");
            assert_eq!(fs::read_to_string(&path).unwrap(), original);
            assert!(!path.with_extension("json.bak").exists());
        }
    }
    let root: Value = serde_json::from_str(
        &fs::read_to_string(temp.path().join("workbuddy/settings.json")).unwrap(),
    )
    .unwrap();
    assert!(root["hooks"]["PreToolUse"].as_array().unwrap().is_empty());
}

#[test]
#[cfg(unix)] // Windows resolves the home through Known Folders, not HOME.
fn workbuddy_unset_and_empty_override_use_global_home_settings() {
    for override_value in [None, Some("")] {
        let temp = TempDir::new().unwrap();
        let path = temp.path().join(".workbuddy/settings.json");
        let project = temp.path().join("project");
        fs::create_dir(&project).unwrap();
        let run = |args: &[&str]| {
            let mut cmd = command(&temp);
            cmd.current_dir(&project);
            match override_value {
                Some(value) => {
                    cmd.env("WORKBUDDY_CONFIG_DIR", value);
                }
                None => {
                    cmd.env_remove("WORKBUDDY_CONFIG_DIR");
                }
            }
            let output = cmd.args(args).output().unwrap();
            assert!(output.status.success(), "{output:?}");
            String::from_utf8(output.stdout).unwrap()
        };
        let args = ["init", "-g", "--agent", "workbuddy"];
        let absent = run(&[args.as_slice(), &["--show"]].concat());
        assert!(absent.contains("not installed"));
        assert!(absent.contains(path.to_str().unwrap()));
        assert!(!path.exists());
        run(&[args.as_slice(), &["--auto-patch", "--dry-run"]].concat());
        assert!(!path.exists());
        run(&[args.as_slice(), &["--auto-patch"]].concat());
        let installed = fs::read_to_string(&path).unwrap();
        assert!(installed.contains("rtk hook workbuddy"));
        let shown = run(&[args.as_slice(), &["--show"]].concat());
        assert!(shown.contains("WorkBuddy hook: installed"));
        assert!(shown.contains(path.to_str().unwrap()));
        assert!(!temp.path().join("workbuddy").exists());
        assert!(!project.join(".workbuddy").exists());
        assert!(!project.join(".codebuddy").exists());
        run(&[args.as_slice(), &["--uninstall"]].concat());
        assert!(run(&[args.as_slice(), &["--show"]].concat()).contains("not installed"));
        assert_eq!(
            fs::read_to_string(path.with_extension("json.bak")).unwrap(),
            installed
        );
    }
}
