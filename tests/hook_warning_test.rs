use std::process::{Command, Stdio};

#[test]
fn hook_claude_does_not_warn_when_global_hook_is_missing() {
    let home = tempfile::tempdir().expect("temp home");
    std::fs::create_dir(home.path().join(".claude")).expect("create .claude");

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["hook", "claude"])
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join(".local/share"))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .and_then(|mut child| {
            use std::io::Write;

            let input = r#"{"tool_name":"Bash","tool_input":{"command":"git status"}}"#;
            child
                .stdin
                .as_mut()
                .expect("stdin")
                .write_all(input.as_bytes())?;
            child.wait_with_output()
        })
        .expect("run rtk hook claude");

    assert!(
        output.status.success(),
        "hook command failed: {:?}",
        output.status
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("No hook installed"),
        "hook command should not emit no-hook warning while it is processing a hook payload; stderr: {stderr}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("rtk git status"),
        "expected hook rewrite output, got: {stdout}"
    );
}

#[test]
fn rewrite_does_not_warn_when_global_hook_is_missing() {
    let home = tempfile::tempdir().expect("temp home");
    std::fs::create_dir(home.path().join(".claude")).expect("create .claude");

    let output = Command::new(env!("CARGO_BIN_EXE_rtk"))
        .args(["rewrite", "git", "status"])
        .env("HOME", home.path())
        .env("XDG_DATA_HOME", home.path().join(".local/share"))
        .output()
        .expect("run rtk rewrite");

    assert_eq!(
        output.status.code(),
        Some(3),
        "rewrite should preserve ask-verdict exit code"
    );

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !stderr.contains("No hook installed"),
        "rewrite command should not emit no-hook warning while used by hook scripts; stderr: {stderr}"
    );

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert_eq!(stdout.trim(), "rtk git status");
}
