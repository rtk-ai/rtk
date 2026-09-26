//! Count Bash-active sessions once without dropping non-Bash sessions from totals.

use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;

fn discover(root: &Path, format: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["discover", "--all", "--format", format])
        .current_dir(root)
        .env("CLAUDE_CONFIG_DIR", root.join("claude"))
        .env("RTK_DB_PATH", root.join("rtk.db"))
        .output()
        .expect("run discover");
    assert!(
        output.status.success(),
        "discover failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 report")
}

#[test]
fn counts_bash_sessions_separately_in_text_and_json() {
    let root = tempfile::tempdir().unwrap();
    let project = root.path().join("claude/projects/fixture");
    fs::create_dir_all(&project).unwrap();
    fs::write(project.join("empty.jsonl"), "").unwrap();
    fs::write(
        project.join("observer.jsonl"),
        json!({
            "type": "assistant",
            "message": {"content": [{
                "type": "tool_use", "name": "Read", "id": "read-1",
                "input": {"file_path": "README.md"}
            }]}
        })
        .to_string(),
    )
    .unwrap();

    for bash_sessions in 0..=2 {
        if bash_sessions > 0 {
            fs::write(
                project.join(format!("bash-{bash_sessions}.jsonl")),
                json!({
                    "type": "assistant",
                    "message": {"content": [
                        {"type": "tool_use", "name": "Bash", "id": "bash-1",
                         "input": {"command": "git status"}},
                        {"type": "tool_use", "name": "Bash", "id": "bash-2",
                         "input": {"command": "git diff"}}
                    ]}
                })
                .to_string(),
            )
            .unwrap();
        }

        let report: Value = serde_json::from_str(&discover(root.path(), "json")).unwrap();
        assert_eq!(report["sessions_scanned"], 2 + bash_sessions);
        assert_eq!(report["sessions_with_bash"], bash_sessions);
        assert_eq!(report["total_commands"], 2 * bash_sessions);

        let text = discover(root.path(), "text");
        assert!(text.contains(&format!(
            "Scanned: {} sessions ({} with Bash commands, last 30 days), {} Bash commands",
            2 + bash_sessions,
            bash_sessions,
            2 * bash_sessions
        )));
    }
}
