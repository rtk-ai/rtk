mod common;

use std::fs;

#[test]
fn discover_does_not_report_shell_syntax_as_missing_tools() {
    let dir = common::tempdir();
    let claude_dir = dir.path().join("claude");
    let project_dir = claude_dir.join("projects").join("synthetic-project");
    fs::create_dir_all(&project_dir).expect("create synthetic project");
    let commands = [
        "# Check",
        "# Test",
        "[ -f output.txt ]",
        "[[ -n $result ]]",
        ": no-op",
        "then",
        "else",
        "do",
        "elif",
        "esac",
        "custom-tool build",
        "custom-tool build",
        "custom-tool '# Check'",
        "[custom-tool] build",
    ];
    let lines: Vec<String> = commands
        .iter()
        .enumerate()
        .map(|(i, command)| {
            serde_json::json!({
                "type": "assistant",
                "message": {"content": [{
                    "type": "tool_use", "name": "Bash", "id": format!("tool-{i}"),
                    "input": {"command": command}
                }]}
            })
            .to_string()
        })
        .collect();
    fs::write(project_dir.join("session.jsonl"), lines.join("\n")).expect("write session");
    let output = common::rtk_command()
        .env("CLAUDE_CONFIG_DIR", &claude_dir)
        .args(["discover", "--all", "--format", "json"])
        .output()
        .expect("run discover");
    assert!(output.status.success(), "{:?}", output.stderr);
    let report: serde_json::Value = serde_json::from_slice(&output.stdout).expect("JSON report");
    assert_eq!(report["sessions_scanned"], 1);
    let unsupported = report["unsupported"].as_array().expect("unsupported list");
    assert_eq!(unsupported.len(), 3, "{report:#}");
    assert!(unsupported.iter().any(|entry| entry["count"] == 2));
    assert!(
        unsupported.iter().all(|entry| {
            entry["example"].as_str().is_some_and(|command| {
                command.starts_with("custom-tool ") || command.starts_with("[custom-tool] ")
            })
        }),
        "{report:#}"
    );
}
