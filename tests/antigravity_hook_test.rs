use serde_json::{Value, json};
use std::io::Write;
use std::path::Path;
use std::process::{Command, Output, Stdio};

fn run_antigravity_hook(command: &str, home: &Path, audit: bool) -> Output {
    let payload = json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": command
            }
        },
        "stepIdx": 1,
        "conversationId": "test-conversation-id"
    })
    .to_string();

    run_antigravity_payload(&payload, home, audit)
}

fn run_antigravity_payload(payload: &str, home: &Path, audit: bool) -> Output {
    let mut child = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["hook", "antigravity"])
        .env("HOME", home)
        .env("RTK_TELEMETRY_DISABLED", "1")
        .env("RTK_HOOK_AUDIT", if audit { "1" } else { "0" })
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("failed to spawn rtk hook antigravity");

    child
        .stdin
        .take()
        .expect("missing hook stdin")
        .write_all(payload.as_bytes())
        .expect("failed to write hook payload");

    child.wait_with_output().expect("hook process failed")
}

#[test]
fn antigravity_hook_rewrites_git_status() {
    let home = tempfile::tempdir().unwrap();
    let output = run_antigravity_hook("git status", home.path(), false);

    assert!(output.status.success(), "hook process failed");
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid json: {e}\nstdout: {stdout}"));

    assert_eq!(v["decision"], "allow");
    assert_eq!(v["overwrite"]["CommandLine"], "rtk git status");
}

#[test]
fn antigravity_hook_defers_unattestable_shell_constructs() {
    let home = tempfile::tempdir().unwrap();

    for command in [
        "git status $(whoami)",
        "git status `whoami`",
        "git status <(whoami)",
        "git status > /tmp/status.txt",
    ] {
        let output = run_antigravity_hook(command, home.path(), false);
        assert!(output.status.success(), "hook failed for `{command}`");
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("invalid json: {e}\nstdout: {stdout}"));

        assert_eq!(v["decision"], "allow");
        assert!(
            v.get("overwrite").is_none(),
            "unattestable command must defer without overwrite: `{command}` produced `{stdout}`"
        );
    }
}

#[test]
fn antigravity_hook_rewrites_bom_prefixed_payloads() {
    let home = tempfile::tempdir().unwrap();
    let payload = json!({
        "toolCall": {
            "name": "run_command",
            "args": {
                "CommandLine": "git status"
            }
        }
    })
    .to_string();

    let plain = run_antigravity_payload(&payload, home.path(), false);
    assert!(plain.status.success());
    assert!(!plain.stdout.is_empty());

    for prefix in ["\u{feff}", "\u{feff}\u{feff}"] {
        let output = run_antigravity_payload(&format!("{prefix}{payload}"), home.path(), false);
        assert!(output.status.success());
        assert_eq!(output.stdout, plain.stdout);
        assert!(output.stderr.is_empty());
    }
}

#[test]
// dirs::home_dir uses the Windows Known Folder API, so HOME cannot isolate this log.
#[cfg(unix)]
fn antigravity_hook_records_successful_rewrite_in_audit_log() {
    let home = tempfile::tempdir().unwrap();
    let output = run_antigravity_hook("git status", home.path(), true);

    assert!(output.status.success());
    assert!(
        !output.stdout.is_empty(),
        "expected an Antigravity rewrite response"
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
fn antigravity_hook_fails_open_on_corrupt_payload() {
    let home = tempfile::tempdir().unwrap();

    for bad in ["", "   ", "{invalid json"] {
        let output = run_antigravity_payload(bad, home.path(), false);
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        let v: Value = serde_json::from_str(&stdout)
            .unwrap_or_else(|e| panic!("invalid json: {e}\nstdout: {stdout}"));
        assert_eq!(v["decision"], "allow");
        assert!(v.get("overwrite").is_none());
    }
}

#[test]
fn antigravity_hook_ignores_non_command_tools() {
    let home = tempfile::tempdir().unwrap();
    let payload = json!({
        "toolCall": {
            "name": "view_file",
            "args": {
                "AbsolutePath": "/path/to/file.rs"
            }
        }
    })
    .to_string();

    let output = run_antigravity_payload(&payload, home.path(), false);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("invalid json: {e}\nstdout: {stdout}"));
    assert_eq!(v["decision"], "allow");
    assert!(v.get("overwrite").is_none());
}

/// Runs `rtk init` in `project` with an isolated HOME and closed stdin.
fn run_init(project: &Path, home: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_rtk"))
        .arg("init")
        .args(args)
        .current_dir(project)
        .env("HOME", home)
        .env("RTK_TELEMETRY_DISABLED", "1")
        .stdin(Stdio::null())
        .output()
        .expect("failed to spawn rtk init")
}

#[test]
fn antigravity_init_reports_rules_and_restart() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Rules:  rules/AGENTS.md"), "{stdout}");
    assert!(stdout.contains("Restart Antigravity"), "{stdout}");
}

#[test]
fn antigravity_uninstall_with_nothing_installed_says_so() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();

    for args in [
        &["--agent", "antigravity", "--uninstall"][..],
        &["--agent", "antigravity", "--uninstall", "--dry-run"][..],
    ] {
        let output = run_init(project.path(), home.path(), args);
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("nothing to remove"), "{args:?}: {stdout}");
        assert!(!stdout.contains("RTK uninstalled"), "{args:?}: {stdout}");
    }
}

#[test]
fn antigravity_dry_runs_change_nothing() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let plugin_dir = project.path().join(".agents/plugins/rtk");

    let output = run_init(
        project.path(),
        home.path(),
        &["--agent", "antigravity", "--dry-run"],
    );
    assert!(output.status.success());
    assert!(String::from_utf8_lossy(&output.stdout).contains("[dry-run] Nothing written."));
    assert!(!plugin_dir.exists(), "install dry run must write nothing");

    let output = run_init(project.path(), home.path(), &["--agent", "antigravity"]);
    assert!(output.status.success());

    let output = run_init(
        project.path(),
        home.path(),
        &["--agent", "antigravity", "--uninstall", "--dry-run"],
    );
    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("[dry-run] would uninstall RTK for Google Antigravity:"),
        "{stdout}"
    );
    assert!(stdout.contains("[dry-run] Nothing written."), "{stdout}");
    assert!(
        plugin_dir.join("rules/AGENTS.md").is_file(),
        "uninstall dry run must leave the plugin in place"
    );
}
