//! Regression coverage for the shared hook configuration I/O.
use std::fs;
use std::io::Write;
use std::process::Stdio;
use tempfile::TempDir;

mod common;

#[test]
fn copilot_cli_rewrite_preserves_vscode_tool_input() {
    let temp = TempDir::new().unwrap();
    // Stop the project-root walk inside the temp dir so permission rules from an
    // ancestor `.claude/settings.json` on the test machine cannot decide the outcome.
    fs::create_dir(temp.path().join(".claude")).unwrap();
    let input = serde_json::json!({
        "tool_name": "run_in_terminal",
        "tool_input": {"command": "git status", "timeout": 5000, "description": "Inspect changes"}
    });
    let mut child = common::rtk_command()
        .args(["hook", "copilot"])
        .current_dir(temp.path())
        .env("HOME", temp.path())
        .env("CLAUDE_CONFIG_DIR", temp.path().join("claude"))
        .env("XDG_CONFIG_HOME", temp.path().join("config"))
        .env("RTK_DB_PATH", temp.path().join("rtk.db"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.to_string().as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let response: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    let mut expected = input["tool_input"].clone();
    expected["command"] = serde_json::json!("rtk git status");
    assert_eq!(response["hookSpecificOutput"]["updatedInput"], expected);
    assert!(
        response["hookSpecificOutput"]
            .get("permissionDecision")
            .is_none()
    );
}

#[test]
fn codex_status_handles_global_and_local_empty_bom_and_invalid_files() {
    let temp = TempDir::new().unwrap();
    let global = temp.path().join("codex-home");
    let project = temp.path().join("project");
    fs::create_dir_all(&global).unwrap();
    fs::create_dir_all(project.join(".codex")).unwrap();
    let global_path = global.join("hooks.json");
    let local_path = project.join(".codex/hooks.json");
    let show = || {
        let output = common::rtk_command()
            .args(["init", "--codex", "--show"])
            .current_dir(&project)
            .env("HOME", temp.path())
            .env("CODEX_HOME", &global)
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("RTK_DB_PATH", temp.path().join("rtk.db"))
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };
    let missing = show();
    for label in ["Global", "Local"] {
        assert!(missing.contains(&format!("[--] {label} hook: not found")));
    }
    for content in ["", "  \n", "\u{feff}"] {
        fs::write(&global_path, content).unwrap();
        fs::write(&local_path, content).unwrap();
        let status = show();
        for label in ["Global", "Local"] {
            assert!(status.contains(&format!(
                "[--] {label} hooks.json exists but RTK hook is not configured"
            )));
        }
        assert!(!status.contains("invalid JSON"));
    }
    let installed = "\u{feff}{\"hooks\":{\"PreToolUse\":[{\"matcher\":\"Bash\",\"hooks\":[{\"type\":\"command\",\"command\":\"rtk hook codex\"}]}]}}";
    fs::write(&global_path, installed).unwrap();
    fs::write(&local_path, installed).unwrap();
    let status = show();
    assert!(status.contains("[ok] Global hook:"));
    assert!(status.contains("[ok] Local hook:"));
    fs::write(&global_path, "{broken").unwrap();
    fs::write(&local_path, "{broken").unwrap();
    let status = show();
    for label in ["Global", "Local"] {
        assert!(status.contains(&format!("[!!] {label} hooks.json is invalid JSON")));
    }
    assert_eq!(fs::read_to_string(&global_path).unwrap(), "{broken");
    assert_eq!(fs::read_to_string(&local_path).unwrap(), "{broken");
}

#[test]
fn project_hook_only_cli_patches_only_project_settings() {
    let temp = TempDir::new().unwrap();
    let project = temp.path().join("project");
    let global = temp.path().join("global-claude");
    fs::create_dir_all(project.join(".claude")).unwrap();
    fs::create_dir_all(&global).unwrap();
    let project_settings = project.join(".claude/settings.json");
    let global_settings = global.join("settings.json");
    let original = r#"{"permissions":{"allow":["Bash(ls)"]}}"#;
    fs::write(&project_settings, original).unwrap();
    fs::write(&global_settings, "{}").unwrap();

    let run = |args: &[&str]| {
        let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
            .args(args)
            .current_dir(&project)
            .env("HOME", temp.path())
            .env("CLAUDE_CONFIG_DIR", &global)
            .env("XDG_CONFIG_HOME", temp.path().join("config"))
            .env("RTK_DB_PATH", temp.path().join("rtk.db"))
            .env("RTK_TELEMETRY_DISABLED", "1")
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap()
    };

    let manual = run(&["init", "--hook-only", "--no-patch"]);
    assert!(manual.contains(&project_settings.display().to_string()));
    assert_eq!(fs::read_to_string(&project_settings).unwrap(), original);

    run(&["init", "--hook-only", "--auto-patch"]);
    let installed = fs::read_to_string(&project_settings).unwrap();
    let root: serde_json::Value = serde_json::from_str(&installed).unwrap();
    assert_eq!(root["permissions"]["allow"][0], "Bash(ls)");
    assert_eq!(
        root["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
        "rtk hook claude"
    );
    assert_eq!(
        fs::read_to_string(project_settings.with_extension("json.bak")).unwrap(),
        original
    );
    assert_eq!(fs::read_to_string(&global_settings).unwrap(), "{}");
    assert!(!project.join("CLAUDE.md").exists());
    assert!(!project.join(".claude/RTK.md").exists());

    let status = run(&["init", "--show"]);
    assert!(status.contains("[ok] Project settings.json: RTK hook configured"));
    run(&["init", "--hook-only", "--auto-patch"]);
    assert_eq!(fs::read_to_string(&project_settings).unwrap(), installed);
}
