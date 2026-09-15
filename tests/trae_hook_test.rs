use serde_json::json;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run_trae_hook(command: &str, home: &Path, audit: bool) -> Output {
    let payload = json!({
        "tool_name": "RunCommand",
        "tool_input": {
            "command": command,
            "description": "Trae hook integration test"
        }
    })
    .to_string();

    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["hook", "trae"])
        .env("HOME", home)
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("RTK_HOOK_AUDIT", if audit { "1" } else { "0" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn rtk hook trae");

    child
        .stdin
        .take()
        .expect("missing hook stdin")
        .write_all(payload.as_bytes())
        .expect("failed to write hook payload");

    child.wait_with_output().expect("hook process failed")
}

#[test]
fn trae_hook_defers_unattestable_shell_constructs() {
    let home = tempfile::tempdir().unwrap();

    for command in [
        "git status $(whoami)",
        "git status `whoami`",
        "git status <(whoami)",
        "git status > /tmp/status.txt",
    ] {
        let output = run_trae_hook(command, home.path(), false);
        assert!(output.status.success(), "hook failed for `{command}`");
        assert!(
            output.stdout.is_empty(),
            "unattestable command must defer without output: `{command}` produced `{}`",
            String::from_utf8_lossy(&output.stdout)
        );
    }
}

#[test]
fn trae_hook_records_successful_rewrite_in_audit_log() {
    let home = tempfile::tempdir().unwrap();
    let output = run_trae_hook("git status", home.path(), true);

    assert!(output.status.success());
    assert!(
        !output.stdout.is_empty(),
        "expected a Trae rewrite response"
    );

    let audit_path = home.path().join(".local/share/rtk/hook-audit.log");
    let audit = std::fs::read_to_string(&audit_path)
        .unwrap_or_else(|error| panic!("missing audit log at {}: {error}", audit_path.display()));
    assert!(
        audit.contains(" | rewrite | git status | rtk git status"),
        "unexpected audit log: {audit}"
    );
}
