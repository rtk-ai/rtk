//! The hook writer and audit reader must resolve the same configured directory.

mod common;

use std::io::Write;
use std::process::{Command, Stdio};

#[test]
fn hook_audit_round_trip_respects_data_directory_and_override() {
    for explicit_override in [false, true] {
        let root = common::tempdir();
        let data = root.path().join("data directory");
        let audit = root.path().join("audit override");
        let configure = |cmd: &mut Command| {
            cmd.env("XDG_DATA_HOME", &data).env("RTK_HOOK_AUDIT", "1");
            if explicit_override {
                cmd.env("RTK_AUDIT_DIR", &audit);
            } else {
                cmd.env_remove("RTK_AUDIT_DIR");
            }
        };

        let mut hook = common::rtk_command();
        configure(&mut hook);
        let mut child = hook
            .args(["hook", "claude"])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .expect("spawn hook");
        child
            .stdin
            .take()
            .expect("hook stdin")
            .write_all(br#"{"tool_name":"Bash","tool_input":{"command":"git status"}}"#)
            .expect("write hook payload");
        let output = child.wait_with_output().expect("hook result");
        assert!(output.status.success());
        let log = if explicit_override {
            audit.join("hook-audit.log")
        } else {
            data.join("rtk/hook-audit.log")
        };
        let contents = std::fs::read_to_string(&log)
            .unwrap_or_else(|error| panic!("missing audit log {}: {error}", log.display()));
        assert!(contents.contains(" | rewrite | git status | rtk git status"));

        let mut reader = common::rtk_command();
        configure(&mut reader);
        let output = reader.args(["hook-audit"]).output().expect("audit result");
        assert!(output.status.success());
        let stdout = String::from_utf8_lossy(&output.stdout);
        assert!(stdout.contains("Total invocations: 1"), "{stdout}");
        assert!(stdout.contains("git status (1)"), "{stdout}");
        if explicit_override {
            assert!(!data.join("rtk/hook-audit.log").exists());
        }
    }
}
