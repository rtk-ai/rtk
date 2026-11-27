use serde_json::{Value, json};
use std::{
    fs,
    io::Write,
    path::Path,
    process::{Command, Output, Stdio},
};

mod common;

fn command(project: &Path, claude: &Path) -> Command {
    let mut cmd = common::rtk_command();
    cmd.current_dir(project)
        .env("CLAUDE_CONFIG_DIR", claude)
        .env("RTK_HOOK_AUDIT", "0");
    cmd
}

fn run(project: &Path, claude: &Path, args: &[&str]) -> Output {
    command(project, claude).args(args).output().unwrap()
}

fn success(output: Output) -> String {
    assert!(output.status.success(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn init(project: &Path, claude: &Path, extra: &[&str]) -> Output {
    let mut args = vec!["init", "--agent", "qoder"];
    args.extend_from_slice(extra);
    run(project, claude, &args)
}

fn read_json(path: &Path) -> Value {
    serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap()
}

fn hook(project: &Path, claude: &Path, tool: &str, input: Value) -> String {
    let payload = json!({
        "tool_name": tool,
        "tool_input": input,
        "cwd": project,
    });
    let mut child = command(project, claude)
        .args(["hook", "qoder"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    write!(child.stdin.take().unwrap(), "\u{feff}{payload}").unwrap();
    success(child.wait_with_output().unwrap())
}

#[test]
fn qoder_hook_rewrites_only_bash_using_qoder_approvals() {
    let tmp = common::tempdir();
    let claude = tmp.path().join(".claude");
    let project = claude.join("worktrees").join("project");
    fs::create_dir_all(&project).unwrap();
    fs::write(
        claude.join("settings.json"),
        r#"{"permissions":{"deny":["Bash(git status)"],"allow":["Bash(git log:*)"]}}"#,
    )
    .unwrap();

    for raw_command in ["git status", "git log -1"] {
        let output: Value = serde_json::from_str(&hook(
            &project,
            &claude,
            "Bash",
            json!({"command": raw_command, "timeout": 2000}),
        ))
        .unwrap();
        let response = &output["hookSpecificOutput"];
        assert_eq!(response["hookEventName"], "PreToolUse");
        assert_eq!(
            response["updatedInput"]["command"],
            format!("rtk {raw_command}")
        );
        assert_eq!(response["updatedInput"]["timeout"], 2000);
        assert!(response.get("permissionDecision").is_none());
    }

    for (tool, input) in [
        ("Read", json!({"command": "git status"})),
        ("Bash", json!({"command": "rtk git status"})),
        ("Bash", json!({"command": "echo hello"})),
        ("Bash", json!({"command": "git status $(whoami)"})),
        ("Bash", json!({"command": ""})),
    ] {
        assert_eq!(hook(&project, &claude, tool, input), "");
    }

    let diagnostic = success(run(
        &project,
        &claude,
        &["hook", "check", "--agent", "qoder", "git", "status"],
    ));
    assert_eq!(diagnostic.trim(), "rtk git status");
}

#[test]
fn qoder_project_lifecycle_preserves_other_settings_and_hooks() {
    let project = common::tempdir();
    let path = project.path();
    let claude = path.join(".claude");
    let qoder = path.join(".qoder");
    fs::create_dir(&claude).unwrap();
    let claude_settings = claude.join("settings.json");
    fs::write(&claude_settings, "{}").unwrap();
    fs::write(path.join("AGENTS.md"), "# Project instructions\n").unwrap();

    success(init(path, &claude, &["--dry-run"]));
    assert!(!qoder.exists());

    fs::create_dir(&qoder).unwrap();
    let settings_path = qoder.join("settings.json");
    let existing = json!({
        "model": "custom",
        "hooks": {
            "PreToolUse": [
                {"matcher": "Read", "hooks": [{"type": "command", "command": "python rtk_metrics.py"}]},
                {"matcher": "Bash", "hooks": [{"type": "command", "command": "python rtk_metrics.py"}]}
            ],
            "PostToolUse": [{"matcher": "*", "hooks": [{"type": "command", "command": "echo done"}]}]
        }
    });
    fs::write(&settings_path, format!("\u{feff}{existing}")).unwrap();

    success(init(path, &claude, &[]));
    let installed = read_json(&settings_path);
    let groups = installed["hooks"]["PreToolUse"].as_array().unwrap();
    assert_eq!(groups.len(), 3);
    assert_eq!(
        groups[..2],
        existing["hooks"]["PreToolUse"].as_array().unwrap()[..]
    );
    assert_eq!(groups[2]["hooks"][0]["command"], "rtk hook qoder");
    assert!(path.join("RTK.md").is_file());
    assert!(
        fs::read_to_string(path.join("AGENTS.md"))
            .unwrap()
            .contains("@RTK.md")
    );

    let original_bytes = fs::read(&settings_path).unwrap();
    success(init(path, &claude, &[]));
    assert_eq!(fs::read(&settings_path).unwrap(), original_bytes);

    let shown = success(init(path, &claude, &["--show"]));
    assert!(shown.contains("Qoder CLI configuration"), "{shown}");
    assert!(shown.contains("rtk hook qoder"), "{shown}");

    let gain = run(path, &claude, &["gain"]);
    assert!(gain.status.success(), "{gain:?}");
    assert!(!String::from_utf8_lossy(&gain.stderr).contains("No hook installed"));

    success(init(path, &claude, &["--uninstall", "--dry-run"]));
    assert_eq!(fs::read(&settings_path).unwrap(), original_bytes);
    assert!(path.join("RTK.md").exists());
    success(init(path, &claude, &["--uninstall"]));
    success(init(path, &claude, &["--uninstall"]));
    assert_eq!(read_json(&settings_path), existing);
    assert_eq!(fs::read_to_string(claude_settings).unwrap(), "{}");
    assert!(!path.join("RTK.md").exists());
    assert_eq!(
        fs::read_to_string(path.join("AGENTS.md")).unwrap().trim(),
        "# Project instructions"
    );
    assert!(success(init(path, &claude, &["--show"])).contains("not installed"));
}

#[test]
fn qoder_rejects_invalid_settings_without_writing() {
    let project = common::tempdir();
    let path = project.path();
    let claude = path.join(".claude");
    let settings = path.join(".qoder").join("settings.json");
    fs::create_dir(settings.parent().unwrap()).unwrap();
    for bad in [
        r#"{"model":"custom","#,
        "[]",
        r#"{"hooks":false}"#,
        r#"{"hooks":{"PreToolUse":{}}}"#,
    ] {
        fs::write(&settings, bad).unwrap();
        for flags in [&[][..], &["--uninstall"][..]] {
            let output = init(path, &claude, flags);
            assert!(!output.status.success(), "{output:?}");
            assert!(String::from_utf8_lossy(&output.stderr).contains("settings.json"));
            assert_eq!(fs::read_to_string(&settings).unwrap(), bad);
            assert!(!path.join("RTK.md").exists());
            assert!(!path.join("AGENTS.md").exists());
        }
    }
}

#[test]
fn qoder_upgrades_legacy_hook_after_other_handlers() {
    let project = common::tempdir();
    let path = project.path();
    let claude = path.join(".claude");
    let settings = path.join(".qoder").join("settings.json");
    fs::create_dir(settings.parent().unwrap()).unwrap();
    let existing = json!({
        "hooks": {"PreToolUse": [{
            "matcher": "Bash",
            "hooks": [
                {"type": "command", "command": "echo before"},
                {"type": "command", "command": "\"~/.qoder/hooks/rtk-qoder-hook.sh\"", "timeout": 30}
            ]
        }]}
    });
    fs::write(&settings, existing.to_string()).unwrap();
    success(init(path, &claude, &[]));
    let mut expected = existing.clone();
    expected["hooks"]["PreToolUse"][0]["hooks"][1]["command"] = json!("rtk hook qoder");
    assert_eq!(read_json(&settings), expected);
    success(init(path, &claude, &["--uninstall"]));
    expected["hooks"]["PreToolUse"][0]["hooks"]
        .as_array_mut()
        .unwrap()
        .pop();
    assert_eq!(read_json(&settings), expected);
}

#[test]
fn qoder_preserves_existing_project_instructions() {
    let project = common::tempdir();
    let path = project.path();
    let claude = path.join(".claude");
    let rtk_md = path.join("RTK.md");
    let agents_md = path.join("AGENTS.md");
    let instructions = "Project-specific RTK instructions.\n";
    let reference = "# Project instructions\n\n@RTK.md\n";
    fs::write(&rtk_md, instructions).unwrap();
    fs::write(&agents_md, reference).unwrap();

    success(init(path, &claude, &[]));
    success(init(path, &claude, &["--uninstall"]));
    assert_eq!(fs::read_to_string(rtk_md).unwrap(), instructions);
    assert_eq!(fs::read_to_string(agents_md).unwrap(), reference);
}

#[cfg(unix)]
#[test]
fn qoder_global_lifecycle_leaves_claude_and_project_settings_alone() {
    let home = common::tempdir();
    let project = common::tempdir();
    let path = project.path();
    let claude = home.path().join(".claude");
    fs::create_dir(&claude).unwrap();
    let claude_settings = claude.join("settings.json");
    let other_hook = r#"{"hooks":{"PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"rtk hook claude"}]}]}}"#;
    fs::write(&claude_settings, other_hook).unwrap();
    let run_global = |args: &[&str]| {
        success(
            command(path, &claude)
                .env("HOME", home.path())
                .args(["init", "-g", "--agent", "qoder"])
                .args(args)
                .output()
                .unwrap(),
        )
    };

    run_global(&[]);
    let qoder = home.path().join(".qoder");
    assert!(qoder.join("RTK.md").is_file());
    assert!(run_global(&["--show"]).contains("rtk hook qoder"));
    fs::write(&claude_settings, "{}").unwrap();
    let gain = command(path, &claude)
        .env("HOME", home.path())
        .arg("gain")
        .output()
        .unwrap();
    assert!(gain.status.success(), "{gain:?}");
    assert!(!String::from_utf8_lossy(&gain.stderr).contains("No hook installed"));
    fs::write(&claude_settings, other_hook).unwrap();
    run_global(&["--uninstall"]);
    assert!(run_global(&["--show"]).contains("not installed"));
    assert!(!qoder.join("RTK.md").exists());
    assert_eq!(fs::read_to_string(claude_settings).unwrap(), other_hook);
    assert!(!path.join(".qoder").exists());
}
