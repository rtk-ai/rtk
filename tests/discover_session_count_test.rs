//! Count Bash-active sessions once without dropping non-Bash sessions from totals.

use serde_json::{Value, json};
use std::fs;
use std::path::Path;
use std::process::Command;

fn discover(root: &Path, scope: &[&str], format: &str) -> String {
    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("discover")
        .args(scope)
        .args(["--format", format])
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
    let cwd = root.path().canonicalize().unwrap();
    let slug = cwd
        .to_string_lossy()
        .replace(['/', '.', '_', '\\', ' ', '[', ']', ':'], "-");
    let project = root.path().join("claude/projects").join(&slug);
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

    let project_arg = format!("--project={slug}");
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

        for scope in [&["--all"][..], &[project_arg.as_str()][..], &[][..]] {
            let report: Value = serde_json::from_str(&discover(&cwd, scope, "json")).unwrap();
            assert_eq!(report["sessions_scanned"], 2 + bash_sessions);
            assert_eq!(report["sessions_with_bash"], bash_sessions);
            assert_eq!(report["total_commands"], 2 * bash_sessions);
            assert!(report.get("scope").is_none());

            let text = discover(&cwd, scope, "text");
            assert!(text.contains(&format!(
                "Scanned: {} sessions ({} with Bash commands, last 30 days), {} Bash commands",
                2 + bash_sessions,
                bash_sessions,
                2 * bash_sessions
            )));
            assert!(!text.contains("No sessions found"));
            assert!(!text.contains("No Claude Code sessions found"));
        }
    }

    // A filter that matches no project must not borrow counts from other scopes.
    let report: Value = serde_json::from_str(&discover(
        &cwd,
        &["--project", "nonexistent-project"],
        "json",
    ))
    .unwrap();
    assert_eq!(report["sessions_scanned"], 0);
    assert_eq!(report["sessions_with_bash"], 0);
    assert_eq!(report["total_commands"], 0);
}

#[test]
fn zero_sessions_preserves_counts_and_scope_specific_guidance() {
    let root = tempfile::tempdir().unwrap();
    fs::create_dir_all(root.path().join("claude/projects")).unwrap();
    for (scope, message, suggest_all) in [
        (
            &["--all"][..],
            "No Claude Code sessions found in any project in the last 30 days.",
            false,
        ),
        (
            &["--project", "missing-project"][..],
            "No sessions found for project filter `missing-project` in the last 30 days",
            true,
        ),
        (&[][..], "No sessions found for the current project", true),
    ] {
        let report: Value = serde_json::from_str(&discover(root.path(), scope, "json")).unwrap();
        assert_eq!(report["sessions_scanned"], 0);
        assert_eq!(report["sessions_with_bash"], 0);
        assert_eq!(report["total_commands"], 0);
        assert!(report.get("scope").is_none());

        let text = discover(root.path(), scope, "text");
        assert!(text.contains("Scanned: 0 sessions (0 with Bash commands"));
        assert!(text.contains(message), "{text}");
        assert_eq!(text.contains("rtk discover --all"), suggest_all, "{text}");
        assert!(!text.contains("RTK usage looks good"), "{text}");
    }
}
