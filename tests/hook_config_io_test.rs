//! Regression coverage for the shared hook configuration I/O.
use serde_json::Value;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Stdio;
use tempfile::TempDir;

mod common;

struct HookSandbox {
    _root: TempDir,
    home: PathBuf,
    project: PathBuf,
    factory_home: PathBuf,
}

impl HookSandbox {
    fn new() -> Self {
        let root = TempDir::new().expect("tempdir");
        let home = root.path().join("home");
        let project = root.path().join("project");
        let factory_home = root.path().join("factory-home");
        fs::create_dir_all(home.join(".cursor")).expect("cursor dir");
        // The project-root walk is deliberately bounded inside this sandbox.
        fs::create_dir_all(project.join(".claude")).expect("claude dir");
        fs::create_dir_all(factory_home.join(".factory")).expect("factory home");
        fs::create_dir_all(project.join(".factory")).expect("factory project");
        Self {
            _root: root,
            home,
            project,
            factory_home,
        }
    }

    fn run(&self, hook: &str, payload: Value) -> Value {
        let mut child = common::rtk_command()
            .args(["hook", hook])
            .current_dir(&self.project)
            .env("HOME", &self.home)
            .env("FACTORY_HOME_OVERRIDE", &self.factory_home)
            .env("CLAUDE_CONFIG_DIR", self.home.join("claude"))
            .env("XDG_CONFIG_HOME", self.home.join(".config"))
            .env("RTK_DB_PATH", self.project.join("rtk.db"))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn hook");
        child
            .stdin
            .take()
            .expect("hook stdin")
            .write_all(payload.to_string().as_bytes())
            .expect("write hook payload");
        let output = child.wait_with_output().expect("wait hook");
        assert!(
            output.status.success(),
            "hook exited {:?}: {}",
            output.status.code(),
            String::from_utf8_lossy(&output.stderr)
        );
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout.trim().is_empty() {
            // Droid's deny/step-aside protocol is an empty stdout response.
            return Value::Null;
        }
        serde_json::from_str(stdout.trim()).unwrap_or_else(|error| {
            panic!(
                "hook emitted invalid JSON ({error}): stdout={stdout:?}, stderr={:?}",
                String::from_utf8_lossy(&output.stderr)
            )
        })
    }

    #[cfg(unix)]
    fn cursor(&self, command: &str) -> Value {
        self.run(
            "cursor",
            serde_json::json!({
                "tool_name": "Bash",
                "tool_input": {"command": command}
            }),
        )
    }

    fn droid(&self, command: &str) -> Value {
        self.run(
            "droid",
            serde_json::json!({
                "tool_name": "Execute",
                "tool_input": {"command": command}
            }),
        )
    }

    #[cfg(unix)]
    fn cursor_settings(&self) -> PathBuf {
        self.home.join(".cursor/cli-config.json")
    }

    fn droid_settings(&self, scope: &str, file: &str) -> PathBuf {
        let root = match scope {
            "user" => self.factory_home.join(".factory"),
            "project" => self.project.join(".factory"),
            _ => panic!("unknown Droid scope: {scope}"),
        };
        root.join(file)
    }
}

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
// Cursor's settings live under the home directory, and dirs::home_dir uses the
// Windows Known Folder API, so HOME cannot point the hook at this sandbox there.
#[cfg(unix)]
fn cursor_settings_keep_missing_and_empty_behavior_but_step_aside_on_failures() {
    let sandbox = HookSandbox::new();

    let missing = sandbox.cursor("git status");
    assert_eq!(missing["permission"], "ask");
    assert_eq!(missing["updated_input"]["command"], "rtk git status");

    let settings = sandbox.cursor_settings();
    fs::write(&settings, "  \n").expect("write empty Cursor settings");
    let empty = sandbox.cursor("git status");
    assert_eq!(empty["permission"], "ask");
    assert_eq!(empty["updated_input"]["command"], "rtk git status");

    fs::write(
        &settings,
        r#"{"permissions":{"deny":["Shell(git push *)"],"allow":["Shell(git status)"]}}"#,
    )
    .expect("write valid Cursor settings");
    let valid = sandbox.cursor("git status");
    assert_eq!(valid["permission"], "allow");
    assert_eq!(valid["updated_input"]["command"], "rtk git status");
    assert_eq!(
        sandbox.cursor("git push origin main"),
        serde_json::json!({})
    );

    fs::write(&settings, "{broken").expect("write malformed Cursor settings");
    assert_eq!(sandbox.cursor("git status"), serde_json::json!({}));

    fs::remove_file(&settings).expect("remove Cursor settings");
    fs::create_dir(&settings).expect("make unreadable Cursor settings");
    assert_eq!(sandbox.cursor("git status"), serde_json::json!({}));
}

#[test]
fn droid_settings_honor_all_scopes_and_step_aside_on_failures() {
    let sandbox = HookSandbox::new();

    let missing = sandbox.droid("git status");
    assert_eq!(
        missing["hookSpecificOutput"]["updatedInput"]["command"],
        "rtk git status"
    );

    for scope in ["user", "project"] {
        for file in ["settings.json", "settings.local.json"] {
            fs::write(sandbox.droid_settings(scope, file), "\n")
                .expect("write empty Droid settings");
        }
    }
    let empty = sandbox.droid("git status");
    assert_eq!(
        empty["hookSpecificOutput"]["updatedInput"]["command"],
        "rtk git status"
    );

    let valid_scopes = [
        (
            "user",
            "settings.json",
            r#"{"commandDenylist":["git status"]}"#,
            "git status",
        ),
        (
            "user",
            "settings.local.json",
            r#"{"commandBlocklist":["cargo test"]}"#,
            "cargo test",
        ),
        (
            "project",
            "settings.json",
            r#"{"commandDenylist":["ls"]}"#,
            "ls",
        ),
        (
            "project",
            "settings.local.json",
            r#"{"commandBlocklist":["pwd"]}"#,
            "pwd",
        ),
    ];
    for (scope, file, content, command) in valid_scopes {
        fs::write(sandbox.droid_settings(scope, file), content)
            .expect("write valid Droid settings");
        assert!(sandbox.droid(command).is_null());
    }

    let rewritable = sandbox.droid("git diff");
    assert_eq!(
        rewritable["hookSpecificOutput"]["updatedInput"]["command"],
        "rtk git diff"
    );

    for (scope, file, content, _) in valid_scopes {
        let settings = sandbox.droid_settings(scope, file);
        fs::write(&settings, "{broken").expect("write malformed Droid settings");
        assert!(sandbox.droid("git diff").is_null());
        fs::write(&settings, content).expect("restore valid Droid settings");
    }

    for (scope, file, content, _) in valid_scopes {
        let settings = sandbox.droid_settings(scope, file);
        fs::remove_file(&settings).expect("remove Droid settings");
        fs::create_dir(&settings).expect("make unreadable Droid settings");
        assert!(sandbox.droid("git diff").is_null());
        fs::remove_dir(&settings).expect("remove unreadable Droid settings");
        fs::write(&settings, content).expect("restore valid Droid settings");
    }
}

mod opencode_global_permissions {
    use super::*;
    use std::path::Path;

    const ASK: &str = r#"{"permission":{"bash":{"ls *":"ask"}}}"#;

    fn write(path: &Path, content: &str) {
        fs::create_dir_all(path.parent().expect("fixture parent")).expect("fixture directory");
        fs::write(path, content).expect("write OpenCode fixture");
    }

    fn sandbox() -> HookSandbox {
        let sandbox = HookSandbox::new();
        // Keep the OpenCode project search inside the sandbox, including when
        // a nearer config.json belongs to another application.
        write(&sandbox._root.path().join("opencode.json"), "{}");
        sandbox
    }

    fn answer(sandbox: &HookSandbox, xdg: Option<&Path>, agent: Option<&str>) -> Value {
        let mut command = common::rtk_command();
        command
            .args(["hook", "opencode"])
            .current_dir(&sandbox.project)
            .env("HOME", &sandbox.home);
        if let Some(xdg) = xdg {
            command.env("XDG_CONFIG_HOME", xdg);
        } else {
            command.env_remove("XDG_CONFIG_HOME");
        }
        if let Some(agent) = agent {
            command.args(["--agent", agent]);
        }
        let output = command
            .args(["--", "ls", "-la"])
            .output()
            .expect("run OpenCode hook");
        assert_eq!(
            output.status.code(),
            Some(0),
            "OpenCode hook failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        assert!(
            output.stderr.is_empty(),
            "unexpected stderr: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        serde_json::from_slice(&output.stdout).expect("OpenCode hook JSON")
    }

    #[test]
    fn xdg_global_asks_preserve_the_original_command() {
        for file in ["opencode.json", "config.json"] {
            let sandbox = sandbox();
            let xdg = sandbox._root.path().join("xdg");
            write(&xdg.join("opencode").join(file), ASK);
            assert_eq!(
                answer(&sandbox, Some(&xdg), None),
                serde_json::json!({}),
                "ask from XDG {file} must survive the rewrite"
            );
        }
    }

    // Windows resolves HOME through the shell API, which cannot be redirected
    // by the child's environment. The XDG cases above remain platform-neutral.
    #[cfg(unix)]
    #[test]
    fn home_global_asks_work_with_xdg_unset_or_empty() {
        for xdg in [None, Some(Path::new(""))] {
            for file in ["opencode.json", "config.json"] {
                let sandbox = sandbox();
                write(&sandbox.home.join(".config/opencode").join(file), ASK);
                assert_eq!(
                    answer(&sandbox, xdg, None),
                    serde_json::json!({}),
                    "HOME {file}, XDG={xdg:?}"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn a_selected_xdg_root_never_falls_back_to_home() {
        for create_xdg in [false, true] {
            let sandbox = sandbox();
            let xdg = sandbox._root.path().join("xdg");
            write(&sandbox.home.join(".config/opencode/opencode.json"), ASK);
            if create_xdg {
                fs::create_dir_all(xdg.join("opencode")).expect("empty XDG directory");
            }
            assert_eq!(
                answer(&sandbox, Some(&xdg), None),
                serde_json::json!({"command": "rtk ls -la"}),
                "HOME must not supply rules when XDG has no config"
            );
        }
    }

    #[test]
    fn later_global_files_do_not_hide_an_earlier_ask() {
        for (first, second) in [
            ("config.json", "opencode.json"),
            ("opencode.json", "opencode.jsonc"),
        ] {
            let sandbox = sandbox();
            let xdg = sandbox._root.path().join("xdg");
            write(&xdg.join("opencode").join(first), ASK);
            write(&xdg.join("opencode").join(second), "{}");
            assert_eq!(
                answer(&sandbox, Some(&xdg), None),
                serde_json::json!({}),
                "empty {second} must preserve the ask in {first}"
            );
        }
    }

    #[test]
    fn global_jsonc_asks_are_loaded_alongside_json() {
        let sandbox = sandbox();
        let xdg = sandbox._root.path().join("xdg");
        write(&xdg.join("opencode/opencode.json"), "{}");
        write(&xdg.join("opencode/opencode.jsonc"), ASK);
        assert_eq!(answer(&sandbox, Some(&xdg), None), serde_json::json!({}));
    }

    #[test]
    fn a_generic_project_config_json_does_not_supply_opencode_rules() {
        let sandbox = sandbox();
        let xdg = sandbox._root.path().join("xdg");
        write(&sandbox.project.join("config.json"), ASK);
        assert_eq!(
            answer(&sandbox, Some(&xdg), None),
            serde_json::json!({"command": "rtk ls -la"})
        );
    }

    #[test]
    fn xdg_markdown_agents_with_rules_preserve_the_original_command() {
        for directory in ["agents", "agent"] {
            for frontmatter in [
                "permission:\n  bash:\n    \"ls *\": ask\n",
                "permissions:\n  - action: shell\n    resource: ls *\n    effect: ask\n",
            ] {
                let sandbox = sandbox();
                let xdg = sandbox._root.path().join("xdg");
                write(
                    &xdg.join("opencode").join(directory).join("probe.md"),
                    &format!("---\n{frontmatter}---\nPrompt\n"),
                );
                assert_eq!(
                    answer(&sandbox, Some(&xdg), Some("probe")),
                    serde_json::json!({}),
                    "agent from XDG {directory}/ must keep its rules"
                );
            }
        }
    }
}
