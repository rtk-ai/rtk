use serde_json::json;
use std::io::Write;
use std::path::Path;
use std::process::{Output, Stdio};

mod common;

fn run_trae_hook(command: &str, home: &Path, audit: bool) -> Output {
    let payload = json!({
        "tool_name": "RunCommand",
        "tool_input": {
            "command": command,
            "description": "Trae hook integration test"
        }
    })
    .to_string();

    run_trae_payload(&payload, home, audit)
}

fn run_trae_payload(payload: &str, home: &Path, audit: bool) -> Output {
    let mut child = common::rtk_command()
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
// dirs::home_dir uses the Windows Known Folder API, so HOME cannot isolate this log.
#[cfg(unix)]
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

#[test]
fn trae_hook_rewrites_bom_prefixed_payloads() {
    let home = tempfile::tempdir().unwrap();
    let payload = json!({"tool_name": "RunCommand", "tool_input": {
        "command": "git status", "description": "keep", "timeout": 60
    }})
    .to_string();
    let plain = run_trae_payload(&payload, home.path(), false);
    assert!(plain.status.success());
    assert!(!plain.stdout.is_empty());
    for prefix in ["\u{feff}", "\u{feff}\u{feff}"] {
        let output = run_trae_payload(&format!("{prefix}{payload}"), home.path(), false);
        assert!(output.status.success());
        assert_eq!(output.stdout, plain.stdout);
        assert!(output.stderr.is_empty());
    }
}
