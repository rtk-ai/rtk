//! CLI regressions for #4073: small scalar arrays in `rtk json` keep every value.
//!
//! File and stdin paths share `render_json`; both are exercised here without a
//! Unix-only gate or a platform-specific shell.

use std::fs;
use std::io::Write;
use std::process::{Command, Output, Stdio};

const RTK: &str = env!("CARGO_BIN_EXE_rtk");

/// Follow-up pretty-printed reproduction: compact path (pretty input is larger
/// than the compact form, so `never_worse` keeps the filtered output).
const PRETTY_ROLES: &str = r#"{
  "status": "ok",
  "count": 8,
  "allowed_roles": ["admin", "editor", "viewer", "owner", "member", "guest", "auditor", "support"]
}"#;
const PRETTY_ROLE_VALUES: [&str; 8] = [
    "admin", "editor", "viewer", "owner", "member", "guest", "auditor", "support",
];

/// Issue-body example, fully minified so `never_worse` may select raw fallback.
const MINIFIED_ROLES: &str = r#"{"status":"ok","allowed_roles":["admin","editor","reviewer","publisher","auditor","moderator","archivist","guest"],"count":8}"#;
const MINIFIED_ROLE_VALUES: [&str; 8] = [
    "admin",
    "editor",
    "reviewer",
    "publisher",
    "auditor",
    "moderator",
    "archivist",
    "guest",
];

fn rtk_json_file(content: &str) -> Output {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("roles.json");
    fs::write(&path, content).expect("write json");
    Command::new(RTK)
        .arg("json")
        .arg(&path)
        .output()
        .expect("run rtk json <file>")
}

fn rtk_json_stdin(content: &str) -> Output {
    let mut child = Command::new(RTK)
        .args(["json", "-"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn rtk json -");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(content.as_bytes())
        .expect("write stdin");
    child.wait_with_output().expect("wait rtk json -")
}

fn stdout_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn stderr_str(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}

fn assert_success_preserves_roles(out: &Output, roles: &[&str], compact_path: bool) {
    assert!(
        out.status.success(),
        "rtk json should exit 0\nstdout: {}\nstderr: {}",
        stdout_str(out),
        stderr_str(out)
    );
    let stdout = stdout_str(out);
    assert!(
        !stdout.contains("... +"),
        "no array omission marker may appear: {stdout}"
    );
    let mut rest = stdout.as_str();
    for role in roles {
        let needle = format!("\"{role}\"");
        match rest.find(&needle) {
            Some(i) => rest = &rest[i + needle.len()..],
            None => panic!("missing role {role} in order in: {stdout}"),
        }
    }
    if compact_path {
        // Compact object format quotes values, not keys. Do not require the
        // whole object to parse as JSON. `"allowed_roles":` (quoted, raw/pretty)
        // also contains the substring `allowed_roles:`, so reject quoted keys.
        assert!(
            stdout.contains("allowed_roles:") && !stdout.contains("\"allowed_roles\""),
            "pretty input should use the compact path: {stdout}"
        );
    }
}

#[test]
fn json_file_pretty_uses_compact_path_and_keeps_roles() {
    let out = rtk_json_file(PRETTY_ROLES);
    assert_success_preserves_roles(&out, &PRETTY_ROLE_VALUES, true);
}

#[test]
fn json_stdin_pretty_uses_compact_path_and_keeps_roles() {
    let out = rtk_json_stdin(PRETTY_ROLES);
    assert_success_preserves_roles(&out, &PRETTY_ROLE_VALUES, true);
}

#[test]
fn json_file_minified_keeps_roles_even_if_raw_fallback() {
    let out = rtk_json_file(MINIFIED_ROLES);
    assert_success_preserves_roles(&out, &MINIFIED_ROLE_VALUES, false);
}

#[test]
fn json_stdin_minified_keeps_roles_even_if_raw_fallback() {
    let out = rtk_json_stdin(MINIFIED_ROLES);
    assert_success_preserves_roles(&out, &MINIFIED_ROLE_VALUES, false);
}

#[test]
fn json_file_and_stdin_strip_bom_and_stay_never_worse() {
    for (label, content, roles) in [
        ("pretty", PRETTY_ROLES, PRETTY_ROLE_VALUES.as_slice()),
        ("minified", MINIFIED_ROLES, MINIFIED_ROLE_VALUES.as_slice()),
    ] {
        let bom_prefixed = format!("\u{feff}{content}");
        for out in [rtk_json_file(&bom_prefixed), rtk_json_stdin(&bom_prefixed)] {
            assert!(
                out.status.success(),
                "{label} BOM input failed: {}",
                stderr_str(&out)
            );
            let stdout = stdout_str(&out);
            assert!(
                !stdout.starts_with('\u{feff}'),
                "{label}: BOM must not leak into output"
            );
            // println! appends a trailing newline after never_worse runs.
            let body = stdout.strip_suffix('\n').unwrap_or(stdout.as_str());
            assert!(
                body.len() / 4 <= content.len() / 4,
                "{label}: never_worse violated ({} bytes out of {} in)",
                body.len(),
                content.len()
            );
            let mut rest = stdout.as_str();
            for role in roles {
                let needle = format!("\"{role}\"");
                match rest.find(&needle) {
                    Some(i) => rest = &rest[i + needle.len()..],
                    None => panic!("{label}: missing role {role} in: {stdout}"),
                }
            }
        }
    }
}

#[test]
fn json_malformed_input_is_parse_error_nonzero_exit() {
    let file = rtk_json_file("{not json");
    let stdin = rtk_json_stdin("{not json");
    for (label, out) in [("file", file), ("stdin", stdin)] {
        assert_eq!(
            out.status.code(),
            Some(1),
            "{label}: malformed JSON must exit 1, got {:?}",
            out.status.code()
        );
        let err = stderr_str(&out);
        assert!(
            err.contains("Failed to parse JSON"),
            "{label}: expected parse error, got: {err}"
        );
    }
}

#[test]
fn json_rejects_non_json_extension() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("config.toml");
    fs::write(&path, "roles = [\"admin\"]").expect("write toml");
    let out = Command::new(RTK)
        .arg("json")
        .arg(&path)
        .output()
        .expect("run rtk json config.toml");
    assert_eq!(
        out.status.code(),
        Some(1),
        "non-JSON extension must exit 1, got {:?}",
        out.status.code()
    );
    let err = stderr_str(&out);
    assert!(
        err.contains("not a JSON file"),
        "extension rejection must remain intact: {err}"
    );
    assert!(err.contains("TOML"), "error should name the format: {err}");
}
